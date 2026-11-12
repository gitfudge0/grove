//! Synchronize AppKit chrome while retaining a titled, resizable window.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
#![allow(unexpected_cfgs)]

/// Platform policy is explicit so every target can be checked on one host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPlatform {
    Mac,
    Windows,
    Linux,
}
impl ControlPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Close,
    Minimize,
    Fullscreen,
    Maximize,
}
impl WindowAction {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Close => "window-close",
            Self::Minimize => "window-minimize",
            Self::Fullscreen => "window-fullscreen",
            Self::Maximize => "window-maximize",
        }
    }
    pub fn label(self, fullscreen: bool, maximized: bool) -> &'static str {
        match self {
            Self::Close => "Close window",
            Self::Minimize => "Minimize window",
            Self::Fullscreen if fullscreen => "Exit full screen",
            Self::Fullscreen => "Enter full screen",
            Self::Maximize if maximized => "Restore window",
            Self::Maximize => "Maximize window",
        }
    }
}
pub fn controls(
    platform: ControlPlatform,
    decorations: gpui::Decorations,
    supported: gpui::WindowControls,
) -> Vec<WindowAction> {
    use WindowAction::{Close, Fullscreen, Maximize, Minimize};
    match platform {
        ControlPlatform::Mac => vec![Close, Minimize, Fullscreen],
        // Transparent titlebars hide the native Windows caption buttons.
        ControlPlatform::Windows => vec![Minimize, Maximize, Close],
        ControlPlatform::Linux if matches!(decorations, gpui::Decorations::Server) => vec![],
        ControlPlatform::Linux => [
            supported.minimize.then_some(Minimize),
            supported.maximize.then_some(Maximize),
            Some(Close),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

#[cfg(target_os = "macos")]
pub fn sync_native_buttons(window: &gpui::Window) {
    use objc::runtime::{Object, BOOL, YES};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // The app-owned header renders the themed window controls. Hide the AppKit
    // duplicates while retaining titled-window menus, resizing and fullscreen.
    let hidden = YES;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_controls_have_native_order_and_semantics() {
        use WindowAction::{Close, Fullscreen, Maximize, Minimize};
        let supported = gpui::WindowControls::default();
        let client = gpui::Decorations::Client {
            tiling: Default::default(),
        };
        assert_eq!(
            controls(ControlPlatform::Mac, client, supported),
            [Close, Minimize, Fullscreen]
        );
        assert_eq!(
            controls(
                ControlPlatform::Windows,
                gpui::Decorations::Server,
                supported
            ),
            [Minimize, Maximize, Close]
        );
        assert_eq!(
            controls(ControlPlatform::Linux, client, supported),
            [Minimize, Maximize, Close]
        );
        assert!(controls(ControlPlatform::Linux, gpui::Decorations::Server, supported).is_empty());
        for minimize in [false, true] {
            for maximize in [false, true] {
                let actions = controls(
                    ControlPlatform::Linux,
                    client,
                    gpui::WindowControls {
                        minimize,
                        maximize,
                        ..supported
                    },
                );
                assert_eq!(actions.contains(&Minimize), minimize);
                assert_eq!(actions.contains(&Maximize), maximize);
                assert_eq!(actions.last(), Some(&Close));
                assert!(!actions.contains(&Fullscreen));
            }
        }
        assert_eq!(Maximize.label(false, false), "Maximize window");
        assert_eq!(Maximize.label(false, true), "Restore window");
        assert_eq!(Fullscreen.label(false, false), "Enter full screen");
        assert_eq!(Fullscreen.label(true, false), "Exit full screen");
    }
}
