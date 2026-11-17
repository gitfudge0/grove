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
                grove_core::codex::load_session_history_metadata(),
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
                this.schedule_history_search(cx);
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
        self.hydrate_history_session(key, true, window, cx);
    }

    pub(super) fn hydrate_history_session(
        &mut self,
        key: &str,
        reopen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(metadata) = self.session_history.iter().find(|s| s.key == key).cloned() else {
            return;
        };
        self.history_load_generation = self.history_load_generation.wrapping_add(1);
        let generation = self.history_load_generation;
        let workspace = self.active_workspace;
        let key = key.to_owned();
        self.session_history_selected = Some(HistorySelection::Native(key.clone()));
        self.session_history_view = None;
        let load = cx.background_spawn(async move {
            // Inline transcripts also support in-memory recovered records and test fixtures.
            if !metadata.items.is_empty() {
                Ok(metadata)
            } else {
                grove_core::codex::load_history_session(&metadata.key)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.history_load_generation != generation || this.active_workspace != workspace
                    || !this.session_history.iter().any(|saved| saved.key == key)
                    || !matches!(&this.session_history_selected, Some(HistorySelection::Native(selected)) if selected == &key) { return; }
                match result {
                    Ok(saved) if reopen => {
                        match this.runtime.update(cx, |runtime, cx| runtime.reopen_session_history(saved, cx)) {
                            Ok(id) => {
                                this.session_history.retain(|s| s.key != key);
                                this.session_history_selected = None;
                                this.navigate_to_session(id, window, cx);
                            }
                            Err(error) => this.session_history_error = Some(error),
                        }
                    }
                    Ok(saved) => {
                        let session = cx.new(|cx| crate::entities::codex_session::CodexSession::history(saved, cx));
                        this.session_history_view = Some(cx.new(|cx| super::super::codex_view::CodexView::history(session, window, cx)));
                    }
                    Err(error) => this.session_history_error = Some(error),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    pub(super) fn schedule_history_search(&mut self, cx: &mut Context<Self>) {
        let query = self.history_search.read(cx).value().trim().to_lowercase();
        self.history_search_query.clone_from(&query);
        self.history_search_results.clear();
        self.history_search_task = None;
        if query.is_empty() {
            return;
        }
        let workspace = self.active_workspace;
        let store = &cx.global::<SettingsState>().store;
        let sessions = self
            .session_history
            .iter()
            .filter(|saved| history_in_scope(store, workspace, &saved.project, &saved.cwd, None))
            .cloned()
            .collect::<Vec<_>>();
        self.history_search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let search_query = query.clone();
            let matches = cx
                .background_spawn(async move {
                    let mut matches = HashMap::new();
                    for metadata in sessions {
                        let saved = if metadata.items.is_empty() {
                            grove_core::codex::load_history_session(&metadata.key).ok()
                        } else {
                            Some(metadata)
                        };
                        if let Some(saved) = saved {
                            if let Some(excerpt) = transcript_match(&saved, &search_query) {
                                matches.insert(saved.key, excerpt);
                            }
                        }
                    }
                    matches
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.active_workspace == workspace && this.history_search_query == query {
                    this.history_search_results = matches;
                    cx.notify();
                }
            });
        }));
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
        let popup = PopupMenu::build(window, cx, move |mut menu, window, _| {
            menu = crate::views::components::dropdown_menu(menu, window);
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
        self.render_closed_history(window, cx, false)
    }

    pub(super) fn render_history_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_closed_history(window, cx, true)
    }

    fn render_closed_history(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        sidebar: bool,
    ) -> AnyElement {
        use gpui_component::{input::Input, Sizable};
        let store = &cx.global::<SettingsState>().store;
        let query = self.history_search.read(cx).value().trim().to_lowercase();
        let in_scope = |project: &str, cwd: &str, apply_filter: bool| {
            history_in_scope(
                store,
                self.active_workspace,
                project,
                cwd,
                if apply_filter {
                    self.history_project_filter.as_deref()
                } else {
                    None
                },
            )
        };
        let matches = |title: &str, project: &str, cwd: &str, agent: Agent| {
            in_scope(project, cwd, true)
                && self
                    .history_provider_filter
                    .is_none_or(|filter| filter == agent)
                && (query.is_empty() || history_matches(&query, title, project, cwd))
        };
        let all_native = self
            .session_history
            .iter()
            .filter(|saved| {
                in_scope(&saved.project, &saved.cwd, false)
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
            .filter(|saved| in_scope(&saved.project, &saved.cwd, false))
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
                ) || (!query.is_empty()
                    && self.history_search_query == query
                    && self.history_search_results.contains_key(&s.key)
                    && {
                        in_scope(&s.project, &s.cwd, true)
                            && self
                                .history_provider_filter
                                .is_none_or(|filter| filter == s.provider.agent())
                    })
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
                crate::views::components::content_header()
                    .p(rpx(SPACE_3XL))
                    .child(crate::views::components::content_header_title("History"))
                    .child(
                        self.control(
                            "refresh-session-history",
                            "Refresh history",
                            Action::RefreshSessionHistory,
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child(icon("refresh", ICON_SM, c::FG_DIM()))
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
        let mut list =
            div()
                .id("history-list")
                .debug_selector(|| "history-list".into())
                .min_w_0()
                .min_h_0()
                .flex_1()
                .flex()
                .flex_col()
                .when(true, |list| {
                    list.child(
                        div()
                            .px(rpx(SPACE_3XL))
                            .pt(rpx(SPACE_3XL))
                            .pb(rpx(SPACE_LG))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_LG))
                            .when(sidebar, |header| {
                                header.child(
                                    div()
                                        .text_size(rpx(TEXT_TITLE))
                                        .text_color(c::FG())
                                        .child("History"),
                                )
                            })
                            .child(
                                crate::views::components::search_field_well(focused)
                                    .w_full()
                                    .child(icon("search", ICON_MD, c::FG_DIM()))
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.history_search)
                                                .file_drop_overlay(
                                                    crate::views::components::file_drop_overlay(),
                                                )
                                                .small()
                                                .text_size(rpx(TEXT_TITLE))
                                                .aria_label("Search sessions and transcripts")
                                                .appearance(false)
                                                .bordered(false)
                                                .focus_bordered(false)
                                                .p_0(),
                                        ),
                                    )
                                    .when(!query.is_empty(), |field| {
                                        field.child(
                                            self.control(
                                                "history-clear-search",
                                                "Clear search",
                                                Action::ClearHistorySearch,
                                                cx,
                                            )
                                            .child(icon("close", ICON_SM, c::FG_DIM())),
                                        )
                                    }),
                            )
                            .when(!sidebar, |container| {
                                container.child(
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
                                )
                            }),
                    )
                })
                .when(true, |list| {
                    list.child(
                        div()
                            .px(rpx(if sidebar {
                                SIDEBAR_CONTENT_INSET
                            } else {
                                SPACE_3XL + SPACE_SM
                            }))
                            .py(rpx(SPACE_MD))
                            .flex()
                            .justify_between()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if query.is_empty() {
                                "Recent sessions".to_string()
                            } else {
                                let count = records.len() + cli_records.len();
                                format!("{count} {}", if count == 1 { "result" } else { "results" })
                            }),
                    )
                });
        let mut rows = div()
            .id("history-list-scroll")
            .debug_selector(|| "history-list-scroll".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(rpx(if sidebar { SPACE_LG } else { SPACE_3XL }))
            .flex()
            .flex_col();
        if records.is_empty() && cli_records.is_empty() && !self.session_history_loading {
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
        for s in &records {
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
                .min_h(rpx(ROW_H))
                .text_size(rpx(TEXT_TITLE))
                .text_color(c::FG())
                .justify_start()
                .px(rpx(SPACE_LG))
                .py(rpx(SPACE_SM))
                .gap(rpx(SPACE_LG))

                .when(
                    matches!(&self.session_history_selected, Some(HistorySelection::Native(key)) if key == &s.key),
                    |d| d.bg(c::BG_HOVER()),
                )
                .child(history_card_content(title, &query, self.history_search_results.get(&s.key).filter(|_| self.history_search_query == query).cloned()));
            rows = rows.child(row);
        }
        for s in &cli_records {
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
                .min_h(rpx(ROW_H))
                .text_size(rpx(TEXT_TITLE))
                .text_color(c::FG())
                .justify_start()
                .px(rpx(SPACE_LG))
                .py(rpx(SPACE_SM))
                .gap(rpx(SPACE_LG))

                .when(
                    matches!(&self.session_history_selected, Some(HistorySelection::Cli(key)) if key == &s.key),
                    |d| d.bg(c::BG_HOVER()),
                )
                .child(history_card_content(title, &query, None));
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
                cx.global::<SettingsState>()
                    .store
                    .project_appearance(&s.cwd)
                    .current(),
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
                .child(icon("restore", ICON_SM, c::FG_DIM()))
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
                cx.global::<SettingsState>()
                    .store
                    .project_appearance(&s.cwd)
                    .current(),
            );
            detail = detail.child(history_header_container(header))
                .child(div().px(rpx(SPACE_20)).py(rpx(SPACE_3XL)).flex().flex_col().items_center().child(
                    div().w_full().max_w(rpx(super::super::codex_view::CHAT_MAX_W))
                        .child(div().child("Transcript unavailable"))
                        .child(div().text_color(c::FG_DIM()).child("This CLI session retains its identity and checkout, but its terminal transcript was not saved. Start a new session from New session to continue working.")),
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
    /// Contextual checkout utility. Project administration lives in Settings.
    pub(super) fn render_checkout_management(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(index)
            .cloned()
        else {
            return div()
                .child("Project is no longer available.")
                .into_any_element();
        };
        let mut page = div()
            .w_full()
            .max_w(rpx(super::super::codex_view::CHAT_MAX_W))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        crate::views::components::back_action(
                            "checkouts-back",
                            "Back to New session",
                            window,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_destination(
                                super::global_navigation::Destination::Home,
                                window,
                                cx,
                            );
                        })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(rpx(TEXT_TITLE))
                            .child(format!("{} · Checkouts", project.name)),
                    )
                    .child(
                        self.control(
                            "checkouts-add",
                            "New worktree",
                            Action::NewWorktree(project.path.clone()),
                            cx,
                        )
                        .debug_selector(|| "checkouts-add".into())
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("plus", ICON_SM, c::FG_DIM()))
                        .child("New worktree"),
                    ),
            )
            .child(crate::views::components::readonly_value(
                "checkouts-project-path",
                project.path.clone(),
            ));
        if let Some(error) = &self.content_error {
            page = page.child(div().text_color(c::FORM_ERROR()).child(error.clone()));
        }
        if let Some(snapshot) = self.snapshot.projects.iter().find(|p| p.idx == index) {
            for (row, worktree) in snapshot.worktrees.iter().enumerate() {
                let path = worktree.path.clone();
                let name = sidebar_worktree_name(&worktree.name, worktree.is_main).to_string();
                let mut item = div()
                    .w_full()
                    .min_w_0()
                    .py(rpx(SPACE_LG))
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(div().truncate().child(name.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(path.clone()),
                            ),
                    )
                    .child(
                        self.control(
                            format!("checkouts-use-{row}"),
                            "Use checkout",
                            Action::DashboardCheckout {
                                project: index,
                                root: project.path.clone(),
                                path: path.clone(),
                            },
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("check", ICON_SM, c::FG_DIM()))
                        .child("Use"),
                    );
                if worktree.is_main {
                    item = item.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Main checkout"),
                    );
                } else {
                    item = item.child(
                        self.control(
                            format!("checkouts-delete-{row}"),
                            format!("Delete worktree {name}"),
                            Action::RemoveWorktree(index, path),
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("trash", ICON_SM, c::FG_DIM()))
                        .child("Delete"),
                    );
                }
                page = page.child(item);
            }
            if snapshot.worktrees.is_empty() {
                page = page.child(
                    div()
                        .text_color(c::FG_DIM())
                        .child("Checkouts are unavailable. Check the folder and try again."),
                );
            }
        } else {
            page = page.child(div().text_color(c::FG_DIM()).child("Loading checkouts…"));
        }
        div()
            .id("checkout-management")
            .debug_selector(|| "checkout-management".into())
            .size_full()
            .min_w_0()
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

fn history_detail_header(
    title: &str,
    project: &str,
    checkout: &str,
    symbol: grove_core::storage::identity::ProjectIdentity,
) -> Div {
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
                                .child(super::super::components::project_symbol(symbol, ICON_SM))
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

fn transcript_match(saved: &grove_core::codex::SavedSession, query: &str) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    saved
        .items
        .iter()
        .map(|item| grove_core::codex::item_text(item))
        .find(|text| text.to_lowercase().contains(query))
        .map(|text| {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let position = text.to_lowercase().find(query).unwrap_or(0);
            let start = text
                .char_indices()
                .rev()
                .filter(|(i, _)| *i < position)
                .nth(24)
                .map_or(0, |(i, _)| i);
            format!(
                "{}{}",
                if start > 0 { "…" } else { "" },
                text[start..].chars().take(240).collect::<String>()
            )
        })
}

fn history_match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    // Map case-folded bytes back to original character boundaries (e.g. İ expands).
    let mut folded = String::new();
    let mut boundaries = Vec::new();
    for (start, ch) in text.char_indices() {
        let lower = ch.to_lowercase().to_string();
        boundaries.extend(std::iter::repeat_n(
            (start, start + ch.len_utf8()),
            lower.len(),
        ));
        folded.push_str(&lower);
    }
    folded
        .match_indices(query)
        .map(|(start, matched)| boundaries[start].0..boundaries[start + matched.len() - 1].1)
        .collect()
}

fn highlighted_history_text(text: &str, query: &str, lines: usize) -> Div {
    let highlights = history_match_ranges(text, query).into_iter().map(|range| {
        (
            range,
            gpui::HighlightStyle {
                background_color: Some(c::AMBER_ROW_TINT()),
                ..Default::default()
            },
        )
    });
    div()
        .min_w_0()
        .w_full()
        .line_clamp(lines)
        .child(gpui::StyledText::new(text.to_string()).with_highlights(highlights))
}

fn history_card_content(title: &str, query: &str, excerpt: Option<String>) -> Div {
    let mut content = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(highlighted_history_text(title, query, 1));
    if let Some(excerpt) = excerpt {
        content = content.child(
            div()
                .min_w_0()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .mt(rpx(SPACE_SM))
                .line_height(rpx(SPACE_3XL))
                .child(highlighted_history_text(&excerpt, query, 2)),
        );
    }
    content
}

fn history_matches(query: &str, title: &str, project: &str, cwd: &str) -> bool {
    query.is_empty()
        || [title, project, cwd]
            .iter()
            .any(|value| value.to_lowercase().contains(query))
}

#[cfg(test)]
mod history_filter_tests {
    use super::{history_checkout_name, history_match_ranges, history_matches};
    #[test]
    fn highlights_every_match_at_original_unicode_boundaries() {
        assert_eq!(
            history_match_ranges("Highlight and highlight", "highlight"),
            vec![0..9, 14..23]
        );
        assert_eq!(
            history_match_ranges("İ highlight", "highlight"),
            vec![3..12]
        );
        assert_eq!(history_match_ranges("İ", "i"), vec![0..2]);
        assert!(history_match_ranges("anything", "").is_empty());
    }
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

fn history_in_scope(
    store: &grove_core::storage::Store,
    workspace: u64,
    project: &str,
    cwd: &str,
    project_filter: Option<&str>,
) -> bool {
    if project.is_empty() {
        return project_filter.is_none();
    }
    grove_core::session_meta::project_owner(&store.projects, cwd)
        .ok()
        .is_some_and(|owner| {
            store.project_workspace_id(&owner.path) == workspace
                && project_filter.is_none_or(|path| path == owner.path)
        })
}

#[cfg(test)]
mod projectless_history_tests {
    #[test]
    fn projectless_history_is_global_but_excluded_by_project_filter() {
        let store = grove_core::storage::Store::default();
        let workspace = store.workspaces.active;
        assert!(super::history_in_scope(
            &store,
            workspace,
            "",
            "/home/user",
            None
        ));
        assert!(!super::history_in_scope(
            &store,
            workspace,
            "",
            "/home/user",
            Some("/project")
        ));
        assert!(!super::history_in_scope(
            &store, workspace, "Missing", "/missing", None
        ));
    }
}
