//! The live activity source: the 480ms classification pass, the attention
//! pulse, the dock badge/bounce and the waiting queue.
//!
//! Port of `src/gui/update/tick.rs:104-287` (`refresh_activity`). Pure rules
//! (`classify`, `Tracker`, timing constants) live in [`crate::activity`].
//!
//! Runs as a plain `Timer::after(480ms)` loop, deliberately not coupled to
//! [`crate::entities::animation_clock::AnimationClock`] — gpui gives each
//! concern its own task, unlike iced's single `tick % 8 == 0` timer.
//!
//! Runs on the foreground executor (reads/writes entities, calls
//! `cx.notify()`); the blocking work is already off-thread in grove-core, and
//! moving it to a background task would reorder signals against the snapshot
//! they were captured with.
//!
//! The pulse is `with_animation`-free: that helper animates an element being
//! rendered, but this is a scalar read by six call sites. [`pulse_at`]
//! reproduces `attention_animation`'s output (`update/mod.rs:246-252`) as a
//! triangle wave: 1000ms half-period, `EaseInOut`, auto-reverse, forever, and
//! a constant `0.0` while nothing waits.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Context, Entity, Task};
use grove_core::agent::Agent;
use grove_core::attention::{self, AttentionState};
use grove_core::claude_agents::{NativeStatus, Poller};

use crate::activity::{classify, ActivityState, Signals, Tracker, INPUT_QUIET, SCROLL_QUIET};
use crate::entities::session_registry::{SessionId, SessionRegistry};
use crate::entities::workspace_state::WorkspaceState;
use crate::platform::dock;

/// A period, not a tick multiple. See the module docs.
pub const CLASSIFY_PERIOD: Duration = Duration::from_millis(480);

/// How many rows of the screen the scrape looks at (`tick.rs:157`).
const TAIL_ROWS: usize = 15;

/// Half-period of the attention pulse: 1000ms up, 1000ms back down (`update/mod.rs:246-252`).
#[allow(clippy::duration_suboptimal_units)]
const PULSE_HALF: Duration = Duration::from_millis(1000);

/// The classic cubic pair; `ease(0.5) == 0.5` is the midpoint the pulse test pins.
fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0f32.mul_add(t, -2.0);
        1.0 - (u * u * u) / 2.0
    }
}

/// Needs-attention pulse phase in `[0, 1]`; a constant 0.0 while `since` is `None` (`update/mod.rs:719-726`).
#[must_use]
pub fn pulse_at(since: Option<Instant>, now: Instant) -> f32 {
    let Some(since) = since else { return 0.0 };
    let half = PULSE_HALF.as_secs_f32();
    let elapsed = now.saturating_duration_since(since).as_secs_f32();
    let phase = elapsed % (2.0 * half);
    // Auto-reverse: up over the first half-period, back down over the second.
    let linear = if phase <= half {
        phase / half
    } else {
        (2.0 * half - phase) / half
    };
    ease_in_out(linear.clamp(0.0, 1.0))
}

/// The dock badge is pushed only when the waiting count actually changes (`tick.rs:272-275`).
#[must_use]
pub fn badge_transition(prev: usize, next: usize) -> Option<usize> {
    (prev != next).then_some(next)
}

/// One bounce per session *entering* `WaitingForInput` while unfocused (`tick.rs:256-260,284-286`).
#[must_use]
pub const fn should_bounce(newly_waiting: bool, window_focused: bool) -> bool {
    newly_waiting && !window_focused
}

/// A completed hook belongs to the previous turn. Fresh live evidence can start a new turn,
/// while a quiet screen keeps the persisted completion state.
fn resolve_done_hook(live: ActivityState) -> ActivityState {
    match live {
        ActivityState::Working | ActivityState::WaitingForInput => live,
        ActivityState::Idle | ActivityState::Done => ActivityState::Done,
        ActivityState::Exited => ActivityState::Exited,
    }
}

pub struct ActivityStore {
    trackers: HashMap<SessionId, Tracker>,
    /// Waiting sessions in `visible_session_order`, resolved once per pass.
    waiting: Vec<SessionId>,
    /// `Some(t)` while `waiting` is non-empty — the pulse's phase origin.
    pulse_since: Option<Instant>,
    /// `Grove::last_badge` (`state.rs:151`) — diffed before touching the dock.
    last_badge: usize,
    window_focused: bool,
    /// Held for the process lifetime, exactly as `Grove::claude_poller` is.
    poller: Option<Poller>,
    wiring: Option<Wiring>,
    _task: Task<()>,
    _observers: Vec<gpui::Subscription>,
}

struct Wiring {
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
}

impl Default for ActivityStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityStore {
    /// An inert store: no poller thread, no timer, everything `Idle`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            trackers: HashMap::new(),
            waiting: Vec::new(),
            pulse_since: None,
            last_badge: 0,
            window_focused: true,
            poller: None,
            wiring: None,
            _task: Task::ready(()),
            _observers: Vec::new(),
        }
    }

    pub fn start(
        state: Entity<WorkspaceState>,
        registry: Entity<SessionRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Acknowledgment is *not* on the 480ms clock: `WorkspaceState` only records the id and notifies.
        let observer = cx.observe(&state, |this: &mut Self, _, cx| this.drain_acks(cx));
        Self {
            trackers: HashMap::new(),
            waiting: Vec::new(),
            pulse_since: None,
            last_badge: 0,
            window_focused: true,
            poller: Some(Poller::new()),
            wiring: Some(Wiring { state, registry }),
            _task: Self::spawn_pass(cx),
            _observers: vec![observer],
        }
    }

    /// Unknown sessions render `Idle`, same as the iced build before its first classification tick (`update/mod.rs:710-716`).
    #[must_use]
    pub fn state_of(&self, id: SessionId) -> ActivityState {
        self.trackers
            .get(&id)
            .map_or(ActivityState::Idle, |t| t.state)
    }

    #[cfg(test)]
    pub fn set_state_for_test(&mut self, id: SessionId, state: ActivityState) {
        self.trackers.entry(id).or_default().state = state;
    }

    /// When a session's classified state last actually changed — the rail's activity clock.
    #[must_use]
    pub fn since_of(&self, id: SessionId) -> Option<Instant> {
        self.trackers.get(&id).map(|t| t.state_since)
    }

    #[cfg(test)]
    pub fn set_state_since_for_test(&mut self, id: SessionId, since: Instant) {
        self.trackers.entry(id).or_default().state_since = since;
    }

    /// Advances only on ticks where the session classifies as `Working`, regardless of state transitions.
    #[must_use]
    pub fn active_of(&self, id: SessionId) -> Option<Instant> {
        self.trackers.get(&id).map(|t| t.last_active)
    }

    #[cfg(test)]
    pub fn set_last_active_for_test(&mut self, id: SessionId, at: Instant) {
        self.trackers.entry(id).or_default().last_active = at;
    }

    #[must_use]
    pub fn pulse(&self) -> f32 {
        pulse_at(self.pulse_since, Instant::now())
    }

    /// Tree order, resolved once per pass and shared by the appbar pill, dropdown and `mod+'` (`update/mod.rs:728-740`).
    #[must_use]
    pub fn waiting_sessions(&self) -> &[SessionId] {
        &self.waiting
    }

    #[must_use]
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    /// Regaining focus acknowledges the visible session immediately (`layout.rs:34-49`) — governs acknowledgment, not classification.
    pub fn set_window_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        let regained = focused && !self.window_focused;
        self.window_focused = focused;
        if !regained {
            return;
        }
        let Some(wiring) = self.wiring.as_ref() else {
            return;
        };
        if let Some(id) = wiring.state.read(cx).active_session() {
            self.acknowledge(id, cx);
        }
        cx.notify();
    }

    /// Both the tracker and the file (truncated, not deleted, so hooks keep appending) — always, or a stale `needs-you` resurfaces (`update/mod.rs:697-707`).
    pub fn acknowledge(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let Some(t) = self.trackers.get_mut(&id) {
            t.acknowledge();
        }
        self.waiting.retain(|&w| w != id);
        if self.waiting.is_empty() {
            self.pulse_since = None;
        }
        if let Some(wiring) = self.wiring.as_ref() {
            if let Some(files) = wiring.registry.read(cx).attention_files(id) {
                attention::acknowledge(&files.state_file);
            }
        }
    }

    fn drain_acks(&mut self, cx: &mut Context<Self>) {
        let Some(wiring) = self.wiring.as_ref() else {
            return;
        };
        let pending = wiring
            .state
            .clone()
            .update(cx, |s, _| s.take_pending_acks());
        if pending.is_empty() {
            return;
        }
        for id in pending {
            self.acknowledge(id, cx);
        }
        cx.notify();
    }

    fn spawn_pass(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(CLASSIFY_PERIOD).await;
            if this
                .update(cx, |this: &mut Self, cx| this.pass(cx))
                .is_err()
            {
                break;
            }
        })
    }

    /// One classification pass, ported from `tick.rs:111-287` in order.
    fn pass(&mut self, cx: &mut Context<Self>) {
        let Some(wiring) = self.wiring.as_ref() else {
            return;
        };
        let (state, registry) = (wiring.state.clone(), wiring.registry.clone());

        let active_session = state.read(cx).active_session();
        let window_focused = self.window_focused;
        let sessions: Vec<(SessionId, Agent, String, Option<_>)> = registry
            .read(cx)
            .all()
            .iter()
            .map(|m| {
                (
                    m.id,
                    m.agent,
                    m.wt_path.clone(),
                    m.attention.as_ref().map(|f| f.state_file.clone()),
                )
            })
            .collect();
        let entities: HashMap<SessionId, _> = sessions
            .iter()
            .filter_map(|(id, ..)| registry.read(cx).session(*id).map(|e| (*id, e.clone())))
            .collect();

        // Only worth polling while a live Claude session exists to inform (`tick.rs:120-124`).
        if let Some(poller) = self.poller.as_ref() {
            let any_claude = sessions
                .iter()
                .any(|(_, agent, ..)| *agent == Agent::Claude);
            poller.set_wanted(any_claude);
        }

        let mut newly_waiting = false;
        let mut changed = false;

        for (id, agent, wt_path, state_file) in &sessions {
            let Some(entity) = entities.get(id) else {
                continue;
            };
            let focused = active_session == Some(*id) && window_focused;

            let (alive, bells, output_age, title, scroll_age, input_age, root_pid) =
                entity.update(cx, |s, _| {
                    let alive = s.alive();
                    (
                        alive,
                        s.bell_count(),
                        s.output_age(),
                        if alive { s.title() } else { None },
                        s.scroll_age(),
                        s.input_age(),
                        s.root_pid(),
                    )
                });

            let tracker = self.trackers.entry(*id).or_default();
            // A counter that went backwards means the parser was reset — resync rather than go silent forever.
            if bells < tracker.bell_seen {
                tracker.bell_seen = bells;
            } else if bells > tracker.bell_seen {
                tracker.bell_seen = bells;
                if !focused {
                    tracker.bell_pending = true;
                }
            }

            let sig = Signals {
                alive,
                output_age,
                bell_pending: tracker.bell_pending,
                was_working: tracker.was_working,
                focused,
                scrolling: scroll_age.is_some_and(|a| a < SCROLL_QUIET),
                interacting: input_age.is_some_and(|a| a < INPUT_QUIET),
                title,
            };

            // Takes the term lock and copies 15 rows; higher-precedence signals discard it for most sessions, so lazy.
            let mut scrape = || {
                if alive {
                    entity.update(cx, |s, _| s.tail_contents(TAIL_ROWS))
                } else {
                    String::new()
                }
            };

            // Precedence: native poll > hook state file > screen scrape.
            let native = if alive && *agent == Agent::Claude {
                self.poller
                    .as_ref()
                    .and_then(|p| p.status_for(root_pid, wt_path))
            } else {
                None
            };
            let new_state = if let Some(status) = native {
                match status {
                    NativeStatus::Busy => ActivityState::Working,
                    // `Waiting` while focused is treated as already seen, mirroring the `NeedsYou` downgrade below.
                    NativeStatus::Waiting if !focused => ActivityState::WaitingForInput,
                    NativeStatus::Waiting => ActivityState::Working,
                    NativeStatus::Idle if sig.was_working => ActivityState::Done,
                    NativeStatus::Idle => ActivityState::Idle,
                }
            } else {
                // A dead process short-circuits to `classify` before the hook file, so a stale `working` reads `Exited`.
                let hook = if alive {
                    state_file.as_deref().and_then(attention::read_state)
                } else {
                    None
                };
                match (alive, hook) {
                    (true, Some(AttentionState::NeedsYou)) if !focused => {
                        ActivityState::WaitingForInput
                    }
                    (true, Some(AttentionState::NeedsYou | AttentionState::Working)) => {
                        ActivityState::Working
                    }
                    (true, Some(AttentionState::Done)) => {
                        resolve_done_hook(classify(*agent, &scrape(), &sig))
                    }
                    _ => classify(*agent, &scrape(), &sig),
                }
            };

            let tracker = self.trackers.entry(*id).or_default();
            if new_state == ActivityState::Working {
                tracker.was_working = true;
                // Unconditional, so a session that stays Working every tick keeps advancing last-active, not just on entry.
                tracker.last_active = Instant::now();
            }
            if !alive {
                tracker.was_working = false;
                tracker.bell_pending = false;
            }
            if focused {
                tracker.bell_pending = false;
            }
            if new_state == ActivityState::WaitingForInput
                && tracker.state != ActivityState::WaitingForInput
            {
                newly_waiting = true;
            }
            if tracker.state != new_state {
                changed = true;
                tracker.state = new_state;
                tracker.state_since = Instant::now();
            }
        }

        // Prune trackers to live ids: a killed session's tracker must not keep its `WaitingForInput` in the badge count.
        let before = self.trackers.len();
        self.trackers
            .retain(|id, _| sessions.iter().any(|(live, ..)| live == id));
        changed |= self.trackers.len() != before;

        // Tree order, not HashMap order (`update/mod.rs:728-740`); before the first rail paint it's empty, so fall back.
        let order = state.read(cx).visible_session_order().to_vec();
        let order = if order.is_empty() {
            sessions.iter().map(|(id, ..)| *id).collect::<Vec<_>>()
        } else {
            order
        };
        let waiting: Vec<SessionId> = order
            .into_iter()
            .filter(|id| self.state_of(*id) == ActivityState::WaitingForInput)
            .collect();
        changed |= waiting != self.waiting;
        self.waiting = waiting;

        if let Some(next) = badge_transition(self.last_badge, self.waiting.len()) {
            dock::set_badge(next);
            self.last_badge = next;
        }
        if self.waiting.is_empty() {
            self.pulse_since = None;
        } else if self.pulse_since.is_none() {
            self.pulse_since = Some(Instant::now());
        }
        if should_bounce(newly_waiting, window_focused) {
            dock::request_attention();
        }

        // An all-`Idle` pass on a quiet app must not cost a frame; an active pulse always repaints — that's the animation.
        if changed || self.pulse_since.is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "{a} != {b}");
    }

    #[test]
    fn nothing_waiting_means_a_constant_zero_pulse() {
        let now = Instant::now();
        approx(pulse_at(None, now), 0.0);
        approx(pulse_at(None, now + Duration::from_millis(700)), 0.0);
    }

    /// `update/mod.rs:246-252`: 1000ms, `EaseInOut`, auto-reverse — a ~2s round trip.
    #[test]
    fn the_pulse_is_an_ease_in_out_triangle_wave() {
        let t0 = Instant::now();
        let at = |ms: u64| pulse_at(Some(t0), t0 + Duration::from_millis(ms));
        approx(at(0), 0.0);
        approx(at(500), 0.5);
        approx(at(1000), 1.0);
        approx(at(1500), 0.5);
        approx(at(2000), 0.0);
        approx(at(3000), 1.0);
    }

    #[test]
    fn the_pulse_is_monotone_on_each_half() {
        let t0 = Instant::now();
        let at = |ms: u64| pulse_at(Some(t0), t0 + Duration::from_millis(ms));
        for ms in (0..1000).step_by(25) {
            assert!(at(ms) <= at(ms + 25), "rising at {ms}ms");
        }
        for ms in (1000..2000).step_by(25) {
            assert!(at(ms) >= at(ms + 25), "falling at {ms}ms");
        }
    }

    #[test]
    fn the_pulse_never_leaves_the_unit_interval() {
        let t0 = Instant::now();
        for ms in (0..6000).step_by(17) {
            let v = pulse_at(Some(t0), t0 + Duration::from_millis(ms));
            assert!((0.0..=1.0).contains(&v), "{v} at {ms}ms");
        }
    }

    /// `tick.rs:272-275` — the dock is touched only on a real change.
    #[test]
    fn the_badge_is_pushed_only_when_the_count_changes() {
        assert_eq!(badge_transition(0, 0), None);
        assert_eq!(badge_transition(0, 2), Some(2));
        assert_eq!(badge_transition(2, 2), None);
        assert_eq!(badge_transition(2, 0), Some(0));
    }

    /// `tick.rs:256-260,284-286`.
    #[test]
    fn the_bounce_needs_a_fresh_waiting_edge_and_an_unfocused_window() {
        assert!(should_bounce(true, false));
        assert!(!should_bounce(true, true), "never while focused");
        assert!(
            !should_bounce(false, false),
            "never for an already-waiting one"
        );
        assert!(!should_bounce(false, true));
    }

    #[test]
    fn an_inert_store_classifies_everything_idle_with_no_pulse() {
        let store = ActivityStore::new();
        assert_eq!(store.state_of(SessionId::from_raw(1)), ActivityState::Idle);
        approx(store.pulse(), 0.0);
        assert!(store.waiting_sessions().is_empty());
    }

    /// Nothing waiting, window unfocused, no dirty PTYs => the frame clock stays in its 1s lane.
    #[test]
    fn an_idle_app_with_nothing_waiting_stays_on_the_slow_clock() {
        use crate::entities::animation_clock::is_fast;
        let waiting = 0usize;
        assert!(!is_fast(false, true, false, waiting > 0, false));
        // ...and a single waiting session is what wakes it up.
        assert!(is_fast(false, true, false, 1 > 0, false));
    }

    #[test]
    fn a_done_hook_yields_to_fresh_codex_work() {
        let signals = Signals {
            alive: true,
            output_age: Duration::from_secs(1),
            bell_pending: false,
            was_working: false,
            focused: false,
            scrolling: false,
            interacting: false,
            title: None,
        };
        let live = classify(Agent::Codex, "esc to interrupt", &signals);
        assert_eq!(resolve_done_hook(live), ActivityState::Working);
        assert_eq!(
            resolve_done_hook(ActivityState::WaitingForInput),
            ActivityState::WaitingForInput
        );
    }

    #[test]
    fn a_done_hook_survives_stale_quiet_codex_output() {
        let signals = Signals {
            alive: true,
            output_age: Duration::MAX,
            bell_pending: false,
            was_working: false,
            focused: false,
            scrolling: false,
            interacting: false,
            title: None,
        };
        let live = classify(Agent::Codex, "", &signals);
        assert_eq!(live, ActivityState::Idle);
        assert_eq!(resolve_done_hook(live), ActivityState::Done);
    }
}
