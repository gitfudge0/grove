//! Read-only saved-session browsing; provider attachment requires explicit Reopen.
use super::*;
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
    pub(super) fn render_session_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = &cx.global::<SettingsState>().store;

        let records = self
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
        let cli_records = self
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
        let mut page = div()
            .id("session-history-page")
            .debug_selector(|| "session-history-page".into())
            .size_full()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(div().flex_1().text_size(rpx(TEXT_DISPLAY)).child("History"))
                .child(
                    self.control(
                        "refresh-session-history",
                        "Refresh history",
                        Action::RefreshSessionHistory,
                        cx,
                    )
                    .child(icon("reset", ICON_MD, c::FG_DIM())),
                ),
        );
        if let Some(error) = &self.session_history_error {
            page = page.child(div().text_color(c::FORM_ERROR()).child(error.clone()));
        }
        if self.session_history_loading {
            page = page.child(div().text_color(c::FG_DIM()).child("Loading history…"));
        }
        if let Some(saved) = records
            .iter()
            .find(|s| Some(&s.key) == self.session_history_selected.as_ref())
        {
            let title = saved.title.as_deref().unwrap_or(&saved.label);
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(icon(
                        if saved.provider == grove_core::codex::Provider::Claude {
                            "claude"
                        } else {
                            "codex"
                        },
                        ICON_MD,
                        c::FG(),
                    ))
                    .child(div().flex_1().min_w_0().truncate().child(format!(
                            "{} / {} / {title}",
                            saved.project,
                            std::path::Path::new(&saved.cwd)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        )))
                    .child(
                        self.control(
                            "reopen-history-session",
                            "Reopen session",
                            Action::ReopenSessionHistory(saved.key.clone()),
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("reset", ICON_SM, c::FG_DIM()))
                        .child("Reopen session"),
                    ),
            );
            page = page.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Closed session · read only"),
            );
            if let Some(view) = &self.session_history_view {
                page = page
                    .overflow_hidden()
                    .child(div().flex_1().min_h_0().child(view.clone()));
            }
        } else if let Some(saved) = cli_records
            .iter()
            .find(|s| Some(&s.key) == self.session_history_selected.as_ref())
        {
            page=page.child(div().flex().items_center().gap(rpx(SPACE_LG)).child(icon(saved.agent.icon_name(),ICON_MD,c::FG())).child(div().min_w_0().truncate().child(saved.title.clone().unwrap_or_else(||saved.label.clone()))))
                .child(div().text_color(c::FG_DIM()).child(format!("{} / {}",saved.project,saved.cwd)))
                .child(div().text_color(c::FG_DIM()).child("Closed CLI session · transcript unavailable. Start a new session from Projects to continue working."));
        } else {
            page=page.child(div().text_color(c::FG_DIM()).text_size(rpx(TEXT_SMALL)).child("Closed sessions are retained here from now on. Earlier removed sessions are unavailable; CLI records retain identity without a transcript."));
            if records.is_empty() && cli_records.is_empty() && !self.session_history_loading {
                page = page.child(
                    div()
                        .py(rpx(SPACE_3XL))
                        .text_color(c::FG_DIM())
                        .child("No historical sessions yet"),
                );
            }
            for saved in records {
                let title = saved.title.as_deref().unwrap_or(&saved.label);
                page = page.child(
                    self.control(
                        SharedString::from(format!("session-history-{}", saved.key)),
                        format!("{title} · {} · {}", saved.project, saved.cwd),
                        Action::PreviewSessionHistory(saved.key.clone()),
                        cx,
                    )
                    .w_full()
                    .h_auto()
                    .min_h(rpx(HEAD_H))
                    .justify_start()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_LG))
                    .child(icon(
                        if saved.provider == grove_core::codex::Provider::Claude {
                            "claude"
                        } else {
                            "codex"
                        },
                        ICON_MD,
                        c::FG(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(title.to_string()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(format!("{} / {}", saved.project, saved.cwd)),
                            ),
                    ),
                );
            }
            for saved in cli_records {
                let title = saved.title.as_deref().unwrap_or(&saved.label);
                page = page.child(
                    self.control(
                        SharedString::from(format!("cli-history-{}", saved.key)),
                        format!(
                            "{title} · {} · CLI session · transcript unavailable",
                            saved.project
                        ),
                        Action::PreviewSessionHistory(saved.key.clone()),
                        cx,
                    )
                    .w_full()
                    .h_auto()
                    .min_h(rpx(HEAD_H))
                    .justify_start()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_LG))
                    .child(icon(saved.agent.icon_name(), ICON_MD, c::FG()))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(title.to_string()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(format!("{} / {} · CLI", saved.project, saved.cwd)),
                            ),
                    ),
                );
            }
        }
        page.into_any_element()
    }
    pub(super) fn render_projects_destination(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut page = div()
            .id("projects-destination")
            .debug_selector(|| "projects-destination".into())
            .size_full()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .text_size(rpx(TEXT_DISPLAY))
                            .child("Projects"),
                    )
                    .child(self.view_controls_with_gap(SPACE_SM, cx))
                    .child(
                        self.control("projects-add", "Add project", Action::AddProject, cx)
                            .debug_selector(|| "projects-add".into())
                            .w_auto()
                            .px(rpx(SPACE_LG))
                            .gap(rpx(SPACE_SM))
                            .child(icon("plus", ICON_SM, c::FG_DIM()))
                            .child("Add project"),
                    ),
            );
        let projects = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .map(|(idx, p)| (idx, p.clone()))
            .collect::<Vec<_>>();
        for (idx, project) in projects {
            if matches!(self.selection,Some(Selection::Project(selected)) if selected!=idx) {
                continue;
            }
            let count = self
                .snapshot
                .projects
                .iter()
                .find(|p| p.idx == idx)
                .map_or(0, |p| p.sessions.len());
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_2XL))
                    .border_1()
                    .border_color(c::BORDER_SOFT())
                    .rounded(rpx(RADIUS_PANEL))
                    .child(icon("folder", ICON_MD, c::FG_DIM()))
                    .child(
                        self.control(
                            SharedString::from(format!("projects-open-{idx}")),
                            format!("Open project {}", project.name),
                            Action::Select(Selection::Project(idx)),
                            cx,
                        )
                        .debug_selector(move || format!("projects-open-{idx}"))
                        .flex_1()
                        .min_w_0()
                        .h_auto()
                        .justify_start()
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(project.name.clone()))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(format!(
                                            "{} · {count} active sessions",
                                            project.path
                                        )),
                                ),
                        ),
                    )
                    .child(
                        self.control(
                            SharedString::from(format!("projects-manage-{idx}")),
                            format!("Manage {}", project.name),
                            Action::EditProject(project.path.clone()),
                            cx,
                        )
                        .debug_selector(move || format!("projects-manage-{idx}"))
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("cog", ICON_SM, c::FG_DIM()))
                        .child("Manage"),
                    ),
            );
        }
        if let Some(Selection::Project(idx)) = self.selection {
            page = page.child(self.tree_for_project(idx, window, cx));
        }
        page.into_any_element()
    }
}
