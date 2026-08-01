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

    /// Logical PTY grid for an element laid out at `bounds` — findings
    /// amendment 7. `compute_pty_dims`'s chrome subtraction
    /// (`src/gui/metrics.rs:265-295`) is superseded by gpui layout: the
    /// element's own post-layout bounds already exclude every piece of chrome,
    /// so there is nothing left to subtract.
    ///
    /// Returns `(rows, cols)`. Degenerate bounds (zero, negative, NaN) clamp to
    /// a 1x1 grid — a PTY may never be sized 0.
    pub fn pty_dims(&self, width_px: f32, height_px: f32) -> (u16, u16) {
        (fit(height_px, self.cell_h()), fit(width_px, self.cell_w()))
    }

    /// The rem size this zoom level implies.
    pub fn rem_size(&self) -> f32 {
        REM_BASE * self.zoom
    }
}

/// The live single-session body dims, published by the workspace each render
/// so session spawns can size their PTY correctly from the first byte
/// instead of flashing 24x80.
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

/// How many whole `cell`-sized slots fit in `extent`, clamped to `1..=u16::MAX`.
/// NaN and non-positive inputs collapse to 1.
fn fit(extent: f32, cell: f32) -> u16 {
    if !cell.is_finite() || cell <= 0.0 {
        return 1;
    }
    // NaN fails every comparison, so it falls through both guards to the
    // clamp below.
    let n = (extent / cell).floor();
    if n.is_nan() || n < 1.0 {
        return 1;
    }
    if n >= f32::from(u16::MAX) {
        return u16::MAX;
    }
    n as u16
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
    fn pty_dims_at_the_default_window_size() {
        // 1280x800 (main.rs WINDOW_W/H) at zoom 1.0: 800/17 = 47.05 -> 47,
        // 1280/7.5 = 170.67 -> 170.
        assert_eq!(ZoomState::new(1.0).pty_dims(1280.0, 800.0), (47, 170));
    }

    #[test]
    fn pty_dims_clamp_degenerate_bounds() {
        let z = ZoomState::new(1.0);
        assert_eq!(z.pty_dims(0.0, 0.0), (1, 1));
        assert_eq!(z.pty_dims(-100.0, -100.0), (1, 1));
        assert_eq!(z.pty_dims(f32::NAN, f32::NAN), (1, 1));
        assert_eq!(z.pty_dims(f32::INFINITY, f32::NAN), (1, u16::MAX));
        // A bound smaller than one cell is still a 1x1 grid, never 0.
        assert_eq!(z.pty_dims(3.0, 4.0), (1, 1));
    }

    #[test]
    fn pty_dims_halve_at_double_zoom() {
        // Floor, not round: 800/34 = 23.52 -> 23 (not 24), 1280/15 = 85.33 -> 85.
        assert_eq!(ZoomState::new(2.0).pty_dims(1280.0, 800.0), (23, 85));
    }

    #[test]
    fn pty_dims_floor_a_fractional_cell() {
        // zoom 0.6: cell_w = 4.5, cell_h = 10.2. 1000/4.5 = 222.2 -> 222,
        // 1000/10.2 = 98.03 -> 98. The fractional remainder is dropped.
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
