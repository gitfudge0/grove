//! UI zoom: the clamp/step table and the `set_rem_size` application point.

use crate::fonts;

pub const ZOOM_DEFAULT: f32 = 1.0;
pub const ZOOM_MIN: f32 = 0.6;
pub const ZOOM_MAX: f32 = 2.0;
pub const ZOOM_STEP: f32 = 0.1;

/// Applied as `set_rem_size(px(REM_BASE * zoom * text_scale))` once per frame in the root view's render.
pub const REM_BASE: f32 = 16.0;

pub struct ZoomState {
    pub zoom: f32,
    text_scale: f32,
}

impl gpui::Global for ZoomState {}

impl ZoomState {
    pub fn new(zoom: f32) -> Self {
        Self {
            zoom: snap(zoom),
            text_scale: 1.0,
        }
    }

    /// OS text scaling is independent of the persisted, clamped app zoom.
    pub fn with_text_scale(mut self, factor: f32) -> Self {
        self.text_scale = if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        };
        self
    }

    pub fn effective_scale(&self) -> f32 {
        self.zoom * self.text_scale
    }

    pub fn cell_w(&self) -> f32 {
        fonts::CELL_W * self.effective_scale()
    }

    pub fn cell_h(&self) -> f32 {
        fonts::CELL_H * self.effective_scale()
    }

    pub fn font_size(&self) -> f32 {
        fonts::FONT_SIZE * self.effective_scale()
    }

    /// Returns `(rows, cols)`; degenerate bounds (zero, negative, NaN) clamp to a 1x1 grid — a PTY may never be sized 0.
    pub fn pty_dims(&self, width_px: f32, height_px: f32) -> (u16, u16) {
        (fit(height_px, self.cell_h()), fit(width_px, self.cell_w()))
    }

    pub fn rem_size(&self) -> f32 {
        REM_BASE * self.effective_scale()
    }
}

/// Published each render so session spawns can size their PTY correctly from the first byte instead of flashing 24x80.
#[derive(Clone, Copy)]
pub struct CurrentPtyDims {
    pub rows: u16,
    pub cols: u16,
}

impl gpui::Global for CurrentPtyDims {}

impl Default for CurrentPtyDims {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// Clamped to `1..=u16::MAX`; NaN and non-positive inputs collapse to 1.
fn fit(extent: f32, cell: f32) -> u16 {
    if !cell.is_finite() || cell <= 0.0 {
        return 1;
    }
    let n = (extent / cell).floor();
    if n.is_nan() || n < 1.0 {
        return 1;
    }
    if n >= f32::from(u16::MAX) {
        return u16::MAX;
    }
    n as u16
}

/// Second clamp matters: rounding can push a clamped endpoint back out of range.
pub fn snap(zoom: f32) -> f32 {
    let clamped = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    ((clamped * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

// Exercised only by this module's clamp-ladder tests.
#[allow(dead_code)]
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
    fn pty_dims_at_the_default_window_size() {
        assert_eq!(ZoomState::new(1.0).pty_dims(1280.0, 800.0), (47, 170));
    }

    #[test]
    fn pty_dims_clamp_degenerate_bounds() {
        let z = ZoomState::new(1.0);
        assert_eq!(z.pty_dims(0.0, 0.0), (1, 1));
        assert_eq!(z.pty_dims(-100.0, -100.0), (1, 1));
        assert_eq!(z.pty_dims(f32::NAN, f32::NAN), (1, 1));
        assert_eq!(z.pty_dims(f32::INFINITY, f32::NAN), (1, u16::MAX));
        assert_eq!(z.pty_dims(3.0, 4.0), (1, 1));
    }

    #[test]
    fn pty_dims_halve_at_double_zoom() {
        assert_eq!(ZoomState::new(2.0).pty_dims(1280.0, 800.0), (23, 85));
    }

    #[test]
    fn pty_dims_floor_a_fractional_cell() {
        assert_eq!(ZoomState::new(0.6).pty_dims(1000.0, 1000.0), (98, 222));
    }

    #[test]
    fn pty_dims_never_exceed_u16() {
        let z = ZoomState::new(1.0);
        let (rows, cols) = z.pty_dims(1e30, 1e30);
        assert_eq!((rows, cols), (u16::MAX, u16::MAX));
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

#[cfg(test)]
mod text_scale_tests {
    use super::*;
    #[test]
    fn os_scale_is_not_snapped_or_clamped_as_app_zoom() {
        let state = ZoomState::new(1.5).with_text_scale(0.8182);
        assert_eq!(state.zoom, 1.5);
        assert_eq!(state.rem_size(), 16.0 * 1.5 * 0.8182);
        assert_eq!(state.cell_w(), fonts::CELL_W * state.effective_scale());
        assert_eq!(
            state.font_size(),
            fonts::FONT_SIZE * state.effective_scale()
        );
        assert_eq!(
            ZoomState::new(2.).with_text_scale(1.5).effective_scale(),
            3.
        );
    }
    #[test]
    fn invalid_text_scales_fall_back() {
        for scale in [0., -1., f32::NAN, f32::INFINITY] {
            assert_eq!(ZoomState::new(1.).with_text_scale(scale).rem_size(), 16.);
        }
    }
}
