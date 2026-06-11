//! Derived per-session activity state + the per-agent screen classifiers.
//!
//! All agent UI pattern strings live here so agent-UI drift is a one-file fix.
//! Classification is best-effort and cosmetic: states drive only sidebar
//! visuals and the macOS dock badge, never behavior.

use crate::agent::Agent;
use std::time::Duration;

/// Output younger than this counts as "actively producing".
pub const WORKING_RECENT: Duration = Duration::from_secs(2);
/// A scroll within this window discounts output recency: scrolling redraws
/// the PTY, which otherwise reads as fresh agent output.
pub const SCROLL_QUIET: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Working,
    WaitingForInput,
    Done,
    Idle,
    Exited,
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
    /// The user scrolled within the last `SCROLL_QUIET` window. The redraw
    /// that scrolling causes must not count as the agent producing output;
    /// a genuinely working agent is still caught by its working marker.
    pub scrolling: bool,
    /// The OSC 0/1/2 window title the inner app last emitted, if any.
    /// Structured (the agent sets it deliberately), so it outranks the
    /// screen-pattern scrape when it yields a definite answer.
    pub title: Option<String>,
}

/// Per-session bookkeeping kept between classification ticks.
pub struct Tracker {
    pub state: ActivityState,
    /// Sticky "was Working since last acknowledgment" flag.
    pub was_working: bool,
    /// vt100 `audible_bell_count()` we've already consumed.
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
    /// Acknowledge pending attention: called when the user focuses the
    /// session. Bell clears, working-history resets, urgent states downgrade.
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

/// Classify one session from its signals + the bottom rows of its screen.
/// `tail` is the last ~15 rows of the parsed vt100 grid, newline-joined.
pub fn classify(agent: Agent, tail: &str, sig: &Signals) -> ActivityState {
    if !sig.alive {
        return ActivityState::Exited;
    }
    // Structured signal first: the OSC title is set by the agent on purpose,
    // unlike the screen scrape which breaks whenever the TUI is restyled.
    // An unrecognized (or absent) title is "no answer", not "not working" —
    // we fall through to the screen patterns and recency heuristics.
    let title = sig.title.as_deref();
    if title.is_some_and(|t| title_working(agent, t)) {
        return ActivityState::Working;
    }
    if !sig.focused && title.is_some_and(|t| title_waiting(agent, t)) {
        return ActivityState::WaitingForInput;
    }
    let recent = sig.output_age < WORKING_RECENT && !sig.scrolling;
    if recent || matches_working(agent, tail) {
        return ActivityState::Working;
    }
    // Output is quiet from here on.
    if !sig.focused && (sig.bell_pending || matches_waiting(agent, tail)) {
        return ActivityState::WaitingForInput;
    }
    // Plain terminals never reach Done: their "work" signal is just typing
    // echo, and a green ✓ on a shell where nothing ran reads as noise.
    if sig.was_working && agent != Agent::Terminal {
        return ActivityState::Done;
    }
    ActivityState::Idle
}

/// Title shows the agent's "actively working" marker.
///
/// Claude Code titles its window `"{prefix} {task}"`. While a turn runs the
/// prefix animates through `["\u{2802}", "\u{2810}"]` (braille ⠂/⠐, 960ms
/// cycle); at rest it is the static `✳` (verified against the shipped
/// 2.1.173 binary: `Vg4=["⠂","⠐"]`, `hg4="✳"`). Older builds
/// cycled the on-screen spinner frames (✢ ✶ ✻ ✽) instead, so those are
/// accepted too. `✳` is deliberately NOT a signal either way: it is both the
/// at-rest prefix and one historical spinner frame, so it proves nothing.
///
/// Codex does not emit OSC titles (openai/codex#21958) and we could not
/// substantiate any for OpenCode — both stay on the screen-pattern fallback.
fn title_working(agent: Agent, title: &str) -> bool {
    let prefixes: &[&str] = match agent {
        Agent::Claude => &[
            "\u{2802} ", // ⠂ current spinner frame
            "\u{2810} ", // ⠐ current spinner frame
            "\u{2722} ", // ✢ legacy spinner frame
            "\u{2736} ", // ✶ legacy spinner frame
            "\u{273B} ", // ✻ legacy spinner frame
            "\u{273D} ", // ✽ legacy spinner frame
        ],
        Agent::Codex | Agent::OpenCode | Agent::Terminal => &[],
    };
    prefixes.iter().any(|p| title.starts_with(p))
}

/// Title shows a pending question / permission prompt. No agent we support
/// emits a substantiated waiting marker in its title today; this exists so
/// the precedence slot is wired up when one appears.
fn title_waiting(_agent: Agent, _title: &str) -> bool {
    false
}

/// Screen shows the agent's active-work marker. Generic agents (plain
/// terminals) have none — recency alone decides for them.
fn matches_working(agent: Agent, tail: &str) -> bool {
    let patterns: &[&str] = match agent {
        Agent::Claude => &["esc to interrupt"],
        Agent::Codex => &["Esc to interrupt", "esc to interrupt"],
        Agent::OpenCode => &["esc interrupt", "working"],
        Agent::Terminal => &[],
    };
    patterns.iter().any(|p| tail.contains(p))
}

/// Screen bottom shows a pending question / permission prompt.
///
/// Question phrases like "Do you want" routinely appear in agent *response
/// text* too, so a bare phrase match would false-positive (spurious dock
/// bounces). Each phrase must co-occur with menu structure — a selection
/// caret or numbered options — which response prose doesn't have.
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

/// Roll-up urgency for collapsed parent rows: waiting > working > done;
/// idle/exited contribute nothing.
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
            title: None,
        }
    }

    fn with_title(mut s: Signals, title: &str) -> Signals {
        s.title = Some(title.to_string());
        s
    }

    // ── classify: generic rules ─────────────────────────────────────────────

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

    // ── scroll suppression ──────────────────────────────────────────────────

    /// Scrolling a Done session redraws the PTY (fresh output), but must not
    /// flip it to Working — the redraw is user-caused, not agent activity.
    #[test]
    fn scroll_redraw_does_not_resurrect_working() {
        let mut signals = sig(true, 0, false, true, true);
        signals.scrolling = true;
        assert_eq!(
            classify(Agent::Claude, "❯ ", &signals),
            ActivityState::Done
        );
    }

    /// While scrolling, a genuinely working agent is still caught by its
    /// on-screen working marker.
    #[test]
    fn scroll_keeps_working_when_marker_visible() {
        let mut signals = sig(true, 0, false, true, true);
        signals.scrolling = true;
        assert_eq!(
            classify(Agent::Claude, "✻ Cogitating… (esc to interrupt)", &signals),
            ActivityState::Working
        );
    }

    // ── bell handling ───────────────────────────────────────────────────────

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

    // ── per-agent fixtures (captured screen snippets) ───────────────────────

    /// Claude Code's running footer.
    #[test]
    fn claude_working_marker() {
        let tail = "✻ Cogitating… (3s · esc to interrupt)";
        assert_eq!(
            classify(Agent::Claude, tail, &sig(true, 10, false, false, false)),
            ActivityState::Working
        );
    }

    /// Claude Code's permission dialog.
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

    /// Codex spinner/footer.
    #[test]
    fn codex_working_marker() {
        let tail = "▌ Working (12s · Esc to interrupt)";
        assert_eq!(
            classify(Agent::Codex, tail, &sig(true, 10, false, false, false)),
            ActivityState::Working
        );
    }

    /// Codex approval prompt.
    #[test]
    fn codex_approval_is_waiting() {
        let tail = "Allow command?\n▌ Yes (y)\n  No, provide feedback (n)";
        assert_eq!(
            classify(Agent::Codex, tail, &sig(true, 10, false, true, false)),
            ActivityState::WaitingForInput
        );
    }

    /// OpenCode permission prompt.
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

    /// Question phrases inside agent response *prose* (no menu structure)
    /// must not flag waiting — that caused spurious dock bounces.
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

    // ── structured title signal ─────────────────────────────────────────────

    /// Claude's animated braille title prefix means a turn is running, even
    /// when output is quiet and no screen marker is visible.
    #[test]
    fn claude_braille_title_is_working() {
        for t in ["\u{2802} Fix the login bug", "\u{2810} Fix the login bug"] {
            let s = with_title(sig(true, 60, false, false, false), t);
            assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Working);
        }
    }

    /// Legacy Claude builds animated the on-screen spinner frames instead.
    #[test]
    fn claude_legacy_spinner_title_is_working() {
        let s = with_title(sig(true, 60, false, false, false), "✶ Refactor parser");
        assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Working);
    }

    /// Title says working, screen shows a (stale) permission menu: the
    /// structured signal wins over the screen scrape.
    #[test]
    fn title_working_beats_screen_waiting() {
        let tail = "│ Do you want to make this edit?  │\n│ ❯ 1. Yes  │";
        let s = with_title(sig(true, 10, false, true, false), "\u{2802} Edit main.rs");
        assert_eq!(classify(Agent::Claude, tail, &s), ActivityState::Working);
    }

    /// The static ✳ prefix is the at-rest glyph (and a legacy spinner
    /// frame): ambiguous, so it must NOT count as working — fall through.
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

    /// The braille glyphs only mean "working" for Claude; other agents'
    /// titles are not interpreted.
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

    // ── most_urgent roll-up ─────────────────────────────────────────────────

    #[test]
    fn urgency_waiting_beats_working() {
        let got =
            most_urgent([ActivityState::Working, ActivityState::WaitingForInput].into_iter());
        assert_eq!(got, Some(ActivityState::WaitingForInput));
    }

    #[test]
    fn urgency_working_beats_done() {
        let got = most_urgent([ActivityState::Done, ActivityState::Working].into_iter());
        assert_eq!(got, Some(ActivityState::Working));
    }

    #[test]
    fn urgency_idle_and_exited_roll_up_to_nothing() {
        let got = most_urgent([ActivityState::Idle, ActivityState::Exited].into_iter());
        assert_eq!(got, None);
    }

    #[test]
    fn rollup_empty_iterator_is_none() {
        assert_eq!(most_urgent(std::iter::empty()), None);
    }

    // ── tracker acknowledgment ──────────────────────────────────────────────

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
}
