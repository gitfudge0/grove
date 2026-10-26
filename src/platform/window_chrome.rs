//! Synchronize AppKit chrome while retaining a titled, resizable window.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn sync_native_buttons(window: &gpui::Window) {
    use objc::runtime::{Object, BOOL, NO, YES};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // The system's fullscreen titlebar owns these controls when revealed at the
    // top edge. Outside fullscreen, Grove supplies its own window buttons.
    let hidden = if window.is_fullscreen() { NO } else { YES };
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
                let current: BOOL = msg_send![button, isHidden];
                if current != hidden {
                    let _: () = msg_send![button, setHidden: hidden];
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sync_native_buttons(_window: &gpui::Window) {}

/// Match native window appearance to the selected application theme, independently
/// of the OS setting. The window retains the named AppKit appearance.
#[cfg(target_os = "macos")]
pub fn sync_theme_appearance(window: &gpui::Window, dark: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        static NSAppearanceNameAqua: *mut Object;
        static NSAppearanceNameDarkAqua: *mut Object;
    }

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: called during GPUI rendering on AppKit's main thread. GPUI owns
    // the borrowed view/window, AppKit owns the named appearance, and NSWindow
    // retains it through setAppearance:. Compare first to avoid repaint loops.
    unsafe {
        let view = handle.ns_view.as_ptr().cast::<Object>();
        let native: *mut Object = msg_send![view, window];
        if native.is_null() {
            return;
        }
        let name = if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        };
        let desired: *mut Object = msg_send![class!(NSAppearance), appearanceNamed: name];
        if desired.is_null() {
            return;
        }
        let current: *mut Object = msg_send![native, appearance];
        if current != desired {
            let _: () = msg_send![native, setAppearance: desired];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sync_theme_appearance(_window: &gpui::Window, _dark: bool) {}
