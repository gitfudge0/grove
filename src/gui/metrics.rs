//! Layout constants and PTY cell metrics shared across the GUI.

use iced::Font;

pub const ROW_H: f32 = 28.0;
pub const RAIL_W: f32 = 320.0;
pub const APPBAR_H: f32 = 44.0;
pub const STATUS_H: f32 = 26.0;
pub const SESSBAR_H: f32 = 36.0;
pub const SIDEBAR_DIVIDER_W: f32 = 1.0;
pub const PTY_PAD_W: f32 = 36.0;
pub const PTY_PAD_H: f32 = 28.0;

/// Hardcoded cell metrics for IBM Plex Mono at 12.5pt. iced doesn't give
/// us a cheap way to measure glyphs from outside a frame, so these are pinned
/// — the canvas just maps (row, col) → pixel position. Re-tune if the font
/// size or family changes.
pub const CELL_W: f32 = 7.6;
pub const CELL_H: f32 = 17.0;
pub const FONT_SIZE: f32 = 12.5;

pub const PTY_ZOOM_DEFAULT: f32 = 1.0;
pub const PTY_ZOOM_MIN: f32 = 0.6;
pub const PTY_ZOOM_MAX: f32 = 2.0;
pub const PTY_ZOOM_STEP: f32 = 0.1;

#[derive(Clone, Copy)]
pub struct PtyMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
}

pub fn pty_metrics(zoom: f32) -> PtyMetrics {
    PtyMetrics {
        cell_w: CELL_W * zoom,
        cell_h: CELL_H * zoom,
    }
}

/// IBM Plex Mono — used for PTY output and any monospace-sensitive content.
/// Bundled as TTF and registered at startup in `gui::run`.
pub const MONO_FONT: Font = Font::with_name("IBM Plex Mono");

/// IBM Plex Sans — sole UI font used throughout the chrome (sidebar, appbar,
/// modals, session bar). Anything that renders code, paths, or PTY output
/// uses `MONO_FONT` instead.
pub const UI_FONT: Font = Font::with_name("IBM Plex Sans");
pub const UI_BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::with_name("IBM Plex Sans")
};

/// Embedded font bytes, registered with iced at application startup so the
/// `with_name(...)` lookups above resolve to bundled glyphs rather than a
/// system fallback.
pub const PLEX_SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf");
pub const PLEX_SANS_BOLD: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Bold.ttf");
pub const PLEX_MONO_REGULAR: &[u8] = include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf");
pub const PLEX_MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/IBMPlexMono-Bold.ttf");

/// PTY dimensions derived from window pixel size. Subtracts the visible chrome
/// (rail, dividers, appbar, statusbar, sessbar, container padding) and divides
/// by the cell metrics.
pub fn compute_pty_dims(win_w: f32, win_h: f32, zoom: f32, chrome_visible: bool) -> (u16, u16) {
    let metrics = pty_metrics(zoom);
    let visible_w = if chrome_visible {
        RAIL_W + SIDEBAR_DIVIDER_W
    } else {
        0.0
    };
    let visible_h = if chrome_visible {
        APPBAR_H + STATUS_H
    } else {
        0.0
    };
    let usable_w = win_w - zoom * (visible_w + PTY_PAD_W);
    let usable_h = win_h - zoom * (visible_h + SESSBAR_H + PTY_PAD_H);
    let cols = (usable_w / metrics.cell_w).max(10.0) as u16;
    let rows = (usable_h / metrics.cell_h).max(4.0) as u16;
    (rows, cols)
}

#[cfg(test)]
mod tests {
    use super::{
        compute_pty_dims, APPBAR_H, CELL_H, CELL_W, PTY_PAD_H, PTY_PAD_W, RAIL_W, SESSBAR_H,
        SIDEBAR_DIVIDER_W, STATUS_H,
    };

    #[test]
    fn compute_pty_dims_scales_chrome_with_zoom() {
        let win_w = 1280.0;
        let win_h = 800.0;
        let zoom = 1.5;

        let usable_w = win_w - zoom * (RAIL_W + SIDEBAR_DIVIDER_W + PTY_PAD_W);
        let usable_h = win_h - zoom * (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);
        let expected_cols = (usable_w / (CELL_W * zoom)).max(10.0) as u16;
        let expected_rows = (usable_h / (CELL_H * zoom)).max(4.0) as u16;

        assert_eq!(
            compute_pty_dims(win_w, win_h, zoom, true),
            (expected_rows, expected_cols)
        );
    }

    #[test]
    fn compute_pty_dims_enforces_minimum_size() {
        assert_eq!(compute_pty_dims(100.0, 100.0, 2.0, true), (4, 10));
    }

    #[test]
    fn compute_pty_dims_uses_hidden_chrome_area_for_pty() {
        let visible = compute_pty_dims(1280.0, 800.0, 1.0, true);
        let zen = compute_pty_dims(1280.0, 800.0, 1.0, false);

        assert!(zen.0 > visible.0);
        assert!(zen.1 > visible.1);
    }
}
