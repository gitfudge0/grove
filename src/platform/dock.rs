//! macOS dock signal: badge count of waiting sessions + one attention bounce
//! when a session enters WaitingForInput while Grove is unfocused.
//! All no-ops off-macOS. Thin by design — manually verified, not unit-tested.
//!
//! Copied verbatim from `src/gui/dock.rs` (it holds no iced types) rather than
//! moved: the iced build still needs its own copy until Plan 10.
//!
//! **Linux is a no-op by absence, not by omission.** Spec §7 lists the dock
//! badge and the attention bounce under macOS; there is no Wayland or X11
//! badge API in Grove today, so on Linux nothing renders and nothing bounces
//! on either backend, while the waiting *count* still drives the appbar pill.
//! Do not invent a Linux badge here.

// The crate root denies `unsafe_code`; the Objective-C runtime cannot be
// reached without it, and this file is the single audited exception.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
// objc 0.2's macros expand `cfg(feature = "cargo-clippy")` checks that trip
// the modern `unexpected_cfgs` lint; not our code, silence it here.
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn set_badge(count: usize) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: `msg_send!` is an unchecked FFI call into the Objective-C
    // runtime, so the compiler can't verify the selectors exist or that
    // receiver/return types match; correctness rests on the AppKit contract.
    // This function is `#[cfg(target_os = "macos")]` so AppKit is present,
    // `sharedApplication` is null-checked below, `dockTile`/`setBadgeLabel:`
    // match AppKit's documented signatures, and gpui drives the GUI on the
    // main thread, which AppKit requires. A null `NSString*` label is the
    // documented way to clear the badge, hence `null_mut()` when `count == 0`.
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
    // SAFETY: `msg_send!` is an unchecked FFI call into the Objective-C
    // runtime, so the compiler can't verify the selector exists or that
    // receiver/return types match; correctness rests on the AppKit contract.
    // This function is `#[cfg(target_os = "macos")]` so AppKit is present,
    // `sharedApplication` is null-checked below, `requestUserAttention:`
    // matches AppKit's documented signature, and gpui drives the GUI on the
    // main thread, which AppKit requires.
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
