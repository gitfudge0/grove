//! Views — the `Render` implementations that make up the window.

use gpui::{rems, Rems};

/// A design pixel, expressed in rems so it scales with UI zoom.
///
/// The chrome is authored in the pixel numbers the iced build used, but
/// `px()` values are immune to `Window::set_rem_size` — only `rems()` scales.
/// `rpx(12.)` keeps the legible pixel number *and* zooms, because the root
/// view sets the rem size to `REM_BASE * zoom` once per frame
/// (`views::workspace::Workspace::render`, `crate::zoom`).
///
/// Deliberately **not** used for: 1px hairline borders and dividers (a
/// hairline stays a hairline), and anything that is physical window/viewport
/// math rather than element styling (mouse positions, window bounds, the
/// terminal element's own cell grid — that has its own zoom pathway).
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
pub mod session_header;
pub mod sidebar;
pub mod statusbar;
pub mod term_panel;
pub mod terminal_tab;
pub mod terminal_view;
pub mod tokens;
pub mod workspace;
