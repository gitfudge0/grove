//! Turns a `grove_terminal` snapshot and raw input events into colors and PTY bytes; deliberately free of gpui element/paint types so it can be unit-tested headlessly.

pub mod clipboard;
pub mod colors;
pub mod drop;
pub mod keys;
pub mod mouse;
