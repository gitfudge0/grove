//! Shared, quiet motion for application chrome.
//!
//! Elements are keyed by the caller to the state that caused their entrance.
//! Keeping that key stable across ordinary redraws lets GPUI retain the clock;
//! a real state change gets a new key and starts one short transition.

use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement, App, ElementId, Hsla, IntoElement, Styled};

use super::tokens::{MOTION_BASE_MS, MOTION_FADE_FROM, MOTION_FAST_MS, MOTION_SLOW_MS};

pub fn fast<E>(element: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    fade_in(element, id, Duration::from_millis(MOTION_FAST_MS), cx)
}

pub fn base<E>(element: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    fade_in(element, id, Duration::from_millis(MOTION_BASE_MS), cx)
}

pub fn slow<E>(element: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    fade_in(element, id, Duration::from_millis(MOTION_SLOW_MS), cx)
}

/// Fade newly mounted app chrome in from a restrained opacity. GPUI's
/// `with_animation` respects its reduced-motion flag; the explicit branch also
/// avoids creating animation state or requesting frames when that flag is on.
pub fn fade_in<E>(element: E, id: impl Into<ElementId>, duration: Duration, cx: &App) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    if cx.reduce_motion() {
        element.into_any_element()
    } else {
        element
            .with_animation(
                id,
                Animation::new(duration).with_easing(ease_standard),
                |el, t| el.opacity(MOTION_FADE_FROM + (1.0 - MOTION_FADE_FROM) * t),
            )
            .into_any_element()
    }
}

/// Interpolate a semantic background fill when a real row state changes.
pub fn background<E>(
    element: E,
    id: impl Into<ElementId>,
    from: Hsla,
    to: Hsla,
    duration: Duration,
    cx: &App,
) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    if cx.reduce_motion() {
        element.into_any_element()
    } else {
        element
            .with_animation(
                id,
                Animation::new(duration).with_easing(ease_standard),
                move |el, t| el.bg(lerp_color(from, to, t)),
            )
            .into_any_element()
    }
}

/// CSS cubic-bezier(.2,.8,.2,1), solved on x so progress follows elapsed time.
pub(super) fn ease_standard(progress: f32) -> f32 {
    cubic_bezier(progress, 0.2, 0.8, 0.2, 1.0)
}

fn cubic_bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let t = (low + high) * 0.5;
        if bezier_axis(t, x1, x2) < x {
            low = t;
        } else {
            high = t;
        }
    }
    bezier_axis((low + high) * 0.5, y1, y2)
}

fn bezier_axis(t: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}

fn lerp_color(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let hue_delta = ((to.h - from.h + 0.5).rem_euclid(1.0)) - 0.5;
    Hsla {
        h: (from.h + hue_delta * t).rem_euclid(1.0),
        s: from.s + (to.s - from.s) * t,
        l: from.l + (to.l - from.l) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// Interruptible mount/unmount fade. Owners retain exiting content until `duration`
/// elapses, but release its input ownership immediately on dismissal.
#[derive(Clone, Copy, Debug)]
pub struct Presence {
    started: std::time::Instant,
    from: f32,
    to: f32,
    revision: u64,
    transition_duration: Duration,
}
impl Default for Presence {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            from: 0.,
            to: 0.,
            revision: 0,
            transition_duration: Self::duration(),
        }
    }
}
impl Presence {
    pub fn fast() -> Self {
        Self {
            transition_duration: Duration::from_millis(MOTION_FAST_MS),
            ..Self::default()
        }
    }
    pub fn transition_duration(&self) -> Duration {
        self.transition_duration
    }
    pub fn set(&mut self, visible: bool, cx: &App) {
        let from = self.opacity();
        self.from = if cx.reduce_motion() {
            visible as u8 as f32
        } else {
            from
        };
        self.to = visible as u8 as f32;
        self.started = std::time::Instant::now();
        self.revision = self.revision.wrapping_add(1);
    }
    fn opacity(&self) -> f32 {
        let t =
            (self.started.elapsed().as_secs_f32() / self.transition_duration.as_secs_f32()).min(1.);
        self.from + (self.to - self.from) * ease_standard(t)
    }
    pub fn duration() -> Duration {
        Duration::from_millis(MOTION_BASE_MS)
    }
    pub fn render<E>(&self, element: E, id: &str, cx: &App) -> AnyElement
    where
        E: IntoElement + Styled + 'static,
    {
        if cx.reduce_motion() {
            return element.opacity(self.to).into_any_element();
        }
        let from = self.from;
        let to = self.to;
        element
            .with_animation(
                format!("{id}-{}", self.revision),
                Animation::new(self.transition_duration).with_easing(ease_standard),
                move |el, t| el.opacity(from + (to - from) * t),
            )
            .into_any_element()
    }
}
