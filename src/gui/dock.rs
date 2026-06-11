//! macOS dock signal: badge count of waiting sessions + one attention bounce
//! when a session enters WaitingForInput while Grove is unfocused.
//! All no-ops off-macOS. Thin by design — manually verified, not unit-tested.

// objc 0.2's macros expand `cfg(feature = "cargo-clippy")` checks that trip
// the modern `unexpected_cfgs` lint; not our code, silence it here.
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn set_badge(count: usize) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
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
    // NSRequestUserAttentionType::NSInformationalRequest = 10
    const NS_INFORMATIONAL_REQUEST: u64 = 10;
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
