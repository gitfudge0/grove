//! Grove's app-owned header and the empty canvas for the UI rebuild.
use super::components::header_control;
use super::{rpx, tokens::*};
use crate::{icons::icon, theme as c};
use crate::{runtime::Runtime, theme::ThemeState};
use gpui::{
    actions, div, prelude::*, App, Context, Entity, FocusHandle, Focusable, FontWeight,
    MouseButton, Window,
};

const TRAFFIC_LIGHT_D: f32 = 12.0;
const TRAFFIC_CONTROL_W: f32 = 18.0;

actions!(shell, [Quit, CloseWindow]);

/// Bubble only unconsumed navigation keys. Mounting component Root would also
/// install its Copy action context, which can steal PTY Ctrl+C. TerminalView
/// consumes Tab as input first; open menus and confirmations trap it in capture.
fn traverse_unhandled_tab(event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut App) {
    let key = &event.keystroke;
    if key.key == "tab"
        && !key.modifiers.control
        && !key.modifiers.alt
        && !key.modifiers.platform
        && !key.modifiers.function
    {
        window.prevent_default();
        if key.modifiers.shift {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        cx.stop_propagation();
    }
}

pub struct Shell {
    focus: FocusHandle,
    runtime: Entity<Runtime>,
    workspaces: Entity<super::workspace_manager::WorkspaceManager>,
    sidebar: Entity<super::sidebar::Sidebar>,
    window_observers: Option<Vec<gpui::Subscription>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        Self {
            sidebar,
            focus: cx.focus_handle(),
            runtime,
            workspaces: cx.new(|cx| super::workspace_manager::WorkspaceManager::new(window, cx)),
            window_observers: None,
        }
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fullscreen_label = if window.is_fullscreen() {
            "Exit full screen"
        } else {
            "Enter full screen"
        };
        let traffic = |id, label, color, glyph| {
            header_control(id, label)
                .tab_index(0)
                .focus(|style| style.bg(c::BG_HOVER()))
                .w(rpx(TRAFFIC_CONTROL_W))
                .child(
                    div()
                        .size(rpx(TRAFFIC_LIGHT_D))
                        .rounded_full()
                        .bg(color)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icon(glyph, ICON_XS, c::WINDOW_CONTROL_GLYPH())
                                .opacity(0.0)
                                .group_hover("traffic-lights", |s| s.opacity(1.0)),
                        ),
                )
        };
        div()
            .id("app-header")
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(rpx(APPBAR_H))
            .px(rpx(SPACE_2XL))
            .bg(c::BG_STRIP())
            .border_b_1()
            .border_color(c::BORDER_STRONG())
            .child(
                div()
                    .group("traffic-lights")
                    .flex()
                    .items_center()
                    .child(
                        traffic("window-close", "Close window", c::WINDOW_CLOSE(), "close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.flush(cx);
                                window.remove_window();
                            })),
                    )
                    .child(
                        traffic(
                            "window-minimize",
                            "Minimize window",
                            c::WINDOW_MINIMIZE(),
                            "minus",
                        )
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.minimize_window();
                        }),
                    )
                    .child(
                        traffic(
                            "window-fullscreen",
                            fullscreen_label,
                            c::WINDOW_FULLSCREEN(),
                            "plus",
                        )
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.toggle_fullscreen();
                        }),
                    ),
            )
            .child(
                div()
                    .id("header-drag-region")
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(rpx(SPACE_2XL))
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_BRAND))
                            .font_weight(FontWeight::BOLD)
                            .text_color(c::FG())
                            .child("GROVE"),
                    ),
            )
            .child(div().ml(rpx(SPACE_2XL)).child(self.workspaces.clone()))
            .child(
                div()
                    .id("header-empty-drag-region")
                    .flex_1()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .when(self.sidebar.read(cx).is_grid(), |header| {
                header.child(
                    self.sidebar
                        .update(cx, |sidebar, cx| sidebar.view_controls(cx)),
                )
            })
            .child(
                header_control("settings", "Settings coming soon")
                    .role(gpui::Role::Image)
                    .aria_description("Settings is currently unavailable")
                    .border_1()
                    .border_color(c::BORDER())
                    .rounded(rpx(RADIUS_CHROME))
                    .opacity(OPACITY_DISABLED)
                    .child(icon("cog", ICON_MD, c::FG_DIM())),
            )
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

        // AppKit may restore the native buttons during fullscreen transitions.
        if !cfg!(test) {
            crate::platform::window_chrome::hide_native_buttons(window);
        }
        window.set_rem_size(gpui::px(cx.global::<crate::zoom::ZoomState>().rem_size()));
        div()
            .id("grove-shell")
            .track_focus(&self.focus)
            .on_key_down(traverse_unhandled_tab)
            .size_full()
            .flex()
            .flex_col()
            .bg(crate::theme::BG())
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.flush(cx);
                cx.quit();
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.flush(cx);
                window.remove_window();
            }))
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(self.header(window, cx))
                    .when(self.sidebar.read(cx).confirmation_open(), |header| {
                        header.child(
                            div()
                                .absolute()
                                .inset_0()
                                .occlude()
                                .bg(c::alpha(c::BG(), 0.4))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(cx: &mut App) {
        gpui_component::init(cx);
        let project = grove_core::storage::Project {
            name: "navigation".into(),
            path: "/grove-shell-navigation-test".into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            theme: None,
            archived: false,
            worktree_dir: None,
        };
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store {
                projects: vec![project],
                ..grove_core::storage::Store::default()
            },
        ));
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(ThemeState::new(
            false,
            "tokyonight".into(),
            "tokyonight-day".into(),
        ));
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn ordinary_sidebar_controls_traverse_forward_and_backward(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let project = cx.debug_bounds("project-0").expect("project row").center();
        cx.simulate_mouse_down(project, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(project, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        let initial = cx.update(|window, cx| window.focused(cx).expect("project focus"));
        cx.simulate_keystrokes("tab");
        draw(cx);
        let next = cx.update(|window, cx| window.focused(cx).expect("next control"));
        assert_ne!(initial, next);
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(initial)));
    }

    #[gpui::test]
    fn header_workspace_control_traverses_and_activates(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let picker = cx.update(|window, cx| {
            let picker = shell.read(cx).workspaces.focus_handle(cx);
            picker.focus(window, cx);
            picker
        });
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_ne!(window.focused(cx), Some(picker.clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(picker.clone())));
        for key in ["enter", "space"] {
            cx.simulate_keystrokes(key);
            draw(cx);
            assert!(cx.debug_bounds("workspace-popup").is_some());
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(cx.debug_bounds("workspace-popup").is_none());
        }
    }

    #[gpui::test]
    fn worktree_inputs_tab_in_order_then_reach_cancel(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let menu = cx
            .debug_bounds("project-menu-0")
            .expect("project menu")
            .center();
        cx.simulate_mouse_down(menu, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(menu, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        let mut fields = Vec::new();
        for selector in [
            "worktree-name-field",
            "worktree-branch-field",
            "worktree-base-field",
        ] {
            let bounds = cx.debug_bounds(selector).expect("worktree field");
            let point = gpui::point(bounds.center().x, bounds.bottom() - gpui::px(SPACE_LG));
            cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
            draw(cx);
            fields.push(cx.update(|window, cx| window.focused(cx).expect("input focus")));
        }
        assert_ne!(fields[0], fields[1]);
        assert_ne!(fields[1], fields[2]);
        cx.update(|window, cx| fields[0].focus(window, cx));
        for expected in &fields[1..] {
            cx.simulate_keystrokes("tab");
            draw(cx);
            cx.update(|window, cx| assert_eq!(window.focused(cx), Some(expected.clone())));
        }
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[1].clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[2].clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            assert_ne!(
                window.focused(cx),
                Some(fields[2].clone()),
                "Tab leaves base for Cancel"
            );
        });
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        draw(cx);
        assert!(
            cx.debug_bounds("worktree-name-field").is_none(),
            "Cancel closes the worktree editor; disabled Create is skipped"
        );
    }

    struct TerminalFixture {
        terminal: Entity<super::super::terminal_view::TerminalView>,
        session: Entity<crate::entities::terminal_session::TerminalSession>,
    }

    impl Render for TerminalFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_key_down(traverse_unhandled_tab)
                .child(div().id("other-control").tab_index(0).child("Other"))
                .child(self.terminal.clone())
        }
    }

    #[gpui::test]
    fn terminal_consumes_tab_before_shell_traversal(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            // A failed native spawn exercises the real terminal input routing
            // without creating a persistent shell or depending on tmux.
            let session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script("\0", "/", cx)
            });
            assert!(
                session.read(cx).spawn_error().is_some(),
                "NUL script must fail before any PTY reader starts"
            );
            let terminal = cx.new(|cx| {
                super::super::terminal_view::TerminalView::new(session.clone(), None, cx)
            });
            TerminalFixture { terminal, session }
        });
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            fixture.terminal.focus_handle(cx).focus(window, cx);
        });
        let before_key = std::time::Instant::now();
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert!(fixture.terminal.focus_handle(cx).is_focused(window));
            assert!(
                fixture
                    .session
                    .read(cx)
                    .input_age()
                    .is_some_and(|age| age <= before_key.elapsed()),
                "Tab reached TerminalSession::send"
            );
        });
        let before_control_c = std::time::Instant::now();
        cx.simulate_keystrokes("ctrl-c");
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert!(fixture.terminal.focus_handle(cx).is_focused(window));
            assert!(
                fixture
                    .session
                    .read(cx)
                    .input_age()
                    .is_some_and(|age| age <= before_control_c.elapsed()),
                "Ctrl+C reached TerminalSession::send"
            );
        });
    }
}
