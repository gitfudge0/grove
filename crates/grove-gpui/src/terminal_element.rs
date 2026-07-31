//! The custom `Element` that paints one terminal grid.
//!
//! Owns no input: the view (`crate::views::terminal_view`) handles events and
//! hands this element a snapshot-shaped description of what to draw. All the
//! work happens in `prepaint` — shaping needs `&mut Window`, so `paint` only
//! replays quads and already-shaped lines (findings §S1 Step 1).
//!
//! Paint order is `src/gui/pty.rs:216-330`: base fill, merged background
//! quads, text runs, selection overlay, cursor.

// The selection field is wired by Plan 04 Task 5; the constructor already
// carries it so the paint path does not change shape later.
#![allow(dead_code)]

use std::cell::Cell as StdCell;
use std::rc::Rc;

use gpui::{
    fill, point, prelude::*, px, relative, rgba, size, App, Bounds, ElementId, GlobalElementId,
    Hsla, LayoutId, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun,
    Window,
};
use grove_core::theme::Theme;

use crate::entities::terminal_session::TerminalSession;
use crate::fonts;
use crate::terminal::colors;
use crate::terminal::mouse::{self, AbsCell};
use crate::theme as c;
use crate::zoom::ZoomState;

pub struct TerminalElement {
    session: gpui::Entity<TerminalSession>,
    /// Project this PTY belongs to, for the pinned content-theme lookup.
    /// `None` for home terminals, which belong to no project.
    project: Option<String>,
    selection: Option<(AbsCell, AbsCell)>,
    cursor_visible: bool,
    zoom: f32,
    /// Written in `prepaint` so the view can turn window-space pointer events
    /// into element-local pixels.
    bounds_out: Rc<StdCell<Bounds<Pixels>>>,
}

impl TerminalElement {
    pub fn new(
        session: gpui::Entity<TerminalSession>,
        project: Option<String>,
        selection: Option<(AbsCell, AbsCell)>,
        cursor_visible: bool,
        zoom: f32,
        bounds_out: Rc<StdCell<Bounds<Pixels>>>,
    ) -> Self {
        Self {
            session,
            project,
            selection,
            cursor_visible,
            zoom,
            bounds_out,
        }
    }
}

pub struct PrepaintState {
    bg_quads: Vec<PaintQuad>,
    /// Origins are already anchored at `col * cell_w`; nothing here depends on
    /// accumulated glyph advances.
    runs: Vec<(Point<Pixels>, ShapedLine)>,
    selection_quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    dims: (u16, u16),
    line_height: Pixels,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        self.bounds_out.set(bounds);

        let zoom = ZoomState::new(self.zoom);
        let cell_w = zoom.cell_w();
        let cell_h = zoom.cell_h();

        // The single place PTY dims are decided: a window resize and a zoom
        // change both land here (findings amendment 7). Resize *before* reading
        // the snapshot, never the reverse, so the painted frame matches the
        // dims the PTY was just told about.
        let dims = zoom.pty_dims(f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.session.update(cx, |session, _| {
            session.resize(dims.0, dims.1);
        });

        let snapshot = self.session.read(cx).snapshot();
        let (cur_row, cur_col, cur_hidden) = self.session.read(cx).cursor();
        let scrollback = self.session.read(cx).display_offset();

        let regular = gpui::font(fonts::MONO_FAMILY);
        let bold_font = gpui::font(fonts::MONO_FAMILY).bold();
        let font_size = px(zoom.font_size());

        // A PTY belonging to a project with a pinned content theme resolves
        // its *content* against that theme here. App chrome stays on the global
        // theme regardless, which is why the override lives at this call site
        // and nowhere else.
        //
        // The iced build memoizes this per frame (`src/gui/view/terminal.rs:33-46`,
        // reset at the top of `view()`) and invalidates it on picker
        // cancel/submit. **That cache is deliberately not ported**: resolving is
        // a `Store` field read plus a name lookup, done fresh in `prepaint`, so
        // flipping `project_themes_enabled` re-colors on the next frame with no
        // bookkeeping. A future reader looking for the cache will find this note
        // instead.
        //
        // `with_current` is an atomic-load snapshot, not a lock, so the global
        // fallback is equally free to read per frame.
        let pinned = self.project.as_ref().and_then(|name| {
            // Plan 08 carried decision 7: the ONE live-preview hook. The theme
            // picker and the launcher's theme pane both drive it through
            // `ThemePreview`; `Some(None)` means "preview the global theme",
            // `None` means "no preview" and the persisted pin wins. There is
            // deliberately no second theme-override path.
            let preview = crate::views::modals::theme_picker::ThemePreview::for_project(cx, name);
            project_theme_override(
                &cx.global::<crate::settings::SettingsState>().store,
                name,
                preview,
            )
        });
        let render_grid = |theme: &Theme| {
            let rows = snapshot.rows as usize;
            let cols = snapshot.cols as usize;
            let mut bg_quads: Vec<PaintQuad> = Vec::new();
            let mut runs: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();

            // Resolved once per cell; every color in the grid goes through
            // `colors::resolve_pair`, the pipeline's only inverse swap.
            let mut row_cells: Vec<(char, Hsla, Option<Hsla>, bool)> = Vec::with_capacity(cols);

            for r in 0..rows {
                let y = bounds.origin.y + px(r as f32 * cell_h);
                row_cells.clear();
                for col in 0..cols {
                    let cell = snapshot.cell(r as u16, col as u16);
                    let (ch, fg, bg, bold) = match cell {
                        Some(cell) => {
                            let (fg, bg) =
                                colors::resolve_pair(cell.fg, cell.bg, cell.inverse, theme);
                            (cell.text.chars().next().unwrap_or(' '), fg, bg, cell.bold)
                        }
                        None => (' ', c::fg_of(theme).into(), None, false),
                    };
                    row_cells.push((ch, fg, bg, bold));
                }

                // 2. Merged background quads: coalesce adjacent equal
                //    backgrounds. A `None` background emits no quad at all, so
                //    a default-background screen costs nothing here.
                let mut c0 = 0usize;
                while c0 < cols {
                    let bg = row_cells[c0].2;
                    let mut c1 = c0 + 1;
                    while c1 < cols && row_cells[c1].2 == bg {
                        c1 += 1;
                    }
                    if let Some(bg) = bg {
                        bg_quads.push(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(c0 as f32 * cell_w), y),
                                size(px((c1 - c0) as f32 * cell_w), px(cell_h)),
                            ),
                            bg,
                        ));
                    }
                    c0 = c1;
                }

                // 3. Text runs: coalesce adjacent non-blank cells with an equal
                //    `(fg, bold)`. Blanks are skipped entirely — a mostly-empty
                //    screen shapes almost nothing. Each run is painted at its
                //    own `col * cell_w` origin (carried amendment 3), so a
                //    width mismatch inside one run cannot drift the next.
                let mut c0 = 0usize;
                while c0 < cols {
                    if is_blank(row_cells[c0].0) {
                        c0 += 1;
                        continue;
                    }
                    let (fg, bold) = (row_cells[c0].1, row_cells[c0].3);
                    let mut text = String::new();
                    let mut c1 = c0;
                    while c1 < cols
                        && !is_blank(row_cells[c1].0)
                        && row_cells[c1].1 == fg
                        && row_cells[c1].3 == bold
                    {
                        text.push(row_cells[c1].0);
                        c1 += 1;
                    }
                    let force_width = forced_width(&text, cell_w);
                    let run = TextRun {
                        len: text.len(),
                        font: if bold {
                            bold_font.clone()
                        } else {
                            regular.clone()
                        },
                        color: fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(
                        SharedString::from(text),
                        font_size,
                        &[run],
                        force_width,
                    );
                    runs.push((point(bounds.origin.x + px(c0 as f32 * cell_w), y), shaped));
                    c0 = c1;
                }
            }
            (bg_quads, runs)
        };
        let (bg_quads, runs) = match pinned.as_ref() {
            Some(theme) => render_grid(theme),
            None => grove_core::theme::with_current(render_grid),
        };

        // 4. Selection overlay, between the text and the cursor. The endpoints
        //    are absolute (scrollback-stable), so they are converted to the
        //    *current* viewport here — which is what makes the highlight stay
        //    on the same text while the view scrolls underneath it.
        //
        //    The wash is the hardcoded `rgba(0.40, 0.50, 0.78, 0.35)` that spec
        //    Appendix A pins — deliberately not a theme token.
        let (sr, sg, sb_c, sa) = mouse::SELECTION_RGBA;
        let wash = rgba(
            (u32::from((sr * 255.0) as u8) << 24)
                | (u32::from((sg * 255.0) as u8) << 16)
                | (u32::from((sb_c * 255.0) as u8) << 8)
                | u32::from((sa * 255.0) as u8),
        );
        let selection_quads: Vec<PaintQuad> = self
            .selection
            .map(|(a, head)| {
                let rows = snapshot.rows as usize;
                let cols = snapshot.cols as usize;
                let to_view = |c: AbsCell| AbsCell {
                    // Inverse of `pixel_to_abs`: viewport_row = h - 1 - (a_row - sb).
                    a_row: rows
                        .saturating_sub(1)
                        .saturating_sub(c.a_row.saturating_sub(scrollback)),
                    col: c.col,
                };
                mouse::selection_rects(to_view(a), to_view(head), rows, cols, cell_w, cell_h)
                    .into_iter()
                    .map(|(x, y, w, h)| {
                        fill(
                            Bounds::new(
                                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                size(px(w), px(h)),
                            ),
                            wash,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // 5. Block cursor. `GroveTerm::cursor` already folds the display offset
        //    in, so a scrolled-back view leaves the caret parked on its line.
        let cursor =
            if self.cursor_visible && !cur_hidden && (cur_row as usize) < snapshot.rows as usize {
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x + px(f32::from(cur_col) * cell_w),
                            bounds.origin.y + px(f32::from(cur_row) * cell_h),
                        ),
                        size(px(cell_w), px(cell_h)),
                    ),
                    c::FG(),
                ))
            } else {
                None
            };

        PrepaintState {
            bg_quads,
            runs,
            selection_quads,
            cursor,
            dims,
            line_height: px(cell_h),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        pre: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 1. One full-bounds fill, so short rows and the sub-cell remainder at
        //    the right/bottom edge carry the terminal background.
        window.paint_quad(fill(bounds, c::BG()));
        for quad in pre.bg_quads.drain(..) {
            window.paint_quad(quad);
        }
        for (origin, line) in &pre.runs {
            // `paint` returning `Err` means the line could not be rendered;
            // there is nothing useful to do per-run but skip it.
            let _ = line.paint(*origin, pre.line_height, TextAlign::Left, None, window, cx);
        }
        for quad in pre.selection_quads.drain(..) {
            window.paint_quad(quad);
        }
        if let Some(cursor) = pre.cursor.take() {
            window.paint_quad(cursor);
        }
    }
}

/// A cell counts as blank when it has no text or holds a space. The trailing
/// `WIDE_CHAR_SPACER` of a wide character is emitted blank
/// (`crates/grove-terminal/src/cell.rs:29-33`), which is exactly why a run ends
/// at a wide character — [`forced_width`] then pins that one-glyph run to its
/// true two-cell slot.
fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\0'
}

/// Columns a character occupies in the terminal grid.
///
/// The East Asian Wide / Fullwidth ranges of UAX #11 — the same accounting the
/// terminal itself uses to reserve a spacer cell. Deliberately *not*
/// `str::chars().count()`: the whole point is that a wide glyph is two cells.
fn wide_cells(ch: char) -> usize {
    let c = ch as u32;
    // Fast path: everything below U+1100 (which includes all of ASCII) is
    // narrow, so ordinary text never touches the table below.
    if c < 0x1100 {
        return 1;
    }
    let wide = matches!(c,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x2FFFD
            | 0x30000..=0x3FFFD);
    if wide {
        2
    } else {
        1
    }
}

/// Wide chars fall back to a system CJK face that shapes to ~1.33 cells instead
/// of 2 (findings §S1 Step 1). `shape_line`'s `force_width` is the fix, and it
/// **does exist** at the pinned rev
/// (`gpui/src/text_system.rs:397-403`, `force_width: Option<Pixels>`).
///
/// Returns `None` — the untouched fast path — when every character in the run
/// is narrow, so ASCII text is never forced. Per-run anchoring (the run origin
/// at `col * cell_w`) remains the primary, non-negotiable mitigation; this only
/// fixes the glyph's own width inside its run.
///
/// If the manual CJK check (Plan 04 Task 6 Step 3 row 2) finds forcing distorts
/// the glyph, delete the `force_width` argument at the `shape_line` call site
/// and keep anchoring alone.
fn forced_width(run_text: &str, cell_w: f32) -> Option<Pixels> {
    let mut cells = 0usize;
    let mut any_wide = false;
    for ch in run_text.chars() {
        let w = wide_cells(ch);
        any_wide |= w == 2;
        cells += w;
    }
    if !any_wide {
        return None;
    }
    Some(px(cells as f32 * cell_w))
}

/// The theme a PTY belonging to `project_name` renders its **content** in, or
/// `None` to fall back to the global active theme. Ported from
/// `src/app/theme_picker.rs:65-128` and `src/gui/view/terminal.rs:48-73`.
///
/// **App chrome always stays on the global theme regardless**
/// (`crates/grove-core/src/storage.rs:151-155`): every `c::*` call site in this
/// crate is untouched by this function, which is exactly why the override lives
/// at the single PTY-content call site and nowhere else.
///
/// `preview` is the project-scoped theme picker's live highlight, and its shape
/// is load-bearing: `Some(None)` means "preview the global theme", which is
/// **not** `None` ("no preview"). The preview check comes *before* the toggle
/// check — `theme_picker.rs:111-118` orders it that way, so a preview renders
/// even while Project themes is off. That ordering is the parity contract.
pub fn project_theme_override(
    store: &grove_core::storage::Store,
    project_name: &str,
    preview: Option<Option<Theme>>,
) -> Option<Theme> {
    if let Some(preview) = preview {
        return preview;
    }
    if !store.project_themes_enabled {
        return None;
    }
    store
        .projects
        .iter()
        .find(|p| p.name == project_name)
        .and_then(|p| p.theme.as_deref())
        .and_then(grove_core::theme::by_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::storage::{Project, Store};

    const CELL_W: f32 = 7.5;

    #[test]
    fn ascii_runs_are_never_forced() {
        assert_eq!(forced_width("hello world", CELL_W), None);
        assert_eq!(forced_width("", CELL_W), None);
        // Narrow non-ASCII (Latin-1, Greek, Cyrillic) is still one cell.
        assert_eq!(forced_width("café αβγ", CELL_W), None);
    }

    #[test]
    fn a_wide_char_is_forced_to_two_cells() {
        assert_eq!(forced_width("漢", CELL_W), Some(px(2.0 * CELL_W)));
        assert_eq!(forced_width("漢字", CELL_W), Some(px(4.0 * CELL_W)));
    }

    #[test]
    fn a_mixed_run_sums_cells_not_chars() {
        // 2 (wide) + 1 + 1 = 4 cells across 3 chars — `chars().count()` would
        // say 3 and squash the glyph.
        assert_eq!(forced_width("漢ab", CELL_W), Some(px(4.0 * CELL_W)));
    }

    #[test]
    fn wide_cells_covers_the_usual_suspects() {
        assert_eq!(wide_cells('a'), 1);
        assert_eq!(wide_cells('é'), 1);
        assert_eq!(wide_cells('─'), 1, "box drawing must stay one cell");
        assert_eq!(wide_cells('漢'), 2);
        assert_eq!(wide_cells('あ'), 2);
        assert_eq!(wide_cells('한'), 2);
        assert_eq!(wide_cells('！'), 2, "fullwidth punctuation");
    }

    #[test]
    fn blank_detection_skips_spacers_and_spaces() {
        assert!(is_blank(' '));
        assert!(is_blank('\0'));
        assert!(!is_blank('a'));
    }

    // ── per-project pinned content themes (Plan 05 Task 6 Step 3) ────────

    fn store_with(project_themes_enabled: bool, pin: Option<&str>) -> Store {
        Store {
            project_themes_enabled,
            projects: vec![Project {
                name: "alpha".to_string(),
                path: "/a".to_string(),
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: pin.map(ToString::to_string),
                archived: false,
            }],
            ..Store::default()
        }
    }

    fn a_theme() -> Theme {
        let Some(t) = grove_core::theme::by_name("tokyonight-day") else {
            unreachable!("a builtin theme must resolve")
        };
        t
    }

    /// `src/app/theme_picker.rs:119-121` — the universal toggle.
    #[test]
    fn the_toggle_being_off_beats_a_pin() {
        let store = store_with(false, Some("tokyonight-day"));
        assert!(project_theme_override(&store, "alpha", None).is_none());
    }

    /// `theme_picker.rs:122-128`.
    #[test]
    fn a_pin_resolves_when_the_toggle_is_on() {
        let store = store_with(true, Some("tokyonight-day"));
        let Some(t) = project_theme_override(&store, "alpha", None) else {
            unreachable!("a pinned project resolves its theme")
        };
        assert_eq!(t.name, a_theme().name);
    }

    #[test]
    fn an_unresolvable_pin_falls_back_to_the_global_theme() {
        let store = store_with(true, Some("no-such-theme"));
        assert!(project_theme_override(&store, "alpha", None).is_none());
        // An unknown project name is the same fallback.
        assert!(project_theme_override(&store, "nobody", None).is_none());
    }

    /// `theme_picker.rs:111-118` — the preview check comes **before** the
    /// toggle check, and that ordering is the parity contract.
    #[test]
    fn a_preview_of_none_means_the_global_theme_even_with_a_pin() {
        let store = store_with(true, Some("tokyonight-day"));
        assert!(project_theme_override(&store, "alpha", Some(None)).is_none());
    }

    #[test]
    fn a_preview_bypasses_the_toggle_entirely() {
        let store = store_with(false, None);
        let Some(t) = project_theme_override(&store, "alpha", Some(Some(a_theme()))) else {
            unreachable!("the preview wins outright")
        };
        assert_eq!(t.name, a_theme().name);
    }
}
