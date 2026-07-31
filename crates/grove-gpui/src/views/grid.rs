//! The tile grid (Plan 07 Task 4). Port of `src/gui/view/terminal.rs:90-179`
//! (`grid_workspace`) and `:811-1175` (`grid_tile`).
//!
//! **Free render functions, not a `Render` entity** — the same deviation
//! [`crate::views::appbar`] records: every input is already an entity the
//! [`crate::views::workspace::Workspace`] holds and observes, and clicks travel
//! back out through [`GridAction`].
//!
//! **Supersession (carried amendment 7).** iced routes keyboard, scroll,
//! selection and copy through `focused_session`/`selection_pane`
//! (`terminal.rs:1180-1198`), which branch on `grid_view`/`grid_focused`
//! because iced has no focus system. In gpui every tile hosts the **same**
//! `TerminalView` entity the single-session view would (one per `SessionId`,
//! memoized by `Workspace`), each owning a `FocusHandle`, and all four follow
//! gpui focus. `grid_focused` and the focused handle are kept in lockstep by
//! [`crate::views::workspace::Workspace`]: focusing a tile focuses its view.
//! Those two iced functions are therefore **not ported**.
//!
//! **The slide is paint-time (carried amendment 6).** iced needed
//! `src/gui/slide.rs`, a whole custom `Widget`, to translate drawing without
//! perturbing layout. gpui gets it from CSS-relative positioning: an `inset` on
//! a normally-positioned (relative) element offsets where it is drawn and
//! leaves every sibling's layout untouched.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, App, Entity, Hsla, MouseButton, MouseDownEvent, Window,
};

use crate::entities::session_registry::SessionId;
use crate::entities::workspace_state::{GridDrag, GridSlide};
use crate::grid::{grid_layout, slide_progress};
use crate::icons::icon;
use crate::keymap::platform_mod_label;
use crate::theme as c;
use crate::views::terminal_view::TerminalView;

/// Height of the tile header bar (`src/gui/metrics.rs:51`).
pub const TILE_HEAD_H: f32 = 22.0;
/// Horizontal padding inside each tile's PTY container — `pty()`'s own 16×2
/// (`src/gui/metrics.rs:53-54`).
pub const TILE_PTY_PAD_W: f32 = 32.0;
/// Vertical padding inside each tile's PTY container — `pty()`'s 12×2
/// (`src/gui/metrics.rs:55-56`).
pub const TILE_PTY_PAD_H: f32 = 24.0;

/// Horizontal padding inside the **single-session** / terminal-tab PTY
/// container. `src/gui/metrics.rs:21-22` defines `PTY_PAD_W = 36.0` /
/// `PTY_PAD_H = 28.0` and `compute_pty_dims` (`metrics.rs:265-295`) subtracts
/// them from the viewport to derive `(rows, cols)`. Those two numbers are a
/// **fudge constant, not a container padding**: iced's `pty()` container is
/// padded `[12, 16]` (`src/gui/view/terminal.rs:790`) — i.e. 32×24, the same
/// as `TILE_PTY_PAD_*` — so the extra 4px per axis is slack that keeps the
/// iced scrollable from ever showing a scrollbar.
///
/// grove-gpui's terminal element derives its grid from its own post-layout
/// bounds, so reproducing iced's *result* means padding the container by the
/// full fudge constant, half per side. Cited here because `src/gui/metrics.rs`
/// is deleted in this plan's Phase C and this comment is the only record that
/// survives.
pub const PTY_PAD_W: f32 = 36.0;
/// Vertical half of the same fudge constant (`src/gui/metrics.rs:22`).
pub const PTY_PAD_H: f32 = 28.0;

/// What a click inside the grid asks the workspace to do. Tiles never reach
/// into state themselves (the `rows::RowAction` contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridAction {
    /// A press on a tile's header or scrim: focus + acknowledge + arm a drag
    /// (`layout.rs:308-321`).
    Press(usize),
    /// The pointer entered a tile; a no-op unless a drag is armed. The
    /// release edge is not a `GridAction`: it is listened for at the root
    /// (`Workspace::on_root_mouse_up`), because a pointer released outside
    /// the tile it started on must still commit (`layout.rs:323-342`).
    Hover(usize),
    /// The tile's own zen button (`layout.rs:344-356`).
    TileZen(SessionId),
    /// The tile's kill button — two-step, exactly like the session bar's.
    RequestKill(SessionId),
    Kill(SessionId),
}

pub type GridDispatch = Rc<dyn Fn(GridAction, &mut Window, &mut App)>;

/// One tile, resolved once per frame by the workspace.
pub struct TileData {
    pub id: SessionId,
    pub agent_label: &'static str,
    pub icon_name: &'static str,
    pub project: String,
    /// Blank for branchless sessions, which skip the segment entirely.
    pub branch: String,
    pub waiting: bool,
    pub focused: bool,
    pub confirming_kill: bool,
    /// The **same** entity the single-session body would use.
    pub view: Entity<TerminalView>,
}

pub struct GridCtx {
    pub tiles: Vec<TileData>,
    /// The 480ms attention pulse, for the respond chip.
    pub pulse: f32,
    /// The 40-tick triangle wave the scrim breathes on —
    /// [`crate::entities::animation_clock::toast_pulse`]'s first consumer.
    pub scrim_pulse: f32,
    pub drag: Option<GridDrag>,
    pub slide: Option<GridSlide>,
    /// Nominal tile size in logical px, for the slide's draw offset.
    pub tile_size: (f32, f32),
    pub dispatch: GridDispatch,
}

/// The shared "nothing here" panel (`src/gui/widgets/primitives.rs:232-251`).
pub fn empty_state(title: &'static str, subtitle: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .size_full()
        .bg(c::BG())
        .child(crate::views::rows::ui_text(title, 14.0, c::FG_DIM()))
        .child(crate::views::rows::ui_text(subtitle, 12.0, c::FG_MUTE()))
        .into_any_element()
}

fn mono(content: impl Into<gpui::SharedString>, size: f32, color: Hsla) -> gpui::Div {
    div()
        .font(gpui::font(crate::fonts::MONO_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into())
}

fn on_grid(
    dispatch: &GridDispatch,
    action: GridAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

/// The columns-of-tiles workspace (`terminal.rs:90-179`).
///
/// `grid_layout(n)` gives `(cols, rows)`; the render walks **columns**, and each
/// column stacks only the tiles whose row-major index `row * cols + col` is
/// `< n`. That is why a 3-session grid puts one full-height tile beside a
/// 2-stack — and why per-tile PTY dims fall out for free (carried amendment 1):
/// each `TerminalElement` sizes itself from its own post-layout bounds.
pub fn grid(ctx: &GridCtx) -> AnyElement {
    let n = ctx.tiles.len();
    if n == 0 {
        return empty_state(
            "no session selected",
            "click a worktree's start button to spawn an agent",
        );
    }
    let (cols, rows) = grid_layout(n);

    let mut columns = div()
        .flex()
        .flex_row()
        .size_full()
        // 1px of the container's BORDER_SOFT background shows through as the
        // inter-tile gap (`terminal.rs:165-173`).
        .gap(px(1.0))
        .bg(c::BORDER_SOFT());

    for col_idx in 0..cols {
        let mut column = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .gap(px(1.0));
        for row_idx in 0..rows {
            let tile_idx = row_idx * cols + col_idx;
            let Some(data) = ctx.tiles.get(tile_idx) else {
                continue;
            };
            column = column.child(tile(tile_idx, data, ctx));
        }
        columns = columns.child(column);
    }
    columns.into_any_element()
}

/// The draw-only offset for a tile mid-slide, in logical px, or `None` once the
/// animation has settled. Port of `terminal.rs:127-158`.
fn slide_offset(tile_idx: usize, ctx: &GridCtx) -> Option<(f32, f32)> {
    let slide = ctx.slide?;
    let &(_, d_col, d_row) = slide.tiles.iter().find(|(idx, ..)| *idx == tile_idx)?;
    let t = slide_progress(slide.start, std::time::Instant::now());
    if t >= 1.0 {
        return None;
    }
    let remaining = 1.0 - t;
    let (tile_w, tile_h) = ctx.tile_size;
    Some((
        d_col as f32 * (tile_w + 1.0) * remaining,
        d_row as f32 * (tile_h + 1.0) * remaining,
    ))
}

fn tile(tile_idx: usize, data: &TileData, ctx: &GridCtx) -> AnyElement {
    let is_drag_src = ctx.drag.is_some_and(|d| d.source_idx == tile_idx);
    let is_drop_zone = ctx
        .drag
        .is_some_and(|d| d.hover_idx == tile_idx && d.source_idx != tile_idx);

    // Waiting wins over focused — attention beats focus (`terminal.rs:1082-1090`).
    let (border_color, border_w) = if data.waiting {
        (c::AMBER(), 1.5)
    } else if data.focused {
        (c::CYAN(), 1.5)
    } else {
        (gpui::transparent_black(), 0.0)
    };

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .child(tile_header(tile_idx, data, ctx))
        .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
        .child(
            // The tile's PTY, padded exactly as iced's `pty()` is
            // (`metrics.rs:53-56`) so a tile's cell grid matches the iced
            // build's; the element derives its own dims from these bounds.
            div()
                .flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .px(px(TILE_PTY_PAD_W / 2.0))
                .py(px(TILE_PTY_PAD_H / 2.0))
                .child(data.view.clone()),
        );

    let dispatch = Rc::clone(&ctx.dispatch);
    let mut root = div()
        .id(gpui::SharedString::from(format!("grid-tile-{tile_idx}")))
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .w_full()
        .overflow_hidden()
        .bg(c::BG())
        .border(px(border_w))
        .border_color(border_color)
        // `on_enter` fires even while a button is held; the handler ignores it
        // when no drag is armed (`layout.rs:323-328`).
        .on_hover(move |hovered, window, cx| {
            if *hovered {
                dispatch(GridAction::Hover(tile_idx), window, cx);
            }
        })
        .child(body);

    if is_drop_zone {
        root = root.child(overlay().border(px(1.5)).border_color(c::CYAN()).bg(Hsla {
            a: 0.06,
            ..c::CYAN()
        }));
    }
    if is_drag_src {
        root = root.child(overlay().bg(Hsla { a: 0.72, ..c::BG() }));
    }
    if data.waiting {
        root = root.child(scrim(tile_idx, ctx));
    }

    // The slide: a relative inset moves the drawing, not the layout.
    if let Some((dx, dy)) = slide_offset(tile_idx, ctx) {
        root = root.left(px(dx)).top(px(dy));
    }
    root.into_any_element()
}

/// A full-tile absolutely positioned layer.
fn overlay() -> gpui::Div {
    div().absolute().top(px(0.0)).left(px(0.0)).size_full()
}

/// `terminal.rs:988-1018`. A denser variant of the session bar's identity row —
/// see [`crate::views::session_header`], which Plan 06 built parameterized by
/// session for exactly this.
fn tile_header(tile_idx: usize, data: &TileData, ctx: &GridCtx) -> AnyElement {
    let mut identity = div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .overflow_hidden()
        .child(icon(data.icon_name, 11.0, c::FG_DIM()))
        .child(
            crate::views::rows::ui_text(data.agent_label, 10.0, c::FG_DIM())
                .font_weight(gpui::FontWeight::SEMIBOLD),
        )
        .child(crate::views::rows::ui_text("·", 10.0, c::FG_MUTE()))
        .child(crate::views::rows::ui_text(
            data.project.clone(),
            10.0,
            c::FG_MUTE(),
        ));
    // Branchless sessions skip the segment entirely — otherwise the header
    // shows a trailing dot with nothing after it.
    if !data.branch.trim().is_empty() {
        identity = identity
            .child(crate::views::rows::ui_text("·", 10.0, c::FG_MUTE()))
            .child(crate::views::rows::ui_text(
                data.branch.clone(),
                10.0,
                c::FG_MUTE(),
            ));
    }

    let kill_color = if data.confirming_kill {
        c::RED()
    } else {
        c::FG_MUTE()
    };
    let kill_action = if data.confirming_kill {
        GridAction::Kill(data.id)
    } else {
        GridAction::RequestKill(data.id)
    };

    div()
        .id(gpui::SharedString::from(format!("tile-head-{tile_idx}")))
        .h(px(TILE_HEAD_H))
        .w_full()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(6.0))
        .bg(if data.focused {
            c::BG_HL()
        } else {
            c::BG_STRIP()
        })
        .overflow_hidden()
        .child(identity)
        .child(div().flex_1())
        .when(data.waiting, |d| d.child(respond_chip(tile_idx, ctx.pulse)))
        .when(tile_idx < 9, |d| d.child(num_hint(tile_idx, data.focused)))
        .child(tile_btn(
            format!("tile-zen-{tile_idx}"),
            "zen",
            c::FG_MUTE(),
            &ctx.dispatch,
            GridAction::TileZen(data.id),
        ))
        .child(tile_btn(
            format!("tile-kill-{tile_idx}"),
            "trash",
            kill_color,
            &ctx.dispatch,
            kill_action,
        ))
        // A press anywhere on the header focuses the tile and arms the drag.
        .on_mouse_down(
            MouseButton::Left,
            on_grid(&ctx.dispatch, GridAction::Press(tile_idx)),
        )
        .into_any_element()
}

fn tile_btn(
    id: String,
    icon_name: &'static str,
    color: Hsla,
    dispatch: &GridDispatch,
    action: GridAction,
) -> AnyElement {
    div()
        .id(gpui::SharedString::from(id))
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(icon(icon_name, 10.0, color))
        .on_mouse_down(MouseButton::Left, on_grid(dispatch, action))
        .into_any_element()
}

/// `"{mod}+{n}"` for the first nine tiles, as the **registry** spells the
/// modifier — never a literal (`terminal.rs:934-974`).
#[must_use]
pub fn num_hint_label(tile_idx: usize) -> Option<String> {
    (tile_idx < 9).then(|| format!("{}+{}", platform_mod_label(), tile_idx + 1))
}

fn chord(tile_idx: usize, size: f32, color: Hsla) -> AnyElement {
    let n = tile_idx + 1;
    if cfg!(target_os = "macos") {
        return div()
            .flex()
            .items_center()
            .gap(px(1.0))
            .child(icon("command", size, color))
            .child(mono(n.to_string(), size, color))
            .into_any_element();
    }
    mono(format!("{}+{n}", platform_mod_label()), size, color).into_any_element()
}

fn num_hint(tile_idx: usize, focused: bool) -> AnyElement {
    let color = if focused { c::FG_DIM() } else { c::FG_MUTE() };
    div()
        .flex()
        .items_center()
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(c::BG())
        .border_1()
        .border_color(c::BORDER())
        .child(chord(tile_idx, 9.0, color))
        .into_any_element()
}

/// The respond chip's amber alpha (`terminal.rs:882`).
#[must_use]
pub fn respond_alpha(pulse: f32) -> f32 {
    0.35f32.mul_add(-pulse, 1.0)
}

/// `"respond · "` for the first nine tiles (the chord follows), a bare
/// `"respond"` beyond them (`terminal.rs:888-911`).
#[must_use]
pub fn respond_label(tile_idx: usize) -> &'static str {
    if tile_idx < 9 {
        "respond · "
    } else {
        "respond"
    }
}

fn respond_chip(tile_idx: usize, pulse: f32) -> AnyElement {
    let a = respond_alpha(pulse);
    let amber = Hsla { a, ..c::AMBER() };
    let amber_bg = Hsla {
        a: a * 0.08,
        ..c::AMBER()
    };
    div()
        .flex()
        .items_center()
        .gap(px(1.0))
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(amber_bg)
        .border_1()
        .border_color(amber)
        .child(mono(respond_label(tile_idx), 9.0, amber))
        .when(tile_idx < 9, |d| d.child(chord(tile_idx, 9.0, amber)))
        .into_any_element()
}

/// The scrim's text alpha: a 0.7..1.0 breathe off the 40-tick triangle wave
/// (`terminal.rs:1097-1100`).
#[must_use]
pub fn scrim_alpha(scrim_pulse: f32) -> f32 {
    0.3f32.mul_add(scrim_pulse, 0.7)
}

/// `"click to respond · {mod}+{n}"` for the first nine tiles, else the bare
/// instruction (`terminal.rs:1106-1115`).
#[must_use]
pub fn scrim_sub_line(tile_idx: usize) -> String {
    num_hint_label(tile_idx).map_or_else(
        || "click to respond".to_string(),
        |chord| format!("click to respond · {chord}"),
    )
}

/// The full-tile "needs attention" overlay (`terminal.rs:1092-1155`). The
/// letters are spaced **by the literal** — gpui, like iced, has no
/// letter-spacing — and the wash is the theme's deepest surface at 0.92 rather
/// than a blur, which neither toolkit has.
fn scrim(tile_idx: usize, ctx: &GridCtx) -> AnyElement {
    let amber = Hsla {
        a: scrim_alpha(ctx.scrim_pulse),
        ..c::AMBER()
    };
    overlay()
        .id(gpui::SharedString::from(format!("tile-scrim-{tile_idx}")))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .bg(Hsla {
            a: 0.92,
            ..c::BG_STRIP()
        })
        .child(
            crate::views::rows::ui_text("N E E D S   A T T E N T I O N", 20.0, amber)
                .font_weight(gpui::FontWeight::SEMIBOLD),
        )
        .child(mono(scrim_sub_line(tile_idx), 10.0, c::FG_MUTE()))
        // Clicking the scrim focuses/acknowledges the tile, exactly like
        // clicking its header.
        .on_mouse_down(
            MouseButton::Left,
            on_grid(&ctx.dispatch, GridAction::Press(tile_idx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{CELL_H, CELL_W};
    use crate::views::appbar::APPBAR_H;
    use crate::views::statusbar::STATUS_H;

    /// `terminal.rs:882` — the chip dims but never vanishes.
    #[test]
    fn the_respond_chip_dims_without_disappearing() {
        assert!((respond_alpha(0.0) - 1.0).abs() < 1e-6);
        assert!((respond_alpha(1.0) - 0.65).abs() < 1e-6);
        for i in 0..=100 {
            let a = respond_alpha(i as f32 / 100.0);
            assert!((0.65..=1.0).contains(&a), "{a}");
        }
    }

    /// `terminal.rs:1097-1100` — 0.7..1.0 off the 40-tick triangle wave.
    #[test]
    fn the_scrim_breathes_between_seventy_and_one_hundred_percent() {
        assert!((scrim_alpha(0.0) - 0.7).abs() < 1e-6);
        assert!((scrim_alpha(1.0) - 1.0).abs() < 1e-6);
        assert!((scrim_alpha(0.5) - 0.85).abs() < 1e-6);
    }

    /// `terminal.rs:888-911,1106-1115` — the tenth tile and beyond lose the
    /// chord, never render `mod+10`.
    #[test]
    fn only_the_first_nine_tiles_advertise_a_chord() {
        assert_eq!(respond_label(0), "respond · ");
        assert_eq!(respond_label(8), "respond · ");
        assert_eq!(respond_label(9), "respond");
        assert!(num_hint_label(9).is_none());
        assert_eq!(scrim_sub_line(9), "click to respond");
        let Some(first) = num_hint_label(0) else {
            unreachable!("tile 0 has a chord");
        };
        assert!(first.ends_with("+1"));
        // The modifier comes from the registry, never a literal.
        assert!(first.starts_with(platform_mod_label()));
        assert_eq!(scrim_sub_line(0), format!("click to respond · {first}"));
    }

    // ── carried amendment 2: the grid parity assertion ───────────────────

    /// `grid_tile_cols` (`src/gui/metrics.rs:336-344`), reimplemented **here**
    /// as the oracle — never exported from production code (amendment 1).
    fn oracle_tile_cols(win_w: f32, zoom: f32, n: usize) -> u16 {
        let (grid_cols, _) = grid_layout(n);
        let zoom = zoom.max(0.1);
        let workspace_w = win_w / zoom;
        let tile_w = (workspace_w - (grid_cols as f32 - 1.0)) / grid_cols as f32;
        let pty_w = tile_w - TILE_PTY_PAD_W;
        (pty_w / CELL_W).max(10.0) as u16
    }

    /// `grid_tile_rows_for_col` (`src/gui/metrics.rs:351-360`).
    fn oracle_tile_rows(win_h: f32, zoom: f32, tiles_in_col: usize) -> u16 {
        let zoom = zoom.max(0.1);
        let workspace_h = win_h / zoom - APPBAR_H - STATUS_H;
        let k = tiles_in_col.max(1) as f32;
        let tile_h = (workspace_h - (k - 1.0)) / k;
        let pty_h = tile_h - TILE_HEAD_H - TILE_PTY_PAD_H;
        (pty_h / CELL_H).max(4.0) as u16
    }

    /// What the **gpui** layout actually hands the element: the same nominal
    /// window, walked through this module's own constants and
    /// [`crate::zoom::ZoomState::pty_dims`], which is what every
    /// `TerminalElement` uses on its post-layout bounds.
    fn gpui_tile_dims(
        win_w: f32,
        win_h: f32,
        zoom: f32,
        n: usize,
        tiles_in_col: usize,
    ) -> (u16, u16) {
        let z = crate::zoom::ZoomState::new(zoom);
        let (cols, _) = grid_layout(n);
        // Inter-tile gaps: 1px between columns, 1px between stacked tiles.
        let tile_w = (win_w - (cols as f32 - 1.0)) / cols as f32;
        let k = tiles_in_col.max(1) as f32;
        let tile_h = (win_h - APPBAR_H - STATUS_H - (k - 1.0)) / k;
        // The header, its hairline, and the PTY container's own padding.
        let pty_w = tile_w - TILE_PTY_PAD_W;
        let pty_h = tile_h - TILE_HEAD_H - 1.0 - TILE_PTY_PAD_H;
        z.pty_dims(pty_w * zoom, pty_h * zoom)
    }

    /// Carried amendment 2: a nominal 1280×800 window at zoom 1.0 must land
    /// within **±1 cell** of the iced oracle for a 2-, 3- and 5-tile grid. A
    /// larger divergence is a real layout bug, not a tolerance to widen.
    #[test]
    fn grid_tile_dims_match_the_iced_oracle_within_one_cell() {
        for (n, tiles_in_col) in [(2usize, 1usize), (3, 2), (3, 1), (5, 2), (5, 1)] {
            let (rows, cols) = gpui_tile_dims(1280.0, 800.0, 1.0, n, tiles_in_col);
            let want_cols = oracle_tile_cols(1280.0, 1.0, n);
            let want_rows = oracle_tile_rows(800.0, 1.0, tiles_in_col);
            assert!(
                cols.abs_diff(want_cols) <= 1,
                "n={n}: cols {cols} vs oracle {want_cols}"
            );
            assert!(
                rows.abs_diff(want_rows) <= 1,
                "n={n}/{tiles_in_col}: rows {rows} vs oracle {want_rows}"
            );
        }
    }

    /// The ragged-grid promise the columns-of-tiles layout exists for: with 3
    /// sessions the lone right-hand tile gets roughly twice the rows of the
    /// stacked pair (`metrics.rs:589-604`).
    #[test]
    fn a_short_column_gets_a_taller_pty() {
        let (full, _) = gpui_tile_dims(1280.0, 800.0, 1.0, 3, 1);
        let (half, _) = gpui_tile_dims(1280.0, 800.0, 1.0, 3, 2);
        assert!(full > half);
        assert!(full >= 2 * half - 4);
    }
}
