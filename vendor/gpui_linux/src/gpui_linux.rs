#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::current_platform;

use std::sync::atomic::{AtomicU32, Ordering};
static XFT_TEXT_SCALE: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());

/// Supply the desktop text factor before platform creation. Xft.dpi on
/// GNOME-compatible desktops includes this factor; it is not display scaling.
/// Explicit GPUI and GDK display scale overrides are unaffected.
pub fn set_xft_text_scale(factor: f32) {
    let factor = if factor.is_finite() && factor > 0.0 {
        factor
    } else {
        1.0
    };
    XFT_TEXT_SCALE.store(factor.to_bits(), Ordering::Relaxed);
}

pub(crate) fn xft_text_scale() -> f32 {
    f32::from_bits(XFT_TEXT_SCALE.load(Ordering::Relaxed))
}
