//! Per-session activity state and screen classifiers, ported from `src/gui/activity.rs:11-222`. Cosmetic only (sidebar + dock badge); kept gpui-free so it's testable without an `App`.

// Ported verbatim, units included; the constants are read against each other (`INPUT_QUIET` tracks `WORKING_RECENT` on purpose), so don't rewrite one to satisfy a lint.
#![allow(clippy::duration_suboptimal_units)]

use std::time::Duration;

use grove_core::agent::Agent;

/// Output younger than this counts as "actively producing".
pub const WORKING_RECENT: Duration = Duration::from_secs(2);
/// Past this output age, an animated title reads as hung, not working.
pub const TITLE_STALE: Duration = Duration::from_secs(60);
/// Discounts output recency within this window: scrolling redraws the PTY, which otherwise reads as fresh output.
pub const SCROLL_QUIET: Duration = Duration::from_secs(3);
/// Discounts self-induced echo/SIGWINCH redraws from a keystroke or resize; tracks `WORKING_RECENT` intentionally.
pub const INPUT_QUIET: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Working,
    WaitingForInput,
    Done,
    Idle,
    Exited,
}

/// Roll-up urgency for collapsed parent rows: waiting > working > done; idle/exited contribute nothing (`src/gui/activity.rs:201-215`).
pub fn most_urgent(states: impl Iterator<Item = ActivityState>) -> Option<ActivityState> {
    let mut best: Option<ActivityState> = None;
    for s in states {
        let rank = urgency_rank(s);
        if rank.is_none() {
            continue;
        }
        if rank > best.and_then(urgency_rank) {
            best = Some(s);
        }
    }
    best
}

fn urgency_rank(s: ActivityState) -> Option<u8> {
    match s {
        ActivityState::WaitingForInput => Some(3),
        ActivityState::Working => Some(2),
        ActivityState::Done => Some(1),
        ActivityState::Idle | ActivityState::Exited => None,
    }
}

/// Signals sampled per session on the classification tick.
pub struct Signals {
    /// Process alive (SessionStatus::Running).
    pub alive: bool,
    /// Time since the last PTY read.
    pub output_age: Duration,
    /// BEL rang since the user last viewed this session.
    pub bell_pending: bool,
    /// Session was `Working` earlier in this stretch (since last ack).
    pub was_working: bool,
    /// This session is the focused/visible one right now.
    pub focused: bool,
    /// Within `SCROLL_QUIET`; suppresses the scroll-redraw false positive.
    pub scrolling: bool,
    /// Within `INPUT_QUIET`; suppresses the echo/SIGWINCH false positive.
    pub interacting: bool,
    /// The OSC 0/1/2 title, if any; outranks the screen-pattern scrape when definite.
    pub title: Option<String>,
}

/// Per-session bookkeeping kept between classification ticks.
pub struct Tracker {
    pub state: ActivityState,
    /// Sticky "was Working since last acknowledgment" flag.
    pub was_working: bool,
    /// The terminal's bell count we've already consumed.
    pub bell_seen: usize,
    /// A bell rang while the session was unfocused and hasn't been acked.
    pub bell_pending: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            state: ActivityState::Idle,
            was_working: false,
            bell_seen: 0,
            bell_pending: false,
        }
    }
}

impl Tracker {
    /// Called on focus: clears bell, resets working-history, downgrades urgent states.
    pub fn acknowledge(&mut self) {
        self.bell_pending = false;
        self.was_working = false;
        if matches!(
            self.state,
            ActivityState::WaitingForInput | ActivityState::Done
        ) {
            self.state = ActivityState::Idle;
        }
    }
}

/// Classify one session from its signals + the bottom rows of its screen. `tail` is the last ~15 rows of the parsed grid, newline-joined.
#[must_use]
pub fn classify(agent: Agent, tail: &str, sig: &Signals) -> ActivityState {
    if !sig.alive {
        return ActivityState::Exited;
    }
    // Waiting evidence outranks the title: masking WaitingForInput (drives the dock badge) would be the worst failure.
    let waiting = !sig.focused && (sig.bell_pending || matches_waiting(agent, tail));
    let title = sig.title.as_deref();
    if !waiting && sig.output_age < TITLE_STALE && title.is_some_and(|t| title_working(agent, t)) {
        return ActivityState::Working;
    }
    let recent = sig.output_age < WORKING_RECENT && !sig.scrolling && !sig.interacting;
    if recent || matches_working(agent, tail) {
        return ActivityState::Working;
    }
    // Output is quiet from here on.
    if waiting {
        return ActivityState::WaitingForInput;
    }
    // Plain terminals never reach Done: their "work" signal is just typing echo, and a green ✓ on a shell where nothing ran reads as noise.
    if sig.was_working && agent != Agent::Terminal {
        return ActivityState::Done;
    }
    ActivityState::Idle
}

/// Claude's animated title prefix (verified against 2.1.173); `✳` excluded as ambiguous. Codex/OpenCode have no usable title (openai/codex#21958).
fn title_working(agent: Agent, title: &str) -> bool {
    let prefixes: &[&str] = match agent {
        Agent::Claude => &[
            "\u{2802} ", // ⠂ current spinner frame
            "\u{2810} ", // ⠐ current spinner frame
        ],
        Agent::Codex | Agent::OpenCode | Agent::Terminal => &[],
    };
    prefixes.iter().any(|p| title.starts_with(p))
}

/// Screen shows the agent's active-work marker. Generic agents (plain terminals) have none — recency alone decides for them.
fn matches_working(agent: Agent, tail: &str) -> bool {
    let patterns: &[&str] = match agent {
        Agent::Claude => &["esc to interrupt"],
        Agent::Codex => &["Esc to interrupt", "esc to interrupt"],
        Agent::OpenCode => &["esc interrupt", "working"],
        Agent::Terminal => &[],
    };
    patterns.iter().any(|p| tail.contains(p))
}

/// Requires menu structure (caret/numbered options) alongside the phrase — bare phrase text also appears in response prose and would false-positive.
fn matches_waiting(agent: Agent, tail: &str) -> bool {
    match agent {
        Agent::Claude => {
            tail.contains("❯ 1.")
                || ((tail.contains("Do you want") || tail.contains("Would you like"))
                    && tail.contains("1."))
        }
        Agent::Codex => {
            tail.contains("Allow command?")
                || tail.contains("select an option")
                || (tail.contains("Yes (y)") && tail.contains("(n)"))
        }
        Agent::OpenCode => {
            (tail.contains("permission") || tail.contains("Permission"))
                && (tail.contains("1.") || tail.contains("Yes"))
        }
        Agent::Terminal => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(alive: bool, age_secs: u64, bell: bool, was_working: bool, focused: bool) -> Signals {
        Signals {
            alive,
            output_age: Duration::from_secs(age_secs),
            bell_pending: bell,
            was_working,
            focused,
            scrolling: false,
            interacting: false,
            title: None,
        }
    }

    fn with_title(mut s: Signals, title: &str) -> Signals {
        s.title = Some(title.to_string());
        s
    }

    #[test]
    fn dead_process_is_exited() {
        assert_eq!(
            classify(Agent::Claude, "", &sig(false, 999, true, true, false)),
            ActivityState::Exited
        );
    }

    #[test]
    fn recent_output_is_working() {
        assert_eq!(
            classify(Agent::Terminal, "", &sig(true, 0, false, false, false)),
            ActivityState::Working
        );
    }

    #[test]
    fn quiet_with_working_history_is_done() {
        assert_eq!(
            classify(Agent::Claude, "❯ ", &sig(true, 10, false, true, false)),
            ActivityState::Done
        );
    }

    #[test]
    fn quiet_no_history_is_idle() {
        assert_eq!(
            classify(Agent::Terminal, "$ ", &sig(true, 60, false, false, false)),
            ActivityState::Idle
        );
    }

    /// The scroll redraw must not flip a Done session back to Working.
    #[test]
    fn scroll_redraw_does_not_resurrect_working() {
        let mut signals = sig(true, 0, false, true, true);
        signals.scrolling = true;
        assert_eq!(classify(Agent::Claude, "❯ ", &signals), ActivityState::Done);
    }

    /// While scrolling, a genuinely working agent is still caught by its on-screen working marker.
    #[test]
    fn scroll_keeps_working_when_marker_visible() {
        let mut signals = sig(true, 0, false, true, true);
        signals.scrolling = true;
        assert_eq!(
            classify(Agent::Claude, "✻ Cogitating… (esc to interrupt)", &signals),
            ActivityState::Working
        );
    }

    /// Self-induced echo/SIGWINCH redraw must not read as the agent working.
    #[test]
    fn interaction_redraw_is_not_working() {
        let mut signals = sig(true, 0, false, false, true);
        signals.interacting = true;
        assert_eq!(
            classify(Agent::Terminal, "❯ ", &signals),
            ActivityState::Idle
        );
    }

    /// A Done session that the user resizes must not flip back to Working.
    #[test]
    fn interaction_redraw_does_not_resurrect_working() {
        let mut signals = sig(true, 0, false, true, true);
        signals.interacting = true;
        assert_eq!(classify(Agent::Claude, "❯ ", &signals), ActivityState::Done);
    }

    /// While interacting, a genuinely working Claude is still caught by its animated title marker.
    #[test]
    fn interaction_keeps_working_when_title_marker_present() {
        let mut signals = sig(true, 0, false, false, true);
        signals.interacting = true;
        let signals = with_title(signals, "\u{2802} Cogitating…");
        assert_eq!(
            classify(Agent::Claude, "", &signals),
            ActivityState::Working
        );
    }

    /// While interacting, a genuinely working agent is still caught by its on-screen working marker (Codex/OpenCode have no title signal).
    #[test]
    fn interaction_keeps_working_when_marker_visible() {
        let mut signals = sig(true, 0, false, false, true);
        signals.interacting = true;
        assert_eq!(
            classify(Agent::Codex, "esc to interrupt", &signals),
            ActivityState::Working
        );
    }

    /// Interaction must never mask the highest-urgency waiting state: a permission prompt that appears as the user types still wins.
    #[test]
    fn interaction_does_not_mask_waiting() {
        let mut signals = sig(true, 0, false, false, false);
        signals.interacting = true;
        assert_eq!(
            classify(Agent::Claude, "Do you want to proceed?\n❯ 1. Yes", &signals),
            ActivityState::WaitingForInput
        );
    }

    #[test]
    fn bell_plus_quiet_is_waiting() {
        assert_eq!(
            classify(Agent::Terminal, "", &sig(true, 10, true, false, false)),
            ActivityState::WaitingForInput
        );
    }

    /// A decorative BEL during active output must not flag waiting.
    #[test]
    fn bell_while_output_recent_stays_working() {
        assert_eq!(
            classify(Agent::Claude, "", &sig(true, 0, true, true, false)),
            ActivityState::Working
        );
    }

    /// The focused session never shows WaitingForInput.
    #[test]
    fn focused_session_never_waiting() {
        let s = classify(
            Agent::Claude,
            "Do you want to proceed?",
            &sig(true, 10, true, true, true),
        );
        assert_ne!(s, ActivityState::WaitingForInput);
        assert_eq!(s, ActivityState::Done); // was_working + quiet
    }

    #[test]
    fn claude_working_marker() {
        let tail = "✻ Cogitating… (3s · esc to interrupt)";
        assert_eq!(
            classify(Agent::Claude, tail, &sig(true, 10, false, false, false)),
            ActivityState::Working
        );
    }

    #[test]
    fn claude_permission_box_is_waiting() {
        let tail = "\
│ Do you want to make this edit to main.rs?          │
│ ❯ 1. Yes                                            │
│   2. Yes, allow all edits during this session       │
│   3. No, and tell Claude what to do differently     │";
        assert_eq!(
            classify(Agent::Claude, tail, &sig(true, 10, false, true, false)),
            ActivityState::WaitingForInput
        );
    }

    /// Claude at-rest prompt with working history → Done, not Waiting.
    #[test]
    fn claude_at_rest_after_work_is_done() {
        let tail = "╭──────╮\n│ >    │\n╰──────╯";
        assert_eq!(
            classify(Agent::Claude, tail, &sig(true, 10, false, true, false)),
            ActivityState::Done
        );
    }

    #[test]
    fn codex_working_marker() {
        let tail = "▌ Working (12s · Esc to interrupt)";
        assert_eq!(
            classify(Agent::Codex, tail, &sig(true, 10, false, false, false)),
            ActivityState::Working
        );
    }

    #[test]
    fn codex_approval_is_waiting() {
        let tail = "Allow command?\n▌ Yes (y)\n  No, provide feedback (n)";
        assert_eq!(
            classify(Agent::Codex, tail, &sig(true, 10, false, true, false)),
            ActivityState::WaitingForInput
        );
    }

    #[test]
    fn opencode_permission_is_waiting() {
        let tail = "permission required: edit src/main.rs\n1. Yes  2. No";
        assert_eq!(
            classify(Agent::OpenCode, tail, &sig(true, 10, false, false, false)),
            ActivityState::WaitingForInput
        );
    }

    /// Unknown/plain terminal: no pattern matches → pure recency.
    #[test]
    fn terminal_ignores_agent_patterns() {
        let tail = "Do you want fries with that? (shell output, not a prompt)";
        assert_eq!(
            classify(Agent::Terminal, tail, &sig(true, 60, false, false, false)),
            ActivityState::Idle
        );
    }

    /// Prose without menu structure must not flag waiting (caused spurious dock bounces).
    #[test]
    fn claude_prose_question_is_not_waiting() {
        let tail = "Do you want me to also update the docs? I can do that\n\
                    in a follow-up. Let me know which approach you prefer.";
        assert_eq!(
            classify(Agent::Claude, tail, &sig(true, 10, false, false, false)),
            ActivityState::Idle
        );
    }

    /// Plain terminals never show Done — typing echo isn't "work".
    #[test]
    fn terminal_never_done() {
        assert_eq!(
            classify(Agent::Terminal, "$ ", &sig(true, 10, false, true, false)),
            ActivityState::Idle
        );
    }

    /// Claude's animated braille title prefix means a turn is running, even when output is quiet and no screen marker is visible.
    #[test]
    fn claude_braille_title_is_working() {
        for t in ["\u{2802} Fix the login bug", "\u{2810} Fix the login bug"] {
            let s = with_title(sig(true, 30, false, false, false), t);
            assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Working);
        }
    }

    /// Waiting evidence must win over a coarse working title (masking the dock-badge state is the worst failure).
    #[test]
    fn screen_waiting_beats_working_title() {
        let tail = "│ Do you want to make this edit?  │\n│ ❯ 1. Yes  │";
        let s = with_title(sig(true, 10, false, true, false), "\u{2802} Edit main.rs");
        assert_eq!(
            classify(Agent::Claude, tail, &s),
            ActivityState::WaitingForInput
        );
    }

    /// Past TITLE_STALE a working title alone must not assert Working (reads Done with history).
    #[test]
    fn stale_working_title_does_not_assert_working() {
        let s = with_title(
            sig(true, 120, false, true, false),
            "\u{2802} Fix the login bug",
        );
        assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Done);
    }

    /// `✳` is ambiguous (at-rest glyph and legacy spinner frame) — must not count as working.
    #[test]
    fn claude_static_asterisk_title_is_no_answer() {
        let s = with_title(sig(true, 60, false, false, false), "✳ Fix the login bug");
        assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Idle);
    }

    /// An unrecognized title falls through to the screen patterns.
    #[test]
    fn unrecognized_title_falls_back_to_screen() {
        let tail = "✻ Cogitating… (3s · esc to interrupt)";
        let s = with_title(sig(true, 10, false, false, false), "some shell title");
        assert_eq!(classify(Agent::Claude, tail, &s), ActivityState::Working);
    }

    /// No title at all behaves exactly like before (screen fallback).
    #[test]
    fn missing_title_falls_back_to_screen() {
        let tail = "Allow command?\n▌ Yes (y)\n  No, provide feedback (n)";
        assert_eq!(
            classify(Agent::Codex, tail, &sig(true, 10, false, true, false)),
            ActivityState::WaitingForInput
        );
    }

    /// The braille glyphs only mean "working" for Claude; other agents' titles are not interpreted.
    #[test]
    fn title_patterns_are_per_agent() {
        let s = with_title(sig(true, 60, false, false, false), "\u{2802} doing stuff");
        assert_eq!(classify(Agent::Codex, "", &s), ActivityState::Idle);
        let s = with_title(sig(true, 60, false, false, false), "\u{2802} doing stuff");
        assert_eq!(classify(Agent::Terminal, "", &s), ActivityState::Idle);
    }

    /// A dead process is Exited regardless of what the stale title claims.
    #[test]
    fn title_never_resurrects_dead_process() {
        let s = with_title(sig(false, 0, false, false, false), "\u{2802} working hard");
        assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Exited);
    }

    #[test]
    fn acknowledge_clears_bell_and_downgrades() {
        let mut t = Tracker {
            state: ActivityState::WaitingForInput,
            was_working: true,
            bell_seen: 3,
            bell_pending: true,
        };
        t.acknowledge();
        assert!(!t.bell_pending);
        assert!(!t.was_working);
        assert_eq!(t.state, ActivityState::Idle);
    }

    #[test]
    fn acknowledge_leaves_working_alone() {
        let mut t = Tracker {
            state: ActivityState::Working,
            was_working: true,
            bell_seen: 0,
            bell_pending: true,
        };
        t.acknowledge();
        assert_eq!(t.state, ActivityState::Working);
        assert!(!t.bell_pending);
    }

    #[test]
    fn waiting_outranks_working_outranks_done() {
        use ActivityState::{Done, WaitingForInput, Working};
        assert_eq!(
            most_urgent([Done, Working, WaitingForInput].into_iter()),
            Some(WaitingForInput)
        );
        assert_eq!(most_urgent([Done, Working].into_iter()), Some(Working));
        assert_eq!(most_urgent([Done].into_iter()), Some(Done));
    }

    #[test]
    fn idle_and_exited_contribute_nothing() {
        use ActivityState::{Exited, Idle};
        assert_eq!(most_urgent([Idle, Exited].into_iter()), None);
        assert_eq!(
            most_urgent([Idle, ActivityState::Done, Exited].into_iter()),
            Some(ActivityState::Done)
        );
    }

    #[test]
    fn an_empty_iterator_has_no_rollup() {
        assert_eq!(most_urgent(std::iter::empty()), None);
    }
}
