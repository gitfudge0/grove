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

/// Ask AppKit to redraw the native backdrop after returning to the window.
///
/// GPUI redraws its Metal surface on activation, but its separate visual-effect
/// view is not invalidated. Refresh it when returning from window transitions
/// such as Mission Control, where the backdrop can appear stale.
/// Invalidate the existing effect rather than changing its material or toggling
/// the window's background appearance. Solid windows have no effect to refresh.
#[cfg(target_os = "macos")]
pub fn refresh_native_backdrop(window: &gpui::Window) {
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the activation observer runs on AppKit's main thread. GPUI owns
    // the borrowed native view/window and their subviews throughout this call.
    // NSVisualEffectView inherits setNeedsDisplay: from NSView; marking it dirty
    // lets AppKit refresh its effect during its normal display pass.
    unsafe {
        let view = handle.ns_view.as_ptr().cast::<Object>();
        let native: *mut Object = msg_send![view, window];
        if native.is_null() {
            return;
        }
        let content: *mut Object = msg_send![native, contentView];
        if content.is_null() {
            return;
        }
        let subviews: *mut Object = msg_send![content, subviews];
        let count: usize = msg_send![subviews, count];
        for index in 0..count {
            let subview: *mut Object = msg_send![subviews, objectAtIndex: index];
            let is_effect: BOOL = msg_send![subview, isKindOfClass: class!(NSVisualEffectView)];
            if is_effect == YES {
                let state: isize = msg_send![subview, state];
                let hidden: BOOL = msg_send![subview, isHidden];
                tracing::debug!(
                    state,
                    hidden = hidden == YES,
                    "refreshing native window backdrop"
                );
                let _: () = msg_send![subview, setNeedsDisplay: YES];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn refresh_native_backdrop(_window: &gpui::Window) {}
