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
// Settings and theme editing APIs are retained without settings controls.
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
#[allow(dead_code)]
mod scripts;
#[allow(dead_code)]
mod theme_preview;
// Persisted zoom and PTY sizing policy remain available for future surfaces.
#[allow(dead_code)]
mod zoom;

use gpui::{prelude::*, px, size, Bounds, TitlebarOptions, WindowBounds, WindowOptions};

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
    logging::init();
    // Before `app::boot` so a panic inside boot is still reported; only the scrubbed location is sent.
    telemetry::install_panic_hook();
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut gpui::App| {
            app::boot(cx);
            cx.bind_keys([
                gpui::KeyBinding::new(
                    &format!("{}q", keymap::platform_mod_prefix()),
                    views::shell::Quit,
                    None,
                ),
                gpui::KeyBinding::new(
                    &format!("{}w", keymap::platform_mod_prefix()),
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
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(WINDOW_MIN_W), px(WINDOW_MIN_H))),
                app_owns_titlebar_drag: true,
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            // Shell owns lifecycle and unconsumed Tab traversal. Keep it as the
            // window root so component Root Copy bindings cannot intercept PTY input.
            let window =
                match cx.open_window(opts, |window, cx| cx.new(|cx| Shell::new(window, cx))) {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::error!("grove-gpui: could not open window: {e}");
                        eprintln!("grove-gpui: could not open window: {e}");
                        std::process::exit(1);
                    }
                };
            // Seed OS appearance here so follow-system resolves on the first frame (`src/gui/mod.rs:63-68`).
            let _ = window.update(cx, |view, window, cx| {
                let handle = gpui::Focusable::focus_handle(view, cx);
                window.focus(&handle, cx);
                let mode = window.appearance();
                theme::ThemeState::set_system_mode(cx, mode);
            });
            cx.activate(true);
        });
}
