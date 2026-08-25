//! The tile grid. Port of `terminal.rs:90-179,811-1175`. Free render functions, not `Render`; gpui focus replaces `grid_focused`; the slide is a paint-time `inset`.

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, App, CursorStyle, Entity, Hsla, MouseButton, MouseDownEvent,
    Window,
};

use crate::entities::session_registry::SessionId;
use crate::entities::workspace_state::{GridDrag, GridSizing, GridSlide};
use crate::grid::{grid_layout, slide_progress, weighted_spans, GridAxis, GridBoundary};
use crate::icons::icon;
use crate::keymap::platform_mod_label;
use crate::theme as c;
use crate::views::components::{divider_h, icon_btn, keycap_filled, mono, tracked, ui};
use crate::views::session_header;
use crate::views::terminal_view::TerminalView;

/// Height of the tile header bar (`src/gui/metrics.rs:51`).
pub const TILE_HEAD_H: f32 = 22.0;
/// Below [`CONTROL_H`] deliberately: must fit inside the [`TILE_HEAD_H`] bar.
pub const TILE_BTN_BOX: f32 = 18.0;
/// Horizontal padding inside each tile's PTY container (`src/gui/metrics.rs:53-54`).
pub const TILE_PTY_PAD_W: f32 = 32.0;
/// Vertical padding inside each tile's PTY container (`src/gui/metrics.rs:55-56`).
pub const TILE_PTY_PAD_H: f32 = 24.0;

/// A fudge constant, not real padding — `src/gui/metrics.rs:21-22`, half applied per side to match iced's derived `(rows, cols)`. Only surviving record; metrics.rs is deleted.
pub const PTY_PAD_W: f32 = 36.0;
/// Vertical half of the same fudge constant (`src/gui/metrics.rs:22`).
pub const PTY_PAD_H: f32 = 28.0;

/// The hit zone extends equally beyond the 1px seam without taking layout space.
const DIVIDER_HIT_OFFSET: f32 = (DIVIDER_DRAG_HIT_W - GRID_SEAM_PX) / 2.0;
/// Grid seams are physical hairlines and do not scale with application zoom.
pub const GRID_SEAM_PX: f32 = 1.0;
/// Waiting emphasis is one physical hairline on every edge.
pub const WAITING_BORDER_PX: f32 = 1.0;
/// Header/body separation is a physical hairline supplied by [`divider_h`].
pub const TILE_HEADER_DIVIDER_PX: f32 = 1.0;
/// Covers flex pixel snapping so a clamped tile cannot round one device pixel below its PTY floor.
pub const GRID_TILE_SNAP_GUARD_PX: f32 = 1.0;
/// Column handles deliberately paint above row handles at divider junctions.
const DIVIDER_JUNCTION_AXIS: GridAxis = GridAxis::Columns;
/// Existing PTY sizing floors use ten columns and four rows; resize clamping uses the same contract.
pub const GRID_MIN_PTY_COLS: f32 = 10.0;
pub const GRID_MIN_PTY_ROWS: f32 = 4.0;

/// What a click inside the grid asks the workspace to do; tiles never touch state directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridAction {
    /// Focus + acknowledge + arm a drag (`layout.rs:308-321`).
    Press(usize),
    /// A body press: focus + acknowledge only, no drag armed.
    Focus(usize),
    /// No-op unless a drag is armed; the release is handled at the root instead (`Workspace::on_root_mouse_up`, `layout.rs:323-342`).
    Hover(usize),
    /// A clean double-click resets this one split; a single press starts root-owned resizing.
    ResizePress {
        boundary: GridBoundary,
        x: f32,
        y: f32,
        click_count: usize,
    },
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
    /// Sanitized OSC title; same string the session bar derives via `rows::session_context`.
    pub context: Option<String>,
    /// Gates the in-progress dots — a dead session's stale title must not animate.
    pub running: bool,
    /// Decided by [`fit_segments`] from this session's own measured widths, not a fixed threshold.
    pub fit: HeaderFit,
    /// Uncommitted diff vs `HEAD`; `None` (no poll yet) draws nothing, like [`crate::views::rows::diff_chips`].
    pub diff: Option<(u32, u32)>,
    /// The same entity the single-session body uses.
    pub view: Entity<TerminalView>,
}

pub struct GridCtx {
    pub tiles: Vec<TileData>,
    /// The 480ms attention pulse, for the respond chip.
    pub pulse: f32,
    /// The 40-tick triangle wave the scrim breathes on.
    pub scrim_pulse: f32,
    /// Raw tick for the title zone's dot walk; `pulse`/`scrim_pulse` can't recover it.
    pub tick: u64,
    pub drag: Option<GridDrag>,
    pub sizing: GridSizing,
    pub slide: Option<GridSlide>,
    /// Physical grid content size after the appbar/statusbar have taken their space.
    pub grid_size: (f32, f32),
    pub dispatch: GridDispatch,
}

/// Which identity segments survive; each stays whole (dropped, never shrunk) — only the title truncates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderFit {
    pub project: bool,
    pub branch: bool,
    pub title: bool,
}

/// Measured widths in device px, including the leading `·` and its flex gaps; a blank segment records `0.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SegmentWidths {
    pub project: f32,
    pub branch: f32,
    /// Only consulted for blankness — the title zone truncates instead of dropping.
    pub title: f32,
}

/// Below this leftover budget (device px) the title is dropped rather than truncated to a useless sliver.
const MIN_TITLE_PX: f32 = 48.0;

/// Re-add margin (device px): a segment drops the moment it overflows but needs this much slack to come back, so a jittering width can't oscillate it.
const HYSTERESIS_PX: f32 = 10.0;

/// Fits optional segments by priority (project, then branch); `prev` supplies hysteresis, `None` starts pessimistic.
#[must_use]
pub fn fit_segments(budget: f32, seg: &SegmentWidths, prev: Option<HeaderFit>) -> HeaderFit {
    let prev = prev.unwrap_or_default();
    // A shown segment's drop threshold is its bare width; a hidden one's re-add threshold is that plus the margin.
    let keep = |w: f32, shown: bool, remaining: f32| {
        w > 0.0 && remaining >= if shown { w } else { w + HYSTERESIS_PX }
    };

    let mut remaining = budget;
    let project = keep(seg.project, prev.project, remaining);
    if project {
        remaining -= seg.project;
    }
    let branch = keep(seg.branch, prev.branch, remaining);
    if branch {
        remaining -= seg.branch;
    }
    // The title's threshold is a floor on the space left for it rather than its own width, since it truncates instead of dropping whole.
    let title_floor = if prev.title {
        MIN_TITLE_PX
    } else {
        MIN_TITLE_PX + HYSTERESIS_PX
    };
    HeaderFit {
        project,
        branch,
        title: seg.title > 0.0 && remaining >= title_floor,
    }
}

/// The shared "nothing here" panel (`src/gui/widgets/primitives.rs:232-251`).
pub fn empty_state(title: &'static str, subtitle: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(rpx(SPACE_MD))
        .size_full()
        .bg(c::BG())
        .child(ui(title, TEXT_TITLE, c::FG_DIM()))
        .child(ui(subtitle, TEXT_BODY, c::FG_MUTE()))
        .into_any_element()
}

fn on_grid(
    dispatch: &GridDispatch,
    action: GridAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

/// Walks columns, stacking tiles whose row-major index `row * cols + col` is `< n` — why a 3-session grid puts one full-height tile beside a 2-stack (`terminal.rs:90-179`).
pub fn grid(ctx: &GridCtx) -> AnyElement {
    let n = ctx.tiles.len();
    if n == 0 {
        return empty_state(
            "no session selected",
            "click a worktree's start button to spawn an agent",
        );
    }
    let (cols, rows) = grid_layout(n);

    let mut columns = div().flex().flex_row().size_full().bg(c::BORDER_SOFT());

    for col_idx in 0..cols {
        let column_weight = ctx
            .sizing
            .column_weights()
            .get(col_idx)
            .copied()
            .unwrap_or(1.0);
        let mut column = div()
            .flex()
            .flex_col()
            .flex_basis(px(0.0))
            .flex_grow(column_weight)
            .h_full()
            .overflow_hidden();
        let row_weights = ctx.sizing.row_weights(col_idx);
        let row_count = (col_idx..n).step_by(cols).count();
        for row_idx in 0..rows {
            let tile_idx = row_idx * cols + col_idx;
            let Some(data) = ctx.tiles.get(tile_idx) else {
                continue;
            };
            let row_weight = row_weights.get(row_idx).copied().unwrap_or(1.0);
            column = column.child(tile(tile_idx, row_weight, data, ctx));
            if row_idx + 1 < row_count {
                column = column.child(row_divider());
            }
        }
        columns = columns.child(column);
        if col_idx + 1 < cols {
            columns = columns.child(column_divider());
        }
    }
    div()
        .relative()
        .size_full()
        .child(columns)
        // Hit zones paint last so both halves of each 6px target win over adjacent terminal views.
        .child(resize_handles(ctx, n, cols, rows))
        .into_any_element()
}

fn divider_press(
    boundary: GridBoundary,
    ctx: &GridCtx,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(&ctx.dispatch);
    move |event, window, cx| {
        cx.stop_propagation();
        dispatch(
            GridAction::ResizePress {
                boundary,
                x: f32::from(event.position.x),
                y: f32::from(event.position.y),
                click_count: event.click_count,
            },
            window,
            cx,
        );
    }
}

fn column_divider() -> AnyElement {
    div()
        .flex_shrink_0()
        .w(px(GRID_SEAM_PX))
        .h_full()
        .bg(c::BORDER_SOFT())
        .into_any_element()
}

fn row_divider() -> AnyElement {
    div()
        .flex_shrink_0()
        .w_full()
        .h(px(GRID_SEAM_PX))
        .bg(c::BORDER_SOFT())
        .into_any_element()
}

fn resize_handles(ctx: &GridCtx, tile_count: usize, cols: usize, rows: usize) -> AnyElement {
    // Axis ownership is deterministic at intersections: row targets paint first and column
    // targets paint last, so the column cursor/action wins across the complete 6px junction.
    debug_assert_eq!(DIVIDER_JUNCTION_AXIS, GridAxis::Columns);
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .child(row_resize_handles(ctx, tile_count, cols, rows))
        .child(column_resize_handles(ctx, cols))
        .into_any_element()
}

fn row_resize_handles(ctx: &GridCtx, tile_count: usize, cols: usize, rows: usize) -> AnyElement {
    let mut layer = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .flex()
        .flex_row();
    for column in 0..cols {
        let column_weight = ctx
            .sizing
            .column_weights()
            .get(column)
            .copied()
            .unwrap_or(1.0);
        let row_weights = ctx.sizing.row_weights(column);
        let row_count = (column..tile_count).step_by(cols).count();
        let mut row_guides = div()
            .flex()
            .flex_col()
            .flex_basis(px(0.0))
            .flex_grow(column_weight)
            .h_full();
        for row in 0..rows {
            if row >= row_count {
                continue;
            }
            row_guides = row_guides.child(
                div()
                    .flex_basis(px(0.0))
                    .flex_grow(row_weights.get(row).copied().unwrap_or(1.0))
                    .w_full(),
            );
            if row + 1 < row_count {
                let split = GridBoundary {
                    axis: GridAxis::Rows,
                    boundary: row,
                    column: Some(column),
                };
                row_guides = row_guides.child(
                    div().relative().flex_shrink_0().w_full().h(px(1.0)).child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "grid-row-handle-{column}-{row}"
                            )))
                            .absolute()
                            .left(px(0.0))
                            .top(px(-DIVIDER_HIT_OFFSET))
                            .w_full()
                            .h(px(DIVIDER_DRAG_HIT_W))
                            .cursor(CursorStyle::ResizeUpDown)
                            .on_mouse_down(MouseButton::Left, divider_press(split, ctx)),
                    ),
                );
            }
        }
        layer = layer.child(row_guides);
        if column + 1 < cols {
            layer = layer.child(div().flex_shrink_0().w(px(GRID_SEAM_PX)).h_full());
        }
    }
    layer.into_any_element()
}

fn column_resize_handles(ctx: &GridCtx, cols: usize) -> AnyElement {
    let mut layer = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .flex()
        .flex_row();
    for column in 0..cols {
        let column_weight = ctx
            .sizing
            .column_weights()
            .get(column)
            .copied()
            .unwrap_or(1.0);
        layer = layer.child(div().flex_basis(px(0.0)).flex_grow(column_weight).h_full());
        if column + 1 < cols {
            let split = GridBoundary {
                axis: GridAxis::Columns,
                boundary: column,
                column: None,
            };
            layer = layer.child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(GRID_SEAM_PX))
                    .h_full()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "grid-column-handle-{column}"
                            )))
                            .absolute()
                            .top(px(0.0))
                            .left(px(-DIVIDER_HIT_OFFSET))
                            .w(px(DIVIDER_DRAG_HIT_W))
                            .h_full()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_mouse_down(MouseButton::Left, divider_press(split, ctx)),
                    ),
            );
        }
    }
    layer.into_any_element()
}

fn weighted_slide_delta(
    total_px: f32,
    current_weights: &[f32],
    current_index: usize,
    original_weights: &[f32],
    original_index: usize,
) -> Option<f32> {
    let current = weighted_spans(total_px, GRID_SEAM_PX, current_weights)
        .get(current_index)?
        .start;
    let original = weighted_spans(total_px, GRID_SEAM_PX, original_weights)
        .get(original_index)?
        .start;
    Some(original - current)
}

/// The draw-only offset for a tile mid-slide, in physical px, or `None` once the animation has settled. Port of `terminal.rs:127-158`.
fn slide_offset(tile_idx: usize, ctx: &GridCtx) -> Option<(f32, f32)> {
    let slide = ctx.slide?;
    let &(_, d_col, d_row) = slide.tiles.iter().find(|(idx, ..)| *idx == tile_idx)?;
    let t = slide_progress(slide.start, std::time::Instant::now());
    if t >= 1.0 {
        return None;
    }
    let remaining = 1.0 - t;
    let (cols, _) = grid_layout(ctx.tiles.len());
    let current_col = tile_idx % cols;
    let current_row = tile_idx / cols;
    let original_col = usize::try_from(current_col as i32 + d_col).ok()?;
    let original_row = usize::try_from(current_row as i32 + d_row).ok()?;
    let dx = weighted_slide_delta(
        ctx.grid_size.0,
        ctx.sizing.column_weights(),
        current_col,
        ctx.sizing.column_weights(),
        original_col,
    )?;
    let dy = weighted_slide_delta(
        ctx.grid_size.1,
        ctx.sizing.row_weights(current_col),
        current_row,
        ctx.sizing.row_weights(original_col),
        original_row,
    )?;
    Some((dx * remaining, dy * remaining))
}

fn tile(tile_idx: usize, weight: f32, data: &TileData, ctx: &GridCtx) -> AnyElement {
    let is_drag_src = ctx.drag.is_some_and(|d| d.source_idx == tile_idx);
    let is_drop_zone = ctx
        .drag
        .is_some_and(|d| d.hover_idx == tile_idx && d.source_idx != tile_idx);

    let (border_color, border_w) = if data.waiting {
        // §7.2: exactly one border weight — the hairline. A waiting tile is called out by the amber *tone*, never by a heavier stroke.
        (c::AMBER(), WAITING_BORDER_PX)
    } else {
        (gpui::transparent_black(), 0.0)
    };

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .child(tile_header(tile_idx, data, ctx))
        .child(divider_h())
        .child(
            // Padded exactly as iced's `pty()` (`metrics.rs:53-56`) so the cell grid matches.
            div()
                .flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .px(rpx(TILE_PTY_PAD_W / 2.0))
                .py(rpx(TILE_PTY_PAD_H / 2.0))
                .on_mouse_down(
                    MouseButton::Left,
                    on_grid(&ctx.dispatch, GridAction::Focus(tile_idx)),
                )
                .child(data.view.clone()),
        );

    let dispatch = Rc::clone(&ctx.dispatch);
    let mut root = div()
        .id(gpui::SharedString::from(format!("grid-tile-{tile_idx}")))
        .relative()
        .flex()
        .flex_col()
        .flex_basis(px(0.0))
        .flex_grow(weight)
        .w_full()
        .overflow_hidden()
        .bg(c::BG())
        .border(px(border_w))
        .border_color(border_color)
        // `on_enter` fires even while a button is held; the handler ignores it when no drag is armed (`layout.rs:323-328`).
        .on_hover(move |hovered, window, cx| {
            if *hovered {
                dispatch(GridAction::Hover(tile_idx), window, cx);
            }
        })
        .child(body);

    if is_drop_zone {
        root = root.child(
            overlay()
                .border_1()
                .border_color(c::CYAN())
                .bg(c::alpha(c::CYAN(), 0.06)),
        );
    }
    if is_drag_src {
        root = root.child(overlay().bg(c::alpha(c::BG(), 0.72)));
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

fn overlay() -> gpui::Div {
    div().absolute().top(px(0.0)).left(px(0.0)).size_full()
}

/// `terminal.rs:988-1018`. A denser variant of the session bar's identity row — see [`crate::views::session_header`], which Plan 06 built parameterized by session for exactly this.
fn tile_header(tile_idx: usize, data: &TileData, ctx: &GridCtx) -> AnyElement {
    let fit = data.fit;

    // Identity never truncates — the title zone below absorbs the squeeze.
    let mut identity = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(rpx(SPACE_SM))
        .overflow_hidden()
        .child(icon(data.icon_name, ICON_SM, c::FG_DIM()))
        .child(
            ui(data.agent_label, TEXT_MICRO, c::FG_DIM()).font_weight(gpui::FontWeight::SEMIBOLD),
        );
    if fit.project {
        identity = identity.child(ui("·", TEXT_MICRO, c::FG_MUTE())).child(ui(
            data.project.clone(),
            TEXT_MICRO,
            c::FG_MUTE(),
        ));
    }
    // Branchless sessions skip the segment entirely — otherwise the header shows a trailing dot with nothing after it.
    if fit.branch && !data.branch.trim().is_empty() {
        identity = identity.child(ui("·", TEXT_MICRO, c::FG_MUTE())).child(ui(
            data.branch.clone(),
            TEXT_MICRO,
            c::FG_MUTE(),
        ));
    }

    // Only this element ever truncates.
    let title_zone = if fit.title && data.context.is_some() {
        let title = data.context.as_deref().unwrap_or_default();
        let show_progress = data.running && session_header::is_in_progress_title(title);
        let content: AnyElement = if show_progress {
            let phase = session_header::in_progress_phase(ctx.tick);
            let step = |i: u64| {
                crate::views::components::status_dot(
                    DOT_SM,
                    if i == phase { c::GREEN() } else { c::FG_MUTE() },
                )
            };
            // A partially drawn 3-dot cluster is meaningless — it never shrinks or truncates.
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(step(0))
                .child(step(1))
                .child(step(2))
                .into_any_element()
        } else {
            ui(title, TEXT_MICRO, c::FG_DIM())
                .id(gpui::SharedString::from(format!("tile-ctx-{tile_idx}")))
                .truncate()
                .tooltip({
                    let hint = gpui::SharedString::from(title.to_string());
                    move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
                    }
                })
                .into_any_element()
        };
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(rpx(SPACE_SM))
            .child(ui("·", TEXT_MICRO, c::FG_MUTE()))
            .child(content)
            .into_any_element()
    } else {
        div().flex_1().into_any_element()
    };

    // Arming the kill changes the glyph too (§2.3, §12) — colour alone isn't enough of a signal.
    let (kill_icon, kill_color) = if data.confirming_kill {
        ("question", c::RED())
    } else {
        ("trash", c::FG_MUTE())
    };
    let kill_action = if data.confirming_kill {
        GridAction::Kill(data.id)
    } else {
        GridAction::RequestKill(data.id)
    };

    div()
        .id(gpui::SharedString::from(format!("tile-head-{tile_idx}")))
        .cursor_pointer()
        .h(rpx(TILE_HEAD_H))
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .px(rpx(SPACE_MD))
        .bg(if data.focused {
            c::BG_HL()
        } else {
            c::BG_STRIP()
        })
        // Backstop only — the title zone's `min_w_0()` is what actually absorbs the squeeze.
        .overflow_hidden()
        .child(identity)
        .child(title_zone)
        .when(
            crate::views::rows::diff_display(data.diff) != crate::views::rows::DiffDisplay::Unknown,
            |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .child(crate::views::rows::diff_chips(data.diff)),
                )
            },
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(rpx(SPACE_SM))
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
                    kill_icon,
                    kill_color,
                    &ctx.dispatch,
                    kill_action,
                )),
        )
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
    let d = std::rc::Rc::clone(dispatch);
    icon_btn(
        gpui::SharedString::from(id),
        icon_name,
        TILE_BTN_BOX,
        TILE_BTN_BOX,
        ICON_XS,
        color,
        c::BG_HOVER(),
        None,
        false,
        move |window, cx| d(action, window, cx),
    )
    .into_any_element()
}

/// `"{mod}+{n}"` for the first nine tiles, as the **registry** spells the modifier — never a literal (`terminal.rs:934-974`).
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
    keycap_filled(c::BG(), chord(tile_idx, TEXT_MICRO, color))
        .flex()
        .items_center()
        .border_1()
        .border_color(c::BORDER())
        .into_any_element()
}

/// The respond chip's amber alpha (`terminal.rs:882`).
#[must_use]
pub fn respond_alpha(pulse: f32) -> f32 {
    0.35f32.mul_add(-pulse, 1.0)
}

/// `"respond · "` for the first nine tiles (the chord follows), a bare `"respond"` beyond them (`terminal.rs:888-911`).
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
    let amber = c::alpha(c::AMBER(), a);
    let amber_bg = c::alpha(c::AMBER(), a * 0.08);
    let inner = div()
        .flex()
        .items_center()
        .gap(px(1.0))
        .child(mono(respond_label(tile_idx), TEXT_MICRO, amber))
        .when(tile_idx < 9, |d| {
            d.child(chord(tile_idx, TEXT_MICRO, amber))
        });
    keycap_filled(amber_bg, inner)
        .flex()
        .items_center()
        .border_1()
        .border_color(amber)
        .into_any_element()
}

/// The scrim's text alpha: a 0.7..1.0 breathe off the 40-tick triangle wave (`terminal.rs:1097-1100`).
#[must_use]
pub fn scrim_alpha(scrim_pulse: f32) -> f32 {
    0.3f32.mul_add(scrim_pulse, 0.7)
}

/// `"click to respond · {mod}+{n}"` for the first nine tiles, else the bare instruction (`terminal.rs:1106-1115`).
#[must_use]
pub fn scrim_sub_line(tile_idx: usize) -> String {
    num_hint_label(tile_idx).map_or_else(
        || "click to respond".to_string(),
        |chord| format!("click to respond · {chord}"),
    )
}

/// The full-tile "needs attention" overlay (`terminal.rs:1092-1155`); tracking comes from [`crate::views::rows::tracked`] (U+2009 spaces), not literal spaces (§5.4).
fn scrim(tile_idx: usize, ctx: &GridCtx) -> AnyElement {
    let amber = c::alpha(c::AMBER(), scrim_alpha(ctx.scrim_pulse));
    overlay()
        .id(gpui::SharedString::from(format!("tile-scrim-{tile_idx}")))
        .cursor_pointer()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(rpx(SPACE_LG))
        .bg(c::alpha(c::BG_STRIP(), 0.92))
        .child(
            mono(tracked("NEEDS ATTENTION"), TEXT_TITLE, amber)
                .font_weight(gpui::FontWeight::SEMIBOLD),
        )
        .child(mono(scrim_sub_line(tile_idx), TEXT_MICRO, c::FG_MUTE()))
        // Clicking the scrim focuses/acknowledges the tile, exactly like clicking its header.
        .on_mouse_down(MouseButton::Left, {
            let dispatch = Rc::clone(&ctx.dispatch);
            move |_, window, cx| {
                // Without this the release would fall through to the pty and answer the prompt.
                cx.stop_propagation();
                dispatch(GridAction::Press(tile_idx), window, cx);
            }
        })
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

    #[test]
    fn column_axis_owns_every_divider_junction() {
        assert_eq!(DIVIDER_JUNCTION_AXIS, GridAxis::Columns);
    }

    #[test]
    fn slide_distance_reserves_one_physical_seam_at_every_zoom() {
        for zoom in [0.6, 1.0, 1.75] {
            let total = 200.0 * zoom;
            let Some(distance) = weighted_slide_delta(total, &[0.5, 0.5], 0, &[0.5, 0.5], 1) else {
                unreachable!("second region exists");
            };
            let expected = (total - GRID_SEAM_PX) / 2.0 + GRID_SEAM_PX;
            assert!((distance - expected).abs() < 1e-5, "zoom {zoom}");
        }
    }

    /// `terminal.rs:888-911,1106-1115` — the tenth tile and beyond lose the chord, never render `mod+10`.
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

    fn cold(budget: f32, project: f32, branch: f32, title: f32) -> HeaderFit {
        fit_segments(
            budget,
            &SegmentWidths {
                project,
                branch,
                title,
            },
            None,
        )
    }

    /// Branch is the first sacrifice: a budget that fits exactly one of the two spends it on the project name.
    #[test]
    fn fit_segments_drops_branch_before_project() {
        let fit = cold(60.0 + MIN_TITLE_PX, 50.0, 50.0, 80.0);
        assert!(fit.project);
        assert!(!fit.branch);
    }

    /// An unknown diff (no poll yet) must stay distinct from a known-clean one.
    #[test]
    fn an_unknown_tile_diff_is_distinguished_from_a_known_clean_one() {
        assert_eq!(
            crate::views::rows::diff_display(None),
            crate::views::rows::DiffDisplay::Unknown
        );
        assert_eq!(
            crate::views::rows::diff_display(Some((0, 0))),
            crate::views::rows::DiffDisplay::Clean
        );
        assert_ne!(
            crate::views::rows::diff_display(None),
            crate::views::rows::diff_display(Some((0, 0)))
        );
    }

    #[test]
    fn a_wide_budget_keeps_everything_and_a_tiny_one_keeps_nothing() {
        assert_eq!(
            cold(1000.0, 50.0, 40.0, 80.0),
            HeaderFit {
                project: true,
                branch: true,
                title: true
            }
        );
        assert_eq!(cold(8.0, 50.0, 40.0, 80.0), HeaderFit::default());
        assert_eq!(cold(0.0, 50.0, 40.0, 80.0), HeaderFit::default());
    }

    /// Why measuring beats thresholding: a short name survives where a long one doesn't.
    #[test]
    fn a_short_project_name_survives_a_budget_a_long_one_does_not() {
        let budget = 100.0;
        assert!(cold(budget, 30.0, 0.0, 80.0).project);
        assert!(!cold(budget, 95.0, 0.0, 80.0).project);
    }

    /// Asymmetric thresholds: `B` is the re-add threshold, so a segment already shown survives below it while a hidden one waits for it.
    #[test]
    fn hysteresis_separates_the_drop_and_re_add_thresholds() {
        let seg = SegmentWidths {
            project: 50.0,
            branch: 0.0,
            title: 0.0,
        };
        let shown = Some(HeaderFit {
            project: true,
            ..HeaderFit::default()
        });
        let hidden = Some(HeaderFit::default());
        let b = seg.project + HYSTERESIS_PX;
        assert!(fit_segments(b - 0.1, &seg, shown).project);
        assert!(!fit_segments(b - 0.1, &seg, hidden).project);
        assert!(fit_segments(b, &seg, hidden).project);
        assert!(!fit_segments(seg.project - 0.1, &seg, shown).project);
    }

    /// A branchless tile must not render an orphan `·` at any budget, and `context: None` must not reserve title space.
    #[test]
    fn a_branchless_tile_renders_no_orphan_dot_at_any_fit_level() {
        let shown = Some(HeaderFit {
            project: true,
            branch: true,
            title: true,
        });
        for budget in [0.0, 100.0, 400.0, 10_000.0] {
            let seg = SegmentWidths {
                project: 50.0,
                branch: 0.0,
                title: 0.0,
            };
            for prev in [None, shown] {
                let fit = fit_segments(budget, &seg, prev);
                assert!(!fit.branch, "budget={budget}");
                assert!(!fit.title, "budget={budget}");
            }
        }
    }

    /// A sliver of title is worse than no title.
    #[test]
    fn the_title_is_dropped_rather_than_shown_below_its_floor() {
        let seg = SegmentWidths {
            project: 0.0,
            branch: 0.0,
            title: 200.0,
        };
        let shown = Some(HeaderFit {
            title: true,
            ..HeaderFit::default()
        });
        assert!(fit_segments(MIN_TITLE_PX, &seg, shown).title);
        assert!(!fit_segments(MIN_TITLE_PX - 0.1, &seg, shown).title);
    }

    /// Mirrors `session_header`'s equivalent test.
    #[test]
    fn a_dead_tile_does_not_animate_its_stale_in_progress_title() {
        let title = "migration in progress";
        assert!(session_header::is_in_progress_title(title));
        let running = false;
        let show_progress = running && session_header::is_in_progress_title(title);
        assert!(!show_progress);
    }

    /// `grid_tile_cols` (`src/gui/metrics.rs:336-344`), reimplemented here as the oracle, not exported from production code.
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

    /// What the gpui layout actually hands the element, via this module's constants and [`crate::zoom::ZoomState::pty_dims`].
    fn gpui_tile_dims(
        win_w: f32,
        win_h: f32,
        zoom: f32,
        n: usize,
        tiles_in_col: usize,
    ) -> (u16, u16) {
        let z = crate::zoom::ZoomState::new(zoom);
        let (cols, _) = grid_layout(n);
        let tile_w = (win_w - (cols as f32 - 1.0)) / cols as f32;
        let k = tiles_in_col.max(1) as f32;
        let tile_h = (win_h - APPBAR_H - STATUS_H - (k - 1.0)) / k;
        let pty_w = tile_w - TILE_PTY_PAD_W;
        let pty_h = tile_h - TILE_HEAD_H - 1.0 - TILE_PTY_PAD_H;
        z.pty_dims(pty_w * zoom, pty_h * zoom)
    }

    /// Must land within ±1 cell of the iced oracle; a larger divergence is a real bug, not a tolerance to widen.
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

    /// The ragged-grid promise: a lone tile gets roughly twice the rows of a stacked pair (`metrics.rs:589-604`).
    #[test]
    fn a_short_column_gets_a_taller_pty() {
        let (full, _) = gpui_tile_dims(1280.0, 800.0, 1.0, 3, 1);
        let (half, _) = gpui_tile_dims(1280.0, 800.0, 1.0, 3, 2);
        assert!(full > half);
        assert!(full >= 2 * half - 4);
    }
}
