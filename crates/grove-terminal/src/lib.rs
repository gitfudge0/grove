//! Headless terminal model for Grove: an `alacritty_terminal` wrapper emitting
//! token-space cells. Contains no gpui types and no theme resolution.
#![forbid(unsafe_code)]

pub mod cell;
pub mod color;
pub mod pty;
pub mod term;

pub use cell::{Cell, Snapshot};
pub use color::TermColor;
pub use pty::PtyHandle;
pub use term::{GroveTerm, MouseEncoding, MouseMode};
