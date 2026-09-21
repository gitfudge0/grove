//! Platform-gated leaves. Everything here is `#[cfg]`-split at the function
//! level, so callers never branch on the target themselves.

pub mod dock;

pub mod window_chrome;
