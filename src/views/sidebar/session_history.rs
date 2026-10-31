//! Read-only saved-session browsing; provider attachment requires explicit Reopen.
use super::*;

#[derive(PartialEq, Eq)]
pub(super) enum HistorySelection {
    Native(String),
    Cli(String),
}
impl Sidebar {
    pub(super) fn load_session_history(&mut self, cx: &mut Context<Self>) {
        if self.session_history_loading {
            return;
        }
        self.session_history_loading = true;
        self.session_history_error = None;
        let load = cx.background_spawn(async move {
            (
                grove_core::codex::load_session_history(),
                grove_core::session_meta::load_cli_session_history(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (result, cli) = load.await;
            let _ = this.update(cx, |this, cx| {
                this.session_history_loading = false;
                match result {
                    Ok(records) => {
                        this.session_history = records.sessions;
                        if !records.warnings.is_empty() {
                            this.session_history_error = Some(records.warnings.join("\n"));
                        }
                    }
                    Err(error) => this.session_history_error = Some(error),
                }
                match cli {
                    Ok(records) => {
                        this.cli_session_history = records.sessions;
                        if !records.warnings.is_empty() { this.session_history_error=Some(records.warnings.join("\n")); }
                    },
                    Err(error) => this.session_history_error = Some(error),
                }
                for record in this.runtime.read(cx).registry.read(cx).unpersisted_cli_history() {
                    if !this.cli_session_history.iter().any(|r|r.key==record.key) {this.cli_session_history.push(record.clone());}
                }
                if !this.runtime.read(cx).registry.read(cx).unpersisted_cli_history().is_empty() {
                    this.session_history_error=Some("Some CLI history records could not be saved. They remain available in this window; Refresh retries saving them.".into());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn reopen_history_session(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = self.session_history.iter().find(|s| s.key == key).cloned() else {
            return;
        };
        let result = self
            .runtime
            .update(cx, |runtime, cx| runtime.reopen_session_history(saved, cx));
        match result {
            Ok(id) => {
                self.session_history.retain(|s| s.key != key);
                self.session_history_selected = None;
                self.session_history_view = None;
                self.navigate_to_session(id, window, cx);
            }
            Err(error) => self.session_history_error = Some(error),
        }
        cx.notify();
    }
    pub(super) fn open_history_filter(
        &mut self,
        project: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};
        let owner = cx.entity().downgrade();
        let selected_project = self.history_project_filter.clone();
        let selected_provider = self.history_provider_filter;
        let projects = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .filter(|p| {
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&p.path)
                    == self.active_workspace
            })
            .map(|p| (p.name.clone(), p.path.clone()))
            .collect::<Vec<_>>();
        let focus = window.focused(cx);
        let popup = PopupMenu::build(window, cx, move |mut menu, _, _| {
            let target = owner.clone();
            menu = menu.item(
                PopupMenuItem::new(if project {
                    "All projects"
                } else {
                    "All providers"
                })
                .checked(if project {
                    selected_project.is_none()
                } else {
                    selected_provider.is_none()
                })
                .on_click(move |_, _, cx| {
                    let _ = target.update(cx, |this, cx| {
                        if project {
                            this.history_project_filter = None;
                        } else {
                            this.history_provider_filter = None;
                        }
                        cx.notify();
                    });
                }),
            );
            if project {
                for (name, path) in projects {
                    let target = owner.clone();
                    let checked = selected_project.as_ref() == Some(&path);
                    menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(
                        move |_, _, cx| {
                            let _ = target.update(cx, |this, cx| {
                                this.history_project_filter = Some(path.clone());
                                cx.notify();
                            });
                        },
                    ));
                }
            } else {
                for (name, agent) in [
                    ("Codex", Agent::Codex),
                    ("Claude", Agent::Claude),
                    ("OpenCode", Agent::OpenCode),
                ] {
                    let target = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .checked(selected_provider == Some(agent))
                            .on_click(move |_, _, cx| {
                                let _ = target.update(cx, |this, cx| {
                                    this.history_provider_filter = Some(agent);
                                    cx.notify();
                                });
                            }),
                    );
                }
            }
            menu.scrollable(true)
        });
        self.history_filter_position = self.action_source_bounds.get().bottom_left();
        self.history_filter_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &gpui::DismissEvent, window, cx| {
                this.history_filter_popup = None;
                this.history_filter_subscription = None;
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.history_filter_popup = Some(popup);
        cx.notify();
    }

    pub(super) fn render_session_history(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_closed_history(window, cx, false, false)
    }

    pub(super) fn render_history_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        collapsed: bool,
    ) -> AnyElement {
        self.render_closed_history(window, cx, true, collapsed)
    }

    fn render_closed_history(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        sidebar: bool,
        collapsed: bool,
    ) -> AnyElement {
        use gpui_component::input::Input;
        let store = &cx.global::<SettingsState>().store;
        let query = self.history_search.read(cx).value().trim().to_lowercase();
        let matches = |title: &str, project: &str, cwd: &str, agent: Agent| {
            let owner = grove_core::session_meta::project_owner(&store.projects, cwd).ok();
            owner.is_some_and(|owner| {
                store.project_workspace_id(&owner.path) == self.active_workspace
                    && self
                        .history_project_filter
                        .as_ref()
                        .is_none_or(|path| path == &owner.path)
            }) && self
                .history_provider_filter
                .is_none_or(|filter| filter == agent)
                && history_matches(&query, title, project, cwd)
        };
        let all_native = self
            .session_history
            .iter()
            .filter(|saved| {
                grove_core::session_meta::project_owner(&store.projects, &saved.cwd)
                    .ok()
                    .is_some_and(|owner| {
                        store.project_workspace_id(&owner.path) == self.active_workspace
                    })
                    && !self
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .all()
                        .iter()
                        .any(|m| {
                            self.runtime
                                .read(cx)
                                .registry
                                .read(cx)
                                .codex(m.id)
                                .is_some_and(|session| {
                                    session.read(cx).state.saved.key == saved.key
                                })
                        })
            })
            .collect::<Vec<_>>();
        let all_cli = self
            .cli_session_history
            .iter()
            .filter(|saved| {
                grove_core::session_meta::project_owner(&store.projects, &saved.cwd)
                    .ok()
                    .is_some_and(|owner| {
                        store.project_workspace_id(&owner.path) == self.active_workspace
                    })
            })
            .collect::<Vec<_>>();
        let records = all_native
            .iter()
            .copied()
            .filter(|s| {
                matches(
                    s.title.as_deref().unwrap_or(&s.label),
                    &s.project,
                    &s.cwd,
                    s.provider.agent(),
                )
            })
            .collect::<Vec<_>>();
        let cli_records = all_cli
            .iter()
            .copied()
            .filter(|s| {
                matches(
                    s.title.as_deref().unwrap_or(&s.label),
                    &s.project,
                    &s.cwd,
                    s.agent,
                )
            })
            .collect::<Vec<_>>();
        let checkout_names: HashMap<String, String> = records
            .iter()
            .map(|s| s.cwd.as_str())
            .chain(cli_records.iter().map(|s| s.cwd.as_str()))
            .map(|cwd| {
                let owner = grove_core::session_meta::project_owner(&store.projects, cwd).ok();
                (
                    cwd.to_string(),
                    history_checkout_name(cwd, owner.map(|p| p.path.as_str())),
                )
            })
            .collect();
        let selected_native = records
            .iter()
            .find(|s| matches!(&self.session_history_selected, Some(HistorySelection::Native(key)) if key == &s.key));
        let selected_cli = cli_records
            .iter()
            .find(|s| matches!(&self.session_history_selected, Some(HistorySelection::Cli(key)) if key == &s.key));
        let project_label = self
            .history_project_filter
            .as_ref()
            .and_then(|path| store.projects.iter().find(|p| &p.path == path))
            .map_or("All projects", |p| p.name.as_str())
            .to_string();
        let mut page = div()
            .id("session-history-page")
            .debug_selector(|| "session-history-page".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col();
        if selected_native.is_none() && selected_cli.is_none() {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_3XL))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(rpx(TEXT_DISPLAY)).child("History")),
                    )
                    .child(
                        self.control(
                            "refresh-session-history",
                            "Refresh history",
                            Action::RefreshSessionHistory,
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child(icon("reset", ICON_SM, c::FG_DIM()))
                        .child("Refresh"),
                    ),
            );
        }
        if let Some(error) = &self.session_history_error {
            page = page.child(
                div()
                    .px(rpx(SPACE_3XL))
                    .text_color(c::FORM_ERROR())
                    .child(error.clone()),
            );
        }
        if self.session_history_loading {
            page = page.child(
                div()
                    .px(rpx(SPACE_3XL))
                    .text_color(c::FG_DIM())
                    .child("Loading history…"),
            );
        }
        let provider_label = self
            .history_provider_filter
            .map_or("All providers", |a| match a {
                Agent::Codex => "Codex",
                Agent::Claude => "Claude",
                Agent::OpenCode => "OpenCode",
                Agent::Terminal => "Terminal",
            });
        let focused = self
            .history_search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let mut list = div()
            .id("history-list")
            .debug_selector(|| "history-list".into())
            .min_w_0()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .when(!collapsed && !sidebar, |list| {
                list.child(
                    div()
                        .p(rpx(SPACE_2XL))
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_LG))
                        .child(
                            crate::views::components::project_field_well(focused, false)
                                .w_full()
                                .child(
                                    Input::new(&self.history_search)
                                        .aria_label("Search history")
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .p_0(),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    self.control(
                                        "history-project-filter",
                                        format!("Filter by project: {project_label}"),
                                        Action::HistoryFilterMenu(true),
                                        cx,
                                    )
                                    .w_auto()
                                    .px(rpx(SPACE_LG))
                                    .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                                    .child(project_label),
                                )
                                .child(
                                    self.control(
                                        "history-provider-filter",
                                        format!("Filter by provider: {provider_label}"),
                                        Action::HistoryFilterMenu(false),
                                        cx,
                                    )
                                    .w_auto()
                                    .px(rpx(SPACE_LG))
                                    .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                                    .child(provider_label),
                                ),
                        ),
                )
            })
            .when(!collapsed && !sidebar, |list| {
                list.child(
                    div()
                        .px(rpx(SPACE_2XL))
                        .py(rpx(SPACE_MD))
                        .flex()
                        .justify_between()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Closed sessions")
                        .child(format!("{} sessions", records.len() + cli_records.len())),
                )
            });
        let mut rows = div()
            .id("history-list-scroll")
            .debug_selector(|| "history-list-scroll".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(rpx(SPACE_LG))
            .flex()
            .flex_col();
        if records.is_empty()
            && cli_records.is_empty()
            && !self.session_history_loading
            && !collapsed
        {
            if all_native.is_empty() && all_cli.is_empty() {
                rows =
                    rows.child(div().p(rpx(SPACE_2XL)).text_color(c::FG_DIM()).child(
                        "No closed sessions yet. Earlier removed sessions are unavailable.",
                    ));
            } else {
                rows = rows
                    .child(
                        div()
                            .p(rpx(SPACE_2XL))
                            .text_color(c::FG_DIM())
                            .child("No sessions match your search or filters."),
                    )
                    .child(
                        self.control(
                            "history-clear-filters",
                            "Clear filters",
                            Action::ClearHistoryFilters,
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_2XL))
                        .justify_start()
                        .child(icon("close", ICON_SM, c::FG_DIM()))
                        .child("Clear filters"),
                    );
            }
        }
        for s in records.iter() {
            let title = s.title.as_deref().unwrap_or(&s.label);
            let selector = format!("session-history-{}", s.key);
            let row = self
                .control(
                    SharedString::from(selector.clone()),
                    format!("{title} · {} · {}", s.project, s.cwd),
                    Action::PreviewSessionHistory(s.key.clone()),
                    cx,
                )
                .debug_selector(move || selector.clone())
                .aria_selected(matches!(&self.session_history_selected, Some(HistorySelection::Native(key)) if key == &s.key))
                .w_full()
                .h_auto()
                .min_h(rpx(HEAD_H))
                .justify_start()
                .p(rpx(SPACE_2XL))
                .gap(rpx(SPACE_LG))

                .when(
                    matches!(&self.session_history_selected, Some(HistorySelection::Native(key)) if key == &s.key),
                    |d| d.bg(c::BG_HOVER()),
                )
                .when(collapsed, |row| row.child(icon(s.provider.agent().icon_name(), ICON_LG, c::FG())))
                .when(!collapsed, |row| row.child(history_card_content(
                    s.provider.agent(), &s.project, title, &checkout_names[&s.cwd], false,
                )));
            rows = rows.child(row);
        }
        for s in cli_records.iter() {
            let title = s.title.as_deref().unwrap_or(&s.label);
            let selector = format!("cli-history-{}", s.key);
            let row = self
                .control(
                    SharedString::from(selector.clone()),
                    format!("{title} · {} · {} · CLI session · transcript unavailable", s.project, s.cwd),
                    Action::PreviewCliHistory(s.key.clone()),
                    cx,
                )
                .debug_selector(move || selector.clone())
                .aria_selected(matches!(&self.session_history_selected, Some(HistorySelection::Cli(key)) if key == &s.key))
                .w_full()
                .h_auto()
                .min_h(rpx(HEAD_H))
                .justify_start()
                .p(rpx(SPACE_2XL))
                .gap(rpx(SPACE_LG))

                .when(
                    matches!(&self.session_history_selected, Some(HistorySelection::Cli(key)) if key == &s.key),
                    |d| d.bg(c::BG_HOVER()),
                )
                .when(collapsed, |row| row.child(icon(s.agent.icon_name(), ICON_LG, c::FG())))
                .when(!collapsed, |row| row.child(history_card_content(
                    s.agent, &s.project, title, &checkout_names[&s.cwd], true,
                )));
            rows = rows.child(row);
        }
        list = list.child(rows);
        if sidebar {
            return list.into_any_element();
        }
        let mut detail = div()
            .id("history-detail")
            .debug_selector(|| "history-detail".into())
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col();
        if let Some(s) = selected_native {
            let header = history_detail_header(
                s.title.as_deref().unwrap_or(&s.label),
                &s.project,
                &checkout_names[&s.cwd],
            )
            .child(
                self.control(
                    "reopen-history-session",
                    "Reopen session",
                    Action::ReopenSessionHistory(s.key.clone()),
                    cx,
                )
                .debug_selector(|| "reopen-history-session".into())
                .w_auto()
                .flex_shrink_0()
                .px(rpx(SPACE_LG))
                .gap(rpx(SPACE_SM))
                .child(icon("reset", ICON_SM, c::FG_DIM()))
                .child("Reopen session"),
            );
            detail = detail.child(history_header_container(header));
            if let Some(view) = &self.session_history_view {
                detail = detail.child(div().flex_1().min_h_0().child(view.clone()));
            }
        } else if let Some(s) = selected_cli {
            let header = history_detail_header(
                s.title.as_deref().unwrap_or(&s.label),
                &s.project,
                &checkout_names[&s.cwd],
            );
            detail = detail.child(history_header_container(header))
                .child(div().px(rpx(SPACE_20)).py(rpx(SPACE_3XL)).flex().flex_col().items_center().child(
                    div().w_full().max_w(rpx(super::super::codex_view::CHAT_MAX_W))
                        .child(div().child("Transcript unavailable"))
                        .child(div().text_color(c::FG_DIM()).child("This CLI session retains its identity and checkout, but its terminal transcript was not saved. Start a new session from Projects to continue working.")),
                ));
        } else {
            detail = detail.child(
                div()
                    .p(rpx(SPACE_2XL))
                    .text_color(c::FG_DIM())
                    .child("Select a closed session to inspect it."),
            );
        }
        page = page.child(detail.flex_1());
        if let Some(popup) = &self.history_filter_popup {
            page = page.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopLeft)
                    .position(self.history_filter_position)
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(popup.clone()),
            ));
        }
        page.into_any_element()
    }
    pub(super) fn render_projects_destination(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_component::input::Input;
        let query = self.projects_search.read(cx).value().trim().to_lowercase();
        let focused = self
            .projects_search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let projects = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .map(|(idx, p)| (idx, p.clone()))
            .collect::<Vec<_>>();
        let mut page = div()
            .debug_selector(|| "projects-content".into())
            .w_full()
            .max_w(rpx(super::super::codex_view::CHAT_MAX_W))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .text_size(rpx(TEXT_DISPLAY))
                            .child("Projects"),
                    )
                    .child(
                        self.control("projects-add", "Add project", Action::AddProject, cx)
                            .debug_selector(|| "projects-add".into())
                            .w_auto()
                            .px(rpx(SPACE_LG))
                            .gap(rpx(SPACE_SM))
                            .child(icon("plus", ICON_SM, c::FG_DIM()))
                            .child("Add project"),
                    ),
            )
            .child(
                div()
                    .text_color(c::FG_DIM())
                    .child("Manage projects and worktrees"),
            )
            .child(
                crate::views::components::project_field_well(focused, false)
                    .w_full()
                    .child(
                        Input::new(&self.projects_search)
                            .aria_label("Find a project")
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .p_0(),
                    ),
            );
        if let Some(error) = &self.content_error {
            page = page.child(div().text_color(c::RED()).child(error.clone()));
        }
        let mut shown = 0;
        for (idx, project) in &projects {
            if !query.is_empty()
                && !format!("{} {}", project.name, project.path)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            shown += 1;
            let idx = *idx;
            let expanded = self.projects_expanded.contains(&project.path);
            let snapshot = self.snapshot.projects.iter().find(|p| p.idx == idx);
            let count = snapshot.map_or(0, |p| p.worktrees.len());
            let initials = project
                .name
                .split_whitespace()
                .take(2)
                .filter_map(|word| word.chars().next())
                .flat_map(char::to_uppercase)
                .collect::<String>();
            let mut row = div()
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .size(rpx(CONTROL_H))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_1()
                                .border_color(c::BORDER_SOFT())
                                .rounded(rpx(RADIUS_CONTROL))
                                .child(initials),
                        )
                        .child(
                            self.control(
                                SharedString::from(format!("projects-open-{idx}")),
                                format!(
                                    "{} project {}",
                                    if expanded { "Collapse" } else { "Expand" },
                                    project.name
                                ),
                                Action::ToggleProjectsDestination(project.path.clone()),
                                cx,
                            )
                            .debug_selector(move || format!("projects-open-{idx}"))
                            .aria_expanded(expanded)
                            .flex_1()
                            .min_w_0()
                            .h_auto()
                            .justify_start()
                            .py(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().truncate().child(project.name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(project.path.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(if count == 0 {
                                        "Worktrees unavailable".to_string()
                                    } else {
                                        format!(
                                            "{count} {}",
                                            if count == 1 { "worktree" } else { "worktrees" }
                                        )
                                    }),
                            )
                            .child(icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            )),
                        )
                        .child(
                            self.control(
                                SharedString::from(format!("projects-manage-{idx}")),
                                format!("Manage {}", project.name),
                                Action::Menu(idx),
                                cx,
                            )
                            .debug_selector(move || format!("projects-manage-{idx}"))
                            .w_auto()
                            .px(rpx(SPACE_LG))
                            .child("Manage")
                            .when_some(
                                self.project_menu_focus.get(&idx),
                                gpui::InteractiveElement::track_focus,
                            )
                            .when_some(
                                self.project_menu_bounds.get(&idx).cloned(),
                                |button, bounds| {
                                    button.child(
                                        gpui::canvas(
                                            move |rect, _, _| bounds.set(rect),
                                            |_, (), _, _| {},
                                        )
                                        .absolute()
                                        .inset_0(),
                                    )
                                },
                            )
                            .when(self.menu == Some(idx), |button| {
                                button.child(gpui::deferred(self.project_popup(idx, window, cx)))
                            }),
                        ),
                );
            if expanded {
                let mut tree = div()
                    .w_full()
                    .min_w_0()
                    .pl(rpx(SPACE_3XL))
                    .pb(rpx(SPACE_2XL))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("WORKTREES"),
                            )
                            .child(
                                self.control(
                                    SharedString::from(format!("projects-add-worktree-{idx}")),
                                    "Add worktree",
                                    Action::NewWorktree(project.path.clone()),
                                    cx,
                                )
                                .w_auto()
                                .px(rpx(SPACE_LG))
                                .gap(rpx(SPACE_SM))
                                .child(icon("plus", ICON_SM, c::FG_DIM()))
                                .child("Add worktree"),
                            ),
                    );
                if let Some(snapshot) = snapshot {
                    if snapshot.worktrees.is_empty() {
                        tree = tree.child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("Worktrees are unavailable. Check the project folder and try again."),
                        );
                    }
                    for (worktree_index, worktree) in snapshot.worktrees.iter().enumerate() {
                        let path = worktree.path.clone();
                        let name =
                            sidebar_worktree_name(&worktree.name, worktree.is_main).to_string();
                        let mut wt = div()
                            .w_full()
                            .min_w_0()
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .py(rpx(SPACE_LG))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().truncate().child(name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(format!("{} · {}", worktree.branch, path)),
                                    ),
                            );
                        if worktree.is_main {
                            wt = wt.child(
                                div()
                                    .flex_none()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("Main checkout"),
                            );
                        } else {
                            wt = wt.child(
                                self.control(
                                    SharedString::from(format!("projects-delete-{path}")),
                                    format!("Delete worktree {name}"),
                                    Action::RemoveWorktree(idx, path.clone()),
                                    cx,
                                )
                                .debug_selector(move || {
                                    format!("projects-delete-{idx}-{worktree_index}")
                                })
                                .w_auto()
                                .px(rpx(SPACE_LG))
                                .when_some(
                                    self.worktree_delete_focus.get(&path),
                                    gpui::InteractiveElement::track_focus,
                                )
                                .child("Delete"),
                            );
                        }
                        tree = tree.child(wt);
                    }
                } else {
                    tree = tree.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(
                            "Worktrees are unavailable. Check the project folder and try again.",
                        ),
                    );
                }
                row = row.child(tree);
            }
            page = page.child(row);
        }
        if shown == 0 {
            page = page.child(div().text_color(c::FG_DIM()).child(if projects.is_empty() {
                "No projects yet. Add a project to get started."
            } else {
                "No projects match your search. Try another name or path."
            }));
        }
        div()
            .id("projects-destination")
            .debug_selector(|| "projects-destination".into())
            .size_full()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .flex()
            .justify_center()
            .child(page)
            .into_any_element()
    }
}

// The transcript uses this same horizontal inset and maximum width.
fn history_header_container(header: Div) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_20))
        .pt(rpx(SPACE_3XL))
        .flex()
        .justify_center()
        .child(header)
}

fn history_detail_header(title: &str, project: &str, checkout: &str) -> Div {
    div()
        .w_full()
        .max_w(rpx(super::super::codex_view::CHAT_MAX_W))
        .flex()
        .items_start()
        .gap(rpx(SPACE_LG))
        .pb(rpx(SPACE_2XL))
        .border_b_1()
        .border_color(c::BORDER_SOFT())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(rpx(TEXT_TITLE))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(icon("folder", ICON_SM, c::FG_DIM()))
                                .child(div().min_w_0().truncate().child(project.to_string())),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                                .child(div().min_w_0().truncate().child(checkout.to_string())),
                        ),
                ),
        )
}

fn history_checkout_name(cwd: &str, project_path: Option<&str>) -> String {
    let path = std::path::Path::new(cwd);
    if project_path.is_some_and(|project| path == std::path::Path::new(project)) {
        return sidebar_worktree_name("", true).to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

fn history_card_content(
    agent: Agent,
    project: &str,
    title: &str,
    checkout: &str,
    cli: bool,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(icon(agent.icon_name(), ICON_MD, c::FG()))
                .child(div().min_w_0().truncate().child(project.to_string()))
                .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                .child(div().min_w_0().truncate().child(checkout.to_string()))
                .when(cli, |row| row.child("CLI")),
        )
        .child(div().min_w_0().truncate().child(title.to_string()))
}

fn history_matches(query: &str, title: &str, project: &str, cwd: &str) -> bool {
    query.is_empty()
        || [title, project, cwd]
            .iter()
            .any(|value| value.to_lowercase().contains(query))
}

#[cfg(test)]
mod history_filter_tests {
    use super::{history_checkout_name, history_matches};
    #[test]
    fn uses_concise_checkout_names() {
        assert_eq!(
            history_checkout_name("/work/grove", Some("/work/grove")),
            "Main checkout"
        );
        assert_eq!(
            history_checkout_name("/worktrees/new-sidebar", Some("/work/grove")),
            "new-sidebar"
        );
    }
    #[test]
    fn matches_titles_projects_and_checkouts_case_insensitively() {
        assert!(history_matches(
            "billing",
            "Fix BILLING",
            "grove",
            "/tmp/main"
        ));
        assert!(history_matches("grove", "Fix", "Grove", "/tmp/main"));
        assert!(history_matches("main", "Fix", "Grove", "/tmp/Main"));
        assert!(!history_matches("other", "Fix", "Grove", "/tmp/Main"));
        assert!(history_matches("", "Fix", "Grove", "/tmp/Main"));
    }
}
