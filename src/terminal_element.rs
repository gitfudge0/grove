//! The custom `Element` that paints one terminal grid.
//!
//! Owns no input: the view (`crate::views::terminal_view`) handles events and
//! hands this element a snapshot-shaped description of what to draw. All the
//! work happens in `prepaint` — shaping needs `&mut Window`, so `paint` only
//! replays quads and already-shaped lines (findings §S1 Step 1).
//!
//! Paint order is `src/gui/pty.rs:216-330`: base fill, merged background
//! quads, text runs, selection overlay, cursor.

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

/// One row's drawing: the merged background quads and the shaped text runs for
/// a single grid line, plus the hash of the raw cells that produced them.
///
/// **Origins are relative to the ROW's top-left, not the element's**, and that
/// is the whole trick: a row that scrolled to a different index is byte-identical
/// drawing, so it can be reused as-is and `paint` supplies the `y` offset. Bake
/// `r * cell_h` in here and scroll reuse silently stops working.
pub struct RowScene {
    /// Hash of the raw [`grove_terminal::Cell`]s, never of the resolved colors:
    /// raw cells avoid hashing floats, and a theme change already invalidates
    /// every row through [`GeomKey::theme`].
    hash: u64,
    /// `x` is anchored at `col * cell_w`, `y` is 0; nothing here depends on
    /// accumulated glyph advances.
    bg_quads: Vec<PaintQuad>,
    runs: Vec<(Point<Pixels>, ShapedLine)>,
}

/// Everything in a terminal frame that only changes when the grid content
/// changes, i.e. the entire cost of `prepaint` (a full `snapshot()` copy,
/// `colors::resolve_pair` per cell, ~180 `shape_line` calls per tile).
///
/// Rows in top-to-bottom order, `Rc` per row so reusing one across frames is a
/// refcount bump. Duplicate rows (blank lines are common) naturally share a
/// single `Rc` — that is correct and desirable.
///
/// The scene itself carries no `bounds.origin`, and that is load-bearing: when
/// two tiles swap, each session's `bounds.origin` moves but its `bounds.size`
/// does not. Absolute origins would be wrong after a swap and would force
/// invalidation on exactly the frames this cache exists to make cheap.
pub struct TermScene {
    pub rows: Vec<Rc<RowScene>>,
}

/// The part of the key whose change invalidates **every** row: geometry moved,
/// or the palette every cell resolves against did.
#[derive(Clone, PartialEq, Eq)]
pub struct GeomKey {
    /// f32 bits — `Pixels` is not `Eq`.
    pub width_bits: u32,
    pub height_bits: u32,
    pub zoom_bits: u32,
    /// Themes are only compared by name; ~40 named themes, all derived, and
    /// `Theme` is `Clone` but not `PartialEq`
    /// (`crates/grove-core/src/theme.rs:17-19`).
    pub theme: SharedString,
}

/// Cheap fingerprint of every input [`TermScene`] depends on. Note the absence
/// of `bounds.origin` — see the [`TermScene`] note.
///
/// Split deliberately: an equal `geom` with a differing tail means the rows are
/// still individually valid, so `prepaint` can re-key them by content hash and
/// reshape only what actually changed.
#[derive(Clone, PartialEq, Eq)]
pub struct TermSceneKey {
    pub geom: GeomKey,
    /// `GroveTerm::damage_generation` (`crates/grove-terminal/src/term.rs:163`),
    /// bumped only when alacritty reported real grid damage. It already gates
    /// repaints (`src/entities/terminal_session.rs:328-338`), so gating render
    /// work on it is consistent.
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

        // The single place PTY dims are decided: a window resize and a zoom
        // change both land here (findings amendment 7). Resize *before* reading
        // the snapshot, never the reverse, so the painted frame matches the
        // dims the PTY was just told about.
        let dims = zoom.pty_dims(f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.session.update(cx, |session, cx| {
            session.resize(dims.0, dims.1);
            // A reattached tmux session deliberately defers its client spawn
            // until here — this is the first moment its *own* tile's dims
            // exist, and in grid view every tile has different ones. Attaching
            // after the resize means the client is born at the final size and
            // the agent never sees an attach-then-relayout SIGWINCH pair
            // (`TerminalSession::attach_now`).
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
        // The scene cache is keyed by the theme *name* only, so the hit path
        // never needs the resolved `Theme` (which is `Clone` but not `Eq`).
        // `with_current` is an atomic-load snapshot, so reading the global
        // name per frame is as free as resolving the theme was.
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
        // Three paths, cheapest first: the full key matches and the whole scene
        // is reused; only `geom` matches and the rows are still individually
        // valid, so they are re-keyed by content hash below; or `geom` moved and
        // every row is discarded.
        let (cached, reusable_rows) = match self.session.read(cx).scene_cache() {
            Some((k, scene)) if *k == key => (Some(scene.clone()), None),
            Some((k, scene)) if k.geom == key.geom => (None, Some(scene.clone())),
            _ => (None, None),
        };

        let scene = match cached {
            // The whole point: no `snapshot()`, no per-cell color resolve, no
            // re-shaping. A tile that produced no bytes costs nothing here.
            Some(scene) => scene,
            None => {
                let snapshot = self.session.read(cx).snapshot();
                // Content-addressed pool of the previous frame's rows. ~48
                // entries, rebuilt per miss; a linear scan would do, the map is
                // just clearer about intent.
                let pool: std::collections::HashMap<u64, Rc<RowScene>> = reusable_rows
                    .iter()
                    .flat_map(|scene| scene.rows.iter())
                    .map(|row| (row.hash, row.clone()))
                    .collect();
                let render_grid = |theme: &Theme| {
                    let rows = snapshot.rows as usize;
                    let cols = snapshot.cols as usize;
                    let mut out: Vec<Rc<RowScene>> = Vec::with_capacity(rows);

                    // Resolved once per cell; every color in the grid goes through
                    // `colors::resolve_pair`, the pipeline's only inverse swap.
                    let mut row_cells: Vec<(char, Hsla, Option<Hsla>, bool)> =
                        Vec::with_capacity(cols);

                    for r in 0..rows {
                        // ponytail: SipHash over the row's raw cells (~4.5k cells
                        // per grid) costs tens of µs — the ceiling. If it ever
                        // shows up in a profile, swap in a faster hasher, or drop
                        // hashing entirely for alacritty's per-line `TermDamage`
                        // ranges, which say directly which rows changed.
                        let mut hasher = DefaultHasher::new();
                        for col in 0..cols {
                            snapshot.cell(r as u16, col as u16).hash(&mut hasher);
                        }
                        let hash = hasher.finish();
                        // The optimization: an unchanged row skips both
                        // `resolve_pair` and `shape_line` outright. Row-local
                        // origins are what make this legal for a row that
                        // scrolled to a different index.
                        if let Some(row) = pool.get(&hash) {
                            out.push(row.clone());
                            continue;
                        }

                        let mut bg_quads: Vec<PaintQuad> = Vec::new();
                        let mut runs: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();
                        // Row-local: `y` is 0 within the row, `paint` adds both
                        // `bounds.origin` and `r * line_height`.
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
                                        point(px(c0 as f32 * cell_w), y),
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
                // The cache lives on the *session*, deliberately not in gpui
                // element state: `with_element_state` is keyed by the full
                // ancestor `GlobalElementId` path, and the terminal sits under
                // `div().id(format!("grid-tile-{tile_idx}"))`
                // (`src/views/grid.rs:253`). That path embeds the tile *slot*,
                // so moving a tile would invalidate the cache on exactly the
                // frames this exists to fix. Do not "fix" this back.
                self.session
                    .update(cx, |session, _| session.set_scene_cache(key, scene.clone()));
                scene
            }
        };

        // 4. Selection overlay, between the text and the cursor. The endpoints
        //    are absolute (scrollback-stable), so they are converted to the
        //    *current* viewport here — which is what makes the highlight stay
        //    on the same text while the view scrolls underneath it.
        //
        //    The wash is the hardcoded `rgba(0.40, 0.50, 0.78, 0.35)` that spec
        //    Appendix A pins — deliberately not a theme token.
        //
        //    Deliberately OUT of the scene cache and rebuilt every frame: it is
        //    cheap, and the cursor below blinks ~2x/second, which would
        //    otherwise bust the text cache on every blink. Their origins stay
        //    ABSOLUTE — there is nothing to translate at paint time.
        //
        //    `dims` replaces the old `snapshot.rows/cols` reads, which are
        //    unavailable on the cache hit path: the term was just resized to
        //    exactly `dims`, so they are equal by construction.
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
        // 1. One full-bounds fill, so short rows and the sub-cell remainder at
        //    the right/bottom edge carry the terminal background.
        window.paint_quad(fill(bounds, c::BG()));
        // The scene is shared with the session's cache, so it is read by
        // reference and translated here — draining it would empty the cache on
        // the first paint. `PaintQuad` is POD-ish, so the clone is cheap.
        //
        // Row origins are row-local, so each row is offset by its index here.
        // ALL backgrounds go down before ANY text: interleaving per row would
        // let one row's background paint over the previous row's descenders.
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
                // `paint` returning `Err` means the line could not be rendered;
                // there is nothing useful to do per-run but skip it.
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

    use crate::fonts::CELL_W;

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
