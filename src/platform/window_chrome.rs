//! Hide AppKit's standard buttons while retaining a titled, resizable window.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn hide_native_buttons(window: &gpui::Window) {
    use objc::runtime::{Object, YES};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI calls this on the AppKit main thread. Its borrowed handle owns a
    // live NSView; the documented window/standardWindowButton selectors return
    // borrowed objects. Only visibility changes, never the window's style mask.
    unsafe {
        let view = handle.ns_view.as_ptr().cast::<Object>();
        let native: *mut Object = msg_send![view, window];
        if native.is_null() {
            return;
        }
        for kind in [0_u64, 1, 2] {
            let button: *mut Object = msg_send![native, standardWindowButton: kind];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: YES];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn hide_native_buttons(_window: &gpui::Window) {}
