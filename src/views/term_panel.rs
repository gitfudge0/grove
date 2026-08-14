//! Port of `src/gui/view/terminal.rs:234-393` (panel/tabs) and `:78-87` (divider).
//! Never rendered in grid view: no single "active worktree" exists there (`pty_input.rs:164-166`).

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Entity, MouseButton, MouseDownEvent, Window};

use crate::theme as c;
use crate::views::components::{divider_h, flat_icon_btn, icon_btn, status_dot, ui};
use crate::views::grid::empty_state;
use crate::views::session_header::SESSBAR_H;
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

/// Two pixels above [`CONTROL_H`] so a tab's dot, glyph and close button still centre inside `SESSBAR_H` (`terminal.rs:319-393`).
const TAB_H: f32 = 24.0;

/// Square, and the tallest box that still leaves a pixel of breathing room inside [`TAB_H`].
const TAB_CLOSE_BOX: f32 = 18.0;

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

/// Matches iced's `Self::hint` chrome (`common.rs:199-220`).
fn hint_tooltip(label: &'static str, _window: &mut Window, cx: &mut App) -> gpui::AnyView {
    cx.new(|_| HintTooltip { label }).into()
}

struct HintTooltip {
    label: &'static str,
}

impl gpui::Render for HintTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_CONTROL))
            .bg(c::BG_STRIP())
            .border_1()
            .border_color(c::BORDER())
            .child(ui(self.label, TEXT_SMALL, c::FG_DIM()))
    }
}

fn on_panel(
    dispatch: &PanelDispatch,
    action: PanelAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

/// The tab strip, its hairline, and the active shell's PTY (`terminal.rs:234-316`).
pub fn term_panel(ctx: &PanelCtx) -> AnyElement {
    // Horizontally scrollable so many shells stay reachable when the strip is narrower than their combined width.
    let mut tabs = div()
        .id("panel-tab-strip")
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .overflow_x_scroll();
    for (i, tab) in ctx.tabs.iter().enumerate() {
        tabs = tabs.child(shell_tab(i, tab, ctx));
    }
    tabs = tabs.child({
        let d = std::rc::Rc::clone(&ctx.dispatch);
        flat_icon_btn(
            "panel-add-shell",
            "plus",
            CONTROL_H,
            ICON_MD,
            move |window, cx| d(PanelAction::NewShell, window, cx),
        )
    });

    let strip = div()
        .h(rpx(SESSBAR_H))
        .w_full()
        .flex()
        .items_center()
        .px(rpx(SPACE_XL))
        .bg(c::BG_STRIP())
        .overflow_hidden()
        .child(div().flex_1().overflow_hidden().child(tabs))
        .child({
            let d = std::rc::Rc::clone(&ctx.dispatch);
            icon_btn(
                "panel-collapse",
                "collapse-right",
                CONTROL_H,
                CONTROL_H,
                ICON_MD,
                c::FG_MUTE(),
                c::BG_HOVER(),
                None,
                false,
                move |window, cx| d(PanelAction::Collapse, window, cx),
            )
            .tooltip(|window, cx| hint_tooltip("collapse panel", window, cx))
        });

    let surface: AnyElement = ctx.view.clone().map_or_else(
        || empty_state("no shell here", "press ＋ to open one in this worktree"),
        |view| {
            div()
                .flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                // Same padding as iced's `pty()` (`metrics.rs:53-56`), so the cell grid matches.
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_2XL))
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
        .child(divider_h())
        .child(surface)
        .into_any_element()
}

/// Icon-only (`terminal.rs:319-393`): several shells share a worktree, so a name wouldn't disambiguate them.
fn shell_tab(idx: usize, tab: &ShellTab, ctx: &PanelCtx) -> AnyElement {
    let dot_color = if tab.running {
        c::GREEN()
    } else {
        c::FG_MUTE()
    };
    let glyph_color = if tab.active { c::CYAN() } else { c::FG_DIM() };
    let close = {
        let d = Rc::clone(&ctx.dispatch);
        icon_btn(
            gpui::SharedString::from(format!("panel-tab-close-{idx}")),
            "close",
            TAB_CLOSE_BOX,
            TAB_CLOSE_BOX,
            ICON_XS,
            c::FG_MUTE(),
            gpui::transparent_black(),
            Some(c::RED()),
            false,
            move |window, cx| d(PanelAction::CloseShell(idx), window, cx),
        )
        .tooltip(|window, cx| hint_tooltip("close shell", window, cx))
    };
    let mut el = div()
        .id(gpui::SharedString::from(format!("panel-tab-{idx}")))
        .h(rpx(TAB_H))
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .px(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_CONTROL))
        .cursor_pointer()
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(status_dot(DOT_SM, dot_color))
        .child(crate::icons::icon("term", ICON_SM, glyph_color))
        .child(close)
        .on_mouse_down(
            MouseButton::Left,
            on_panel(&ctx.dispatch, PanelAction::SelectShell(idx)),
        );
    if tab.active {
        el = el.bg(c::BG_HL()).border_1().border_color(c::CYAN());
    }
    el.into_any_element()
}

/// Grab zone around the hairline (`terminal.rs:78-87`).
pub fn divider(dispatch: &PanelDispatch) -> AnyElement {
    div()
        .id("term-panel-divider")
        .w(rpx(DIVIDER_DRAG_HIT_W))
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
    use crate::views::tokens::DIVIDER_DRAG_HIT_W;

    /// `pty_cols_for_fraction` (`src/gui/metrics.rs:302-321`), reimplemented here as the oracle — never exported from production code.
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
            sidebar_w + DIVIDER_DRAG_HIT_W
        } else {
            0.0
        };
        let work_w = logical_w - visible_w - DIVIDER_DRAG_HIT_W;
        // `PTY_PAD_W` is `pty()`'s own 16×2 (`metrics.rs:53-54`).
        let region_w = work_w * fraction - 32.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (region_w / CELL_W).max(10.0) as u16
        }
    }

    /// What gpui's layout hands the panel: `portion/100` of the workspace minus the divider.
    fn gpui_panel_cols(win_w: f32, zoom: f32, sidebar_w: f32, portion: u16) -> u16 {
        let z = crate::zoom::ZoomState::new(zoom);
        let logical_w = win_w / zoom;
        let work_w = logical_w - (sidebar_w + DIVIDER_DRAG_HIT_W) - DIVIDER_DRAG_HIT_W;
        let region_w = work_w * (f32::from(portion) / 100.0) - 32.0;
        z.pty_dims(region_w * zoom, 100.0).1
    }

    /// The 40% split at a nominal 1280×800 / zoom 1.0 must land within ±1 cell of the iced oracle.
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

    /// The divider's cursor maps straight through `term_portion_for_cursor`, what the drag commits (`layout.rs:184-193`).
    #[test]
    fn dragging_the_divider_left_grows_the_panel() {
        let sidebar = crate::entities::workspace_state::RAIL_W;
        let near = term_portion_for_cursor(1200.0, 1280.0, sidebar);
        let far = term_portion_for_cursor(700.0, 1280.0, sidebar);
        assert!(far > near, "a cursor further left grows the panel");
    }
}
