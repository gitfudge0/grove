//! Layout constants and PTY cell metrics shared across the GUI.

use iced::Font;
use std::collections::HashSet;
use std::sync::OnceLock;

pub const ROW_H: f32 = 28.0;
/// Extra height appended to `ROW_H` for sidebar rows that render a second
/// (subtitle) line — used by the activity-stream view's session rows.
pub const SUBTITLE_H: f32 = 16.0;
/// Default sidebar width, also the reset target for a divider double-click.
pub const RAIL_W: f32 = 320.0;
/// Lower bound for the drag-resizable sidebar.
pub const SIDEBAR_MIN_W: f32 = 220.0;
/// Minimum workspace width the sidebar must leave behind when the window is
/// narrow. Caps the sidebar so the agent view never collapses to nothing.
pub const WORKSPACE_MIN_W: f32 = 400.0;
pub const APPBAR_H: f32 = 44.0;
pub const STATUS_H: f32 = 26.0;
pub const SESSBAR_H: f32 = 36.0;
/// Width of the draggable divider/grab-handle between sidebar and workspace.
/// Counts against the workspace in the PTY column math just like the sidebar.
pub const SIDEBAR_DIVIDER_W: f32 = 6.0;
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

/// Default terminal-panel share of the workspace width (in percent) when the
/// slide-over panel is open; the agent view gets the remaining `100 -
/// TERM_PANEL_PORTION`. The live value is `State::term_panel_portion`, which
/// drives both the iced `FillPortion` weights in `view.rs` and each region's
/// PTY column count (see `pty_cols_for_fraction`), so they stay in sync.
pub const TERM_PANEL_PORTION: u16 = 40;

/// Bounds and step (in percent of the workspace) for resizing the terminal
/// panel with Ctrl+Shift+Left/Right.
pub const TERM_PANEL_PORTION_MIN: u16 = 20;
pub const TERM_PANEL_PORTION_MAX: u16 = 75;
pub const TERM_PANEL_PORTION_STEP: u16 = 5;

pub const PTY_ZOOM_DEFAULT: f32 = 1.0;
pub const PTY_ZOOM_MIN: f32 = 0.6;
pub const PTY_ZOOM_MAX: f32 = 2.0;
pub const PTY_ZOOM_STEP: f32 = 0.1;

/// Height of the tile header bar in grid view.
pub const TILE_HEAD_H: f32 = 22.0;
/// Horizontal padding inside each tile's PTY container (matches `pty()` padding: 16*2).
pub const TILE_PTY_PAD_W: f32 = 32.0;
/// Vertical padding inside each tile's PTY container (matches `pty()` padding: 12*2).
pub const TILE_PTY_PAD_H: f32 = 24.0;

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

/// Codepoints the bundled mono font can actually render, parsed once from the
/// font's `cmap`. The PTY canvas paints with `Shaping::Basic`, which has no
/// font fallback, so any character absent here would render as tofu (□). The
/// renderer consults `mono_covers` to fall back to advanced shaping (system
/// font fallback) for exactly those characters.
static MONO_COVERAGE: OnceLock<HashSet<u32>> = OnceLock::new();

/// Whether the bundled mono font has a glyph for `c`. ASCII is always present,
/// so it short-circuits before touching the parsed `cmap`. Undercounting is
/// harmless — the renderer just routes the char through advanced shaping, which
/// can still find it (including in the bundled font itself).
pub fn mono_covers(c: char) -> bool {
    let cp = c as u32;
    if cp < 0x80 {
        return true;
    }
    MONO_COVERAGE
        .get_or_init(|| build_coverage(MONO_REGULAR))
        .contains(&cp)
}

/// Union the codepoint coverage of every `cmap` subtable we understand (the
/// segment-mapped format 4 for the BMP and the segmented-coverage format 12 for
/// supplementary planes). Other subtable formats are ignored.
fn build_coverage(ttf: &[u8]) -> HashSet<u32> {
    let mut set = HashSet::new();
    let Some(cmap) = find_table(ttf, b"cmap") else {
        return set;
    };
    let num_tables = u16_at(ttf, cmap + 2) as usize;
    for i in 0..num_tables {
        let rec = cmap + 4 + i * 8;
        if rec + 8 > ttf.len() {
            break;
        }
        let sub = cmap + u32_at(ttf, rec + 4) as usize;
        if sub + 2 > ttf.len() {
            continue;
        }
        match u16_at(ttf, sub) {
            4 => parse_cmap_format4(ttf, sub, &mut set),
            12 => parse_cmap_format12(ttf, sub, &mut set),
            _ => {}
        }
    }
    set
}

/// Format 4: segment-mapped BMP coverage. Honours `idRangeOffset`/`idDelta` so
/// codepoints that map to glyph 0 inside a segment are correctly excluded.
fn parse_cmap_format4(ttf: &[u8], off: usize, set: &mut HashSet<u32>) {
    let seg_count = u16_at(ttf, off + 6) as usize / 2;
    let end_codes = off + 14;
    let start_codes = end_codes + seg_count * 2 + 2; // skip reservedPad
    let id_deltas = start_codes + seg_count * 2;
    let id_range_offsets = id_deltas + seg_count * 2;
    for s in 0..seg_count {
        let end = u16_at(ttf, end_codes + s * 2);
        let start = u16_at(ttf, start_codes + s * 2);
        if start > end {
            continue;
        }
        let delta = u16_at(ttf, id_deltas + s * 2);
        let ro_pos = id_range_offsets + s * 2;
        let range_offset = u16_at(ttf, ro_pos);
        for cp in start..=end {
            if cp == 0xFFFF {
                continue;
            }
            let gid = if range_offset == 0 {
                cp.wrapping_add(delta)
            } else {
                let gi = ro_pos + range_offset as usize + (cp - start) as usize * 2;
                if gi + 2 > ttf.len() {
                    0
                } else {
                    let g = u16_at(ttf, gi);
                    if g == 0 {
                        0
                    } else {
                        g.wrapping_add(delta)
                    }
                }
            };
            if gid != 0 {
                set.insert(cp as u32);
            }
        }
    }
}

/// Format 12: segmented coverage for supplementary planes. Each group maps a
/// contiguous codepoint range, so every codepoint in the range is covered.
fn parse_cmap_format12(ttf: &[u8], off: usize, set: &mut HashSet<u32>) {
    let num_groups = u32_at(ttf, off + 12) as usize;
    for g in 0..num_groups {
        let rec = off + 16 + g * 12;
        if rec + 12 > ttf.len() {
            break;
        }
        let start = u32_at(ttf, rec);
        let end = u32_at(ttf, rec + 8);
        if end < start {
            continue;
        }
        // Guard against a malformed range exploding the set.
        let end = end.min(start.saturating_add(0x20000));
        for cp in start..=end {
            set.insert(cp);
        }
    }
}

/// Offset of a top-level font table by tag, or `None` if absent.
fn find_table(ttf: &[u8], tag: &[u8; 4]) -> Option<usize> {
    if ttf.len() < 12 {
        return None;
    }
    let table_count = u16_at(ttf, 4) as usize;
    for i in 0..table_count {
        let rec = 12 + i * 16;
        if rec + 16 > ttf.len() {
            break;
        }
        if &ttf[rec..rec + 4] == tag {
            return Some(u32_at(ttf, rec + 8) as usize);
        }
    }
    None
}

/// Big-endian u16 read; 0 on out-of-bounds so a truncated/malformed font can
/// never panic the parser (a 0 reads as "absent/glyph 0" everywhere it's used).
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    bytes
        .get(offset..offset + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .unwrap_or(0)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0)
}

/// PTY dimensions derived from unzoomed window size. Subtracts the visible chrome
/// (rail, dividers, appbar, statusbar, sessbar, container padding) and divides
/// by the cell metrics that iced lays out in logical pixels.
/// Clamp a requested sidebar width (logical px) to its valid range for the
/// current window. Lower bound is [`SIDEBAR_MIN_W`]; upper bound is the smaller
/// of half the window and "window minus a usable workspace", but never below
/// the lower bound (so tiny windows still yield a sane value).
pub fn clamp_sidebar_width(width: f32, logical_win_w: f32) -> f32 {
    let upper = (logical_win_w * 0.5)
        .min(logical_win_w - WORKSPACE_MIN_W)
        .max(SIDEBAR_MIN_W);
    width.clamp(SIDEBAR_MIN_W, upper)
}

/// Terminal-panel width share (percent) for a divider dragged to logical
/// cursor x. The panel is docked on the right, so a cursor further left grows
/// it. Clamped to `[TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX]`.
pub fn term_portion_for_cursor(cursor_x: f32, logical_win_w: f32, sidebar_w: f32) -> u16 {
    let work_left = sidebar_w + SIDEBAR_DIVIDER_W;
    let work_w = (logical_win_w - work_left).max(1.0);
    let frac = ((logical_win_w - cursor_x) / work_w).clamp(0.0, 1.0);
    let pct = (frac * 100.0).round() as i32;
    pct.clamp(TERM_PANEL_PORTION_MIN as i32, TERM_PANEL_PORTION_MAX as i32) as u16
}

pub fn compute_pty_dims(
    win_w: f32,
    win_h: f32,
    zoom: f32,
    chrome_visible: bool,
    sidebar_w: f32,
) -> (u16, u16) {
    // `ui_zoom` is applied as iced's application scale factor, which reduces
    // the logical viewport available to layout. The terminal grid must be
    // computed against that zoomed viewport; otherwise the same number of
    // cells render into a larger canvas and the PTY scrollable starts showing
    // layout scrollbars.
    let zoom = zoom.max(0.1);
    let logical_w = win_w / zoom;
    let logical_h = win_h / zoom;
    let visible_w = if chrome_visible {
        sidebar_w + SIDEBAR_DIVIDER_W
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

/// Column count for a PTY region that occupies `fraction` of the workspace
/// width (the area right of the sidebar) when the slide-over panel splits it.
/// Accounts for the 1px split divider and the region's own horizontal padding.
/// Height (rows) is unaffected by the split, so callers reuse `compute_pty_dims`
/// rows.
pub fn pty_cols_for_fraction(
    win_w: f32,
    zoom: f32,
    chrome_visible: bool,
    fraction: f32,
    sidebar_w: f32,
) -> u16 {
    let zoom = zoom.max(0.1);
    let logical_w = win_w / zoom;
    let visible_w = if chrome_visible {
        sidebar_w + SIDEBAR_DIVIDER_W
    } else {
        0.0
    };
    // Workspace width, minus the vertical split divider, then this region's
    // share, then the PTY's own padding inside that region.
    let work_w = logical_w - visible_w - SIDEBAR_DIVIDER_W;
    let region_w = work_w * fraction - PTY_PAD_W;
    (region_w / CELL_W).max(10.0) as u16
}

/// Grid dimensions `(cols, rows)` for `n` sessions.
/// Formula: cols = ceil(sqrt(n)).clamp(1,4), rows = ceil(n/cols).min(4).
pub fn grid_layout(n: usize) -> (usize, usize) {
    let n = n.max(1);
    let cols = ((n as f64).sqrt().ceil() as usize).clamp(1, 4);
    let rows = ((n + cols - 1) / cols).min(4);
    (cols, rows)
}

/// Per-tile PTY dimensions `(rows, cols)` for a grid of `n` sessions.
/// Grid mode hides the sidebar, so the full window width is available.
pub fn grid_tile_dims(win_w: f32, win_h: f32, zoom: f32, n: usize) -> (u16, u16) {
    let (grid_cols, grid_rows) = grid_layout(n);
    let zoom = zoom.max(0.1);
    let logical_w = win_w / zoom;
    let logical_h = win_h / zoom;
    // Grid mode keeps appbar + statusbar but hides the sidebar.
    let workspace_h = logical_h - APPBAR_H - STATUS_H;
    let workspace_w = logical_w;
    // Subtract 1px inter-tile gaps, tile header, and pty container padding.
    let tile_h = (workspace_h - (grid_rows as f32 - 1.0)) / grid_rows as f32;
    let tile_w = (workspace_w - (grid_cols as f32 - 1.0)) / grid_cols as f32;
    let pty_h = tile_h - TILE_HEAD_H - TILE_PTY_PAD_H;
    let pty_w = tile_w - TILE_PTY_PAD_W;
    let rows = (pty_h / CELL_H).max(4.0) as u16;
    let cols = (pty_w / CELL_W).max(10.0) as u16;
    (rows, cols)
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_sidebar_width, compute_pty_dims, term_portion_for_cursor, APPBAR_H, CELL_H, CELL_W,
        FONT_SIZE, MONO_REGULAR, PTY_PAD_H, PTY_PAD_W, RAIL_W, SESSBAR_H, SIDEBAR_DIVIDER_W,
        SIDEBAR_MIN_W, STATUS_H, TERM_PANEL_PORTION_MAX, TERM_PANEL_PORTION_MIN, WORKSPACE_MIN_W,
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
            compute_pty_dims(win_w, win_h, 1.0, true, RAIL_W),
            (expected_rows_1x, expected_cols_1x)
        );
        assert_eq!(
            compute_pty_dims(win_w, win_h, 1.5, true, RAIL_W),
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
            let (rows, cols) = compute_pty_dims(win_w, win_h, zoom, true, RAIL_W);
            let logical_w = win_w / zoom;
            let logical_h = win_h / zoom;
            let usable_w = logical_w - (RAIL_W + SIDEBAR_DIVIDER_W + PTY_PAD_W);
            let usable_h = logical_h - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);

            assert!((cols as f32) * CELL_W <= usable_w);
            assert!((rows as f32) * CELL_H <= usable_h);
        }
    }

    #[test]
    fn pty_cols_for_fraction_splits_workspace() {
        use super::{pty_cols_for_fraction, TERM_PANEL_PORTION};
        let panel_frac = TERM_PANEL_PORTION as f32 / 100.0;
        let agent = pty_cols_for_fraction(1280.0, 1.0, true, 1.0 - panel_frac, RAIL_W);
        let panel = pty_cols_for_fraction(1280.0, 1.0, true, panel_frac, RAIL_W);
        let full = compute_pty_dims(1280.0, 800.0, 1.0, true, RAIL_W).1;

        // The 35% panel is narrower than the 65% agent, and both fit inside the
        // full single-pane width.
        assert!(panel < agent);
        assert!(agent < full);
        assert!(panel >= 10); // floor enforced
    }

    #[test]
    fn compute_pty_dims_enforces_minimum_size() {
        assert_eq!(compute_pty_dims(100.0, 100.0, 2.0, true, RAIL_W), (4, 10));
    }

    #[test]
    fn compute_pty_dims_uses_hidden_chrome_area_for_pty() {
        let visible = compute_pty_dims(1280.0, 800.0, 1.0, true, RAIL_W);
        let zen = compute_pty_dims(1280.0, 800.0, 1.0, false, RAIL_W);

        assert!(zen.0 > visible.0);
        assert!(zen.1 > visible.1);
    }

    #[test]
    fn wider_sidebar_yields_fewer_pty_cols() {
        let narrow = compute_pty_dims(1280.0, 800.0, 1.0, true, SIDEBAR_MIN_W);
        let wide = compute_pty_dims(1280.0, 800.0, 1.0, true, 500.0);
        // Rows are unaffected by sidebar width; only columns shrink.
        assert_eq!(narrow.0, wide.0);
        assert!(wide.1 < narrow.1);
    }

    #[test]
    fn hidden_chrome_ignores_sidebar_width() {
        let a = compute_pty_dims(1280.0, 800.0, 1.0, false, SIDEBAR_MIN_W);
        let b = compute_pty_dims(1280.0, 800.0, 1.0, false, 600.0);
        assert_eq!(a, b);
    }

    #[test]
    fn term_portion_for_cursor_maps_and_clamps() {
        let win = 1280.0;
        let sidebar = RAIL_W; // workspace spans (RAIL_W + SIDEBAR_DIVIDER_W) .. win
        let work_left = sidebar + SIDEBAR_DIVIDER_W;
        let work_w = win - work_left;
        // Cursor at the midpoint of the workspace → ~50% panel.
        let mid = work_left + work_w * 0.5;
        assert_eq!(term_portion_for_cursor(mid, win, sidebar), 50);
        // Dragging far right shrinks the panel to its minimum.
        assert_eq!(
            term_portion_for_cursor(win - 1.0, win, sidebar),
            TERM_PANEL_PORTION_MIN
        );
        // Dragging far left grows it to its maximum (not the full workspace).
        assert_eq!(
            term_portion_for_cursor(work_left + 1.0, win, sidebar),
            TERM_PANEL_PORTION_MAX
        );
    }

    #[test]
    fn clamp_sidebar_width_bounds() {
        let win = 1280.0;
        // Pass-through within range.
        assert_eq!(clamp_sidebar_width(360.0, win), 360.0);
        // Below the minimum clamps up.
        assert_eq!(clamp_sidebar_width(50.0, win), SIDEBAR_MIN_W);
        // Above the window-relative cap (half the window) clamps down.
        assert_eq!(clamp_sidebar_width(1000.0, win), win * 0.5);
        // Workspace-minimum is the binding cap on a narrow window.
        let narrow = 700.0;
        assert_eq!(clamp_sidebar_width(1000.0, narrow), narrow - WORKSPACE_MIN_W);
        // Degenerate tiny window never returns below the minimum.
        assert_eq!(clamp_sidebar_width(500.0, 100.0), SIDEBAR_MIN_W);
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

    #[test]
    fn mono_coverage_reflects_bundled_font() {
        use super::mono_covers;

        // ASCII and Latin text the font obviously ships.
        assert!(mono_covers('A'));
        assert!(mono_covers('~'));
        assert!(mono_covers('é'));
        // Box-drawing the prompt relies on.
        assert!(mono_covers('└'));
        assert!(mono_covers('─'));
        // A Nerd Font private-use icon (nf-fa-home) the patched font adds.
        assert!(mono_covers('\u{f015}'));
        // CJK is absent from IBM Plex Mono, so it must route to fallback.
        assert!(!mono_covers('好'));
    }

    #[test]
    fn grid_layout_picks_sensible_dimensions() {
        use super::grid_layout;
        assert_eq!(grid_layout(1),  (1, 1));
        assert_eq!(grid_layout(2),  (2, 1));
        assert_eq!(grid_layout(3),  (2, 2));
        assert_eq!(grid_layout(4),  (2, 2));
        assert_eq!(grid_layout(5),  (3, 2));
        assert_eq!(grid_layout(6),  (3, 2));
        assert_eq!(grid_layout(7),  (3, 3));
        assert_eq!(grid_layout(9),  (3, 3));
        assert_eq!(grid_layout(10), (4, 3));
        assert_eq!(grid_layout(16), (4, 4));
        assert_eq!(grid_layout(20), (4, 4)); // capped at 4×4
    }

    #[test]
    fn grid_tile_dims_shrink_with_more_sessions() {
        use super::grid_tile_dims;
        let (r2, c2) = grid_tile_dims(1280.0, 800.0, 1.0, 2);
        let (r4, c4) = grid_tile_dims(1280.0, 800.0, 1.0, 4);
        assert!(r2 >= r4, "more sessions → smaller tiles");
        assert!(c2 >= c4, "more sessions → fewer cols per tile");
        assert!(r4 >= 4,  "never below floor");
        assert!(c4 >= 10, "never below floor");
    }
}
