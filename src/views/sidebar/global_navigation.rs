//! Persistent app destinations share the existing workspace/session owners.
use super::*;
use grove_core::storage::LandingPage;

const GLOBAL_ITEM_W: f32 = 36.0;
const GLOBAL_ITEM_H: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Home,
    Projects,
    Sessions,
    Terminals,
}
impl From<LandingPage> for Destination {
    fn from(page: LandingPage) -> Self {
        match page {
            LandingPage::Home => Self::Home,
            LandingPage::Projects => Self::Projects,
            LandingPage::Sessions => Self::Sessions,
        }
    }
}
impl Sidebar {
    pub(crate) fn open_landing_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        let destination = cx
            .global::<SettingsState>()
            .store
            .default_landing_page
            .into();
        self.open_destination(destination, window, cx);
        if destination != Destination::Home {
            self.selection = None;
            cx.notify();
        }
    }

    pub(super) fn open_destination(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() {
            return;
        }
        self.close_settings_for_session_navigation(window, cx);
        self.dismiss_project_panel_for_navigation();
        self.dismiss_project_context(cx);
        self.zen_return = None;
        self.pending_canvas_focus = None;
        self.pending_grid_workspace_focus = None;
        self.initial_selection_pending = false;
        if destination == Destination::Home {
            self.open_dashboard(window, cx);
        } else {
            let changing = self.destination != destination || self.dashboard_open;
            self.destination = destination;
            self.dashboard_open = false;
            self.mode = if destination == Destination::Sessions {
                ViewMode::List
            } else {
                ViewMode::Project
            };
            self.last_mode = self.mode;
            if changing {
                self.selection = None;
            }
            self.terminals_collapsed = false;
            self.focus.focus(window, cx);
        }
        if destination != Destination::Home {
            SettingsState::update(cx, |store| store.sidebar_collapsed = false);
        }
        cx.notify();
    }

    pub(super) fn context_visible(&self, window: &Window, cx: &App) -> bool {
        let settings_open = self
            .settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_open());
        let editor_open = self.pending_new_worktree.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some();
        self.destination != Destination::Home
            && !settings_open
            && !self.is_collapsed(cx)
            && self.mode != ViewMode::Grid
            && !self.is_zen()
            && (!editor_open || Self::logical_window_width(window) >= EDITOR_FULL_WIDTH_BREAKPOINT)
    }

    pub(super) fn global_rail(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let settings_open = self
            .settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_open());
        let mut rail = div()
            .id("global-navigation")
            .debug_selector(|| "global-navigation".into())
            .w(rpx(SIDEBAR_COLLAPSED_W))
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(rpx(SPACE_LG))
            .py(rpx(SPACE_LG))
            .border_r_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL());
        if !self.context_visible(window, cx) {
            let workspace = &cx.global::<SettingsState>().store.workspaces;
            let name = workspace.name(workspace.active).to_string();
            let trigger = self
                .workspace_selector
                .as_ref()
                .map(|selector| selector.read(cx).compact_trigger_bounds());
            let mut control = self
                .control(
                    "global-nav-workspace",
                    format!("Switch workspace · {name}"),
                    Action::OpenWorkspaces,
                    cx,
                )
                .debug_selector(|| "global-nav-workspace".into())
                .track_focus(&self.compact_workspace_focus)
                .w(rpx(GLOBAL_ITEM_W))
                .h(rpx(GLOBAL_ITEM_H))
                .flex_shrink_0()
                .child(icon("folder", ICON_LG, c::FG_DIM()));
            if let Some(trigger) = trigger {
                control = control.child(
                    gpui::canvas(move |bounds, _, _| trigger.set(bounds), |_, (), _, _| {})
                        .absolute()
                        .inset_0(),
                );
            }
            rail = rail
                .child(control)
                .when_some(self.workspace_selector.clone(), gpui::ParentElement::child);
            if self.runtime.read(cx).upgrade.read(cx).can_update() {
                rail = rail.child(
                    self.control("sidebar-update", "Update Grove", Action::UpdateApp, cx)
                        .debug_selector(|| "sidebar-update".into())
                        .w(rpx(GLOBAL_ITEM_W))
                        .h(rpx(GLOBAL_ITEM_H))
                        .flex_shrink_0()
                        .child(icon("arrow-up", ICON_LG, c::FG_DIM())),
                );
            }
        }
        for (destination, label, glyph) in [
            (Destination::Home, "Home", "home"),
            (Destination::Projects, "Projects", "folder"),
            (Destination::Sessions, "Sessions", "rail-sessions"),
            (Destination::Terminals, "Terminals", "terminal"),
        ] {
            let selected = !settings_open && self.destination == destination;
            let id = format!("global-nav-{}", label.to_lowercase());
            rail = rail.child(
                self.control(
                    SharedString::from(id.clone()),
                    label,
                    Action::Destination(destination),
                    cx,
                )
                .debug_selector(move || id.clone())
                .w(rpx(GLOBAL_ITEM_W))
                .h(rpx(GLOBAL_ITEM_H))
                .flex_shrink_0()
                .rounded(rpx(RADIUS_CHROME))
                .aria_selected(selected)
                .when(selected, |control| control.bg(c::BG_HOVER()))
                .child(icon(
                    glyph,
                    ICON_LG,
                    if selected { c::FG() } else { c::FG_DIM() },
                )),
            );
        }
        rail.child(div().flex_1().min_h(rpx(SPACE_LG)).on_mouse_down(
            MouseButton::Left,
            |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            },
        ))
        .child(
            self.control("global-nav-settings", "Settings", Action::OpenSettings, cx)
                .debug_selector(|| "global-nav-settings".into())
                .w(rpx(GLOBAL_ITEM_W))
                .h(rpx(GLOBAL_ITEM_H))
                .flex_shrink_0()
                .rounded(rpx(RADIUS_CHROME))
                .aria_selected(settings_open)
                .when(settings_open, |control| control.bg(c::BG_HOVER()))
                .child(icon("cog", ICON_LG, c::FG_DIM())),
        )
        .into_any_element()
    }

    pub(super) fn context_workspace_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let workspace = &cx.global::<SettingsState>().store.workspaces;
        let name = workspace.name(workspace.active).to_string();
        let trigger = self
            .workspace_selector
            .as_ref()
            .map(|selector| selector.read(cx).compact_trigger_bounds());
        let mut control = self
            .control(
                "context-workspace",
                format!("Switch workspace · {name}"),
                Action::OpenWorkspaces,
                cx,
            )
            .debug_selector(|| "context-workspace".into())
            .track_focus(&self.compact_workspace_focus)
            .flex_1()
            .min_w_0()
            .h(rpx(APPBAR_H))
            .justify_start()
            .px(rpx(SIDEBAR_CONTENT_INSET))
            .gap(rpx(SPACE_LG))
            .child(icon("folder", ICON_MD, c::FG_DIM()))
            .child(div().min_w_0().truncate().child(name))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("workspace"),
            );
        if let Some(trigger) = trigger {
            control = control.child(
                gpui::canvas(move |bounds, _, _| trigger.set(bounds), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            );
        }
        div()
            .debug_selector(|| "context-workspace-row".into())
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(control)
            .when(self.runtime.read(cx).upgrade.read(cx).can_update(), |row| {
                row.child(
                    self.control("sidebar-update", "Update Grove", Action::UpdateApp, cx)
                        .debug_selector(|| "sidebar-update".into())
                        .flex_shrink_0()
                        .child(icon("arrow-up", ICON_MD, c::FG_DIM())),
                )
            })
            .when_some(self.workspace_selector.clone(), gpui::ParentElement::child)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn global_destinations_switch_context_and_keep_home_explicit(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                default_landing_page: LandingPage::Projects,
                sidebar_collapsed: true,
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(800.)));
        draw(cx);
        cx.update(|window, cx| {
            let view = sidebar.read(cx);
            assert_eq!(view.destination, Destination::Projects);
            assert!(!view.dashboard_open);
            assert!(view.selection.is_none());
            assert!(view.context_visible(window, cx));
            assert_eq!(view.rail_width(window, cx), SIDEBAR_COLLAPSED_W + SIDEBAR_W);
        });
        assert!(cx.debug_bounds("navigation-empty-canvas").is_some());
        let home = cx.debug_bounds("global-nav-home").unwrap();
        cx.simulate_click(home.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).dashboard_open);
            assert_eq!(sidebar.read(cx).destination, Destination::Home);
            assert!(!sidebar.read(cx).context_visible(window, cx));
            assert_eq!(sidebar.read(cx).rail_width(window, cx), SIDEBAR_COLLAPSED_W);
        });
        for (selector, expected) in [
            ("global-nav-sessions", Destination::Sessions),
            ("global-nav-terminals", Destination::Terminals),
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            draw(cx);
            cx.update(|window, cx| {
                assert_eq!(sidebar.read(cx).destination, expected);
                assert!(sidebar.read(cx).context_visible(window, cx));
                assert!(!sidebar.read(cx).dashboard_open);
            });
        }
        cx.update(|window, cx| {
            sidebar.update(cx, |view, cx| {
                view.open_landing_page(window, cx);
                assert_eq!(view.destination, Destination::Projects);
                assert!(view.selection.is_none());
            });
        });
    }

    #[gpui::test]
    fn settings_preserves_global_destination_and_landing_reopens_empty(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let settings =
                cx.new(|cx| crate::views::settings_panel::SettingsPanel::new(runtime.clone(), cx));
            let mut view = Sidebar::new(runtime, window, cx);
            view.set_settings_panel(settings, cx);
            view
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |view, cx| {
                view.open_destination(Destination::Sessions, window, cx);
                view.selection = Some(Selection::Session(SessionId::from_raw(91)));
                let settings = view.settings_panel.clone().unwrap();
                settings.update(cx, |settings, cx| settings.open(window, cx));
                assert!(!view.context_visible(window, cx));
                assert_eq!(view.destination, Destination::Sessions);
                settings.update(cx, |settings, cx| settings.close(window, cx));
                assert!(view.context_visible(window, cx));
                assert_eq!(
                    view.selection,
                    Some(Selection::Session(SessionId::from_raw(91)))
                );
                SettingsState::update(cx, |store| {
                    store.default_landing_page = LandingPage::Sessions;
                });
                view.pending_close = Some(SessionId::from_raw(91));
                view.open_landing_page(window, cx);
                assert_eq!(
                    view.selection,
                    Some(Selection::Session(SessionId::from_raw(91)))
                );
                view.pending_close = None;
                view.open_landing_page(window, cx);
                assert_eq!(view.destination, Destination::Sessions);
                assert!(view.selection.is_none());
            });
        });
    }
    #[gpui::test]
    fn selections_route_to_their_own_context_and_close_remains_guarded(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |view, cx| {
                view.select(Selection::Project(0), cx);
                assert_eq!(view.destination, Destination::Projects);
                assert_eq!(view.mode, ViewMode::Project);
                assert!(!view.dashboard_open);
                view.open_destination(Destination::Sessions, window, cx);
                view.select(Selection::Worktree(0, "/sample/topic".into()), cx);
                assert_eq!(view.destination, Destination::Projects);
                assert_eq!(view.mode, ViewMode::Project);
                view.select(Selection::Session(SessionId::from_raw(31)), cx);
                view.act(Action::Close(SessionId::from_raw(31)), window, cx);
                view.act(Action::Destination(Destination::Home), window, cx);
                assert_eq!(view.pending_close, Some(SessionId::from_raw(31)));
                assert_eq!(
                    view.selection,
                    Some(Selection::Session(SessionId::from_raw(31)))
                );
                view.act(Action::Cancel, window, cx);
                assert!(!view.confirmation_open());
                assert_eq!(
                    view.selection,
                    Some(Selection::Session(SessionId::from_raw(31)))
                );
                view.open_dashboard(window, cx);
                view.select(Selection::Home(SessionId::from_raw(43)), cx);
                assert_eq!(view.destination, Destination::Terminals);
                assert!(!view.dashboard_open);
            });
        });
    }
    #[gpui::test]
    fn global_rail_scrolls_to_settings_in_short_windows(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(200.)));
        draw(cx);
        let rail = cx.debug_bounds("global-navigation").unwrap();
        let terminal = cx.debug_bounds("global-nav-terminals").unwrap();
        let settings = cx.debug_bounds("global-nav-settings").unwrap();
        assert!(terminal.bottom() <= settings.top());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: rail.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-500.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        let settings = cx.debug_bounds("global-nav-settings").unwrap();
        assert!(settings.top() >= rail.top() && settings.bottom() <= rail.bottom());
        cx.simulate_click(settings.center(), gpui::Modifiers::default());
    }
    #[gpui::test]
    fn closing_standalone_terminals_keeps_destination_with_project_sessions_open(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "mixed".into(),
                    path: "/grove-mixed-terminal-close".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let project = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "mixed".into(),
                    "/grove-mixed-terminal-close".into(),
                    Agent::Codex,
                )
            });
            for _ in 0..2 {
                let terminal = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::spawn_script("\0", "/", cx)
                });
                registry.update(cx, |registry, _| {
                    let mut meta = registry.meta(project).unwrap().clone();
                    meta.id = registry.next_home_id();
                    meta.project.clear();
                    meta.wt_path = "/".into();
                    meta.agent = Agent::Terminal;
                    registry.push_home(meta, terminal);
                });
            }
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |view, cx| {
                let ids = view
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminals()
                    .iter()
                    .map(|meta| meta.id)
                    .collect::<Vec<_>>();
                view.select(Selection::Home(ids[0]), cx);
                view.act(Action::CloseHome(ids[0]), window, cx);
                view.act(Action::ConfirmHome(ids[0]), window, cx);
                assert_eq!(view.destination, Destination::Terminals);
                assert_eq!(view.selection, Some(Selection::Home(ids[1])));
                view.act(Action::CloseHome(ids[1]), window, cx);
                view.act(Action::ConfirmHome(ids[1]), window, cx);
                assert_eq!(view.destination, Destination::Terminals);
                assert!(view.selection.is_none());
                assert_eq!(view.runtime.read(cx).registry.read(cx).all().len(), 1);
                assert_eq!(
                    view.runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .home_terminal_count(),
                    0
                );
            });
        });
    }
}
