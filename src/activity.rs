//! Per-session activity state as the sidebar sees it, plus the per-agent
//! screen classifiers.
//!
//! [`ActivityState`] (`src/gui/activity.rs:30-36`) and [`most_urgent`] /
//! `urgency_rank` (`:201-222`) were ported in Plan 05. Plan 06 completes the
//! port: the four timing constants, [`Signals`], [`Tracker`], [`classify`] and
//! the three private pattern predicates all come across verbatim from
//! `src/gui/activity.rs:11-197`, with their tests.
//!
//! All agent UI pattern strings live here so agent-UI drift is a one-file fix.
//! Classification is best-effort and cosmetic: states drive only sidebar
//! visuals and the macOS dock badge, never behavior.
//!
//! The live [`crate::entities::activity_store::ActivityStore`] — the 480ms
//! task, the hook state file, the native poller and the amber pulse — lives
//! next door in `entities/activity_store.rs`; this module stays pure and
//! gpui-free so every rule above is testable without an `App`.

// The four timing constants are ported verbatim from `src/gui/activity.rs`,
// units included — they are read against each other (`INPUT_QUIET` tracks
// `WORKING_RECENT` on purpose), so rewriting one as `from_mins` to satisfy a
// readability lint would obscure exactly the relationship that matters.
#![allow(clippy::duration_suboptimal_units)]

use std::time::{Duration, Instant};

use grove_core::agent::Agent;

/// Output younger than this counts as "actively producing".
pub const WORKING_RECENT: Duration = Duration::from_secs(2);
/// How long a non-`Working` session stays parked in `IN PROGRESS` after its
/// last genuine `Working` tick before falling back to `IDLE`. Deliberately
/// ~15x `WORKING_RECENT`: that window is kept tight on purpose for colour
/// responsiveness and is far too tight to also govern row position, which
/// should not flap on every brief output pause. Read against `WORKING_RECENT`
/// (see the module-level `duration_suboptimal_units` allow above).
pub const IDLE_DWELL: Duration = Duration::from_secs(30);
/// A working title older than this (by output age) is distrusted: a real
/// working turn always produces output well within this window, so a quiet
/// PTY plus an animated title means the agent is hung with a stale title.
pub const TITLE_STALE: Duration = Duration::from_secs(60);
/// A scroll within this window discounts output recency: scrolling redraws
/// the PTY, which otherwise reads as fresh agent output.
pub const SCROLL_QUIET: Duration = Duration::from_secs(3);
/// A keystroke or resize within this window discounts output recency: the
/// inner app's echo / SIGWINCH repaint flows back through the PTY reader and
/// otherwise reads as fresh agent output. Tracks `WORKING_RECENT` intentionally
/// (the discount should exactly cancel the recency window it guards). Genuine
/// work is still caught by the title marker and `matches_working`, so this only
/// suppresses self-induced redraws — note that for agents without a working
/// marker (plain Terminal, and Codex/OpenCode until their on-screen marker
/// first paints) a freshly typed command shows non-working for up to this long.
pub const INPUT_QUIET: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Working,
    WaitingForInput,
    Done,
    Idle,
    Exited,
}

/// Roll-up urgency for collapsed parent rows: waiting > working > done;
/// idle/exited contribute nothing (`src/gui/activity.rs:201-215`).
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

/// The single total order every attention-sorting call site shares: lower
/// sorts first (more urgent). `WaitingForInput` outranks `Done`, which
/// outranks `Working`, which outranks `Idle`, which outranks `Exited`.
///
/// This one table is the fix for the rail sort and the collapsed-parent
/// roll-up disagreeing on `Done` vs `Idle`: both now read from here, so they
/// cannot drift apart again. It also deliberately makes `Done` outrank
/// `Working` for the collapsed-row roll-up too — a finished agent needs the
/// user's attention, a working one does not.
pub fn attention_rank(s: ActivityState) -> u8 {
    match s {
        ActivityState::WaitingForInput => 0,
        ActivityState::Done => 1,
        ActivityState::Working => 2,
        ActivityState::Idle => 3,
        ActivityState::Exited => 4,
    }
}

fn urgency_rank(s: ActivityState) -> Option<u8> {
    match s {
        ActivityState::Idle | ActivityState::Exited => None,
        _ => Some(4 - attention_rank(s)),
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
    /// The user scrolled within the last `SCROLL_QUIET` window. The redraw
    /// that scrolling causes must not count as the agent producing output;
    /// a genuinely working agent is still caught by its working marker.
    pub scrolling: bool,
    /// The user typed into or resized this session within the last
    /// `INPUT_QUIET` window. The keystroke echo / SIGWINCH repaint this causes
    /// must not count as the agent producing output; a genuinely working agent
    /// is still caught by its working marker.
    pub interacting: bool,
    /// The OSC 0/1/2 window title the inner app last emitted, if any.
    /// Structured (the agent sets it deliberately), so it outranks the
    /// screen-pattern scrape when it yields a definite answer.
    pub title: Option<String>,
}

/// Per-session bookkeeping kept between classification ticks.
pub struct Tracker {
    pub state: ActivityState,
    /// When `state` last actually changed — the rail's activity clock. Stamped
    /// only on a real transition, never every tick, so it measures "how long
    /// has this state held" rather than "how long has this tracker existed".
    pub state_since: Instant,
    /// The rail's last-active clock: stamped on every classification tick
    /// where the session is genuinely `Working`, regardless of whether the
    /// state actually changed, plus once when the user acknowledges the
    /// session ([`Tracker::acknowledge`]) — "last worked on **or** last
    /// interacted with". Unlike `state_since` no classified state other than
    /// `Working` advances it (not `Idle`, `Done`, `WaitingForInput`, or
    /// `Exited`), so a `Working` <-> `Idle` flap — which re-stamps
    /// `state_since` on every transition — leaves this clock untouched. This
    /// is the `IN PROGRESS` / `IDLE` sort key, and the `IDLE_DWELL` cutoff
    /// between those two sections, precisely because of that: it measures
    /// "how long since this session did real work", not "how long has the
    /// current state held", so a flap can repaint a card without ever
    /// moving its row.
    pub last_active: Instant,
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
            state_since: Instant::now(),
            last_active: Instant::now(),
            was_working: false,
            bell_seen: 0,
            bell_pending: false,
        }
    }
}

impl Tracker {
    /// Acknowledge pending attention: called when the user focuses the
    /// session. Bell clears, working-history resets, urgent states downgrade.
    ///
    /// Acknowledgment also stamps `last_active`: that clock means "last worked
    /// on **or** last interacted with", so the session the user just opened is
    /// the most recent thing in the rail rather than sinking below sessions the
    /// agent happened to touch more recently. The deliberate consequence: an
    /// acknowledged `Done` session lands at the *top* of `IN PROGRESS` (fresh
    /// `last_active`, non-`Working`, inside `IDLE_DWELL`) — correct, because
    /// the user is working in it right now.
    pub fn acknowledge(&mut self) {
        self.last_active = Instant::now();
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
/// `tail` is the last ~15 rows of the parsed grid, newline-joined.
#[must_use]
pub fn classify(agent: Agent, tail: &str, sig: &Signals) -> ActivityState {
    if !sig.alive {
        return ActivityState::Exited;
    }
    // Waiting evidence: a visible permission prompt or a pending bell. The
    // title is a coarse whole-turn status signal, so this more specific
    // evidence must outrank it — letting a working title mask
    // WaitingForInput (the highest-urgency state, the one that drives the
    // dock badge) would be the worst possible failure. The title may only
    // assert Working when there is no waiting evidence. The staleness belt:
    // a working title on a long-quiet PTY means a hard-hung agent whose
    // animated title froze, not real work, so the title alone never asserts
    // Working past TITLE_STALE.
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
/// 2.1.173 binary: `Vg4=["⠂","⠐"]`, `hg4="✳"`). Only the current frames are
/// encoded; an unrecognized prefix falls through harmlessly to the screen
/// patterns. `✳` is deliberately NOT a signal either way: it is both the
/// at-rest prefix and one historical spinner frame, so it proves nothing.
///
/// Codex does not emit OSC titles (openai/codex#21958) and we could not
/// substantiate any for OpenCode — both stay on the screen-pattern fallback.
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
        assert_eq!(classify(Agent::Claude, "❯ ", &signals), ActivityState::Done);
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

    /// Typing into or resizing a session redraws the PTY (keystroke echo /
    /// SIGWINCH repaint), but that self-induced output must not read as the
    /// agent working.
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

    /// While interacting, a genuinely working Claude is still caught by its
    /// animated title marker.
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

    /// While interacting, a genuinely working agent is still caught by its
    /// on-screen working marker (Codex/OpenCode have no title signal).
    #[test]
    fn interaction_keeps_working_when_marker_visible() {
        let mut signals = sig(true, 0, false, false, true);
        signals.interacting = true;
        assert_eq!(
            classify(Agent::Codex, "esc to interrupt", &signals),
            ActivityState::Working
        );
    }

    /// Interaction must never mask the highest-urgency waiting state: a
    /// permission prompt that appears as the user types still wins.
    #[test]
    fn interaction_does_not_mask_waiting() {
        let mut signals = sig(true, 0, false, false, false);
        signals.interacting = true;
        assert_eq!(
            classify(Agent::Claude, "Do you want to proceed?\n❯ 1. Yes", &signals),
            ActivityState::WaitingForInput
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
            let s = with_title(sig(true, 30, false, false, false), t);
            assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Working);
        }
    }

    /// A visible permission menu is specific evidence the agent is blocked
    /// on the user; the title is only a coarse turn-status signal. Waiting
    /// must win — masking WaitingForInput (the state that drives the dock
    /// badge) behind a working title would be the worst possible failure.
    #[test]
    fn screen_waiting_beats_working_title() {
        let tail = "│ Do you want to make this edit?  │\n│ ❯ 1. Yes  │";
        let s = with_title(sig(true, 10, false, true, false), "\u{2802} Edit main.rs");
        assert_eq!(
            classify(Agent::Claude, tail, &s),
            ActivityState::WaitingForInput
        );
    }

    /// A working title on a long-quiet PTY is a hard-hung agent with a
    /// frozen animated title, not real work: past TITLE_STALE the title
    /// alone must not assert Working (with working history it reads Done).
    #[test]
    fn stale_working_title_does_not_assert_working() {
        let s = with_title(
            sig(true, 120, false, true, false),
            "\u{2802} Fix the login bug",
        );
        assert_eq!(classify(Agent::Claude, "", &s), ActivityState::Done);
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

    // ── tracker acknowledgment ──────────────────────────────────────────────

    #[test]
    fn acknowledge_clears_bell_and_downgrades() {
        let mut t = Tracker {
            state: ActivityState::WaitingForInput,
            state_since: Instant::now(),
            last_active: Instant::now(),
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
            state_since: Instant::now(),
            last_active: Instant::now(),
            was_working: true,
            bell_seen: 0,
            bell_pending: true,
        };
        t.acknowledge();
        assert_eq!(t.state, ActivityState::Working);
        assert!(!t.bell_pending);
    }

    /// Acknowledgment counts as activity: `last_active` advances, so the
    /// session the user just opened sorts as the most recent one.
    #[test]
    fn acknowledge_advances_last_active() {
        // Fallback is *now* on a clock too young to subtract from, which makes
        // the assertions below fail loudly rather than pass by accident.
        let old = Instant::now()
            .checked_sub(Duration::from_secs(600))
            .unwrap_or_else(Instant::now);
        let mut t = Tracker {
            state: ActivityState::Done,
            state_since: old,
            last_active: old,
            was_working: true,
            bell_seen: 0,
            bell_pending: true,
        };
        t.acknowledge();
        assert!(t.last_active > old);
        assert!(t.last_active.elapsed() < Duration::from_millis(100));
    }

    // ── most_urgent roll-up (Plan 05) ───────────────────────────────────────

    #[test]
    fn waiting_outranks_done_outranks_working() {
        // The single `attention_rank` table intentionally makes `Done`
        // outrank `Working` here too: a finished agent needs the user, a
        // working one does not.
        use ActivityState::{Done, WaitingForInput, Working};
        assert_eq!(
            most_urgent([Done, Working, WaitingForInput].into_iter()),
            Some(WaitingForInput)
        );
        assert_eq!(most_urgent([Done, Working].into_iter()), Some(Done));
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
