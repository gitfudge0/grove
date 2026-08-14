//! The `blink_tick` replacement: one monotonic counter, one adaptive timer. Every blink phase in the app derives from this **single** counter, so the phase relationships between the cursor, the dots and the spinner match the iced app exactly, and an idle window costs one wakeup per second. Not everything animated is clock-derived: the attention amber pulse (1s auto-reverse EaseInOut) and the onboarding entrance map to gpui's `with_animation` instead (spec §4). Do **not** wire them to this tick.

use std::time::Duration;

use gpui::{Context, Task};

/// Fast lane: the 60ms cadence the iced app uses whenever anything is moving (`src/gui/update/mod.rs:368-390`).
pub const FAST: Duration = Duration::from_millis(60);
/// Slow lane: 1s. An idle window still ticks so time-based labels stay fresh, but at 1/16th the wakeups.
pub const SLOW: Duration = Duration::from_secs(1);

/// The gating predicate, verbatim from `src/gui/update/mod.rs:420-435`: 60ms when `busy || (has_ptys && (focused || animating || dirty))`, else 1s. Getting this wrong is an idle-power regression — the spikes measured release idle at 1.23% against Grove's ~3.7%.
pub fn is_fast(busy: bool, has_ptys: bool, focused: bool, animating: bool, dirty: bool) -> bool {
    busy || (has_ptys && (focused || animating || dirty))
}

/// The timer period implied by the gating inputs.
// Exercised only by this module's `#[cfg(test)]` cadence table; the live clock calls `is_fast` directly and stores the period it derived.
#[allow(dead_code)]
pub fn cadence(
    busy: bool,
    has_ptys: bool,
    focused: bool,
    animating: bool,
    dirty: bool,
) -> Duration {
    if is_fast(busy, has_ptys, focused, animating, dirty) {
        FAST
    } else {
        SLOW
    }
}

// ── derived phases (pure functions of the counter) ───────────────────────

/// Cursor blink. At 60ms/beat this is a 960ms period, 480ms on / 480ms off. The *formula* is the parity contract with the iced app (`src/gui/view/terminal.rs:665`) — never re-derive it from the 533ms figure quoted in `src/gui/state.rs:296`.
pub fn cursor_visible(tick: u64) -> bool {
    tick % 16 < 8
}

/// The 3-dot "thinking" animation (`src/gui/view/terminal.rs:540`).
pub fn dots(tick: u64) -> u64 {
    (tick / 5) % 3
}

/// Toast pulse phase (`src/gui/view/terminal.rs:1098`).
pub fn toast_pulse(tick: u64) -> u64 {
    tick % 40
}

/// Number of pre-rotated spinner frames (`src/gui/icons.rs:48`).
pub const SPINNER_FRAMES: u64 = 12;

/// Working/loading spinner: one of `SPINNER_FRAMES` fixed steps every 3 ticks (`src/gui/icons.rs:70`).
pub fn spinner_frame(tick: u64) -> u64 {
    (tick / 3) % SPINNER_FRAMES
}

// ── the entity ───────────────────────────────────────────────────────────

pub struct AnimationClock {
    tick: u64,
    fast: bool,
    /// Dropping a `Task` cancels it, so this field *is* the running timer.
    timer: Task<()>,
}

impl AnimationClock {
    /// Starts in the slow lane: at startup there are no PTYs yet.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tick: 0,
            fast: false,
            timer: Self::spawn_timer(false, cx),
        }
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Recomputes the cadence from the gating inputs; restarts the timer only when the cadence actually changes (restarting every frame would defeat the whole point of the slow lane).
    pub fn set_busy_inputs(
        &mut self,
        busy: bool,
        has_ptys: bool,
        focused: bool,
        animating: bool,
        dirty: bool,
        cx: &mut Context<Self>,
    ) {
        let fast = is_fast(busy, has_ptys, focused, animating, dirty);
        if fast == self.fast {
            return;
        }
        self.fast = fast;
        self.timer = Self::spawn_timer(fast, cx);
    }

    fn spawn_timer(fast: bool, cx: &mut Context<Self>) -> Task<()> {
        let period = if fast { FAST } else { SLOW };
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(period).await;
            // Every observer repaints off `cx.notify()`; nothing polls the counter. An `Err` here means the entity is gone — stop.
            if this
                .update(cx, |this: &mut Self, cx| {
                    this.tick = this.tick.wrapping_add(1);
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_period_and_duty_cycle() {
        let on = (0..16).filter(|t| cursor_visible(*t)).count();
        assert_eq!(on, 8, "50% duty cycle");
        for t in 0..240u64 {
            assert_eq!(cursor_visible(t), cursor_visible(t + 16), "16-tick period");
        }
        assert!(cursor_visible(0));
        assert!(!cursor_visible(8));
    }

    #[test]
    fn dots_cycle_three_phases_every_five_ticks() {
        assert_eq!(dots(0), 0);
        assert_eq!(dots(4), 0);
        assert_eq!(dots(5), 1);
        assert_eq!(dots(10), 2);
        assert_eq!(dots(15), 0);
        for t in 0..240u64 {
            assert_eq!(dots(t), dots(t + 15), "15-tick period");
            assert!(dots(t) < 3);
        }
    }

    /// The two phases share one counter, so their relationship repeats on `lcm(16, 15) = 240` ticks — the property that would break the instant someone gave a phase its own timer.
    #[test]
    fn cursor_and_dots_keep_their_phase_relationship() {
        for t in 0..240u64 {
            assert_eq!(
                (cursor_visible(t), dots(t)),
                (cursor_visible(t + 240), dots(t + 240))
            );
        }
    }

    #[test]
    fn toast_pulse_has_a_forty_tick_period() {
        assert_eq!(toast_pulse(0), 0);
        assert_eq!(toast_pulse(39), 39);
        for t in 0..240u64 {
            assert_eq!(toast_pulse(t), toast_pulse(t + 40));
            assert!(toast_pulse(t) < 40);
        }
    }

    #[test]
    fn spinner_advances_one_frame_every_three_ticks() {
        assert_eq!(spinner_frame(0), 0);
        assert_eq!(spinner_frame(2), 0);
        assert_eq!(spinner_frame(3), 1);
        assert_eq!(spinner_frame(35), 11);
        assert_eq!(spinner_frame(36), 0);
        for t in 0..240u64 {
            assert!(spinner_frame(t) < SPINNER_FRAMES);
        }
    }

    /// Full truth table of the five gating inputs.
    #[test]
    fn cadence_truth_table() {
        for bits in 0..32u8 {
            let busy = bits & 1 != 0;
            let has_ptys = bits & 2 != 0;
            let focused = bits & 4 != 0;
            let animating = bits & 8 != 0;
            let dirty = bits & 16 != 0;
            let expected = busy || (has_ptys && (focused || animating || dirty));
            assert_eq!(
                is_fast(busy, has_ptys, focused, animating, dirty),
                expected,
                "bits={bits:05b}"
            );
            assert_eq!(
                cadence(busy, has_ptys, focused, animating, dirty),
                if expected { FAST } else { SLOW },
                "bits={bits:05b}"
            );
        }
    }

    #[test]
    fn ptys_without_a_reason_stay_slow() {
        assert_eq!(cadence(false, true, false, false, false), SLOW);
        // ...and a reason without PTYs stays slow too, unless we're busy.
        assert_eq!(cadence(false, false, true, true, true), SLOW);
        assert_eq!(cadence(true, false, false, false, false), FAST);
    }
}
