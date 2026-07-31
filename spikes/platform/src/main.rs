//! Spike S4: Linux platform matrix (window basics, close interception,
//! file drag-drop, clipboard). See
//! docs/superpowers/plans/2026-07-31-gpui-rewrite-01-spikes.md Task 5.
//!
//! Run under Wayland (default in a Wayland session):
//!   cargo run -p spike-platform
//! Force X11 (requires an Xwayland / X server reachable via $DISPLAY):
//!   WAYLAND_DISPLAY= cargo run -p spike-platform
//!
//! Press "c" to write a marker string to the clipboard, then "v" to read
//! it back (logged to stderr). Drag a file from a file manager onto the
//! window to exercise the drop target. Click the close button twice: the
//! first click is intercepted (logged, window stays open), the second
//! closes the app.

use gpui::{
    div, guess_compositor, prelude::*, px, App, AppContext, Bounds, ClipboardItem, Context,
    ExternalPaths, KeyDownEvent, Size, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use std::cell::Cell;
use std::rc::Rc;

struct PlatformSpike {
    focus_handle: gpui::FocusHandle,
    last_clipboard_read: Option<String>,
    drop_log: Vec<String>,
}

impl PlatformSpike {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            last_clipboard_read: None,
            drop_log: Vec::new(),
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "c" => {
                let marker = "grove-spike-clipboard-marker";
                cx.write_to_clipboard(ClipboardItem::new_string(marker.to_string()));
                eprintln!("[clipboard] wrote marker string: {marker:?}");
                cx.notify();
            }
            "v" => {
                let read = cx.read_from_clipboard().and_then(|item| item.text());
                eprintln!("[clipboard] read back: {read:?}");
                self.last_clipboard_read = read;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for PlatformSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure the div holds keyboard focus so on_key_down fires.
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        div()
            .id("root")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(gpui::rgb(0x1a1b26))
            .text_color(gpui::rgb(0xc0caf5))
            .on_key_down(cx.listener(Self::handle_key))
            .on_drop(cx.listener(|this: &mut Self, paths: &ExternalPaths, _window, cx| {
                for path in paths.paths() {
                    let line = format!("[drop] received path: {}", path.display());
                    eprintln!("{line}");
                    this.drop_log.push(line);
                }
                cx.notify();
            }))
            .child(format!(
                "spike-platform | compositor guess: {}",
                guess_compositor()
            ))
            .child("press c to write clipboard marker, v to read it back")
            .child("drag a file onto this window to test drop delivery")
            .child(format!(
                "last clipboard read: {:?}",
                self.last_clipboard_read
            ))
            .child(format!("drop events: {}", self.drop_log.len()))
    }
}

fn main() {
    eprintln!(
        "[env] WAYLAND_DISPLAY={:?} DISPLAY={:?} XDG_SESSION_TYPE={:?}",
        std::env::var("WAYLAND_DISPLAY"),
        std::env::var("DISPLAY"),
        std::env::var("XDG_SESSION_TYPE"),
    );
    eprintln!("[env] guess_compositor() = {}", guess_compositor());

    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(
            None,
            Size {
                width: px(1280.),
                height: px(800.),
            },
            cx,
        );

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("spike-platform".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let entity = cx.new(PlatformSpike::new);

                    // Step 2: close-request interception. Returns false on
                    // the first call (window survives), true afterwards.
                    let close_attempts = Rc::new(Cell::new(0u32));
                    window.on_window_should_close(cx, move |_window, _cx| {
                        let attempts = close_attempts.get() + 1;
                        close_attempts.set(attempts);
                        if attempts == 1 {
                            eprintln!(
                                "[close] intercepted close request #{attempts}, vetoing (window stays open)"
                            );
                            false
                        } else {
                            eprintln!(
                                "[close] close request #{attempts}, allowing window to close"
                            );
                            true
                        }
                    });

                    entity.update(cx, |_this, cx| {
                        cx.observe_window_activation(window, |_this, _window, cx| {
                            eprintln!("[focus] window activation changed");
                            cx.notify();
                        })
                        .detach();
                    });

                    entity
                },
            )
            .expect("failed to open spike window");

        window
            .update(cx, |_view, window, _cx| {
                eprintln!(
                    "[window] opened. bounds={:?} title should be 'spike-platform'",
                    window.window_bounds()
                );
            })
            .ok();

        cx.activate(true);

        // Automated clipboard round-trip check (no user interaction needed):
        // the same write_to_clipboard/read_from_clipboard API the "c"/"v"
        // keybindings use above, exercised directly for CI/spike logging.
        let marker = "grove-spike-clipboard-autotest";

        // Immediate attempt, before the compositor has delivered any
        // keyboard/pointer-enter event to our surface.
        cx.write_to_clipboard(ClipboardItem::new_string(marker.to_string()));
        let read_back = cx.read_from_clipboard().and_then(|item| item.text());
        eprintln!(
            "[clipboard-autotest] immediate (pre-focus): wrote {marker:?}, read back {read_back:?}, match={}",
            read_back.as_deref() == Some(marker)
        );

        // gpui's Wayland client only calls wl_data_device.set_selection when
        // it has a mouse- or keyboard-focused window (see
        // gpui_linux::linux::wayland::client::write_to_clipboard); on
        // startup that focus hasn't been granted by the compositor yet, so
        // the write above is a silent no-op on Wayland. Retry after a short
        // delay, by which time the compositor should have sent
        // wl_keyboard::enter / wl_pointer::enter for the new window.
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            cx.update(|cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(marker.to_string()));
                let delayed = cx.read_from_clipboard().and_then(|item| item.text());
                eprintln!(
                    "[clipboard-autotest] delayed(300ms, post-focus): wrote {marker:?}, read back {delayed:?}, match={}",
                    delayed.as_deref() == Some(marker)
                );
            });
        })
        .detach();
    });
}
