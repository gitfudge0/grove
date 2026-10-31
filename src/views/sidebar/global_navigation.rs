//! App destinations and the single workspace-scoped sidebar.
use super::*;
use grove_core::storage::LandingPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Home,
    Projects,
    History,
    Sessions,
    Terminals,
}
impl From<LandingPage> for Destination {
    fn from(page: LandingPage) -> Self {
        match page {
            LandingPage::Home => Self::Home,
            LandingPage::Projects => Self::Projects,
            LandingPage::History => Self::History,
            LandingPage::Sessions => Self::Sessions,
        }
    }
}
impl Sidebar {
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
        if destination == Destination::Home {
            self.open_dashboard(window, cx);
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
    // Kept for contextual overlays that need to know whether full navigation fits.
    pub(super) fn context_visible(&self, _window: &Window, cx: &App) -> bool {
        !self.is_collapsed(cx)
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
        self.compact_rail = !self.context_visible(window, cx)
            || self.context_width(window, cx) < SIDEBAR_COMPACT_THRESHOLD;
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
            .gap(rpx(SPACE_XS))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_2XL));
        let settings = self
            .settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_open());
        for (index, (destination, label, glyph)) in [
            (Destination::Home, "Home", "home"),
            (Destination::Projects, "Projects", "folder"),
            (Destination::History, "History", "history"),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = !settings
                && self.destination == destination
                && (self.selection.is_none() || destination == Destination::Home);
            let id = format!("global-nav-{}", label.to_lowercase());
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
                    this.navigation_focus[index].focus(window, cx);
                }))
                .w_full()
                .h(rpx(HEAD_H))
                .justify_start()
                .gap(rpx(SPACE_LG))
                .px(rpx(SPACE_LG))
                .aria_selected(selected)
                .when(selected, |row| row.bg(c::BG_HOVER()))
                .child(icon(glyph, ICON_MD, c::FG_DIM()))
                .when(!collapsed, |row| row.child(label)),
            );
        }
        let by_project = cx.global::<SettingsState>().store.sidebar_group_by_project;
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
            for (projects, label) in [(false, "Activity"), (true, "Projects")] {
                group = group.child(
                    self.control(
                        SharedString::from(format!("session-group-{projects}")),
                        format!("Group sessions by {label}"),
                        Action::GroupSessions(projects),
                        cx,
                    )
                    .debug_selector(move || format!("session-group-{projects}"))
                    .w_auto()
                    .h(rpx(CONTROL_H))
                    .px(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_MICRO))
                    .aria_selected(projects == by_project)
                    .when(projects == by_project, |row| row.bg(c::BG_HOVER()))
                    .when(self.compact_rail, |row| {
                        row.child(icon(
                            if projects { "folder" } else { "list" },
                            ICON_SM,
                            c::FG_DIM(),
                        ))
                    })
                    .when(!self.compact_rail, |row| row.child(label)),
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
        let groups: Vec<(String, Vec<SessionId>)> = if by_project {
            self.snapshot
                .projects
                .iter()
                .map(|p| (p.name.clone(), p.sessions.clone()))
                .collect()
        } else {
            self.list_session_groups(cx)
                .into_iter()
                .map(|(name, ids)| (name.to_string(), ids))
                .collect()
        };
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
                        .child(format!("{label} · {}", ids.len())),
                );
            }
            for id in ids {
                if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(id).cloned() {
                    count += 1;
                    sessions =
                        sessions.child(self.single_session_row(&meta, collapsed, window, cx));
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
            .px_0()
            .gap(rpx(SPACE_LG))
            .child(icon("folder", ICON_MD, c::FG_DIM()))
            .child(div().min_w_0().truncate().child(name))
            .child(icon("chev-down", ICON_SM, c::FG_DIM()));
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
        let group = cx.debug_bounds("session-group-true").unwrap();
        cx.simulate_click(group.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                assert!(cx.global::<SettingsState>().store.sidebar_group_by_project);
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
                LandingPage::Home,
                LandingPage::Projects,
                LandingPage::History
            ]
        );
    }
}
