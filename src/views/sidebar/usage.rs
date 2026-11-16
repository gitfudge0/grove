use super::*;
use crate::views::usage_panel::{DismissUsage, UsagePanel, UsageSource};
use grove_core::provider::Provider;

impl Sidebar {
    pub(super) fn dismiss_usage(
        &mut self,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.usage_panel = None;
        self.usage_subscription = None;
        self.usage_open_task = None;
        self.usage_hide_task = None;
        self.usage_hover_open = false;
        self.usage_hover_suppressed = self.usage_anchor.get().contains(&window.mouse_position());
        if restore_focus {
            self.usage_button_focus.focus(window, cx);
        }
        cx.notify();
    }
    pub(super) fn toggle_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage_panel.is_some() && !self.usage_hover_open {
            self.dismiss_usage(true, window, cx);
        } else {
            self.open_usage(false, window, cx);
        }
    }
    fn open_usage(&mut self, hover: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.usage_open_task = None;
        self.usage_hide_task = None;
        if let Some(panel) = &self.usage_panel {
            if !hover {
                self.usage_hover_open = false;
                panel.focus_handle(cx).focus(window, cx);
            }
            return;
        }
        if !self.navigation_available()
            || self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.read(cx).is_open())
        {
            return;
        }
        let sources = self.usage_sources(cx);
        let panel = cx.new(|cx| UsagePanel::new(sources, cx));
        self.usage_subscription = Some(cx.subscribe_in(
            &panel,
            window,
            |this, _, event: &DismissUsage, window, cx| {
                if event
                    .1
                    .is_some_and(|position| this.usage_anchor.get().contains(&position))
                {
                    return;
                }
                this.dismiss_usage(event.0, window, cx);
            },
        ));
        if !hover {
            panel.focus_handle(cx).focus(window, cx);
        }
        self.usage_hover_open = hover;
        self.usage_panel = Some(panel);
        cx.notify();
    }
    pub(super) fn hover_usage(
        &mut self,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            self.usage_open_task = None;
            self.usage_hover_suppressed = false;
            self.track_usage_pointer(window.mouse_position(), window, cx);
            return;
        }
        self.usage_hide_task = None;
        if self.usage_hover_suppressed || self.usage_panel.is_some() {
            return;
        }
        self.usage_open_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(MOTION_BASE_MS))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.usage_anchor.get().contains(&window.mouse_position())
                    && !this.usage_hover_suppressed
                {
                    this.open_usage(true, window, cx);
                }
            });
        }));
    }
    fn usage_contains_pointer(&self, point: gpui::Point<gpui::Pixels>, cx: &App) -> bool {
        let anchor = self.usage_anchor.get();
        let popup = self
            .usage_panel
            .as_ref()
            .map(|p| p.read(cx).bounds.get())
            .unwrap_or_default();
        anchor.contains(&point)
            || popup.contains(&point)
            || (point.x >= anchor.left().max(popup.left())
                && point.x <= anchor.right().min(popup.right())
                && point.y >= popup.bottom()
                && point.y <= anchor.top())
    }
    pub(super) fn track_usage_pointer(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.usage_hover_open || self.usage_panel.is_none() {
            return;
        }
        if self.usage_contains_pointer(point, cx) {
            self.usage_hide_task = None;
        } else if self.usage_hide_task.is_none() {
            self.usage_hide_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(MOTION_SLOW_MS))
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let keyboard_inside = this
                        .usage_panel
                        .as_ref()
                        .is_some_and(|panel| panel.focus_handle(cx).contains_focused(window, cx));
                    if this.usage_hover_open
                        && !this.usage_contains_pointer(window.mouse_position(), cx)
                        && !keyboard_inside
                    {
                        this.dismiss_usage(false, window, cx);
                    }
                    this.usage_hide_task = None;
                });
            }));
        }
    }
    fn usage_sources(&self, cx: &App) -> Vec<UsageSource> {
        let registry = self.runtime.read(cx).registry.read(cx);
        let selected = if self.dashboard_open {
            self.dashboard.as_ref().map(|d| d.session.clone())
        } else {
            match self.selection {
                Some(Selection::Session(id)) => registry.codex(id).cloned(),
                _ => None,
            }
        };
        let cwd = if self.dashboard_open {
            self.dashboard
                .as_ref()
                .map(|d| d.tray.read(cx).path.clone())
        } else {
            selected
                .as_ref()
                .map(|s| s.read(cx).state.saved.cwd.clone())
                .or_else(|| self.selected_worktree().map(|(_, path)| path))
        }
        .unwrap_or_else(crate::entities::session_registry::home_dir);
        [Provider::Codex, Provider::Claude]
            .into_iter()
            .map(|provider| {
                let args = selected
                    .as_ref()
                    .filter(|s| s.read(cx).state.saved.provider == provider)
                    .map(|s| s.read(cx).state.saved.config_args.clone())
                    .unwrap_or_default();
                // An alternate provider uses this checkout's default configuration,
                // never an arbitrary matching session's account override.
                let session = selected
                    .as_ref()
                    .filter(|s| {
                        let s = s.read(cx);
                        s.has_transport()
                            && s.state.saved.provider == provider
                            && s.state.saved.cwd == cwd
                            && s.state.saved.config_args == args
                    })
                    .cloned()
                    .or_else(|| {
                        registry
                            .all()
                            .iter()
                            .filter_map(|m| registry.codex(m.id))
                            .find(|s| {
                                let s = s.read(cx);
                                s.has_transport()
                                    && s.state.saved.provider == provider
                                    && s.state.saved.cwd == cwd
                                    && s.state.saved.config_args == args
                            })
                            .cloned()
                    });
                UsageSource {
                    provider,
                    cwd: cwd.clone(),
                    args,
                    session,
                }
            })
            .collect()
    }
    pub(super) fn render_usage_popup(&self, window: &Window, _: &App) -> Option<AnyElement> {
        let panel = self.usage_panel.clone()?;
        Some(
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(gpui::point(
                        self.usage_anchor.get().left(),
                        self.usage_anchor.get().top() - rpx(SPACE_LG).to_pixels(window.rem_size()),
                    ))
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(panel),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::ZoomState::new(1.));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        c::sync_component_theme(cx);
    }
    #[gpui::test]
    fn usage_opens_over_settings_without_closing_it(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let mut sidebar = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let settings =
                cx.new(|cx| crate::views::settings_panel::SettingsPanel::new(runtime.clone(), cx));
            let view = cx.new(|cx| {
                let mut view = Sidebar::new(runtime, window, cx);
                view.set_settings_panel(settings.clone(), cx);
                view
            });
            sidebar = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let sidebar = sidebar.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let settings = sidebar.read(cx).settings_panel.clone().unwrap();
            settings.update(cx, |settings, cx| settings.open(window, cx));
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let anchor = cx.debug_bounds("sidebar-usage").unwrap();
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_BASE_MS + 1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let sidebar = sidebar.read(cx);
            assert!(sidebar.usage_panel.is_some());
            assert!(sidebar.usage_hover_open);
            let settings = sidebar.settings_panel.as_ref().unwrap();
            assert!(settings.read(cx).is_open());
            assert!(settings.focus_handle(cx).contains_focused(window, cx));
        });
        assert!(cx.debug_bounds("provider-usage-panel").is_some());
        cx.simulate_click(anchor.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            assert!(!sidebar.usage_hover_open);
            assert!(sidebar
                .usage_panel
                .as_ref()
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx));
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            assert!(sidebar.usage_panel.is_none());
            assert!(sidebar.settings_panel.as_ref().unwrap().read(cx).is_open());
            assert!(sidebar.usage_button_focus.is_focused(window));
        });
    }
    #[gpui::test]
    fn usage_icon_opens_by_keyboard_and_escape_restores_focus(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let mut sidebar = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let view = cx.new(|cx| Sidebar::new(runtime, window, cx));
            sidebar = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let sidebar = sidebar.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        sidebar.update(cx, |sidebar, cx| {
            let dashboard = sidebar.dashboard.as_ref().unwrap();
            let old_path = dashboard.tray.read(cx).path.clone();
            let old_args = dashboard.session.read(cx).state.saved.config_args.clone();
            let provider = dashboard.session.read(cx).state.saved.provider;
            dashboard.tray.update(cx, |tray, _| {
                tray.path = "/visible-dashboard-checkout".into();
            });
            dashboard.session.update(cx, |session, _| {
                session.state.saved.config_args = vec!["dashboard-configuration".into()];
            });
            let sources = sidebar.usage_sources(cx);
            assert!(sources
                .iter()
                .all(|source| source.cwd == "/visible-dashboard-checkout"));
            assert_eq!(
                sources
                    .iter()
                    .find(|source| source.provider == provider)
                    .unwrap()
                    .args,
                ["dashboard-configuration"]
            );
            assert!(sources
                .iter()
                .find(|source| source.provider != provider)
                .unwrap()
                .args
                .is_empty());
            dashboard.tray.update(cx, |tray, _| tray.path = old_path);
            dashboard
                .session
                .update(cx, |session, _| session.state.saved.config_args = old_args);
        });
        let usage = cx.debug_bounds("sidebar-usage").unwrap();
        let settings = cx.debug_bounds("sidebar-settings").unwrap();
        assert!(usage.right() <= settings.left());
        cx.update(|window, cx| {
            let focus = sidebar.read(cx).usage_button_focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(sidebar.read(cx).usage_panel.is_some());
            assert!(sidebar.read(cx).html_overlay_open());
        });
        assert!(cx.debug_bounds("provider-usage-panel").is_some());
        // A second pointer activation closes rather than reopening after outside-dismiss.
        cx.simulate_click(usage.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, cx| assert!(sidebar.read(cx).usage_panel.is_none()));
        cx.simulate_click(usage.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).usage_panel.is_some());
            let focus = sidebar.read(cx).settings_button_focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).usage_panel.is_none());
            assert!(sidebar.read(cx).usage_button_focus.is_focused(window));
        });
    }
    #[gpui::test]
    fn usage_hover_keeps_focus_and_allows_pointer_crossing(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let mut sidebar = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let view = cx.new(|cx| Sidebar::new(runtime, window, cx));
            sidebar = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let sidebar = sidebar.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let focus = sidebar.read(cx).settings_button_focus.clone();
            focus.focus(window, cx);
        });
        let anchor = cx.debug_bounds("sidebar-usage").unwrap();
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_BASE_MS + 1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(sidebar.read(cx).usage_hover_open);
            assert!(sidebar.read(cx).settings_button_focus.is_focused(window));
        });
        let popup = cx.debug_bounds("provider-usage-panel").unwrap();
        let bridge = gpui::point(anchor.center().x, (popup.bottom() + anchor.top()) / 2.);
        cx.simulate_mouse_move(bridge, None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS + 1));
        cx.run_until_parked();
        cx.update(|_, cx| assert!(sidebar.read(cx).usage_panel.is_some()));
        cx.simulate_mouse_move(popup.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS + 1));
        cx.run_until_parked();
        cx.update(|_, cx| assert!(sidebar.read(cx).usage_panel.is_some()));
        cx.simulate_mouse_move(
            gpui::point(gpui::px(1200.), gpui::px(60.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS + 1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).usage_panel.is_none());
            assert!(sidebar.read(cx).settings_button_focus.is_focused(window));
        });
        // Passing over the icon must not start a provider read.
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_move(
            gpui::point(gpui::px(1200.), gpui::px(60.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_BASE_MS + 1));
        cx.run_until_parked();
        cx.update(|_, cx| assert!(sidebar.read(cx).usage_panel.is_none()));
    }
}
