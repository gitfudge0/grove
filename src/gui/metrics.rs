//! Layout constants and PTY cell metrics shared across the GUI.

use iced::Font;

pub const ROW_H: f32 = 28.0;
pub const RAIL_W: f32 = 320.0;
pub const APPBAR_H: f32 = 44.0;
pub const STATUS_H: f32 = 26.0;
pub const SESSBAR_H: f32 = 36.0;

/// Hardcoded cell metrics for IBM Plex Mono at 12.5pt. iced doesn't give
/// us a cheap way to measure glyphs from outside a frame, so these are pinned
/// — the canvas just maps (row, col) → pixel position. Re-tune if the font
/// size or family changes.
pub const CELL_W: f32 = 7.6;
pub const CELL_H: f32 = 17.0;
pub const FONT_SIZE: f32 = 12.5;

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

/// PTY dimensions derived from window pixel size. Subtracts the fixed chrome
/// (rail, dividers, appbar, statusbar, sessbar, container padding) and divides
/// by the cell metrics.
pub fn compute_pty_dims(win_w: f32, win_h: f32) -> (u16, u16) {
    let usable_w = win_w - RAIL_W - 1.0 - 36.0;
    let usable_h = win_h - APPBAR_H - STATUS_H - SESSBAR_H - 28.0;
    let cols = (usable_w / CELL_W).max(10.0) as u16;
    let rows = (usable_h / CELL_H).max(4.0) as u16;
    (rows, cols)
}
