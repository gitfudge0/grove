//! Checkout launch, project navigation entry points and script actions.

#[cfg(test)]
use super::SessionId;
use super::{
    checkout_panel, Action, Agent, Selection, SettingsState, Sidebar, ViewMode,
    WorktreeRemovalStage, WORKTREE_LAUNCH_AGENTS,
};
use gpui::{App, Context, Focusable, Window};

impl Sidebar {
    pub(super) fn launch_target(
        &self,
        project: usize,
        path: String,
        agent: Agent,
        cx: &App,
    ) -> Option<(String, String, Agent)> {
        if !WORKTREE_LAUNCH_AGENTS
            .iter()
            .zip(self.available)
            .any(|(candidate, available)| *candidate == agent && available)
        {
            return None;
        }
        let name = self.launch_project_name(project, &path, cx)?;
        Some((name, path, agent))
    }

    pub(super) fn launch_project_name(
        &self,
        project: usize,
        path: &str,
        cx: &App,
    ) -> Option<String> {
        self.snapshot
            .projects
            .iter()
            .find(|p| p.idx == project)
            .filter(|p| {
                p.worktrees.iter().any(|worktree| worktree.path == path)
                    || cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .get(project)
                        .is_some_and(|stored| {
                            stored.path == path
                                && !stored.archived
                                && cx
                                    .global::<SettingsState>()
                                    .store
                                    .project_workspace_id(path)
                                    == cx.global::<SettingsState>().store.workspaces.active
                        })
            })
            .map(|p| p.name.clone())
    }

    pub(super) fn launch(
        &mut self,
        project: usize,
        path: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !WORKTREE_LAUNCH_AGENTS
            .iter()
            .zip(self.available)
            .any(|(candidate, available)| *candidate == agent && available)
        {
            self.show_launch_error(format!(
                "{} CLI not found. Install it or refresh the shell environment in Settings, then retry. Check that its executable is on your shell PATH.",
                match agent {
                    Agent::Codex => "Codex",
                    Agent::Claude => "Claude Code",
                    Agent::OpenCode => "OpenCode",
                    Agent::Terminal => "Terminal",
                }
            ), cx);
            return false;
        }
        let Some((name, path, agent)) = self.launch_target(project, path, agent, cx) else {
            self.show_launch_error(
                "This checkout is no longer available in this workspace. Select a current checkout and retry.".into(),
                cx,
            );
            return false;
        };
        if !self
            .runtime
            .update(cx, |r, cx| r.spawn_session_in(name, path, agent, cx))
        {
            return false;
        }
        if let Some(id) = self.runtime.read(cx).state.read(cx).active_session() {
            self.select(Selection::Session(id), cx);
        }
        self.content_error = None;
        cx.notify();
        true
    }

    pub(super) fn show_launch_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.content_error = Some(error.clone());
        if self.project_flyout.is_some() {
            self.project_flyout_launch_error = Some(error.clone());
        }
        self.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
            toast.set_error(error, cx);
        });
        cx.notify();
    }

    pub(crate) fn open_project_checkout_from_settings(
        &mut self,
        root: &str,
        path: &str,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(_, project)| project.path == root)
            .map(|(index, _)| index);
        if let Some(index) = index {
            if self.settings_navigation_blocked(cx) {
                return;
            }
            if self.open_dashboard_for_checkout(index, root, path, window, cx) && create {
                self.begin_worktree(index, window, cx);
            }
        }
    }

    pub(crate) fn manage_project_checkouts(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(_, project)| project.path == path)
            .map(|(index, _)| index);
        if let Some(index) = index {
            if !self.close_settings_for_session_navigation(window, cx) {
                return;
            }
            self.select(Selection::Project(index), cx);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn add_project_from_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() {
            self.act(Action::AddProject, window, cx);
        }
    }

    pub(crate) fn palette_has_run_script(&self, cx: &App) -> bool {
        self.selected_worktree().is_some_and(|(idx, _)| {
            cx.global::<SettingsState>()
                .store
                .projects
                .get(idx)
                .is_some_and(|project| {
                    !project.archived
                        && project.scripts.effective_entries().iter().any(|entry| {
                            matches!(
                                entry.kind,
                                grove_core::storage::ProjectActionKind::Shell { .. }
                            )
                        })
                })
        })
    }

    pub(crate) fn palette_has_diff(&self, cx: &App) -> bool {
        matches!(self.selection, Some(Selection::Session(id)) if self.runtime.read(cx).registry.read(cx).meta(id).is_some())
    }

    pub(crate) fn run_selected_script_from_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() || !self.palette_has_run_script(cx) {
            return;
        }
        if let Some((idx, path)) = self.selected_worktree() {
            if let Some(project_path) = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(idx)
                .map(|project| project.path.clone())
            {
                self.act(Action::RunScript(project_path, path), window, cx);
            }
        }
    }

    pub(crate) fn open_selected_diff_from_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigation_available() && self.palette_has_diff(cx) {
            if let Some(Selection::Session(id)) = self.selection {
                self.act(Action::OpenDiff(id), window, cx);
            }
        }
    }

    pub(crate) fn add_worktree_terminal_from_palette(&mut self, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        let targets = self.checkout_targets(cx);
        if let Some((_, path)) =
            checkout_panel::selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref())
        {
            self.open_worktree_terminal(&path, cx);
        }
    }

    #[cfg(test)]
    pub(crate) fn open_archived_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() {
            self.act(Action::ArchivedProjects, window, cx);
        }
    }

    /// Resolve a launch target only inside the active workspace snapshot.
    pub(crate) fn selected_worktree(&self) -> Option<(usize, String)> {
        match self.selection.as_ref()? {
            Selection::Worktree(project, path) => self
                .snapshot
                .projects
                .iter()
                .find(|row| row.idx == *project)
                .and_then(|row| row.worktrees.iter().find(|worktree| &worktree.path == path))
                .map(|_| (*project, path.clone())),
            Selection::Session(id) => self.snapshot.projects.iter().find_map(|project| {
                project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.sessions.contains(id))
                    .map(|worktree| (project.idx, worktree.path.clone()))
            }),
            Selection::Project(_) | Selection::Home(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_session(&self) -> Option<SessionId> {
        match self.selection {
            Some(Selection::Session(id)) => Some(id),
            _ => None,
        }
    }

    pub(super) fn choose_project_script(
        &mut self,
        project_path: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};
        let entries = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|p| p.path == project_path)
            .map(|p| p.scripts.effective_entries())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| matches!(e.kind, grove_core::storage::ProjectActionKind::Shell { .. }))
            .collect::<Vec<_>>();
        if entries.len() <= 1 {
            self.run_script(project_path, path, cx);
            return;
        }
        let focus = window.focused(cx);
        let owner = cx.entity().downgrade();
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            popup = crate::views::components::dropdown_menu(popup, window);
            for entry in entries {
                let owner = owner.clone();
                let project_path = project_path.clone();
                let path = path.clone();
                popup = popup.item(PopupMenuItem::new(entry.name).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.act(
                                Action::RunNamedScript(
                                    project_path.clone(),
                                    path.clone(),
                                    entry.id.clone(),
                                ),
                                window,
                                cx,
                            );
                        });
                    },
                ));
            }
            popup
        });
        self.checkout_panel.popup_position = Some(window.mouse_position());
        self.checkout_panel.popup_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &gpui::DismissEvent, window, cx| {
                this.checkout_panel.popup = None;
                this.checkout_panel.popup_subscription = None;
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.checkout_panel.popup = Some(popup);
        cx.notify();
    }

    fn run_script(&mut self, project_path: String, path: String, cx: &mut Context<Self>) {
        let valid_target = self.snapshot.projects.iter().any(|project| {
            cx.global::<SettingsState>()
                .store
                .projects
                .get(project.idx)
                .is_some_and(|stored| stored.path == project_path)
                && project
                    .worktrees
                    .iter()
                    .any(|worktree| worktree.path == path)
        });
        if !valid_target {
            return;
        }
        let toast = self.runtime.read(cx).toast.clone();
        let previous_error = toast.read(cx).current().map(|message| message.created);
        if !self.runtime.update(cx, |runtime, cx| {
            runtime.spawn_run_script(&project_path, &path, cx)
        }) {
            if let Some(message) = toast.read(cx).current().filter(|message| {
                message.kind == crate::entities::toast::ToastKind::Error
                    && Some(message.created) != previous_error
            }) {
                self.content_error = Some(message.message.clone());
                cx.notify();
            }
            return;
        }
        if self.mode == ViewMode::Grid {
            self.mode = self.last_mode;
        }
        self.select_active_session(cx);
        self.content_error = None;
        cx.notify();
    }

    pub(super) fn worktree_removal_changed(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(removal) = &self.pending_worktree_removal else {
            return;
        };
        if removal.path != path {
            return;
        }
        let project_path = removal.project_path.clone();
        let finished_ok = self
            .runtime
            .read(cx)
            .projects
            .read(cx)
            .worktree_removal_status(path)
            .is_some_and(|status| {
                status.stage == WorktreeRemovalStage::Finished && status.error.is_none()
            });
        if finished_ok {
            if self.project_path_is_active(&project_path, cx) {
                self.select_project_path(&project_path, cx);
            }
            self.worktree_removal_return_focus = None;
        }
        cx.notify();
    }
}
