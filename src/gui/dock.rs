//! macOS dock badge + attention bounce. No-ops off-macOS.
//! (Real objc implementation lands in the dock task.)

pub fn set_badge(_count: usize) {}
pub fn request_attention() {}
