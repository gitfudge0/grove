//! Layout constants and PTY cell metrics shared across the GUI.

use iced::Font;

pub const ROW_H: f32 = 28.0;
/// Extra height appended to `ROW_H` for sidebar rows that render a second
/// (subtitle) line — used by the activity-stream view's session rows.
pub const SUBTITLE_H: f32 = 16.0;
pub const RAIL_W: f32 = 320.0;
pub const APPBAR_H: f32 = 44.0;
pub const STATUS_H: f32 = 26.0;
pub const SESSBAR_H: f32 = 36.0;
pub const SIDEBAR_DIVIDER_W: f32 = 1.0;
pub const PTY_PAD_W: f32 = 36.0;
pub const PTY_PAD_H: f32 = 28.0;

/// Hardcoded cell metrics for BlexMono Nerd Font (IBM Plex Mono patched with
/// Nerd Font glyphs) at 12.5pt. It keeps Plex Mono's 600-unit advance on a
/// 1000-unit em, which gives a 7.5px cell at this size. The canvas maps
/// (row, col) directly to pixels, so this must stay aligned with the bundled
/// font or the cursor drifts across long rows.
pub const CELL_W: f32 = 7.5;
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

/// BlexMono Nerd Font Mono — IBM Plex Mono patched with Nerd Font icon glyphs.
/// Used for PTY output and any monospace-sensitive content; the Nerd Font
/// coverage means `eza`/`ls` file icons and powerline glyphs render instead of
/// tofu. Bundled as TTF and registered at startup in `gui::run`.
pub const MONO_FONT: Font = Font::with_name("BlexMono Nerd Font Mono");

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
pub const MONO_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/BlexMonoNerdFontMono-Regular.ttf");
pub const MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/BlexMonoNerdFontMono-Bold.ttf");

/// PTY dimensions derived from unzoomed window size. Subtracts the visible chrome
/// (rail, dividers, appbar, statusbar, sessbar, container padding) and divides
/// by the cell metrics that iced lays out in logical pixels.
pub fn compute_pty_dims(win_w: f32, win_h: f32, zoom: f32, chrome_visible: bool) -> (u16, u16) {
    // `ui_zoom` is applied as iced's application scale factor, which reduces
    // the logical viewport available to layout. The terminal grid must be
    // computed against that zoomed viewport; otherwise the same number of
    // cells render into a larger canvas and the PTY scrollable starts showing
    // layout scrollbars.
    let zoom = zoom.max(0.1);
    let logical_w = win_w / zoom;
    let logical_h = win_h / zoom;
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
    let usable_w = logical_w - (visible_w + PTY_PAD_W);
    let usable_h = logical_h - (visible_h + SESSBAR_H + PTY_PAD_H);
    let cols = (usable_w / CELL_W).max(10.0) as u16;
    let rows = (usable_h / CELL_H).max(4.0) as u16;
    (rows, cols)
}

#[cfg(test)]
mod tests {
    use super::{
        compute_pty_dims, APPBAR_H, CELL_H, CELL_W, FONT_SIZE, MONO_REGULAR, PTY_PAD_H, PTY_PAD_W,
        RAIL_W, SESSBAR_H, SIDEBAR_DIVIDER_W, STATUS_H,
    };

    #[test]
    fn cell_width_matches_bundled_mono_advance() {
        let units_per_em = u16_at(MONO_REGULAR, table_offset(MONO_REGULAR, b"head") + 18);
        let advance = u16_at(MONO_REGULAR, table_offset(MONO_REGULAR, b"hmtx"));
        let expected = advance as f32 / units_per_em as f32 * FONT_SIZE;

        assert!((CELL_W - expected).abs() < 0.001);
    }

    #[test]
    fn compute_pty_dims_scales_terminal_grid_with_zoom() {
        let win_w = 1280.0;
        let win_h = 800.0;

        let usable_w = win_w - (RAIL_W + SIDEBAR_DIVIDER_W + PTY_PAD_W);
        let usable_h = win_h - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);
        let expected_cols_1x = (usable_w / CELL_W).max(10.0) as u16;
        let expected_rows_1x = (usable_h / CELL_H).max(4.0) as u16;
        let usable_w_15x = (win_w / 1.5) - (RAIL_W + SIDEBAR_DIVIDER_W + PTY_PAD_W);
        let usable_h_15x = (win_h / 1.5) - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);
        let expected_cols_15x = (usable_w_15x / CELL_W).max(10.0) as u16;
        let expected_rows_15x = (usable_h_15x / CELL_H).max(4.0) as u16;

        assert_eq!(
            compute_pty_dims(win_w, win_h, 1.0, true),
            (expected_rows_1x, expected_cols_1x)
        );
        assert_eq!(
            compute_pty_dims(win_w, win_h, 1.5, true),
            (expected_rows_15x, expected_cols_15x)
        );
        assert!(expected_rows_15x < expected_rows_1x);
        assert!(expected_cols_15x < expected_cols_1x);
    }

    #[test]
    fn compute_pty_dims_keeps_grid_inside_zoomed_viewport() {
        let win_w = 1280.0;
        let win_h = 800.0;

        for zoom in [1.0, 1.5, 2.0] {
            let (rows, cols) = compute_pty_dims(win_w, win_h, zoom, true);
            let logical_w = win_w / zoom;
            let logical_h = win_h / zoom;
            let usable_w = logical_w - (RAIL_W + SIDEBAR_DIVIDER_W + PTY_PAD_W);
            let usable_h = logical_h - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);

            assert!((cols as f32) * CELL_W <= usable_w);
            assert!((rows as f32) * CELL_H <= usable_h);
        }
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

    fn table_offset(ttf: &[u8], tag: &[u8; 4]) -> usize {
        let table_count = u16_at(ttf, 4) as usize;
        for i in 0..table_count {
            let offset = 12 + i * 16;
            if &ttf[offset..offset + 4] == tag {
                return u32_at(ttf, offset + 8) as usize;
            }
        }
        panic!("missing font table {}", std::str::from_utf8(tag).unwrap());
    }

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
}
