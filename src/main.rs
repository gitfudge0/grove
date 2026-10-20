//! Grove's gpui shell. Bootstrap is `gpui_platform::application()` — `gpui` alone has no `Platform` constructor at this rev (spike findings §S1).
// `deny`, not `forbid`: `platform::dock` needs one audited `allow` for the Objective-C runtime on macOS.
#![deny(unsafe_code)]

// Retain activity rules and project-input state while their feature views are absent.
#[allow(dead_code)]
mod activity;
#[allow(dead_code)]
mod add_project;
mod app;
mod assets;
// Entity APIs stay compiled and tested for reconnection to the replacement UI.
#[allow(dead_code)]
mod entities;
mod fonts;
// Layout state remains part of the preserved workspace model.
#[allow(dead_code)]
mod grid;
mod icons;
mod input_policy;
mod keyboard_matrix;
// Keep shortcut definitions and routing policy for the replacement feature views.
#[allow(dead_code)]
mod keymap;
// Preserve launcher and modal state machines independently of rendered controls.
#[allow(dead_code)]
mod launcher;
mod logging;
#[allow(dead_code)]
mod modal;
mod paths;
mod platform;
// Reattachment is explicit until a terminal surface exists again.
#[allow(dead_code)]
mod reattach;
// Settings persist the app-wide appearance and other preferences.
#[allow(dead_code)]
mod settings;
mod telemetry;
// Keep terminal input/rendering primitives; the empty shell mounts no PTYs.
#[allow(dead_code)]
mod terminal;
#[allow(dead_code)]
mod terminal_element;
mod theme;
mod views;
// Preserved services have no feature controls while the shell is empty.
#[allow(dead_code)]
mod project_service;
#[allow(dead_code)]
mod runtime;
// Persisted zoom and PTY sizing policy remain available for future surfaces.
#[allow(dead_code)]
mod zoom;

use gpui::{
    prelude::*, px, size, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions,
};

use assets::Assets;
use views::shell::Shell;

/// When set to `1`, runs the startup metric assertion and exits before opening a window.
const SELFTEST_ENV: &str = "GROVE_GPUI_SELFTEST";

/// Matches `src/gui/mod.rs:85` — `.window_size(Size::new(1280.0, 800.0))`.
const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 800.0;
// Keep the app header's controls reachable at the smallest window size.
const WINDOW_MIN_W: f32 = 320.0;
const WINDOW_MIN_H: f32 = 200.0;

fn main() {
    // Choose one native window system before logging or GPUI can start
    // background threads. WebKitGTK child views require X11; Wayland desktops
    // provide the same host window through XWayland.
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = require_x11_display(std::env::var_os("DISPLAY").as_deref()) {
            eprintln!("grove: {error}");
            std::process::exit(1);
        }
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("GDK_BACKEND", "x11");
    }
    logging::init();
    // Before `app::boot` so a panic inside boot is still reported; only the scrubbed location is sent.
    telemetry::install_panic_hook();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut gpui::App| {
            app::boot(cx);
            let reduce_motion = cx
                .global::<settings::SettingsState>()
                .store
                .reduce_motion
                .unwrap_or(false);
            cx.set_reduce_motion(reduce_motion);
            cx.bind_keys(keymap::shell_bindings());
            #[cfg(debug_assertions)]
            cx.bind_keys([gpui::KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "cmd-shift-h"
                } else {
                    "ctrl-shift-h"
                },
                views::shell::PreviewReleaseHighlights,
                None,
            )]);
            cx.bind_keys([
                gpui::KeyBinding::new(
                    &format!("{}q", keymap::platform_mod_prefix()),
                    views::shell::Quit,
                    None,
                ),
                gpui::KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-shift-w"
                    } else {
                        "ctrl-alt-shift-w"
                    },
                    views::shell::CloseWindow,
                    None,
                ),
            ]);

            // Fonts are measured before any window exists: a wrong advance must abort, not paint a drifting grid.
            let cell_w = fonts::register_and_assert_or_exit(cx);

            if std::env::var(SELFTEST_ENV).as_deref() == Ok("1") {
                println!(
                    "GROVE_GPUI_SELFTEST: cell_w={cell_w} cell_h={} font_size={} family={:?} OK",
                    fonts::CELL_H,
                    fonts::FONT_SIZE,
                    fonts::MONO_FAMILY
                );
                std::process::exit(0);
            }

            let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);
            let sidebar_appearance = cx
                .global::<settings::SettingsState>()
                .store
                .sidebar_appearance;
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(WINDOW_MIN_W), px(WINDOW_MIN_H))),
                window_background: match sidebar_appearance {
                    grove_core::storage::SidebarAppearance::Frosted => {
                        WindowBackgroundAppearance::Blurred
                    }
                    grove_core::storage::SidebarAppearance::Solid => {
                        WindowBackgroundAppearance::Opaque
                    }
                },
                app_owns_titlebar_drag: true,
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            // Root supplies native formatted-text drag selection and overlays.
            // Copy key bindings stay scoped to TextView/Input, preserving PTYs.
            let window = match cx.open_window(opts, |window, cx| {
                let shell = cx.new(|cx| Shell::new(window, cx));
                let handle = gpui::Focusable::focus_handle(shell.read(cx), cx);
                window.focus(&handle, cx);
                cx.new(|cx| {
                    gpui_component::Root::new(shell, window, cx)
                        .bordered(false)
                        .bg(theme::alpha(theme::BG(), 0.0))
                })
            }) {
                Ok(window) => window,
                Err(error) => {
                    tracing::error!("grove-gpui: could not open window: {error}");
                    eprintln!("grove-gpui: could not open window: {error}");
                    std::process::exit(1);
                }
            };
            let _ = window.update(cx, |_, window, cx| {
                theme::ThemeState::set_system_mode(cx, window.appearance());
            });
            cx.activate(true);
        });
}

#[cfg(any(target_os = "linux", test))]
fn require_x11_display(display: Option<&std::ffi::OsStr>) -> Result<(), &'static str> {
    if display.is_none_or(std::ffi::OsStr::is_empty) {
        Err("Grove requires an X11 display. On a Wayland desktop, install and enable XWayland, then launch Grove from the desktop session with DISPLAY set.")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod linux_display_tests {
    use super::require_x11_display;
    use std::ffi::OsStr;

    #[test]
    fn x11_bootstrap_requires_a_nonempty_display() {
        assert!(require_x11_display(None).is_err());
        assert!(require_x11_display(Some(OsStr::new(""))).is_err());
        assert!(require_x11_display(Some(OsStr::new(":0"))).is_ok());
        assert!(require_x11_display(Some(OsStr::new("localhost:10.0"))).is_ok());
    }
}
