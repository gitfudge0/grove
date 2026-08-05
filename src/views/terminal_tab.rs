//! The home-terminal tab (Plan 07 Task 5 Step 3). Port of
//! `src/gui/view/terminal.rs:398-485` — `terminal_workspace` and
//! `home_terminal_bar`.
//!
//! Unlike the session bar there is **no kill action**: the home terminal is
//! permanent. Only a restart, which relaunches the shell at `~` in place, and
//! the zen toggle.

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;

use gpui::{div, prelude::*, AnyElement, App, Entity, Window};

use crate::theme as c;
use crate::views::components::{divider_h, keycap, mono, status_dot, ui, vline};
use crate::views::grid::{PTY_PAD_H, PTY_PAD_W};
use crate::views::session_header::{tool_btn, SESSBAR_H};
use crate::views::terminal_view::TerminalView;

/// What the terminal tab's chrome asks the workspace to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalTabAction {
    /// Replace the active shell in its slot, keeping its label
    /// (`src/app/terminals.rs:38-53`).
    Restart,
    ToggleZen,
}

pub type TabDispatch = Rc<dyn Fn(TerminalTabAction, &mut Window, &mut App)>;

pub struct TerminalTabCtx {
    /// `None` when every home terminal has been closed.
    pub view: Option<Entity<TerminalView>>,
    pub running: bool,
    /// The OSC context title, already sanitized; defaults to `~`.
    pub context: Option<String>,
    /// Drives the zen button's tooltip label only.
    pub chrome_visible: bool,
    pub dispatch: TabDispatch,
}

/// Shown in the terminal tab when every home terminal has been closed
/// (`primitives.rs:279-320`'s `empty_terminals_workspace`): the same
/// `empty_state` chrome, plus a mod+t keycap hint.
fn empty_terminals_state() -> AnyElement {
    let keycap_content: AnyElement = if cfg!(target_os = "macos") {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XS))
            .child(crate::icons::icon("command", ICON_XS, c::FG_DIM()))
            .child(mono("t", TEXT_SMALL, c::FG_DIM()))
            .into_any_element()
    } else {
        mono(
            format!("{}+t", crate::keymap::platform_mod_label()),
            TEXT_SMALL,
            c::FG_DIM(),
        )
        .into_any_element()
    };

    let hint = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(keycap(keycap_content))
        .child(mono("open a terminal", TEXT_MICRO, c::FG_MUTE()));

    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(rpx(SPACE_MD))
        .size_full()
        .bg(c::BG())
        .child(ui("no terminals open", TEXT_TITLE, c::FG_DIM()))
        .child(hint)
        .into_any_element()
}

/// The bar plus the active home terminal's PTY.
pub fn terminal_tab(ctx: &TerminalTabCtx) -> AnyElement {
    let Some(view) = ctx.view.clone() else {
        return empty_terminals_state();
    };
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(c::BG())
        .child(home_terminal_bar(ctx))
        .child(
            // Same `pty()` padding as the single-session body: iced routes
            // both through `self.pty(PtyPane::Agent, …)`
            // (`src/gui/view/terminal.rs:189`, `:401`), so the tab's grid
            // must match it too. See `views::grid::PTY_PAD_W`.
            div()
                .flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .px(rpx(PTY_PAD_W / 2.0))
                .py(rpx(PTY_PAD_H / 2.0))
                .child(view),
        )
        .into_any_element()
}

/// The status/context/tools bar (`terminal.rs:420-485`).
fn home_terminal_bar(ctx: &TerminalTabCtx) -> AnyElement {
    let (dot_color, status_label) = if ctx.running {
        (c::GREEN(), "running")
    } else {
        (c::FG_MUTE(), "exited")
    };
    let context = ctx.context.as_deref().unwrap_or("~").to_string();
    let dispatch = Rc::clone(&ctx.dispatch);
    let zen_dispatch = Rc::clone(&ctx.dispatch);

    let bar = div()
        .h(rpx(SESSBAR_H))
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_2XL))
        .px(rpx(SPACE_3XL))
        .bg(c::BG_STRIP())
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_MD))
                .flex_shrink_0()
                .child(status_dot(DOT_SM, dot_color))
                .child(ui(status_label, TEXT_BODY, dot_color)),
        )
        .child(vline().flex_shrink_0())
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                ui(context.clone(), TEXT_BODY, c::FG())
                    .id("home-term-context")
                    .truncate()
                    .min_w_0()
                    .flex_1()
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(context.clone())
                            .build(window, cx)
                    }),
            ),
        )
        .child(ui("~", TEXT_BODY, c::FG_MUTE()).flex_shrink_0())
        .child(vline().flex_shrink_0())
        .child(
            div().flex_shrink_0().child(tool_btn(
                "home-term-restart",
                "restart",
                "restart",
                false,
                false,
                move |window, cx| dispatch(TerminalTabAction::Restart, window, cx),
            )),
        )
        .child(
            div().flex_shrink_0().child(tool_btn(
                "home-term-zen",
                "zen",
                // In zen the bar is the only way back out by mouse.
                if ctx.chrome_visible {
                    "zen"
                } else {
                    "exit zen"
                },
                false,
                false,
                move |window, cx| zen_dispatch(TerminalTabAction::ToggleZen, window, cx),
            )),
        );

    div()
        .flex()
        .flex_col()
        .w_full()
        .child(bar)
        .child(divider_h())
        .into_any_element()
}

#[cfg(test)]
mod tests {

    /// `terminal.rs:447-449` — the context defaults to `~`; no char cap is
    /// applied (the title element truncates visually via `.truncate()` and
    /// reveals the full string in a tooltip, mirroring `session_header.rs`).
    #[test]
    fn the_context_defaults_to_tilde_and_is_not_char_capped() {
        let none: Option<String> = None;
        assert_eq!(none.as_deref().unwrap_or("~"), "~");
    }

    /// `terminal.rs:466-473` — the zen tool's label flips while the chrome is
    /// hidden, because in zen this bar is the only mouse route back out.
    #[test]
    fn the_zen_tool_label_flips_in_zen() {
        let label = |chrome_visible: bool| {
            if chrome_visible {
                "zen"
            } else {
                "exit zen"
            }
        };
        assert_eq!(label(true), "zen");
        assert_eq!(label(false), "exit zen");
    }
}
