//! The empty native window used while Grove's UI is rebuilt.
use crate::{runtime::Runtime, theme::ThemeState};
use gpui::{actions, div, prelude::*, App, Context, Entity, FocusHandle, Focusable, Window};

actions!(shell, [Quit, CloseWindow]);

pub struct Shell {
    focus: FocusHandle,
    runtime: Entity<Runtime>,
    window_observers: Option<Vec<gpui::Subscription>>,
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            runtime: cx.new(Runtime::new),
            window_observers: None,
        }
    }

    fn flush(&self, cx: &mut Context<Self>) {
        self.runtime.update(cx, Runtime::shutdown);
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.window_observers.is_none() {
            let runtime = self.runtime.clone();
            window.on_window_should_close(cx, move |_, cx| {
                runtime.update(cx, Runtime::shutdown);
                true
            });
            let runtime = self.runtime.clone();
            self.window_observers = Some(vec![
                cx.observe_window_appearance(window, |_, window, cx| {
                    ThemeState::set_system_mode(cx, window.appearance());
                    cx.notify();
                }),
                cx.observe_window_activation(window, move |_, window, cx| {
                    let active = window.is_window_active();
                    runtime.update(cx, |runtime, cx| {
                        runtime
                            .activity
                            .update(cx, |a, cx| a.set_window_focused(active, cx));
                        if active {
                            runtime
                                .upgrade
                                .update(cx, crate::entities::upgrade::Upgrade::check_if_due);
                        }
                    });
                }),
            ]);
        }

        div()
            .id("grove-shell")
            .track_focus(&self.focus)
            .size_full()
            .bg(crate::theme::BG())
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.flush(cx);
                cx.quit();
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.flush(cx);
                window.remove_window();
            }))
    }
}
