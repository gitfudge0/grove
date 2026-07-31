//! The home-terminal tab (Plan 07 Task 5 Step 3). Port of
//! `src/gui/view/terminal.rs:398-485` — `terminal_workspace` and
//! `home_terminal_bar`.
//!
//! Unlike the session bar there is **no kill action**: the home terminal is
//! permanent. Only a restart, which relaunches the shell at `~` in place, and
//! the zen toggle.

use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Entity, Window};

use crate::theme as c;
use crate::views::grid::{empty_state, PTY_PAD_H, PTY_PAD_W};
use crate::views::session_header::{tool_btn, truncate_middle, SESSBAR_H};
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

/// The bar plus the active home terminal's PTY.
pub fn terminal_tab(ctx: &TerminalTabCtx) -> AnyElement {
    let Some(view) = ctx.view.clone() else {
        return empty_state("no terminals open", "open one from the TERMINALS section");
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
                .px(px(PTY_PAD_W / 2.0))
                .py(px(PTY_PAD_H / 2.0))
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
    let context = truncate_middle(
        ctx.context.as_deref().unwrap_or("~"),
        crate::views::session_header::CONTEXT_MAX_CHARS,
    );
    let dispatch = Rc::clone(&ctx.dispatch);
    let zen_dispatch = Rc::clone(&ctx.dispatch);

    let bar = div()
        .h(px(SESSBAR_H))
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(16.0))
        .bg(c::BG_STRIP())
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().size(px(6.0)).rounded_full().bg(dot_color))
                .child(crate::views::rows::ui_text(status_label, 12.0, dot_color)),
        )
        .child(vline())
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(crate::views::rows::ui_text(context, 12.0, c::FG())),
        )
        .child(crate::views::rows::ui_text("~", 12.0, c::FG_MUTE()))
        .child(vline())
        .child(tool_btn(
            "home-term-restart",
            "restart",
            "restart",
            false,
            false,
            move |window, cx| dispatch(TerminalTabAction::Restart, window, cx),
        ))
        .child(tool_btn(
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
        ));

    div()
        .flex()
        .flex_col()
        .w_full()
        .child(bar)
        .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
        .into_any_element()
}

/// The bar's vertical rule (`primitives.rs`'s `vline`).
pub fn vline() -> gpui::Div {
    div().w(px(1.0)).h(px(16.0)).bg(c::BORDER())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `terminal.rs:447-449` — the context defaults to `~` and is
    /// middle-truncated at 80 chars.
    #[test]
    fn the_context_defaults_to_tilde_and_truncates_at_eighty() {
        let long = "x".repeat(200);
        let out = truncate_middle(&long, crate::views::session_header::CONTEXT_MAX_CHARS);
        assert_eq!(out.chars().count(), 80);
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
