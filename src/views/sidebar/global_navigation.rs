//! App destinations and the single workspace-scoped sidebar.
use super::*;
use grove_core::storage::LandingPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    NewSession,
    Home,
    Projects,
    History,
    Sessions,
    Terminals,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionGrouping {
    Activity,
    Worktrees,
}
impl SessionGrouping {
    fn current(store: &grove_core::storage::Store) -> Self {
        if store.sidebar_group_by_worktree {
            Self::Worktrees
        } else {
            Self::Activity
        }
    }
}
impl From<LandingPage> for Destination {
    fn from(page: LandingPage) -> Self {
        match page {
            LandingPage::NewSession => Self::NewSession,
            LandingPage::Home => Self::Home,
            LandingPage::Projects => Self::Projects,
            LandingPage::History => Self::History,
            LandingPage::Sessions => Self::Sessions,
        }
    }
}
impl Sidebar {
    pub(crate) fn open_numbered_destination(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = match index {
            1 => Destination::NewSession,
            2 => Destination::Home,
            3 => Destination::Projects,
            4 => Destination::History,
            _ => return,
        };
        self.open_destination(destination, window, cx);
    }
    pub(crate) fn open_landing_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        self.open_destination(
            cx.global::<SettingsState>()
                .store
                .default_landing_page
                .into(),
            window,
            cx,
        );
    }
    pub(super) fn open_destination(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_panel.is_some() && !self.project_decision {
            self.dismiss_project_panel_for_navigation();
        }
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
        if matches!(destination, Destination::NewSession | Destination::Home) {
            self.open_dashboard(window, cx);
            self.destination = destination;
            if destination == Destination::Home {
                self.focus.focus(window, cx);
            }
        } else {
            self.destination = destination;
            self.dashboard_open = false;
            self.mode = ViewMode::List;
            self.last_mode = self.mode;
            self.selection = None;
            self.session_history_selected = None;
            self.session_history_view = None;
            if destination == Destination::History {
                self.load_session_history(cx);
            }
            self.focus.focus(window, cx);
        }
        cx.notify();
    }
    pub(super) fn animated_sidebar_width(&self, window: &Window, cx: &App) -> f32 {
        if !cx.reduce_motion() {
            if let Some((started, from, to)) = self.navigation_transition {
                let progress = (started.elapsed().as_secs_f32() / (MOTION_BASE_MS as f32 / 1000.))
                    .clamp(0., 1.);
                return from + (to - from) * motion::ease_standard(progress);
            }
        }
        self.rail_width(window, cx)
    }
    pub(super) fn global_rail(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.is_collapsed(cx);
        let width = self.animated_sidebar_width(window, cx);
        if self.navigation_transition.is_some_and(|(started, _, _)| {
            started.elapsed() < Duration::from_millis(MOTION_BASE_MS)
        }) && !cx.reduce_motion()
        {
            window.request_animation_frame();
        } else {
            self.navigation_transition = None;
        }
        let mut nav = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .px(rpx(SPACE_LG))
            .pt(rpx(SPACE_LG))
            .pb(rpx(SPACE_20))
            .child(
                div()
                    .pb(rpx(SPACE_2XL))
                    .child(self.header_workspace_control(cx)),
            );
        let settings = self
            .settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_open());
        for (index, (destination, label, glyph)) in [
            (Destination::NewSession, "New Session", "plus"),
            (Destination::Home, "Home", "home"),
            (Destination::Projects, "Projects", "folder"),
            (Destination::History, "History", "history"),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = !settings
                && self.destination == destination
                && (self.selection.is_none()
                    || matches!(destination, Destination::NewSession | Destination::Home));
            let id = match destination {
                Destination::NewSession => "global-nav-new-session".to_owned(),
                Destination::Home => "global-nav-home".to_owned(),
                _ => format!("global-nav-{}", label.to_lowercase()),
            };
            nav = nav.child(
                self.control(
                    SharedString::from(id.clone()),
                    label,
                    Action::Destination(destination),
                    cx,
                )
                .debug_selector(move || id.clone())
                .track_focus(&self.navigation_focus[index])
                .on_click(cx.listener(move |this, _, window, cx| {
                    if destination != Destination::NewSession {
                        this.navigation_focus[index].focus(window, cx);
                    }
                }))
                .w_full()
                .h(rpx(HEAD_H))
                .justify_start()
                .gap(rpx(SPACE_SM))
                .px(rpx(SPACE_LG))
                .when(collapsed, |row| row.px_0().justify_center())
                .text_size(rpx(TEXT_TITLE))
                .text_color(c::FG_DIM())
                .font_weight(gpui::FontWeight::NORMAL)
                .aria_selected(selected)
                .when(selected, |row| row.bg(c::BG_HOVER()))
                .child(crate::icons::action_icon(glyph, ICON_MD))
                .when(!collapsed, |row| {
                    row.child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_MUTE())
                                .child(if cfg!(target_os = "macos") {
                                    format!("⌘{}", index + 1)
                                } else {
                                    format!("Ctrl+Shift+{}", index + 1)
                                }),
                        )
                }),
            );
        }
        let grouping = SessionGrouping::current(&cx.global::<SettingsState>().store);
        let mut heading = div()
            .px(rpx(SIDEBAR_CONTENT_INSET))
            .h(rpx(HEAD_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM));
        if !collapsed {
            heading = heading.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .truncate()
                    .child("Sessions"),
            );
            let mut group = super::super::components::preference_choices();
            for (choice, label, glyph, key) in [
                (SessionGrouping::Activity, "Activity", "list", "false"),
                (
                    SessionGrouping::Worktrees,
                    "Worktree",
                    "git-branch",
                    "worktrees",
                ),
            ] {
                group = group.child(
                    self.control(
                        SharedString::from(format!("session-group-{key}")),
                        format!("Group sessions by {label}"),
                        Action::GroupSessions(choice),
                        cx,
                    )
                    .debug_selector(move || format!("session-group-{key}"))
                    .size(rpx(STEPPER_BTN_W))
                    .justify_center()
                    .aria_selected(choice == grouping)
                    .when(choice == grouping, |row| row.bg(c::BG_HOVER()))
                    .child(icon(glyph, ICON_SM, c::FG_DIM())),
                );
            }
            heading = heading.child(group);
        }
        let mut sessions = div()
            .id("sidebar-scroll")
            .debug_selector(|| "sidebar-scroll".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(rpx(SPACE_LG));
        let groups = self.sidebar_session_groups(cx);
        let mut count = 0;
        for (label, ids) in groups {
            if ids.is_empty() {
                continue;
            }
            if !collapsed {
                sessions = sessions.child(
                    div()
                        .px(rpx(SPACE_LG))
                        .pt(rpx(SPACE_2XL))
                        .pb(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(ids.len().to_string()),
                );
            }
            for id in ids {
                if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(id).cloned() {
                    count += 1;
                    sessions = sessions.child(if collapsed {
                        self.single_session_row(&meta, true, window, cx)
                    } else {
                        self.session_row(&meta, true, None, false, window, cx)
                    });
                }
            }
        }
        if count == 0 && !collapsed {
            sessions = sessions.child(
                div()
                    .p(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("No active sessions"),
            );
        }
        let mut terminals = div()
            .id("sidebar-terminals")
            .debug_selector(|| "sidebar-terminals".into())
            .flex_shrink_0()
            .max_h(gpui::relative(0.3))
            .overflow_y_scroll()
            .px(rpx(SPACE_LG))
            .pb(rpx(SPACE_LG));
        terminals = terminals.child(
            div()
                .h(rpx(HEAD_H))
                .flex()
                .items_center()
                .px(rpx(SPACE_LG))
                .when(!collapsed, |row| {
                    row.child(
                        div()
                            .flex_1()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Terminals"),
                    )
                })
                .child(
                    self.control("add-terminal", "New terminal", Action::AddTerminal, cx)
                        .debug_selector(|| "add-terminal".into())
                        .child(icon("plus", ICON_SM, c::FG_DIM())),
                ),
        );
        let rows = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .home_terminals()
            .to_vec();
        for meta in rows {
            if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) == self.active_workspace {
                terminals = terminals.child(self.single_session_row(&meta, collapsed, window, cx));
            }
        }
        let terminal_section = if collapsed {
            terminals.into_any_element()
        } else {
            div()
                .id("single-terminals-scroll")
                .max_h(gpui::relative(0.3))
                .overflow_y_scroll()
                .child(self.terminals(window, cx))
                .into_any_element()
        };
        div()
            .id("global-navigation")
            .debug_selector(|| "global-navigation".into())
            .relative()
            .w(rpx(width))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(c::BG_RAIL())
            .child(
                gpui::canvas(
                    {
                        let bounds = self.rail_bounds.clone();
                        move |rect, _, _| bounds.set(rect)
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(nav)
            .child(heading)
            .child(sessions)
            .child(terminal_section)
            .when(!collapsed, |rail| {
                rail.child(
                    div()
                        .id("sidebar-divider")
                        .debug_selector(|| "sidebar-divider".into())
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(rpx(DIVIDER_DRAG_HIT_W))
                        .h_full()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event, window, cx| {
                                this.divider_press(event, window, cx);
                            }),
                        ),
                )
            })
            .into_any_element()
    }
    pub(super) fn sidebar_session_groups(&self, cx: &App) -> Vec<(String, Vec<SessionId>)> {
        match SessionGrouping::current(&cx.global::<SettingsState>().store) {
            SessionGrouping::Activity => self
                .list_session_groups(cx)
                .into_iter()
                .map(|(name, ids)| (name.to_owned(), ids))
                .collect(),
            SessionGrouping::Worktrees => {
                let registry = self.runtime.read(cx).registry.read(cx);
                let mut groups = Vec::<((String, String), String, Vec<SessionId>)>::new();
                let mut seen = HashSet::new();
                for project in &self.snapshot.projects {
                    for id in &project.sessions {
                        if !seen.insert(*id) {
                            continue;
                        }
                        let Some(meta) = registry.meta(*id) else {
                            continue;
                        };
                        // Project and full checkout path own identity. Equal branch names
                        // in distinct repositories or checkouts must never merge.
                        let key = (meta.project.clone(), meta.wt_path.clone());
                        if let Some((_, _, ids)) =
                            groups.iter_mut().find(|(identity, _, _)| *identity == key)
                        {
                            ids.push(*id);
                            continue;
                        }
                        let name = self
                            .snapshot
                            .projects
                            .iter()
                            .find(|owner| owner.name == meta.project)
                            .and_then(|owner| {
                                owner.worktrees.iter().find(|wt| wt.path == meta.wt_path)
                            })
                            .map_or_else(
                                || {
                                    std::path::Path::new(&meta.wt_path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                },
                                |wt| wt.name.clone(),
                            );
                        groups.push((key, format!("{} / {}", meta.project, name), vec![*id]));
                    }
                }
                groups
                    .into_iter()
                    .map(|(_, label, ids)| (label, ids))
                    .collect()
            }
        }
    }
    fn single_session_row(
        &self,
        meta: &SessionMeta,
        collapsed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = meta.agent == Agent::Terminal
            && self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .home_terminals()
                .iter()
                .any(|m| m.id == meta.id);
        let native_title = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(meta.id)
            .and_then(|s| s.read(cx).state.saved.title.clone());
        let live_title = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .session(meta.id)
            .and_then(|session| session.read(cx).title());
        let title = session_display_title(meta, native_title.or(live_title));
        let status = self.status(meta, cx).0;
        let label = format!(
            "{title} · {} / {} · {} · {status}",
            meta.project,
            meta.wt_path,
            meta.agent.label()
        );
        let selected = self.selection
            == Some(if home {
                Selection::Home(meta.id)
            } else {
                Selection::Session(meta.id)
            })
            && !self.dashboard_open;
        let mut row = self
            .control(
                SharedString::from(format!("single-session-{}", meta.id.raw())),
                label,
                Action::Select(if home {
                    Selection::Home(meta.id)
                } else {
                    Selection::Session(meta.id)
                }),
                cx,
            )
            .debug_selector({
                let id = meta.id;
                move || format!("single-session-{}", id.raw())
            })
            .w_full()
            .min_h(rpx(if collapsed { HEAD_H } else { SESSION_ROW_H }))
            .justify_start()
            .px(rpx(SPACE_LG))
            .gap(rpx(SPACE_LG))
            .aria_selected(selected)
            .when(selected, |row| row.bg(c::BG_HOVER()))
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(icon(meta.agent.icon_name(), ICON_MD, c::FG()))
                    .when(!home && collapsed, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .right(rpx(-SPACE_SM))
                                .bottom(rpx(-SPACE_SM))
                                .child(session_status_icon_sized(meta.id, status, ICON_XS, cx)),
                        )
                    }),
            );
        if !collapsed {
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .child(title.clone()),
                    )
                    .when(!home, |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(format!(
                                    "{} / {}",
                                    meta.project,
                                    std::path::Path::new(&meta.wt_path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                )),
                        )
                    }),
            );
        }
        if !home && !collapsed {
            let id = meta.id;
            let anchor = self.session_close_bounds.get(&id).cloned();
            row = row.group("single-session").child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .size(rpx(CHROME_CONTROL_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .group_hover("single-session", |s| s.opacity(0.))
                            .child(session_status_icon(id, status, cx)),
                    )
                    .child(
                        self.control(
                            ("single-close", id.raw()),
                            format!("Close {title}"),
                            Action::Close(id),
                            cx,
                        )
                        .absolute()
                        .inset_0()
                        .opacity(0.)
                        .group_hover("single-session", |s| s.opacity(1.))
                        .focus(|s| s.opacity(1.))
                        .child(icon("close", ICON_SM, c::FG_DIM()))
                        .when_some(anchor, |d, anchor| {
                            d.child(
                                gpui::canvas(
                                    move |bounds, _, _| anchor.set(bounds),
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                        }),
                    ),
            );
        }
        let mut result = div().child(row);
        if self.pending_close == Some(meta.id) && self.canvas_close_anchor.is_none() {
            result = result.child(gpui::deferred(self.confirmation_popup(
                &format!(
                    "Close {} in {}? Its process will stop. The worktree stays on disk.",
                    meta.label, meta.project
                ),
                Action::ConfirmClose(meta.id),
                self.rail_bounds.get(),
                window,
                cx,
            )));
        }
        result.into_any_element()
    }
    pub(crate) fn header_workspace_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = &cx.global::<SettingsState>().store.workspaces;
        let name = ws.name(ws.active).to_string();
        let trigger = self
            .workspace_selector
            .as_ref()
            .map(|s| s.read(cx).compact_trigger_bounds());
        let mut row = self
            .control(
                "header-workspace",
                format!("Switch workspace · {name}"),
                Action::OpenWorkspaces,
                cx,
            )
            .debug_selector(|| "header-workspace".into())
            .track_focus(&self.compact_workspace_focus)
            .w_auto()
            .justify_start()
            .max_w(rpx(SIDEBAR_W))
            .h(rpx(HEAD_H))
            .px(rpx(SPACE_LG))
            .gap(rpx(SPACE_SM))
            .when(self.is_collapsed(cx), |row| row.px_0().justify_center())
            .text_size(rpx(TEXT_TITLE))
            .text_color(c::FG_DIM())
            .child(crate::icons::action_icon("folder", ICON_MD))
            .when(!self.is_collapsed(cx), |row| {
                row.child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child("Workspace"),
                    )
            });
        if let Some(trigger) = trigger {
            row = row.child(
                gpui::canvas(move |bounds, _, _| trigger.set(bounds), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            );
        }
        div()
            .relative()
            .w_auto()
            .min_w_0()
            .max_w(rpx(SIDEBAR_W))
            .child(row.w_full().min_w_0())
            .when(self.runtime.read(cx).upgrade.read(cx).can_update(), |d| {
                d.child(
                    self.control("sidebar-update", "Update Grove", Action::UpdateApp, cx)
                        .debug_selector(|| "sidebar-update".into())
                        .when(self.is_collapsed(cx), |button| {
                            button.w_full().justify_center()
                        })
                        .child(icon("arrow-up", ICON_SM, c::FG_DIM())),
                )
            })
            .when_some(self.workspace_selector.clone(), gpui::ParentElement::child)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn init(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Example".into(),
                path: "/grove-single-sidebar-example".into(),
                scripts: Default::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn single_sidebar_destinations_grouping_and_collapsed_geometry(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(800.)));
        draw(cx);
        assert!(cx.debug_bounds("global-navigation").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("global-nav-home").is_some());
        assert!(cx.debug_bounds("global-nav-history").is_some());
        let projects = cx.debug_bounds("global-nav-projects").unwrap();
        cx.simulate_click(projects.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("projects-destination").is_some());
        assert!(cx.debug_bounds("session-group-true").is_none());
        let group = cx.debug_bounds("session-group-false").unwrap();
        assert_eq!(f32::from(group.size.height), STEPPER_BTN_W);
        assert_eq!(f32::from(group.size.width), STEPPER_BTN_W);
        cx.simulate_click(group.center(), gpui::Modifiers::default());
        draw(cx);
        let worktrees = cx.debug_bounds("session-group-worktrees").unwrap();
        cx.simulate_click(worktrees.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert!(cx.global::<SettingsState>().store.sidebar_group_by_worktree);
            SettingsState::update(cx, |store| store.sidebar_group_by_project = true);
        });
        cx.simulate_click(group.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert!(!cx.global::<SettingsState>().store.sidebar_group_by_worktree);
            assert!(!cx.global::<SettingsState>().store.sidebar_group_by_project);
        });
        cx.simulate_click(worktrees.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                assert!(cx.global::<SettingsState>().store.sidebar_group_by_worktree);
                assert!(!cx.global::<SettingsState>().store.sidebar_group_by_project);
                let expanded = s.rail_width(window, cx);
                s.toggle_sidebar(window, cx);
                assert_eq!(s.rail_width(window, cx), SIDEBAR_COLLAPSED_W);
                assert!(s.animated_sidebar_width(window, cx) >= SIDEBAR_COLLAPSED_W);
                assert!(expanded > SIDEBAR_COLLAPSED_W);
                s.open_destination(Destination::Home, window, cx);
                assert!(s.is_collapsed(cx));
                assert!(s.dashboard_open);
            })
        });
    }
    #[gpui::test]
    fn new_session_and_home_keep_draft_and_route_focus(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let new_session = cx.debug_bounds("global-nav-new-session").unwrap();
        cx.simulate_click(new_session.center(), gpui::Modifiers::default());
        draw(cx);
        let (session, view) = cx.update(|window, cx| {
            let owner = sidebar.read(cx);
            let dashboard = owner.dashboard.as_ref().unwrap();
            assert_eq!(owner.destination, Destination::NewSession);
            assert!(dashboard.view.focus_handle(cx).is_focused(window));
            (dashboard.session.clone(), dashboard.view.clone())
        });
        cx.simulate_input("Keep my workspace draft");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).state.saved.draft,
                "Keep my workspace draft"
            )
        });
        let home = cx.debug_bounds("global-nav-home").unwrap();
        cx.simulate_click(home.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|window, cx| {
            let owner = sidebar.read(cx);
            let dashboard = owner.dashboard.as_ref().unwrap();
            assert_eq!(owner.destination, Destination::Home);
            assert_eq!(dashboard.session, session);
            assert_eq!(dashboard.view, view);
            assert!(!view.focus_handle(cx).is_focused(window));
            assert_eq!(
                session.read(cx).state.saved.draft,
                "Keep my workspace draft"
            );
        });
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        cx.simulate_click(new_session.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|window, cx| {
            assert!(view.focus_handle(cx).is_focused(window));
            assert_eq!(
                session.read(cx).state.saved.draft,
                "Keep my workspace draft"
            );
        });
    }

    #[gpui::test]
    fn worktree_groups_keep_checkout_identity_and_keyboard_order(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
                let registry = sidebar.runtime.read(cx).registry.clone();
                let ids = registry.update(cx, |registry, _| {
                    [
                        ("Alpha", "/alpha/main", Agent::Codex),
                        ("Alpha", "/alpha/review", Agent::Claude),
                        ("Alpha", "/alpha/main", Agent::Claude),
                        ("Beta", "/beta/main", Agent::Codex),
                        ("Hidden", "/hidden/main", Agent::Codex),
                    ]
                    .map(|(project, path, agent)| {
                        registry.insert_meta(project.into(), path.into(), agent)
                    })
                });
                sidebar.snapshot.projects = vec![
                    SnapshotProject {
                        name: "Alpha".into(),
                        sessions: vec![ids[0], ids[1], ids[2]],
                        worktrees: vec![
                            SnapshotWorktree {
                                path: "/alpha/main".into(),
                                name: "main".into(),
                                branch: "same-branch".into(),
                                is_main: true,
                                sessions: vec![ids[0], ids[2]],
                            },
                            SnapshotWorktree {
                                path: "/alpha/review".into(),
                                name: "review".into(),
                                branch: "same-branch".into(),
                                is_main: false,
                                sessions: vec![ids[1]],
                            },
                        ],
                        ..Default::default()
                    },
                    // A multi-root reference also appears in Beta's inventory, once only.
                    SnapshotProject {
                        name: "Beta".into(),
                        sessions: vec![ids[3], ids[0]],
                        worktrees: vec![SnapshotWorktree {
                            path: "/beta/main".into(),
                            name: "main".into(),
                            branch: "same-branch".into(),
                            is_main: true,
                            sessions: vec![ids[3]],
                        }],
                        ..Default::default()
                    },
                ];
                SettingsState::update(cx, |store| store.sidebar_group_by_worktree = true);
                assert_eq!(
                    sidebar.sidebar_session_groups(cx),
                    vec![
                        ("Alpha / main".into(), vec![ids[0], ids[2]]),
                        ("Alpha / review".into(), vec![ids[1]]),
                        ("Beta / main".into(), vec![ids[3]]),
                    ]
                );
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![ids[0], ids[2], ids[1], ids[3]]
                );
                // Worktree grouping persists; retired project-only grouping falls back to Activity.
                let saved = serde_json::to_string(&cx.global::<SettingsState>().store).unwrap();
                let restored: grove_core::storage::Store = serde_json::from_str(&saved).unwrap();
                assert_eq!(
                    SessionGrouping::current(&restored),
                    SessionGrouping::Worktrees
                );
                let legacy: grove_core::storage::Store =
                    serde_json::from_str(r#"{"sidebar_group_by_project":true}"#).unwrap();
                assert_eq!(SessionGrouping::current(&legacy), SessionGrouping::Activity);
            })
        });
    }
    #[gpui::test]
    fn single_sidebar_history_is_separate_from_open_session_status(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |r, _| {
                r.insert_meta(
                    "Example".into(),
                    "/grove-single-sidebar-example".into(),
                    Agent::Codex,
                );
                r.insert_meta(
                    "Example".into(),
                    "/grove-single-sidebar-example".into(),
                    Agent::Claude,
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                let ids = s
                    .list_session_groups(cx)
                    .into_iter()
                    .flat_map(|(_, ids)| ids)
                    .collect::<Vec<_>>();
                assert_eq!(ids.len(), 2);
                s.open_destination(Destination::History, window, cx);
                assert!(s.selection.is_none());
                assert!(!s.dashboard_open);
                assert_eq!(s.runtime.read(cx).registry.read(cx).all().len(), 2);
                s.open_destination(Destination::Home, window, cx);
                assert_eq!(
                    s.list_session_groups(cx)
                        .into_iter()
                        .map(|(_, ids)| ids.len())
                        .sum::<usize>(),
                    2
                );
            })
        });
    }
    #[test]
    fn legacy_sessions_landing_remains_distinct_from_history() {
        assert_eq!(
            Destination::from(LandingPage::Sessions),
            Destination::Sessions
        );
        assert_eq!(
            Destination::from(LandingPage::History),
            Destination::History
        );
        assert_eq!(
            LandingPage::ALL,
            [
                LandingPage::NewSession,
                LandingPage::Home,
                LandingPage::Projects,
                LandingPage::History
            ]
        );
    }
}
