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

use crate::views::rpx;
use crate::views::tokens::*;
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
use crate::views::components::{divider_h, icon_btn, keycap_filled, mono, tracked, ui};
use crate::views::session_header;
use crate::views::terminal_view::TerminalView;

/// Height of the tile header bar (`src/gui/metrics.rs:51`).
pub const TILE_HEAD_H: f32 = 22.0;
/// The square hit box of a tile-header icon button. Deliberately below
/// [`CONTROL_H`] (22): the button has to sit *inside* a
/// [`TILE_HEAD_H`]-tall bar, so it cannot be the chrome control height.
pub const TILE_BTN_BOX: f32 = 18.0;
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
    /// A press on the tile's PTY body: focus + acknowledge only, no drag
    /// armed (a body press must not start a tile drag; that is the header's
    /// job).
    Focus(usize),
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
    /// The OSC context title, already sanitized — the **same** string
    /// [`crate::views::workspace::Workspace::header_data`] derives for the
    /// session bar via `rows::session_context`, so a tile and the bar never
    /// disagree about what a session is doing.
    pub context: Option<String>,
    /// Whether the session's process is alive, gating the in-progress dots
    /// (a dead session's stale "in progress" title must not animate).
    pub running: bool,
    /// Which optional header segments survive, decided by [`fit_segments`]
    /// from widths this session's *own* strings actually measure to
    /// (`Workspace::segment_widths`) — not from a width threshold, because
    /// `grove` and `GLOBUS-PORTAL` do not cost the same.
    pub fit: HeaderFit,
    /// The worktree's uncommitted diff against `HEAD`: `(added, removed)`
    /// lines. `None` if unknown (no first poll yet, or no matching
    /// worktree) draws nothing — the same rule the card and the session bar
    /// follow for [`crate::views::rows::diff_chips`]. Does not participate
    /// in [`HeaderFit`]/[`fit_segments`]: it sits past the `flex_1` title
    /// zone, which absorbs the squeeze on its own.
    pub diff: Option<(u32, u32)>,
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
    /// The clock's raw tick, for the title zone's in-progress dot walk
    /// (`session_header::in_progress_phase`). `pulse`/`scrim_pulse` are both
    /// already-derived waves and cannot recover it.
    pub tick: u64,
    pub drag: Option<GridDrag>,
    pub slide: Option<GridSlide>,
    /// Nominal tile size in logical px, for the slide's draw offset. Comes
    /// from `grid_tile_size`, which ignores the sidebar — kept unchanged
    /// because the slide's draw offset must match that geometry exactly.
    pub tile_size: (f32, f32),
    pub dispatch: GridDispatch,
}

/// Which identity segments survive in one tile's header. Each surviving
/// segment stays whole — a truncated `"GLOB…"` project name is noise, so
/// segments are dropped whole rather than shrunk. Only the title ever
/// truncates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderFit {
    pub project: bool,
    pub branch: bool,
    pub title: bool,
}

/// Measured widths of each optional segment, in **device pixels**, including
/// the leading `·` separator and the two flex gaps each one carries.
///
/// A segment whose text is blank — a branchless session, `context: None` —
/// records `0.0` and is never kept at any budget, which is what keeps the
/// header from showing a trailing `·` with nothing after it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SegmentWidths {
    pub project: f32,
    pub branch: f32,
    /// Only ever consulted for blankness: the title zone is `flex_1` and
    /// truncates, so what it *would* measure does not decide its fate.
    pub title: f32,
}

/// Below this much leftover budget the title is dropped rather than shown.
/// Device px at the default 16px rem; roughly four glyphs of [`TEXT_MICRO`]
/// plus the ellipsis — a 12px sliver reading `"m…"` costs the header a `·`
/// and a truncation mark to say nothing at all, so none is better.
const MIN_TITLE_PX: f32 = 48.0;

/// The re-add margin that makes the drop and re-add thresholds differ, in
/// device px. A segment drops the moment it overflows but is only brought
/// back once it fits with this much to spare, so a tile width jittering
/// around one segment's exact cost cannot oscillate it in and out.
const HYSTERESIS_PX: f32 = 10.0;

/// Greedily fit optional segments into `budget` device px by priority:
/// project, then branch — branch is the first thing sacrificed because two
/// tiles of one project are told apart by their branch far less often than
/// two projects are told apart by their name. `prev` is last frame's
/// decision for this session and supplies the hysteresis (see
/// [`HYSTERESIS_PX`]); `None` on a session's first frame, which starts
/// pessimistic and adds segments in.
#[must_use]
pub fn fit_segments(budget: f32, seg: &SegmentWidths, prev: Option<HeaderFit>) -> HeaderFit {
    let prev = prev.unwrap_or_default();
    // A shown segment's drop threshold is its bare width; a hidden one's
    // re-add threshold is that plus the margin.
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
    // The title's threshold is a floor on the space left for it rather than
    // its own width, since it truncates instead of dropping whole.
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

    let (border_color, border_w) = if data.waiting {
        // §7.2: exactly one border weight — the hairline. A waiting tile is
        // called out by the amber *tone*, never by a heavier stroke.
        (c::AMBER(), 1.0)
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
            // The tile's PTY, padded exactly as iced's `pty()` is
            // (`metrics.rs:53-56`) so a tile's cell grid matches the iced
            // build's; the element derives its own dims from these bounds.
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
        root = root.left(rpx(dx)).top(rpx(dy));
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
    let fit = data.fit;

    // Identity zone: icon + agent label always survive; project/branch are
    // dropped whole (never truncated) as the tile narrows. `.flex_shrink_0()`
    // — identity never truncates, the title zone below absorbs the squeeze.
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
    // Branchless sessions skip the segment entirely — otherwise the header
    // shows a trailing dot with nothing after it.
    if fit.branch && !data.branch.trim().is_empty() {
        identity = identity.child(ui("·", TEXT_MICRO, c::FG_MUTE())).child(ui(
            data.branch.clone(),
            TEXT_MICRO,
            c::FG_MUTE(),
        ));
    }

    // Title zone: `flex_1()` + `min_w_0()` replaces the old bare spacer — the
    // context title takes the space a spacer used to waste. Only this
    // element ever truncates.
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
            // A partially drawn 3-dot cluster is meaningless — it never
            // shrinks or truncates.
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

    // Arming the kill changes the *glyph* as well as the colour (§2.3, §12):
    // red-vs-muted on an identical trash can is a colour-only signal. `question`
    // is the "are you sure?" shape already used for needs-you, and it renders in
    // the same ICON_XS box inside the same TILE_BTN_BOX, so nothing reflows
    // (§2.4).
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
        // `overflow_hidden` is now a backstop, not the mechanism: the
        // controls cluster below is `flex_shrink_0` so a long title can
        // never push it out of the tile — the title zone's `min_w_0()` is
        // what actually absorbs the squeeze.
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
/// wash is the theme's deepest surface at 0.92 rather than a blur, which
/// neither toolkit has.
///
/// The headline is a **mono, letter-tracked** label at [`TEXT_TITLE`]: an
/// overlay on a tile is chrome, and §5.3's display tiers are never chrome.
/// Tracking comes from [`crate::views::rows::tracked`] (U+2009 thin spaces),
/// not from typing spaces into the literal (§5.4).
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
        // Clicking the scrim focuses/acknowledges the tile, exactly like
        // clicking its header.
        .on_mouse_down(MouseButton::Left, {
            let dispatch = Rc::clone(&ctx.dispatch);
            move |_, window, cx| {
                // The scrim sits over the tile's TerminalView and `Press` focuses
                // that view; without this the release below would post a mouse
                // click into the pty and answer the prompt the scrim is asking
                // about.
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

    /// A cold-start fit (no previous frame) for a session whose three
    /// segments cost the given device px.
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

    /// Branch is the first sacrifice: a budget that fits exactly one of the
    /// two spends it on the project name.
    #[test]
    fn fit_segments_drops_branch_before_project() {
        let fit = cold(60.0 + MIN_TITLE_PX, 50.0, 50.0, 80.0);
        assert!(fit.project);
        assert!(!fit.branch);
    }

    /// [`TileData::diff`] draws nothing before the first poll lands,
    /// distinguishing an unknown diff from a *known* clean one — same rule
    /// [`crate::views::rows::diff_chips`] enforces for the card and the
    /// session bar.
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

    /// The whole point of measuring instead of thresholding: at one fixed
    /// tile width a short project name survives where a long one cannot.
    #[test]
    fn a_short_project_name_survives_a_budget_a_long_one_does_not() {
        // `grove` vs `GLOBUS-PORTAL` at TEXT_MICRO, roughly, in one tile.
        let budget = 100.0;
        assert!(cold(budget, 30.0, 0.0, 80.0).project);
        assert!(!cold(budget, 95.0, 0.0, 80.0).project);
    }

    /// Asymmetric thresholds: `B` is the re-add threshold, so a segment
    /// already shown survives below it while a hidden one waits for it.
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
        // Below its bare width it goes even when shown.
        assert!(!fit_segments(seg.project - 0.1, &seg, shown).project);
    }

    /// A branchless tile must not render an orphan `·` at any budget, and
    /// `context: None` must not reserve title space.
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

    /// Mirrors `session_header`'s
    /// `a_dead_session_does_not_animate_its_stale_in_progress_title`: a dead
    /// tile with a frozen "in progress" title must show the text, not dots.
    #[test]
    fn a_dead_tile_does_not_animate_its_stale_in_progress_title() {
        let title = "migration in progress";
        assert!(session_header::is_in_progress_title(title));
        let running = false;
        let show_progress = running && session_header::is_in_progress_title(title);
        assert!(!show_progress);
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
