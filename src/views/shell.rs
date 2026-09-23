//! Grove's app-owned header and the empty canvas for the UI rebuild.
use super::components::header_control;
use super::{rpx, tokens::*};
use crate::{icons::icon, theme as c};
use crate::{runtime::Runtime, theme::ThemeState};
use gpui::{
    actions, div, prelude::*, App, Context, Entity, FocusHandle, Focusable, MouseButton, Window,
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
    statusbar: Entity<super::statusbar::Statusbar>,
    window_observers: Option<Vec<gpui::Subscription>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let runtime = cx.new(Runtime::new);
        let workspaces = cx.new(|cx| super::workspace_manager::WorkspaceManager::new(window, cx));
        let sidebar = cx.new(|cx| super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        sidebar.update(cx, |sidebar, _| {
            sidebar.set_workspace_selector(workspaces.clone());
        });
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        let statusbar =
            cx.new(|cx| super::statusbar::Statusbar::new(runtime.clone(), sidebar.clone(), cx));
        Self {
            statusbar,
            sidebar,
            focus: cx.focus_handle(),
            runtime,
            workspaces,
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
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
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
            .debug_selector(|| "app-header".into())
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(rpx(APPBAR_H))
            .when(self.sidebar.read(cx).is_grid(), |header| {
                header.pr(rpx(SPACE_2XL))
            })
            .bg(c::BG_STRIP())
            .child(
                div()
                    .id("header-rail-segment")
                    .debug_selector(|| "header-rail-segment".into())
                    .group("traffic-lights")
                    .h_full()
                    .px(rpx(SPACE_2XL))
                    .flex_shrink_0()
                    .when(!self.sidebar.read(cx).is_grid(), |segment| {
                        segment
                            .w(rpx(self.sidebar.read(cx).rail_width(window, cx)))
                            .bg(c::BG_RAIL())
                            .border_r_1()
                            .border_color(c::BORDER())
                    })
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
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
            .when(self.sidebar.read(cx).is_grid(), |header| {
                header.child(div().ml(rpx(SPACE_2XL)).child(self.workspaces.clone()))
            })
            .when(self.sidebar.read(cx).is_grid(), |header| {
                header.child(
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
            })
            .when(self.sidebar.read(cx).is_grid(), |header| {
                header.child(
                    self.sidebar
                        .update(cx, |sidebar, cx| sidebar.view_controls(cx)),
                )
            })
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
            .relative()
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
            .child({
                let grid = self.sidebar.read(cx).is_grid();
                let header = div()
                    .relative()
                    .flex_shrink_0()
                    .when(!grid, |header| {
                        header
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(rpx(self.sidebar.read(cx).rail_width(window, cx)))
                            .h(rpx(APPBAR_H))
                    })
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
                    });
                if grid {
                    header.into_any_element()
                } else {
                    gpui::deferred(header).into_any_element()
                }
            })
            .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
            .child(self.statusbar.clone())
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

    fn assert_rail_alignment(cx: &mut gpui::VisualTestContext, expected: f32) {
        let rail = cx.debug_bounds("sidebar-rail").expect("sidebar rail");
        let divider = cx.debug_bounds("sidebar-divider").expect("resize hit zone");
        let header = cx
            .debug_bounds("header-rail-segment")
            .expect("header rail segment");
        let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
        assert!((f32::from(rail.size.width) - expected).abs() <= 1.0);
        assert_eq!(header.right(), rail.right());
        assert_eq!(canvas.left(), rail.right());
        assert!((f32::from(divider.center().x - rail.right())).abs() <= 1.0);
    }

    #[gpui::test]
    fn sidebar_drag_clamps_persists_on_release_and_survives_reconstruction(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 260.0);
        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                None
            );
        });

        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        let far_right = gpui::point(gpui::px(1100.0), edge.y);
        cx.simulate_mouse_move(far_right, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 640.0);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                None
            );
        });
        cx.simulate_mouse_up(far_right, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(640.0)
            );
        });

        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        let far_left = gpui::point(gpui::px(30.0), edge.y);
        cx.simulate_mouse_move(far_left, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        cx.simulate_mouse_up(far_left, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(220.0)
            );
        });

        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        let list = cx.debug_bounds("sidebar-view").unwrap().center();
        cx.simulate_mouse_down(list, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(list, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        let grid = cx.debug_bounds("sidebar-grid").unwrap().center();
        cx.simulate_mouse_down(grid, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("sidebar-divider").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            gpui::px(0.0)
        );
        let grid = cx.debug_bounds("sidebar-grid").unwrap().center();
        cx.simulate_mouse_down(grid, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
    }

    #[gpui::test]
    fn sidebar_narrow_cap_and_double_click_reset_keep_saved_preference(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        cx.update(|cx| {
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_width = Some(500.0);
        });
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 200.0);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(500.0)
            );
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 500.0);

        cx.update(|_, cx| cx.global_mut::<crate::zoom::ZoomState>().zoom = 1.5);
        cx.simulate_resize(gpui::size(gpui::px(1500.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 750.0);
        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_event(gpui::MouseDownEvent {
            position: edge,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            position: edge,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(260.0)
            );
        });
        assert_rail_alignment(cx, 390.0);
    }

    #[gpui::test]
    fn project_marker_and_settings_follow_navigation_cluster(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let project = cx.debug_bounds("project-0").expect("project row");
        let marker = cx.debug_bounds("project-no-git-0").expect("non-git marker");
        assert!(project.contains(&marker.center()));
        let grid = cx.debug_bounds("sidebar-grid").expect("grid control");
        let settings = cx
            .debug_bounds("sidebar-settings")
            .expect("settings indicator");
        assert_eq!(settings.size, grid.size);
        assert!((f32::from(settings.left() - grid.right()) - SPACE_SM).abs() <= 1.0);
        assert_eq!(settings.center().y, grid.center().y);
        assert!(cx.debug_bounds("settings").is_none());
        cx.simulate_mouse_down(grid.center(), MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid.center(), MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        let header = cx.debug_bounds("app-header").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        assert_eq!(canvas.top(), header.bottom());
        assert_eq!(f32::from(header.size.height), APPBAR_H);
        let grid = cx
            .debug_bounds("sidebar-grid")
            .expect("grid control in app header");
        let settings = cx
            .debug_bounds("sidebar-settings")
            .expect("settings in app header");
        assert_eq!(settings.size, grid.size);
        assert!((f32::from(settings.left() - grid.right()) - SPACE_SM).abs() <= 1.0);
        assert_eq!(settings.center().y, grid.center().y);
        assert!(settings.top() < project.top());
        assert!(cx.debug_bounds("settings").is_none());
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
    fn sidebar_workspace_control_traverses_and_activates(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(600.0)));
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").expect("sidebar rail");
        let segment = cx
            .debug_bounds("header-rail-segment")
            .expect("header rail segment");
        assert_eq!(segment.right(), rail.right());
        assert_eq!(segment.top(), rail.top());
        assert_eq!(
            cx.debug_bounds("canvas-section-header").unwrap().bottom(),
            segment.bottom()
        );
        let workspace_header = cx.debug_bounds("sidebar-workspace-header").unwrap();
        assert_eq!(segment.bottom(), workspace_header.top());
        assert_eq!(
            cx.debug_bounds("canvas-section-header").unwrap().top(),
            rail.top()
        );
        let header = cx
            .debug_bounds("sidebar-workspace-header")
            .expect("workspace header");
        let trigger = cx
            .debug_bounds("workspace-picker")
            .expect("workspace selector");
        assert!(rail.contains(&trigger.center()));
        assert!(header.contains(&trigger.center()));
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
            let popup = cx.debug_bounds("workspace-popup").expect("workspace popup");
            assert_eq!(popup.left(), trigger.left());
            assert!(popup.bottom() > header.bottom());
            assert!(
                popup.right() > rail.right(),
                "menu may extend beyond the rail"
            );
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
        cx.simulate_keystrokes("down enter");
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

    #[gpui::test]
    fn statusbar_spans_narrow_window_and_tracks_toast_lifecycle(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(
            gpui::px(crate::WINDOW_MIN_W),
            gpui::px(crate::WINDOW_MIN_H),
        ));
        draw(cx);
        let footer = cx.debug_bounds("statusbar").expect("persistent footer");
        assert_eq!(
            f32::from(footer.size.height),
            super::super::statusbar::STATUS_H
        );
        assert_eq!(f32::from(footer.size.width), crate::WINDOW_MIN_W);
        assert_eq!(f32::from(footer.bottom()), crate::WINDOW_MIN_H);
        cx.update(|_, cx| {
            shell
                .read(cx)
                .runtime
                .read(cx)
                .toast
                .clone()
                .update(cx, |toast, cx| toast.set_error("Example failure", cx));
        });
        draw(cx);
        let toast = cx.debug_bounds("status-toast").expect("error toast");
        assert!(toast.right() <= footer.right());
        assert!(toast.left() >= footer.left());
        cx.executor()
            .advance_clock(crate::entities::toast::Toast::ttl(
                crate::entities::toast::ToastKind::Error,
            ));
        draw(cx);
        assert!(cx.debug_bounds("status-toast").is_none());
    }

    #[gpui::test]
    fn footer_session_scope_changes_before_sidebar_render(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|_, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    grove_core::agent::Agent::Terminal,
                );
                cx.notify();
                id
            });
            assert_eq!(
                sidebar.read(cx).active_canvas_sessions(cx),
                vec![(id, false)]
            );
            cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
                settings.store.workspaces.create("Other").unwrap();
            });
            // No draw or artificial entity notification between changing the
            // active workspace and the footer's scope read.
            assert!(sidebar.read(cx).active_canvas_sessions(cx).is_empty());
            cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
                settings.store.workspaces.select(1);
            });
            assert_eq!(
                sidebar.read(cx).active_canvas_sessions(cx),
                vec![(id, false)]
            );
        });
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
            let terminal =
                cx.new(|cx| super::super::terminal_view::TerminalView::new(session.clone(), cx));
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
