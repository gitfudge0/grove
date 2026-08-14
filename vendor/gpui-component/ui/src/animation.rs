use std::{rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt, ElementId, Hsla, IntoElement, Pixels, Point, Styled, point,
    prelude::FluentBuilder, px,
};
use smallvec::SmallVec;

/// Like CSS `cubic-bezier`; build curves at https://cubic-bezier.com
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 {
    move |t: f32| {
        let one_t = 1.0 - t;
        let one_t2 = one_t * one_t;
        let t2 = t * t;
        let t3 = t2 * t;

        // x0 = 0, y0 = 0, x3 = 1, y3 = 1.
        let _x = 3.0 * x1 * one_t2 * t + 3.0 * x2 * one_t * t2 + t3;
        let y = 3.0 * y1 * one_t2 * t + 3.0 * y2 * one_t * t2 + t3;

        y
    }
}

/// Fast start, slow end — good for enter animations.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Slow start, fast end — good for exit animations.
pub fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

/// Slow start and end — good for position transitions.
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

pub trait Lerp: Clone {
    fn lerp(&self, target: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(&self, target: &Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Pixels {
    fn lerp(&self, target: &Self, t: f32) -> Self {
        let a: f32 = (*self).into();
        let b: f32 = (*target).into();
        px(a + (b - a) * t)
    }
}

impl Lerp for Point<Pixels> {
    fn lerp(&self, target: &Self, t: f32) -> Self {
        point(
            Lerp::lerp(&self.x, &target.x, t),
            Lerp::lerp(&self.y, &target.y, t),
        )
    }
}

impl Lerp for Hsla {
    /// Linear per-channel; fine for near-grayscale UI colors where hue interpolation doesn't matter.
    fn lerp(&self, target: &Self, t: f32) -> Self {
        Hsla {
            h: self.h.lerp(&target.h, t),
            s: self.s.lerp(&target.s, t),
            l: self.l.lerp(&target.l, t),
            a: self.a.lerp(&target.a, t),
        }
    }
}

/// ```ignore
/// Transition::new(Duration::from_millis(150))
///     .ease(ease_out_cubic)
///     .slide_y(px(-4.), px(0.))
///     .fade(0.0, 1.0)
///     .apply(element, "enter-anim")
/// ```
#[derive(Clone)]
pub struct Transition {
    pub duration: Duration,
    easing: Rc<dyn Fn(f32) -> f32>,
    effects: SmallVec<[TransitionEffect; 2]>,
}

#[derive(Clone, Copy)]
enum TransitionEffect {
    SlideY(Pixels, Pixels),
    SlideX(Pixels, Pixels),
    Fade(f32, f32),
    Width(Pixels, Pixels),
    Height(Pixels, Pixels),
}

impl Transition {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            easing: Rc::new(ease_out_cubic),
            effects: SmallVec::new(),
        }
    }

    pub fn ease(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }

    pub fn slide_y(mut self, from: Pixels, to: Pixels) -> Self {
        self.effects.push(TransitionEffect::SlideY(from, to));
        self
    }

    pub fn slide_x(mut self, from: Pixels, to: Pixels) -> Self {
        self.effects.push(TransitionEffect::SlideX(from, to));
        self
    }

    pub fn fade(mut self, from: f32, to: f32) -> Self {
        self.effects.push(TransitionEffect::Fade(from, to));
        self
    }

    pub fn width(mut self, from: Pixels, to: Pixels) -> Self {
        self.effects.push(TransitionEffect::Width(from, to));
        self
    }

    pub fn height(mut self, from: Pixels, to: Pixels) -> Self {
        self.effects.push(TransitionEffect::Height(from, to));
        self
    }

    pub fn apply<E: IntoElement + Styled + 'static>(
        self,
        element: E,
        id: impl Into<ElementId>,
    ) -> gpui::AnimationElement<E> {
        let animation = Animation::new(self.duration).with_easing({
            let easing = self.easing.clone();
            move |t| easing(t)
        });
        let effects = self.effects;
        element.with_animation(id, animation, move |el, delta| {
            let mut el = el;
            for effect in &effects {
                match effect {
                    TransitionEffect::SlideY(from, to) => {
                        el = el.top(Lerp::lerp(from, to, delta));
                    }
                    TransitionEffect::SlideX(from, to) => {
                        el = el.left(Lerp::lerp(from, to, delta));
                    }
                    TransitionEffect::Fade(from, to) => {
                        el = el.opacity(Lerp::lerp(from, to, delta));
                    }
                    TransitionEffect::Width(from, to) => {
                        el = el.w(Lerp::lerp(from, to, delta));
                    }
                    TransitionEffect::Height(from, to) => {
                        el = el.h(Lerp::lerp(from, to, delta));
                    }
                }
            }
            el
        })
    }
}

impl FluentBuilder for Transition {}
