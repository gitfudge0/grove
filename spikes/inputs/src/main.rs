//! Spike S2: gpui-component text inputs.
//!
//! One single-line `Input` styled as a palette search, plus three multiline
//! editors (the scripts-editor shape, `src/gui/scripts_editor.rs:31-33`).
//! Instrumentation is `eprintln!` at app-level handlers so behavior can be
//! read off stderr while driving the window by hand. See
//! `spikes/inputs/FINDINGS.md` for the recorded results.

use gpui::{
    actions, div, prelude::*, px, size, App, AppContext, Context, Entity, FocusHandle, Focusable,
    KeyBinding, Subscription, Window, WindowBounds, WindowOptions,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Root};

// App-level actions, bound with NO context restriction, so they only ever
// fire once a more-specific "Input"-context binding for the same keystroke
// has propagated (i.e. the Input chose not to consume it). This mirrors the
// real app's `should_forward` Escape carve-out.
actions!(spike_inputs, [AppEscape, AppNavLeft, AppNavRight, AppChord]);

struct Spike {
    search: Entity<InputState>,
    setup: Entity<InputState>,
    run: Entity<InputState>,
    teardown: Entity<InputState>,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search projects & sessions..."));
        let setup = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(6)
                .placeholder("setup script")
        });
        let run = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(6)
                .placeholder("run script")
        });
        let teardown = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(6)
                .placeholder("teardown script")
        });

        let mut subs = Vec::new();
        subs.push(cx.subscribe(&search, |_this, _state, ev: &InputEvent, _cx| match ev {
            InputEvent::Focus => eprintln!("[search] Focus"),
            InputEvent::Blur => eprintln!("[search] Blur"),
            InputEvent::Change => eprintln!("[search] Change"),
            InputEvent::PressEnter { .. } => eprintln!("[search] PressEnter"),
        }));
        for (name, ed) in [("setup", &setup), ("run", &run), ("teardown", &teardown)] {
            let name = name.to_string();
            subs.push(cx.subscribe(ed, move |_this, _state, ev: &InputEvent, _cx| match ev {
                InputEvent::Focus => eprintln!("[{name}] Focus"),
                InputEvent::Blur => eprintln!("[{name}] Blur"),
                InputEvent::Change => eprintln!("[{name}] Change"),
                InputEvent::PressEnter { .. } => eprintln!("[{name}] PressEnter"),
            }));
        }

        // Focus-on-open: request focus for the search input immediately, as
        // the real palette does when it opens.
        search.update(cx, |state, cx| state.focus(window, cx));

        let focus_handle = cx.focus_handle();

        Self {
            search,
            setup,
            run,
            teardown,
            focus_handle,
            _subs: subs,
        }
    }

    // App-level handlers. These are only invoked when the keystroke was not
    // consumed by whichever Input has focus (Escape) — or, for the nav
    // actions, when NO Input has focus at all, since Input unconditionally
    // consumes Left/Right in its own context (see FINDINGS.md).
    fn on_app_escape(&mut self, _: &AppEscape, _window: &mut Window, _cx: &mut Context<Self>) {
        eprintln!("[app] Escape reached app-level handler (palette-close contract)");
    }

    fn on_app_nav_left(&mut self, _: &AppNavLeft, _window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.search.read(cx).value().is_empty();
        eprintln!("[app] AppNavLeft reached app-level handler (search empty={empty})");
    }

    fn on_app_nav_right(&mut self, _: &AppNavRight, _window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.search.read(cx).value().is_empty();
        eprintln!("[app] AppNavRight reached app-level handler (search empty={empty})");
    }

    fn on_app_chord(&mut self, _: &AppChord, _window: &mut Window, cx: &mut Context<Self>) {
        let value = self.search.read(cx).value();
        eprintln!("[app] AppChord (cmd/ctrl-k) reached app-level handler; search value={value:?}");
    }

    // move-cursor-to-end API demonstration, driven from a keybinding below.
    fn move_search_cursor_to_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| {
            let len = state.value().len();
            state.set_selected_range(len..len, cx);
            eprintln!(
                "[app] moved search cursor to end via set_selected_range({len}..{len}), \
                 cursor_position={:?}",
                state.cursor_position()
            );
        });
        let _ = window;
    }
}

impl Focusable for Spike {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("spike-inputs-root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_app_escape))
            .on_action(cx.listener(Self::on_app_nav_left))
            .on_action(cx.listener(Self::on_app_nav_right))
            .on_action(cx.listener(Self::on_app_chord))
            .size_full()
            .bg(cx.theme().background)
            .p_4()
            .gap_4()
            .child(
                // Palette-search styling: full-width, single line, rounded.
                div().w(px(480.)).child(
                    Input::new(&self.search)
                        .appearance(true)
                        .cleanable(true)
                        .w_full(),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .size_full()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .child("setup")
                            .child(Input::new(&self.setup).tab_index(0)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .child("run")
                            .child(Input::new(&self.run).tab_index(1)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .child("teardown")
                            .child(Input::new(&self.teardown).tab_index(2)),
                    ),
            )
    }
}

fn main() {
    let app = gpui_platform::application();

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);

        // App-level bindings, deliberately registered with `None` context so
        // they are reachable from anywhere in the focus tree, including
        // through a focused Input — but only after the Input's own
        // "Input"-context binding for the same keystroke has run and chosen
        // to propagate (Escape does; Left/Right never do, see FINDINGS.md).
        cx.bind_keys([
            KeyBinding::new("escape", AppEscape, None),
            KeyBinding::new("left", AppNavLeft, None),
            KeyBinding::new("right", AppNavRight, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-k", AppChord, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-k", AppChord, None),
        ]);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(900.), px(600.)), cx)),
            ..Default::default()
        };

        cx.open_window(window_options, |window, cx| {
            let view = cx.new(|cx| Spike::new(window, cx));

            // Demonstrate the move-cursor-to-end API once, right after open,
            // after typing some text into the search box (simulated via
            // set_value since we have no keyboard driver in this harness).
            view.update(cx, |spike, cx| {
                spike.search.update(cx, |state, cx| {
                    state.set_value("grove-terminal-app", window, cx);
                });
                spike.move_search_cursor_to_end(window, cx);
            });

            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open window");

        cx.activate(true);
    });
}
