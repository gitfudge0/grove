//! The custom `Element` that paints one terminal grid.
//!
//! Owns no input; all work happens in `prepaint` since shaping needs `&mut Window`, so `paint` only replays quads and shaped lines.
//! Paint order ported from `src/gui/pty.rs:216-330`: base fill, background quads, text runs, selection, cursor.

use std::cell::Cell as StdCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
    /// `None` for home terminals, which belong to no project.
    project: Option<String>,
    selection: Option<(AbsCell, AbsCell)>,
    cursor_visible: bool,
    zoom: f32,
    /// Written in `prepaint` so the view can turn window-space pointer events into element-local pixels.
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

/// One row's drawing: merged background quads and shaped text runs, plus the hash of the raw cells that produced them.
/// Origins are relative to the ROW's top-left, not the element's, so a row that scrolled to a different index is byte-identical and reusable.
pub struct RowScene {
    /// Hashes the raw cells, never resolved colors — a theme change already invalidates every row via [`GeomKey::theme`].
    hash: u64,
    bg_quads: Vec<PaintQuad>,
    runs: Vec<(Point<Pixels>, ShapedLine)>,
}

/// Everything in a terminal frame that only changes when the grid content changes.
/// Carries no `bounds.origin`: when two tiles swap, only `bounds.origin` moves, so absolute origins would force needless invalidation.
pub struct TermScene {
    pub rows: Vec<Rc<RowScene>>,
}

/// The part of the key whose change invalidates every row: geometry moved, or the palette did.
#[derive(Clone, PartialEq, Eq)]
pub struct GeomKey {
    /// f32 bits — `Pixels` is not `Eq`.
    pub width_bits: u32,
    pub height_bits: u32,
    pub zoom_bits: u32,
    /// Compared by name only; `Theme` is `Clone` but not `PartialEq`.
    pub theme: SharedString,
}

/// Split deliberately: an equal `geom` with a differing tail means rows are still individually valid, so `prepaint` can re-key by content hash.
#[derive(Clone, PartialEq, Eq)]
pub struct TermSceneKey {
    pub geom: GeomKey,
    /// Bumped only when alacritty reported real grid damage; gates repaints elsewhere too.
    pub damage_gen: u64,
    pub display_offset: usize,
}

pub struct PrepaintState {
    /// Shared with the session's cache — `paint` must never drain it.
    scene: Rc<TermScene>,
    selection_quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
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

        // Resize before reading the snapshot, never the reverse, so the painted frame matches the dims the PTY was just told about.
        let dims = zoom.pty_dims(f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.session.update(cx, |session, cx| {
            session.resize(dims.0, dims.1);
            // Deferred to here so a reattached tmux client is born at the final tile size, avoiding an attach-then-relayout SIGWINCH.
            if session.is_pending_attach() {
                session.attach_now(cx);
            }
        });

        let (cur_row, cur_col, cur_hidden) = self.session.read(cx).cursor();
        let scrollback = self.session.read(cx).display_offset();
        let damage_gen = self.session.read(cx).damage_generation();

        let regular = gpui::font(fonts::MONO_FAMILY);
        let bold_font = gpui::font(fonts::MONO_FAMILY).bold();
        let font_size = px(zoom.font_size());

        // App chrome always stays on the global theme; only a pinned project resolves content against a different one, here.
        // Deliberately not memoized (unlike the old iced build): resolving is cheap, so toggling `project_themes_enabled` re-colors next frame with no bookkeeping.
        let pinned = self.project.as_ref().and_then(|name| {
            // `Some(None)` means "preview the global theme"; `None` means "no preview, use the persisted pin".
            let preview = crate::views::modals::theme_picker::ThemePreview::for_project(cx, name);
            project_theme_override(
                &cx.global::<crate::settings::SettingsState>().store,
                name,
                preview,
            )
        });
        // Keyed by theme name only, so the hit path never needs the resolved `Theme` (`Clone` but not `Eq`).
        let theme_name: SharedString = match pinned.as_ref() {
            Some(theme) => SharedString::from(theme.name.to_string()),
            None => grove_core::theme::with_current(|t| SharedString::from(t.name.to_string())),
        };
        let key = TermSceneKey {
            geom: GeomKey {
                width_bits: f32::from(bounds.size.width).to_bits(),
                height_bits: f32::from(bounds.size.height).to_bits(),
                zoom_bits: self.zoom.to_bits(),
                theme: theme_name,
            },
            damage_gen,
            display_offset: scrollback,
        };
        // Cheapest first: full key match reuses the whole scene; `geom`-only match re-keys rows by content hash; else discard everything.
        let (cached, reusable_rows) = match self.session.read(cx).scene_cache() {
            Some((k, scene)) if *k == key => (Some(scene.clone()), None),
            Some((k, scene)) if k.geom == key.geom => (None, Some(scene.clone())),
            _ => (None, None),
        };

        let scene = match cached {
            Some(scene) => scene,
            None => {
                let snapshot = self.session.read(cx).snapshot();
                // Content-addressed pool of the previous frame's rows, rebuilt per miss.
                let pool: std::collections::HashMap<u64, Rc<RowScene>> = reusable_rows
                    .iter()
                    .flat_map(|scene| scene.rows.iter())
                    .map(|row| (row.hash, row.clone()))
                    .collect();
                let render_grid = |theme: &Theme| {
                    let rows = snapshot.rows as usize;
                    let cols = snapshot.cols as usize;
                    let mut out: Vec<Rc<RowScene>> = Vec::with_capacity(rows);

                    let mut row_cells: Vec<(char, Hsla, Option<Hsla>, bool)> =
                        Vec::with_capacity(cols);

                    for r in 0..rows {
                        // If hashing ever shows up in a profile, swap in a faster hasher or use alacritty's per-line `TermDamage` instead.
                        let mut hasher = DefaultHasher::new();
                        for col in 0..cols {
                            snapshot.cell(r as u16, col as u16).hash(&mut hasher);
                        }
                        let hash = hasher.finish();
                        if let Some(row) = pool.get(&hash) {
                            out.push(row.clone());
                            continue;
                        }

                        let mut bg_quads: Vec<PaintQuad> = Vec::new();
                        let mut runs: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();
                        // Row-local: `paint` adds `bounds.origin` and `r * line_height`.
                        let y = px(0.0);
                        row_cells.clear();
                        for col in 0..cols {
                            let cell = snapshot.cell(r as u16, col as u16);
                            let (ch, fg, bg, bold) = match cell {
                                Some(cell) => {
                                    let (fg, bg) =
                                        colors::resolve_pair(cell.fg, cell.bg, cell.inverse, theme);
                                    (cell.c, fg, bg, cell.bold)
                                }
                                None => (' ', c::fg_of(theme).into(), None, false),
                            };
                            row_cells.push((ch, fg, bg, bold));
                        }

                        // Merges adjacent equal backgrounds; a `None` background emits no quad.
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
                                        point(px(c0 as f32 * cell_w), y),
                                        size(px((c1 - c0) as f32 * cell_w), px(cell_h)),
                                    ),
                                    bg,
                                ));
                            }
                            c0 = c1;
                        }

                        // Coalesces adjacent non-blank cells with equal (fg, bold); each run keeps its own `col * cell_w` origin.
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
                            runs.push((point(px(c0 as f32 * cell_w), y), shaped));
                            c0 = c1;
                        }
                        out.push(Rc::new(RowScene {
                            hash,
                            bg_quads,
                            runs,
                        }));
                    }
                    out
                };
                let rows = match pinned.as_ref() {
                    Some(theme) => render_grid(theme),
                    None => grove_core::theme::with_current(render_grid),
                };
                let scene = Rc::new(TermScene { rows });
                // Cache lives on the session, not gpui element state: element state is keyed by the ancestor id path, which embeds the tile slot — moving a tile would invalidate it.
                self.session
                    .update(cx, |session, _| session.set_scene_cache(key, scene.clone()));
                scene
            }
        };

        // Selection endpoints are absolute (scrollback-stable) and converted to the current viewport here, so the highlight stays on the same text while the view scrolls.
        // Rebuilt every frame, deliberately outside the scene cache — cheap, and the cursor's blink would otherwise bust the text cache constantly.
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
                let rows = dims.0 as usize;
                let cols = dims.1 as usize;
                let to_view = |c: AbsCell| AbsCell {
                    // Inverse of `pixel_to_abs`.
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

        // `GroveTerm::cursor` already folds the display offset in, so a scrolled-back view leaves the caret parked on its line.
        let cursor = if self.cursor_visible && !cur_hidden && (cur_row as usize) < dims.0 as usize {
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
            scene,
            selection_quads,
            cursor,
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
        window.paint_quad(fill(bounds, c::BG()));
        // Scene is read by reference, not drained, since it's shared with the session's cache.
        // All backgrounds go down before any text — interleaving per row would let one row's background paint over the previous row's descenders.
        for (r, row) in pre.scene.rows.iter().enumerate() {
            let offset = bounds.origin + point(px(0.0), pre.line_height * r as f32);
            for quad in &row.bg_quads {
                let mut quad = quad.clone();
                quad.bounds.origin += offset;
                window.paint_quad(quad);
            }
        }
        for (r, row) in pre.scene.rows.iter().enumerate() {
            let offset = bounds.origin + point(px(0.0), pre.line_height * r as f32);
            for (origin, line) in &row.runs {
                let _ = line.paint(
                    *origin + offset,
                    pre.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
        for quad in pre.selection_quads.drain(..) {
            window.paint_quad(quad);
        }
        if let Some(cursor) = pre.cursor.take() {
            window.paint_quad(cursor);
        }
    }
}

/// A cell counts as blank when it has no text or holds a space; a wide character's trailing spacer is also blank, which is why a run ends there.
fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\0'
}

/// Columns a character occupies, per the East Asian Wide/Fullwidth ranges of UAX #11.
fn wide_cells(ch: char) -> usize {
    let c = ch as u32;
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

/// Wide chars fall back to a CJK face that shapes to ~1.33 cells instead of 2; `shape_line`'s `force_width` corrects it. Returns `None` (untouched fast path) when the run is all-narrow.
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

/// The theme a PTY for `project_name` renders its content in, or `None` for the global theme. Ported from `theme_picker.rs:65-128`.
/// `preview`'s shape is load-bearing: `Some(None)` means "preview the global theme", not `None` ("no preview"); the preview check runs before the toggle check.
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

    use crate::fonts::CELL_W;

    #[test]
    fn ascii_runs_are_never_forced() {
        assert_eq!(forced_width("hello world", CELL_W), None);
        assert_eq!(forced_width("", CELL_W), None);
        assert_eq!(forced_width("café αβγ", CELL_W), None);
    }

    #[test]
    fn a_wide_char_is_forced_to_two_cells() {
        assert_eq!(forced_width("漢", CELL_W), Some(px(2.0 * CELL_W)));
        assert_eq!(forced_width("漢字", CELL_W), Some(px(4.0 * CELL_W)));
    }

    #[test]
    fn a_mixed_run_sums_cells_not_chars() {
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

    fn store_with(project_themes_enabled: bool, pin: Option<&str>) -> Store {
        Store {
            project_themes_enabled,
            projects: vec![Project {
                name: "alpha".to_string(),
                path: "/a".to_string(),
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: pin.map(ToString::to_string),
                archived: false,
                worktree_dir: None,
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

    #[test]
    fn the_toggle_being_off_beats_a_pin() {
        let store = store_with(false, Some("tokyonight-day"));
        assert!(project_theme_override(&store, "alpha", None).is_none());
    }

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
        assert!(project_theme_override(&store, "nobody", None).is_none());
    }

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
