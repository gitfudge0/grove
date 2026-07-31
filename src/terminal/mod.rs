//! Terminal-side pure logic: the pieces that turn a `grove_terminal` snapshot
//! and raw input events into colors and PTY bytes.
//!
//! Everything here is deliberately free of gpui element/paint types so it can
//! be unit-tested headlessly; the element (`crate::terminal_element`) and the
//! view (`crate::views::terminal_view`) are the only consumers.

pub mod clipboard;
pub mod colors;
pub mod drop;
pub mod keys;
pub mod mouse;
