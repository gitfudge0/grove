# Session Status Indicators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the static session dot with a five-state derived `ActivityState` (Working / WaitingForInput / Done / Idle / Exited) shown as animated glyphs in both sidebar views, with collapsed-row roll-ups and a macOS dock badge/bounce for waiting sessions.

**Architecture:** A pure classifier in a new `src/gui/activity.rs` consumes per-session signals (process status, output age, BEL count from vt100, last-15-rows screen text) plus per-agent pattern tables, and is re-run every ~480ms from the existing 60ms `Msg::Tick` (every 8th tick). Results live in a `HashMap<usize, Tracker>` on `Grove`, keyed by the session's `dirty` Arc pointer (same keying as `pty_cache`). Rows render glyphs from that map; the dock module is a thin `#[cfg(target_os = "macos")]` objc shim.

**Tech Stack:** Rust, iced 0.13, vt100 0.15 (`audible_bell_count()` — used instead of the spec's reader-thread BEL atomic because raw 0x07 scanning false-positives on OSC title terminators), `objc` crate (macOS only).

**Spec deviations (grounded):**
- BEL detection reads `screen().audible_bell_count()` on the GUI tick rather than a new `bell: Arc<AtomicBool>` in the reader thread. Same information, no OSC false positives, no new thread plumbing.
- No new `iced::time::every(250ms)` subscription: Grove already has an unconditional 60ms tick driving cursor blink. Spinner frame and blink phase derive from the existing `blink_tick`; classification runs every 8th tick.

---

### Task 1: Classifier module (`src/gui/activity.rs`)

**Files:**
- Create: `src/gui/activity.rs`
- Modify: `src/gui/mod.rs` (register module)

- [ ] **Step 1: Write the module with failing-first tests**

Create `src/gui/activity.rs`:

```rust
//! Derived per-session activity state + the per-agent screen classifiers.
//!
//! All agent UI pattern strings live here so agent-UI drift is a one-file fix.
//! Classification is best-effort and cosmetic: states drive only sidebar
//! visuals and the macOS dock badge, never behavior.

use crate::agent::Agent;
use std::time::Duration;

/// Output younger than this counts as "actively producing".
pub const WORKING_RECENT: Duration = Duration::from_secs(2);
/// Quiet for this long with no working-history = plain idle (matches the
/// activity view's existing 45s threshold).
pub const IDLE_AFTER: Duration = Duration::from_secs(45);

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
    let recent = sig.output_age < WORKING_RECENT;
    if recent || matches_working(agent, tail) {
        return ActivityState::Working;
    }
    // Output is quiet from here on.
    if !sig.focused && (sig.bell_pending || matches_waiting(agent, tail)) {
        return ActivityState::WaitingForInput;
    }
    if sig.was_working {
        return ActivityState::Done;
    }
    ActivityState::Idle
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
fn matches_waiting(agent: Agent, tail: &str) -> bool {
    let patterns: &[&str] = match agent {
        Agent::Claude => &[
            "Do you want",
            "Would you like",
            "❯ 1.",
            "1. Yes",
        ],
        Agent::Codex => &[
            "Allow command?",
            "Yes (y)",
            "▌ Yes",
            "select an option",
        ],
        Agent::OpenCode => &["permission", "Permission", "1. Yes"],
        Agent::Terminal => &[],
    };
    patterns.iter().any(|p| tail.contains(p))
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
        if rank > best.and_then(urgency_rank_opt) {
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

fn urgency_rank_opt(s: ActivityState) -> Option<u8> {
    urgency_rank(s)
}

/// Activity-view group for a state: 0 = waiting, 1 = running/working,
/// 2 = done-or-idle, 3 = exited. Lower sorts first.
pub fn activity_group(s: ActivityState) -> u8 {
    match s {
        ActivityState::WaitingForInput => 0,
        ActivityState::Working => 1,
        ActivityState::Done | ActivityState::Idle => 2,
        ActivityState::Exited => 3,
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
        }
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

    // ── most_urgent roll-up ─────────────────────────────────────────────────

    #[test]
    fn urgency_waiting_beats_working() {
        let got = most_urgent(
            [ActivityState::Working, ActivityState::WaitingForInput].into_iter(),
        );
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

    // ── activity_group ordering ─────────────────────────────────────────────

    #[test]
    fn group_order_waiting_running_done_exited() {
        assert!(activity_group(ActivityState::WaitingForInput)
            < activity_group(ActivityState::Working));
        assert!(activity_group(ActivityState::Working) < activity_group(ActivityState::Done));
        assert_eq!(
            activity_group(ActivityState::Done),
            activity_group(ActivityState::Idle)
        );
        assert!(activity_group(ActivityState::Idle) < activity_group(ActivityState::Exited));
    }
}
```

- [ ] **Step 2: Register the module**

In `src/gui/mod.rs`, add to the module list (alphabetical, before `drop`):

```rust
mod activity;
```

- [ ] **Step 3: Run tests**

Run: `cargo test activity`
Expected: all tests in `gui::activity::tests` PASS (fix the classifier, not the tests, if not).

Note: `most_urgent` as written compares `rank > best.and_then(...)` — `Option<u8>` ordering makes `Some(_) > None` true, so this works; verify with the tests.

- [ ] **Step 4: Commit**

```bash
git add src/gui/activity.rs src/gui/mod.rs
git commit -m "feat: activity-state classifier with per-agent patterns"
```

---

### Task 2: Session signal accessors

**Files:**
- Modify: `src/session.rs`

- [ ] **Step 1: Add `tail_contents` and `bell_count` to `impl Session`**

Add after `current_title()` (around `src/session.rs:478`):

```rust
    /// Last `n` rows of the visible screen, newline-joined, for the activity
    /// classifier. Reads the live grid regardless of any user scrollback.
    pub fn tail_contents(&self, n: usize) -> String {
        let Ok(p) = self.parser.lock() else {
            return String::new();
        };
        let contents = p.screen().contents();
        let lines: Vec<&str> = contents.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    }

    /// Total BEL (0x07) count vt100 has seen on this session's stream.
    /// Monotonic; the caller diffs against its last-seen value. Using vt100's
    /// counter (not a raw byte scan) means OSC terminators don't false-ring.
    pub fn bell_count(&self) -> usize {
        self.parser
            .lock()
            .map(|p| p.screen().audible_bell_count())
            .unwrap_or(0)
    }
```

Note: `screen().contents()` returns the *visible* grid even when the user has scrolled back? Verify in vt100 0.15.2 — `contents()` formats the current display grid; if scrollback offset shifts it, snap-read via the screen as-is is acceptable (misclassification is cosmetic). Do not add scrollback gymnastics.

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles (new methods may warn dead_code until Task 4 — that's fine).

- [ ] **Step 3: Commit**

```bash
git add src/session.rs
git commit -m "feat: expose screen tail and bell count for activity detection"
```

---

### Task 3: Grove tracking state + tick classification + acknowledgment

**Files:**
- Modify: `src/gui/state.rs`
- Modify: `src/gui/update.rs`

- [ ] **Step 1: Add fields to `Grove` in `src/gui/state.rs`**

Add to the `Grove` struct (after `dir_cache`):

```rust
    /// Per-session activity trackers, keyed by the session's `dirty` Arc
    /// pointer (same stable key as `pty_cache`). Refreshed every ~480ms by
    /// `Msg::Tick`; stale keys are pruned on the same pass.
    pub activity: HashMap<usize, crate::gui::activity::Tracker>,
    /// Whether the OS window currently has focus — gates the dock bounce.
    pub window_focused: bool,
    /// Last dock badge value pushed, to avoid redundant objc calls.
    pub last_badge: usize,
```

Note: `state.rs` is inside `mod gui`, so the path is `super::activity::Tracker` — use `super::activity::Tracker` in the field type instead of `crate::gui::activity::Tracker` (the `gui` module is private). Same for `update.rs`.

Add a `Msg` variant:

```rust
    /// OS window gained/lost focus (drives dock-bounce gating and
    /// implicit acknowledgment of the visible session).
    WindowFocusChanged(bool),
```

- [ ] **Step 2: Initialize fields in `Grove::new` (`src/gui/update.rs`)**

In the `Self { ... }` literal add:

```rust
            activity: Default::default(),
            window_focused: true,
            last_badge: 0,
```

- [ ] **Step 3: Subscribe to window focus events**

In `Grove::subscription`'s `listen_with` match, add before the `_ => None` arm:

```rust
                Event::Window(iced::window::Event::Focused) => {
                    Some(Msg::WindowFocusChanged(true))
                }
                Event::Window(iced::window::Event::Unfocused) => {
                    Some(Msg::WindowFocusChanged(false))
                }
```

- [ ] **Step 4: Classification pass on `Msg::Tick`**

In `Msg::Tick` (after the `bg` status block), add:

```rust
                // Re-classify session activity every 8th tick (~480ms at 60ms).
                if self.blink_tick % 8 == 0 {
                    self.refresh_activity();
                }
```

Add the method to `impl Grove` (near `invalidate_pty_render_cache`):

```rust
    /// Recompute every session's `ActivityState` from its live signals.
    /// Runs every ~480ms; also prunes trackers for sessions that no longer
    /// exist and pushes dock badge/bounce updates on transitions.
    fn refresh_activity(&mut self) {
        use super::activity::{classify, ActivityState, Signals, Tracker};
        let now = std::time::Instant::now();
        let mut live_keys: Vec<usize> = Vec::with_capacity(self.app.sessions.len());
        let mut newly_waiting = false;

        for (i, s) in self.app.sessions.iter().enumerate() {
            let key = Arc::as_ptr(&s.dirty) as usize;
            live_keys.push(key);
            let focused = self.app.active_session == Some(i) && self.window_focused;
            let tracker = self.activity.entry(key).or_default();

            // Consume new bells: pending only when they ring unfocused.
            let bells = s.bell_count();
            if bells > tracker.bell_seen {
                tracker.bell_seen = bells;
                if !focused {
                    tracker.bell_pending = true;
                }
            }

            let alive = matches!(s.status(), crate::session::SessionStatus::Running);
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            let output_age = now.saturating_duration_since(t);
            // Skip the parser lock for sessions that can't need it.
            let tail = if alive { s.tail_contents(15) } else { String::new() };

            let sig = Signals {
                alive,
                output_age,
                bell_pending: tracker.bell_pending,
                was_working: tracker.was_working,
                focused,
            };
            let new_state = classify(s.agent, &tail, &sig);
            if new_state == ActivityState::Working {
                tracker.was_working = true;
            }
            if !alive {
                tracker.was_working = false;
                tracker.bell_pending = false;
            }
            if focused {
                // Watching it = continuously acknowledged.
                tracker.bell_pending = false;
            }
            if new_state == ActivityState::WaitingForInput
                && tracker.state != ActivityState::WaitingForInput
            {
                newly_waiting = true;
            }
            tracker.state = new_state;
            let _ = tracker; // entry borrow ends here
        }

        self.activity.retain(|k, _| live_keys.contains(k));

        // Dock: badge = waiting count; one bounce per enter-while-unfocused.
        let waiting = self
            .activity
            .values()
            .filter(|t| t.state == ActivityState::WaitingForInput)
            .count();
        if waiting != self.last_badge {
            super::dock::set_badge(waiting);
            self.last_badge = waiting;
        }
        if newly_waiting && !self.window_focused {
            super::dock::request_attention();
        }
        let _ = Tracker::default; // keep import used even on non-macos builds
    }

    /// Acknowledge the given session's tracker (user focused it).
    fn acknowledge_session(&mut self, i: usize) {
        if let Some(s) = self.app.sessions.get(i) {
            let key = Arc::as_ptr(&s.dirty) as usize;
            if let Some(t) = self.activity.get_mut(&key) {
                t.acknowledge();
            }
        }
    }

    /// Read-only state lookup for the view layer. Unknown sessions render
    /// Idle until the first classification tick.
    pub(super) fn activity_state(&self, s: &crate::session::Session) -> super::activity::ActivityState {
        let key = Arc::as_ptr(&s.dirty) as usize;
        self.activity
            .get(&key)
            .map(|t| t.state)
            .unwrap_or(super::activity::ActivityState::Idle)
    }
```

Note: the `let _ = tracker;` / `let _ = Tracker::default;` lines are scaffolding hints, not required — drop them if the borrow checker and unused-import lints are already clean. `super::dock` arrives in Task 7; until then stub it (Step 6).

- [ ] **Step 5: Acknowledge on focus**

In `Msg::SelectSession(i)`, after `self.app.active_session = Some(i);` add:

```rust
                    self.acknowledge_session(i);
```

Add the new message arm (anywhere in the match):

```rust
            Msg::WindowFocusChanged(f) => {
                self.window_focused = f;
                // Regaining focus acknowledges the visible session.
                if f {
                    if let Some(i) = self.app.active_session {
                        self.acknowledge_session(i);
                    }
                }
            }
```

- [ ] **Step 6: Temporary dock stub so this task compiles standalone**

Create `src/gui/dock.rs`:

```rust
//! macOS dock badge + attention bounce. No-ops off-macOS.
//! (Real objc implementation lands in the dock task.)

pub fn set_badge(_count: usize) {}
pub fn request_attention() {}
```

Register in `src/gui/mod.rs`: `mod dock;`

- [ ] **Step 7: Build and test**

Run: `cargo build && cargo test`
Expected: compiles; all existing tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/gui/state.rs src/gui/update.rs src/gui/dock.rs src/gui/mod.rs
git commit -m "feat: per-session activity tracking on the GUI tick"
```

---

### Task 4: Glyph rendering in rows (both views)

**Files:**
- Modify: `src/gui/palette.rs` (amber token)
- Modify: `src/gui/rows.rs` (glyph widget; delete local `ActivityState`; thread state through row builders)
- Modify: `src/gui/view.rs` (pass states + anim tick into rows)

- [ ] **Step 1: Add amber to the palette**

In `src/gui/palette.rs` accents section:

```rust
/// Attention amber — the "needs input" accent. Warmer than YELLOW so it
/// reads as a call to action next to green/working.
pub fn AMBER() -> Color {
    mix(ic(theme::current().yellow), ic(theme::current().red), 0.25)
}
```

- [ ] **Step 2: Replace `rows.rs`'s local `ActivityState` + `state_dot` with a glyph**

In `src/gui/rows.rs`:
1. Delete the local `pub enum ActivityState { Running, Idle, Exited }` and `state_dot`.
2. Add `use super::activity::ActivityState;` and `use super::metrics::MONO_FONT;`.
3. Add:

```rust
/// Spinner frames for the Working state, advanced by the GUI tick.
const SPINNER: [&str; 6] = ["◜", "◠", "◝", "◞", "◡", "◟"];

/// Status glyph replacing the old state dot. `tick` is `Grove::blink_tick`
/// (~60ms): spinner advances every 2 ticks (~8fps), the waiting `?` blinks
/// at ~1Hz (8 ticks on, 8 off) by dimming — never hiding — the glyph, so
/// row layout is stable.
pub fn state_glyph<'a>(state: ActivityState, tick: u32) -> Element<'a, Msg> {
    let (glyph, color) = match state {
        ActivityState::Working => (SPINNER[(tick / 2) as usize % SPINNER.len()], c::GREEN()),
        ActivityState::WaitingForInput => {
            let on = (tick / 8) % 2 == 0;
            ("?", if on { c::AMBER() } else { c::FG_MUTE() })
        }
        ActivityState::Done => ("✓", c::GREEN()),
        ActivityState::Idle => ("·", c::FG_MUTE()),
        ActivityState::Exited => ("○", c::FG_MUTE()),
    };
    container(
        text(glyph)
            .font(MONO_FONT)
            .size(11)
            .color(color)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(14)
    .center_x(14)
    .into()
}
```

- [ ] **Step 3: Thread state through the row builders**

`session_row` — change signature to:

```rust
pub fn session_row<'a>(
    idx: usize,
    s: &Session,
    wt_name: &str,
    active: bool,
    pending_kill: bool,
    state: ActivityState,
    tick: u32,
) -> Element<'a, Msg> {
```

and replace the leading `Space::with_width(Length::Fixed(0.0))` in `main_row` with the glyph:

```rust
    let main_row: Element<'a, Msg> = row![state_glyph(state, tick), meta, close_btn,]
```

(keep padding; the glyph's fixed 14px width replaces the zero-space slot). Also derive the label color from the state instead of the raw status lock: `WaitingForInput` keeps the label full-brightness (`c::FG()`), `Done` slightly dims (`c::FG_DIM()`):

```rust
    let agent_color = if active {
        c::CYAN()
    } else {
        match state {
            ActivityState::Working | ActivityState::WaitingForInput => c::FG(),
            ActivityState::Done | ActivityState::Idle => c::FG_DIM(),
            ActivityState::Exited => c::FG_MUTE(),
        }
    };
```

(The old `running` lock read in `session_row` becomes unused — delete it.)

`session_activity_row` — change signature: drop the internal status→state mapping, accept `state: ActivityState` and `tick: u32`, and pass both straight to `activity_row_inner`. Delete `session_activity_row_idle` entirely (the caller now passes the real state).

`activity_row_inner` — change `state: ActivityState` handling:

```rust
    let agent_color = match state {
        ActivityState::Working | ActivityState::WaitingForInput => c::FG(),
        ActivityState::Done | ActivityState::Idle => c::FG_DIM(),
        ActivityState::Exited => c::FG_MUTE(),
    };
```

add `tick: u32` parameter and replace `state_dot(&state)` with `state_glyph(state, tick)`.

`worktree_activity_row` — its `state_dot(&ActivityState::Exited)` placeholder becomes `state_glyph(ActivityState::Exited, 0)`.

- [ ] **Step 4: Update `view.rs` call sites**

Tree view (`src/gui/view.rs:369`):

```rust
                        col = col.push(session_row(
                            si,
                            s,
                            &wname,
                            active,
                            pending_kill,
                            self.activity_state(s),
                            self.blink_tick,
                        ));
```

`activity_row_wrapped`: drop the `force_idle` parameter; call:

```rust
        let row_el = session_activity_row(
            si,
            s,
            &s.project,
            wname,
            active,
            pending_kill,
            last,
            hovered,
            coords,
            self.activity_state(s),
            self.blink_tick,
        );
```

and update its two call sites (the grouping rewrite in Task 5 replaces them anyway — if doing tasks in order, just pass `false`-equivalent by removing the param now).

- [ ] **Step 5: Build, test, run clippy**

Run: `cargo build && cargo test && cargo clippy --all-targets`
Expected: compiles, tests pass, no new clippy errors (allow `too_many_arguments` already present on these fns — extend the existing `#[allow]`).

- [ ] **Step 6: Commit**

```bash
git add src/gui/palette.rs src/gui/rows.rs src/gui/view.rs
git commit -m "feat: status glyphs replace session state dots"
```

---

### Task 5: Activity-view grouping with `waiting` on top

**Files:**
- Modify: `src/gui/view.rs` (`activity_view` grouping pass)

- [ ] **Step 1: Rewrite the grouping pass**

In `activity_view` (`src/gui/view.rs:380`), replace the `running`/`idle`/`exited` bucketing loop with state-based buckets. Delete the `IDLE_AFTER` const there (the classifier owns thresholds now):

```rust
        use super::activity::ActivityState;
        let now = std::time::Instant::now();

        let mut waiting: Vec<usize> = Vec::new();
        let mut running: Vec<usize> = Vec::new();
        let mut idle: Vec<(usize, std::time::Instant)> = Vec::new();
        let mut exited: Vec<(usize, std::time::Instant)> = Vec::new();
        for (i, s) in self.app.sessions.iter().enumerate() {
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            match self.activity_state(s) {
                ActivityState::WaitingForInput => waiting.push(i),
                ActivityState::Working => running.push(i),
                ActivityState::Done | ActivityState::Idle => idle.push((i, t)),
                ActivityState::Exited => exited.push((i, t)),
            }
        }
        idle.extend(exited);
        waiting.sort_by_key(|i| std::cmp::Reverse(*i));
        running.sort_by_key(|i| std::cmp::Reverse(*i));
        idle.sort_by_key(|&(_, t)| std::cmp::Reverse(t));
        let idle: Vec<usize> = idle.into_iter().map(|(i, _)| i).collect();
```

Then render the new top group before `running`:

```rust
        if !waiting.is_empty() {
            col = col.push(activity_group_header("waiting", waiting.len(), true, None));
            for si in waiting {
                col = col.push(self.activity_row_wrapped(si, &session_wnames[si], now, &project_idx));
            }
        }
```

(`waiting` is hidden when empty — it's an attention group, unlike the always-visible running/idle scaffolding.) Keep the `running` and `idle` groups as-is, minus the removed `force_idle` argument; the `done` state renders inside `idle`'s group with its own ✓ glyph (the spec's "done/idle" combined group).

- [ ] **Step 2: Build and eyeball**

Run: `cargo build && cargo test`
Expected: compiles, tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/gui/view.rs
git commit -m "feat: waiting group tops the activity view"
```

---

### Task 6: Roll-ups on collapsed tree rows

**Files:**
- Modify: `src/gui/rows.rs` (`project_row`, `worktree_row` accept an optional roll-up state)
- Modify: `src/gui/view.rs` (compute roll-up for collapsed rows)

- [ ] **Step 1: Add the trailing roll-up glyph to `worktree_row` and `project_row`**

Both signatures gain `rollup: Option<ActivityState>, tick: u32`. In `worktree_row`, insert the glyph between `left_btn` and `actions`:

```rust
    let rollup_el: Element<'a, Msg> = match rollup {
        Some(st) => state_glyph(st, tick),
        None => Space::with_width(Length::Fixed(0.0)).into(),
    };
    container(row![left_btn, rollup_el, actions].align_y(iced::Alignment::Center))
```

In `project_row`, insert the same `rollup_el` into the right-side row before `add_btn`:

```rust
    let right = row![rollup_el, add_btn, remove_btn]
```

- [ ] **Step 2: Compute roll-ups in `tree_view`**

In `src/gui/view.rs` `tree_view`, before building each worktree row:

```rust
                // Collapsed rows surface the most urgent descendant state as a
                // trailing glyph; expanded parents show nothing extra.
                let wt_rollup = if !wt_expanded {
                    super::activity::most_urgent(
                        self.app
                            .sessions
                            .iter()
                            .filter(|s| s.wt_path == w.path)
                            .map(|s| self.activity_state(s)),
                    )
                } else {
                    None
                };
```

pass `wt_rollup, self.blink_tick` to `worktree_row`.

For the project row (find where `project_row(pi, ...)` is called with its `expanded` flag): when the project is collapsed, roll up over all sessions in any of its worktrees:

```rust
            let proj_rollup = if !expanded {
                let wts = self.worktrees_for_project_view(pi);
                super::activity::most_urgent(
                    self.app
                        .sessions
                        .iter()
                        .filter(|s| wts.iter().any(|w| w.path == s.wt_path))
                        .map(|s| self.activity_state(s)),
                )
            } else {
                None
            };
```

Note: `tree_view` already resolves the project's worktree slice (the `wts` binding used for the inner loop) — reuse that binding rather than adding a helper; adjust borrow order so the slice is computed before `project_row` is pushed. `worktree_activity_row` (activity view) is never collapsed-with-children, so it takes no roll-up.

- [ ] **Step 3: Ordering/roll-up unit tests**

Already covered by `most_urgent` tests in Task 1 (`urgency_*`) and `group_order_*`. Add one integration-shaped test to `activity.rs` tests if missing:

```rust
    #[test]
    fn rollup_empty_iterator_is_none() {
        assert_eq!(most_urgent(std::iter::empty()), None);
    }
```

- [ ] **Step 4: Build, test, commit**

Run: `cargo build && cargo test`

```bash
git add src/gui/rows.rs src/gui/view.rs src/gui/activity.rs
git commit -m "feat: collapsed project/worktree rows roll up descendant urgency"
```

---

### Task 7: macOS dock badge + bounce

**Files:**
- Modify: `Cargo.toml` (macOS-only `objc` dep)
- Modify: `src/gui/dock.rs` (real implementation)

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc = "0.2"
```

- [ ] **Step 2: Implement `src/gui/dock.rs`**

```rust
//! macOS dock signal: badge count of waiting sessions + one attention bounce
//! when a session enters WaitingForInput while Grove is unfocused.
//! All no-ops off-macOS. Thin by design — manually verified, not unit-tested.

#[cfg(target_os = "macos")]
pub fn set_badge(count: usize) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let dock_tile: *mut Object = msg_send![app, dockTile];
        let label: *mut Object = if count == 0 {
            std::ptr::null_mut()
        } else {
            let s = format!("{count}\0");
            msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()]
        };
        let _: () = msg_send![dock_tile, setBadgeLabel: label];
    }
}

#[cfg(target_os = "macos")]
pub fn request_attention() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // NSRequestUserAttentionType::NSInformationalRequest = 10
    const NS_INFORMATIONAL_REQUEST: u64 = 10;
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let _: i64 = msg_send![app, requestUserAttention: NS_INFORMATIONAL_REQUEST];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_badge(_count: usize) {}

#[cfg(not(target_os = "macos"))]
pub fn request_attention() {}
```

- [ ] **Step 3: Build and manually verify**

Run: `cargo build && cargo test`
Manual check (best-effort, requires running the app): spawn a Claude session, switch focus away from Grove, trigger a permission prompt → dock badge shows `1` and the icon bounces once; focusing the session clears the badge.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/gui/dock.rs
git commit -m "feat: macOS dock badge and attention bounce for waiting sessions"
```

---

### Task 8: Final verification

- [ ] **Step 1: Full check**

Run: `cargo build && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: clean build, all tests pass, clippy clean (match the repo's existing clippy bar — if main has pre-existing warnings, just don't add new ones).

- [ ] **Step 2: Manual smoke test**

Run `cargo run`; verify: spinner animates on a working session; `?` blinks amber on an unfocused session at a Claude permission prompt and clears on click; `✓` after a turn completes; collapsed worktree rows show the trailing roll-up glyph; activity view shows `waiting` group on top.

- [ ] **Step 3: Update the spec status**

Edit `docs/superpowers/specs/2026-06-11-session-status-indicators-design.md`: `**Status:** Implemented`.

```bash
git add docs/superpowers/specs/2026-06-11-session-status-indicators-design.md
git commit -m "docs: mark session status indicators spec implemented"
```
