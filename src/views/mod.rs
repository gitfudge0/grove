//! Views — the `Render` implementations that make up the window.

use gpui::{rems, Rems};

/// A design pixel, expressed in rems so it scales with UI zoom: `px()` is immune to `Window::set_rem_size`, only
/// `rems()` scales, and the root view sets rem size to `REM_BASE * zoom` once per frame (`workspace::Workspace::render`).
///
/// Deliberately **not** used for 1px hairlines, or anything that's physical window/viewport math rather than
/// element styling (mouse positions, window bounds, the terminal's own cell grid).
#[inline]
pub fn rpx(v: f32) -> Rems {
    rems(v / crate::zoom::REM_BASE)
}

pub mod appbar;
pub mod components;
mod conformance;
pub mod dispatch;
pub mod grid;
pub mod modals;
pub mod rows;
pub(crate) mod scripts;
pub mod session_header;
pub mod sidebar;
pub mod statusbar;
pub mod term_panel;
pub mod terminal_tab;
pub mod terminal_view;
pub mod tokens;
pub mod workspace;
