//! macOS dock signal: badge count of waiting sessions + one attention bounce when a session enters WaitingForInput while Grove is unfocused. All no-ops off-macOS.
//! Copied verbatim from `src/gui/dock.rs` rather than moved, since the iced build still needs its own copy until Plan 10.
//! Linux has no Wayland/X11 badge API today, so it's a no-op by absence, not omission — do not invent one.

// The crate root denies `unsafe_code`; this file is the single audited exception needed to reach the Objective-C runtime.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
// objc 0.2's macros trip the modern `unexpected_cfgs` lint; not our code, silence it here.
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn set_badge(count: usize) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: unchecked FFI into Objective-C; correctness rests on the AppKit contract (macOS-gated, null-checked, documented selectors, main-thread GUI). Null `NSString*` is the documented way to clear the badge.
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let dock_tile: *mut Object = msg_send![app, dockTile];
        let label: *mut Object = if count == 0 {
            std::ptr::null_mut()
        } else {
            let s = format!("{count}\0");
            msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()]
        };
        let _: () = msg_send![dock_tile, setBadgeLabel: label];
    }
}

#[cfg(target_os = "macos")]
pub fn request_attention() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    const NS_INFORMATIONAL_REQUEST: u64 = 10;
    // SAFETY: unchecked FFI into Objective-C; correctness rests on the AppKit contract (macOS-gated, null-checked, documented selector, main-thread GUI).
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let _: i64 = msg_send![app, requestUserAttention: NS_INFORMATIONAL_REQUEST];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_badge(_count: usize) {}

#[cfg(not(target_os = "macos"))]
pub fn request_attention() {}
