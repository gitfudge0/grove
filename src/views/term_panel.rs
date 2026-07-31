//! The right-docked worktree terminal slide-over (Plan 07 Task 6). Port of
//! `src/gui/view/terminal.rs:234-393` (the panel and its tabs) and `:78-87`
//! (the divider).
//!
//! The panel is **never rendered in grid view**: `workspace()` returns
//! `grid_workspace()` at `terminal.rs:182-184`, before the split at `:204`, and
//! `focused_session` checks `grid_view` first (`pty_input.rs:164-166`) — there
//! is no single "active worktree" in a grid. Confirmed against the oracle in
//! the Plan 07 Task 6 Step 4 report.

use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Entity, MouseButton, MouseDownEvent, Window};

use crate::theme as c;
use crate::views::grid::empty_state;
use crate::views::session_header::SESSBAR_H;
use crate::views::sidebar::SIDEBAR_DIVIDER_W;
use crate::views::terminal_view::TerminalView;

/// What a click in the panel asks the workspace to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// `＋` — spawn another shell in this worktree and focus it.
    NewShell,
    SelectShell(usize),
    CloseShell(usize),
    /// The `collapse-right` button: dismiss the whole panel from inside it.
    Collapse,
    /// A press on the divider (or the double-click reset).
    DividerPress,
}

pub type PanelDispatch = Rc<dyn Fn(PanelAction, &mut Window, &mut App)>;

/// One panel shell as a tab draws it.
pub struct ShellTab {
    pub running: bool,
    pub active: bool,
}

pub struct PanelCtx {
    pub tabs: Vec<ShellTab>,
    /// The active shell's view; `None` while the worktree has no shell.
    pub view: Option<Entity<TerminalView>>,
    pub dispatch: PanelDispatch,
}

fn on_panel(
    dispatch: &PanelDispatch,
    action: PanelAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

/// The tab strip, its hairline, and the active shell's PTY
/// (`terminal.rs:234-316`).
pub fn term_panel(ctx: &PanelCtx) -> AnyElement {
    let mut tabs = div().flex().items_center().gap(px(6.0)).overflow_hidden();
    for (i, tab) in ctx.tabs.iter().enumerate() {
        tabs = tabs.child(shell_tab(i, tab, ctx));
    }
    tabs = tabs.child(
        div()
            .id("panel-add-shell")
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|s| s.bg(c::BG_HOVER()))
            .child(crate::icons::icon("plus", 13.0, c::FG_DIM()))
            .on_mouse_down(
                MouseButton::Left,
                on_panel(&ctx.dispatch, PanelAction::NewShell),
            ),
    );

    let strip = div()
        .h(px(SESSBAR_H))
        .w_full()
        .flex()
        .items_center()
        .px(px(10.0))
        .bg(c::BG_STRIP())
        .overflow_hidden()
        .child(div().flex_1().overflow_hidden().child(tabs))
        .child(
            div()
                .id("panel-collapse")
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .hover(|s| s.bg(c::BG_HOVER()))
                .child(crate::icons::icon("collapse-right", 13.0, c::FG_MUTE()))
                .on_mouse_down(
                    MouseButton::Left,
                    on_panel(&ctx.dispatch, PanelAction::Collapse),
                ),
        );

    let surface: AnyElement = ctx.view.clone().map_or_else(
        || empty_state("no shell here", "press ＋ to open one in this worktree"),
        |view| {
            div()
                .flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                // The same padding iced's `pty()` applies (`metrics.rs:53-56`),
                // so the panel's cell grid matches the iced build's.
                .px(px(16.0))
                .py(px(12.0))
                .child(view)
                .into_any_element()
        },
    );

    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(c::BG())
        .child(strip)
        .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
        .child(surface)
        .into_any_element()
}

/// Icon-only tabs (`terminal.rs:319-393`): several shells share one worktree,
/// so names would not disambiguate them — the dot carries status and the
/// cyan outline carries "active".
fn shell_tab(idx: usize, tab: &ShellTab, ctx: &PanelCtx) -> AnyElement {
    let dot_color = if tab.running {
        c::GREEN()
    } else {
        c::FG_MUTE()
    };
    let glyph_color = if tab.active { c::CYAN() } else { c::FG_DIM() };
    let mut el = div()
        .id(gpui::SharedString::from(format!("panel-tab-{idx}")))
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .rounded(px(4.0))
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(div().size(px(6.0)).rounded_full().bg(dot_color))
        .child(crate::icons::icon("term", 13.0, glyph_color))
        .child(
            div()
                .id(gpui::SharedString::from(format!("panel-tab-close-{idx}")))
                .w(px(16.0))
                .h(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c::FG_MUTE())
                .hover(|s| s.text_color(c::RED()))
                .child(crate::icons::icon("close", 11.0, c::FG_MUTE()))
                .on_mouse_down(
                    MouseButton::Left,
                    on_panel(&ctx.dispatch, PanelAction::CloseShell(idx)),
                ),
        )
        .on_mouse_down(
            MouseButton::Left,
            on_panel(&ctx.dispatch, PanelAction::SelectShell(idx)),
        );
    if tab.active {
        el = el.bg(c::BG_HL()).border_1().border_color(c::CYAN());
    }
    el.into_any_element()
}

/// The `SIDEBAR_DIVIDER_W` grab zone around a `BORDER()` hairline, full height,
/// with a horizontal-resize cursor (`terminal.rs:78-87`).
pub fn divider(dispatch: &PanelDispatch) -> AnyElement {
    div()
        .id("term-panel-divider")
        .w(px(SIDEBAR_DIVIDER_W))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .child(div().w(px(1.0)).h_full().bg(c::BORDER()))
        .on_mouse_down(
            MouseButton::Left,
            on_panel(dispatch, PanelAction::DividerPress),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::entities::workspace_state::{
        term_portion_for_cursor, TERM_PANEL_PORTION, TERM_PANEL_PORTION_MAX, TERM_PANEL_PORTION_MIN,
    };
    use crate::fonts::CELL_W;
    use crate::views::sidebar::SIDEBAR_DIVIDER_W;

    /// `pty_cols_for_fraction` (`src/gui/metrics.rs:302-321`), reimplemented
    /// **here** as the oracle — never exported from production code
    /// (carried amendment 1).
    fn oracle_cols_for_fraction(
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
        let work_w = logical_w - visible_w - SIDEBAR_DIVIDER_W;
        // `PTY_PAD_W` is `pty()`'s own 16×2 (`metrics.rs:53-54`).
        let region_w = work_w * fraction - 32.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (region_w / CELL_W).max(10.0) as u16
        }
    }

    /// What gpui's layout hands the panel element: the split's flex weights
    /// give it `portion/100` of the workspace minus the divider, and the
    /// element sizes itself from those bounds.
    fn gpui_panel_cols(win_w: f32, zoom: f32, sidebar_w: f32, portion: u16) -> u16 {
        let z = crate::zoom::ZoomState::new(zoom);
        let logical_w = win_w / zoom;
        let work_w = logical_w - (sidebar_w + SIDEBAR_DIVIDER_W) - SIDEBAR_DIVIDER_W;
        let region_w = work_w * (f32::from(portion) / 100.0) - 32.0;
        z.pty_dims(region_w * zoom, 100.0).1
    }

    /// Carried amendment 2's second parity assertion: the 40% split at a
    /// nominal 1280×800 / zoom 1.0 must land within **±1 cell** of the iced
    /// oracle.
    #[test]
    fn the_panel_split_matches_the_iced_oracle_within_one_cell() {
        let sidebar = crate::entities::workspace_state::RAIL_W;
        for portion in [
            TERM_PANEL_PORTION_MIN,
            TERM_PANEL_PORTION,
            TERM_PANEL_PORTION_MAX,
        ] {
            let got = gpui_panel_cols(1280.0, 1.0, sidebar, portion);
            let want =
                oracle_cols_for_fraction(1280.0, 1.0, true, f32::from(portion) / 100.0, sidebar);
            assert!(
                got.abs_diff(want) <= 1,
                "portion {portion}: {got} cols vs oracle {want}"
            );
        }
    }

    /// The divider's cursor maps straight through `term_portion_for_cursor`,
    /// which is what the drag commits (`layout.rs:184-193`).
    #[test]
    fn dragging_the_divider_left_grows_the_panel() {
        let sidebar = crate::entities::workspace_state::RAIL_W;
        let near = term_portion_for_cursor(1200.0, 1280.0, sidebar);
        let far = term_portion_for_cursor(700.0, 1280.0, sidebar);
        assert!(far > near, "a cursor further left grows the panel");
    }
}
