//! Grove's gpui shell. Bootstrap is `gpui_platform::application()` — `gpui` alone has no `Platform` constructor at this rev (spike findings §S1).
// `deny`, not `forbid`: `platform::dock` needs one audited `allow` for the Objective-C runtime on macOS. Everywhere else this is still a hard stop.
#![deny(unsafe_code)]

mod activity;
mod add_project;
mod app;
mod assets;
mod entities;
mod fonts;
mod grid;
mod icons;
mod keyboard_matrix;
mod keymap;
mod launcher;
mod logging;
mod modal;
mod platform;
mod reattach;
mod settings;
mod telemetry;
mod terminal;
mod terminal_element;
mod theme;
mod views;
mod zoom;

use gpui::{prelude::*, px, size, Bounds, TitlebarOptions, WindowBounds, WindowOptions};

use assets::Assets;
use views::workspace::Workspace;

/// When set to `1`, the shell runs the startup metric assertion, prints one machine-checkable line, and exits **before** opening a window, so the exit gate is verifiable without a human eyeballing a desktop.
const SELFTEST_ENV: &str = "GROVE_GPUI_SELFTEST";

/// Matches `src/gui/mod.rs:85` — `.window_size(Size::new(1280.0, 800.0))`.
const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 800.0;

fn main() {
    logging::init();
    // Before `app::boot`, so a panic inside boot is still reported. The panic *message* stays on this machine; only the scrubbed location is sent (`src/main.rs:11-30`).
    telemetry::install_panic_hook();
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut gpui::App| {
            // Startup sequence first (Plan 03 Task 7 Step 1: `boot` precedes font registration, which precedes `open_window`).
            app::boot(cx);

            // Fonts are registered and measured before any window exists: a wrong advance must abort the process, not paint a drifting grid.
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
                titlebar: Some(TitlebarOptions {
                    title: Some("grove".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            // The close-request interception is registered by `Workspace` on its first render, not here: `Window::on_window_should_close` needs the `Workspace` entity to count running native sessions and to run `shutdown`, and the first render is the one place that has both a `&mut Window` and `&mut Context<Workspace>` (the same reason `observe_window_activation` is registered there). See `views::workspace::Workspace::register_close_hook`.
            let window = match cx.open_window(opts, |_window, cx| cx.new(Workspace::new)) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("grove-gpui: could not open window: {e}");
                    eprintln!("grove-gpui: could not open window: {e}");
                    std::process::exit(1);
                }
            };
            // Focus the root so the global key bindings have a dispatch path, and seed the OS appearance so follow-system resolves on the first frame rather than after the first OS notification (`src/gui/mod.rs:63-68`).
            let _ = window.update(cx, |view, window, cx| {
                let handle = gpui::Focusable::focus_handle(view, cx);
                window.focus(&handle, cx);
                let mode = window.appearance();
                theme::ThemeState::set_system_mode(cx, mode);
            });
            cx.activate(true);
        });
}
