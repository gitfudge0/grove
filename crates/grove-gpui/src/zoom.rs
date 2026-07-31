//! UI zoom: the clamp/step table and the `set_rem_size` application point.

// The zoomed cell metrics are Plan 04's inputs; they have no caller until the
// terminal element lands.
#![allow(dead_code)]

use crate::fonts;

/// Bounds, step and reset target, from `src/gui/metrics.rs:46-49`.
pub const ZOOM_DEFAULT: f32 = 1.0;
pub const ZOOM_MIN: f32 = 0.6;
pub const ZOOM_MAX: f32 = 2.0;
pub const ZOOM_STEP: f32 = 0.1;

/// gpui's default rem size. Zoom is applied as `set_rem_size(px(REM_BASE *
/// zoom))` once per frame in the root view's render, so every `rems()`-styled
/// piece of chrome scales off that single call.
///
/// `WithRemSize` does **not** exist at this rev — `Window::with_rem_size`
/// exists for scoped overrides, but the shell needs the global one.
pub const REM_BASE: f32 = 16.0;

pub struct ZoomState {
    pub zoom: f32,
}

impl gpui::Global for ZoomState {}

impl ZoomState {
    pub fn new(zoom: f32) -> Self {
        Self { zoom: snap(zoom) }
    }

    /// Zoomed cell width.
    ///
    /// Plan 04's terminal element derives PTY dimensions from its own
    /// post-layout bounds in `prepaint`:
    /// `cols = (bounds.size.width / cell_w()).floor().max(1.0)` (and the
    /// same for rows against `cell_h()`). The iced side's `compute_pty_dims`
    /// chrome-subtraction arithmetic is **superseded** by gpui layout
    /// (findings amendment 7) and must not be ported.
    pub fn cell_w(&self) -> f32 {
        fonts::CELL_W * self.zoom
    }

    pub fn cell_h(&self) -> f32 {
        fonts::CELL_H * self.zoom
    }

    pub fn font_size(&self) -> f32 {
        fonts::FONT_SIZE * self.zoom
    }

    /// The rem size this zoom level implies.
    pub fn rem_size(&self) -> f32 {
        REM_BASE * self.zoom
    }
}

/// Clamp then snap to the 0.1 grid, then clamp again — verbatim behavior from
/// `set_ui_zoom` (`src/gui/update/layout.rs:495-497`). The second clamp
/// matters: rounding can push a clamped endpoint back out of range.
pub fn snap(zoom: f32) -> f32 {
    let clamped = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    ((clamped * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// One step of zoom in the given direction.
pub fn step(zoom: f32, delta: f32) -> f32 {
    snap(zoom + delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_steps_down_clamps_at_min() {
        let mut z = ZOOM_DEFAULT;
        for _ in 0..9 {
            z = step(z, -ZOOM_STEP);
        }
        assert_eq!(z, ZOOM_MIN);
    }

    #[test]
    fn ten_steps_up_clamps_at_max() {
        let mut z = ZOOM_DEFAULT;
        for _ in 0..10 {
            z = step(z, ZOOM_STEP);
        }
        assert_eq!(z, ZOOM_MAX);
    }

    #[test]
    fn reset_returns_to_default() {
        assert_eq!(snap(ZOOM_DEFAULT), 1.0);
    }

    #[test]
    fn no_step_escapes_the_range() {
        let mut z = ZOOM_MIN;
        for _ in 0..40 {
            z = step(z, ZOOM_STEP);
            assert!((ZOOM_MIN..=ZOOM_MAX).contains(&z), "{z} out of range");
        }
        for _ in 0..40 {
            z = step(z, -ZOOM_STEP);
            assert!((ZOOM_MIN..=ZOOM_MAX).contains(&z), "{z} out of range");
        }
    }

    #[test]
    fn snap_rounds_to_the_tenth() {
        assert_eq!(snap(1.234), 1.2);
        assert_eq!(snap(1.26), 1.3);
        assert_eq!(snap(-5.0), ZOOM_MIN);
        assert_eq!(snap(99.0), ZOOM_MAX);
    }

    #[test]
    fn cell_metrics_scale_with_zoom() {
        let s = ZoomState::new(2.0);
        assert_eq!(s.cell_w(), fonts::CELL_W * 2.0);
        assert_eq!(s.cell_h(), fonts::CELL_H * 2.0);
        assert_eq!(s.font_size(), fonts::FONT_SIZE * 2.0);
        assert_eq!(s.rem_size(), 32.0);
    }

    #[test]
    fn constructor_snaps_a_junk_persisted_value() {
        assert_eq!(ZoomState::new(17.3).zoom, ZOOM_MAX);
        assert_eq!(ZoomState::new(0.0).zoom, ZOOM_MIN);
    }
}
