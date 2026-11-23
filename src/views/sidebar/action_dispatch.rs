//! Dispatch sidebar actions with the current navigation and modal gates.

#[cfg(test)]
use super::PendingWorktreeRemoval;
use super::{
    branch_history, change_mode, global_navigation, projects, session_history, Agent,
    BranchHistoryState, DiffViewerState, Instant, Runtime, Selection, SessionId, SettingsState,
    Sidebar, SidebarContext, SidebarEvent, ViewMode, WorktreeRemovalStage, SIDEBAR_COLLAPSED_W,
};
use gpui::AppContext as _;
use gpui::{Context, Focusable, Window};

#[derive(Clone)]
pub(super) enum Action {
    Destination(global_navigation::Destination),
    Select(Selection),
    #[cfg(test)]
    ToggleProject(String),
    Mode(ViewMode),
    #[cfg(test)]
    Menu(usize),
    NewWorktree(String),
    InitializeGit(String),
    RefreshGit(String),
    #[cfg(test)]
    EditProject(String),
    EditProjectScripts(String, bool),
    #[cfg(test)]
    MoveProject(String),
    AddProject,
    #[cfg(test)]
    ArchivedProjects,
    Reveal(String),
    #[cfg(test)]
    RemoveProject(String),
    #[cfg(test)]
    ConfirmRemove(usize),
    #[cfg(test)]
    RemoveWorktree(usize, String),
    ConfirmWorktreeRemoval,
    SkipWorktreeTeardown,
    DismissWorktreeRemoval,
    #[cfg(test)]
    Launch(usize, String, Agent),
    RunScript(String, String),
    RunNamedScript(String, String, String),
    Close(SessionId),
    FinishSession(SessionId),
    MarkActivityDone(SessionId),
    MarkTerminalReviewed(SessionId, Instant),
    MarkReviewed(SessionId, usize),
    ConfirmClose(SessionId),
    Retry(SessionId),
    OpenDiff(SessionId),
    OpenWorktreeDiff(String),
    OpenHistory(String),
    OpenBranchHistory(String, Option<Selection>, Option<[u8; 32]>),
    RefreshBranchPreview,
    CloseHistory,
    RefreshHistory,
    MoreHistory,
    SelectCommit(String),
    ExpandCommit,
    HistoryGraph,
    HistoryDiffMode(crate::entities::branch_history::HistoryDiffMode),
    ToggleHistoryMetadata,
    CopyHistoryCommit,
    SelectHistoryFile(String),
    CloseDiff,
    SelectDiffFile(String),
    RefreshDiff,
    AddTerminal,
    OpenSettings,
    ToggleUsage,
    UpdateApp,
    #[cfg(test)]
    NewMultiProjectSession,
    ToggleSidebar,
    ToggleGrid,
    GroupSessions(global_navigation::SessionGrouping),
    RefreshSessionHistory,
    HistoryFilterMenu(bool),
    ClearHistoryFilters,
    ClearHistorySearch,
    PreviewSessionHistory(String),
    PreviewCliHistory(String),
    ReopenSessionHistory(String),
    OpenWorkspaces,
    CloseHome(SessionId),
    ConfirmHome(SessionId),
    Cancel,
}
impl Action {
    pub(super) fn is_fixture_launch(&self) -> bool {
        #[cfg(test)]
        {
            matches!(self, Self::Launch(..))
        }
        #[cfg(not(test))]
        {
            false
        }
    }
    pub(super) fn records_source_bounds(&self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::RemoveWorktree(..)) {
            return true;
        }
        self.is_fixture_launch()
            || matches!(
                self,
                Self::RunScript(..)
                    | Self::RunNamedScript(..)
                    | Self::HistoryFilterMenu(..)
                    | Self::ToggleUsage
            )
    }

    pub(super) fn is_fixture_remove(&self) -> bool {
        #[cfg(test)]
        {
            matches!(self, Self::ConfirmRemove(_))
        }
        #[cfg(not(test))]
        {
            false
        }
    }
}

impl Sidebar {
    pub(super) fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_navigation_blocked(cx) {
            return;
        }
        if let Some(removal) = &self.pending_worktree_removal {
            let finished = removal.started
                && (removal.error.is_some()
                    || self
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .is_some_and(|status| status.stage == WorktreeRemovalStage::Finished));
            let allowed = match action {
                Action::ConfirmWorktreeRemoval => !removal.started,
                Action::SkipWorktreeTeardown => removal.started && !finished,
                Action::DismissWorktreeRemoval | Action::Cancel => !removal.started || finished,
                _ => false,
            };
            if !allowed {
                return;
            }
        }
        if (self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some())
            && !action.is_fixture_remove()
            && !matches!(
                action,
                Action::ConfirmClose(_) | Action::ConfirmHome(_) | Action::Cancel
            )
        {
            return;
        }
        if !matches!(action, Action::ToggleUsage) {
            self.dismiss_usage(false, window, cx);
        }
        let was_project_flyout = self.project_flyout.is_some();
        if !action.is_fixture_launch()
            && !matches!(
                &action,
                Action::Close(_)
                    | Action::FinishSession(_)
                    | Action::ConfirmClose(_)
                    | Action::Cancel
            )
        {
            self.dismiss_project_context(cx);
            self.track_project_flyout_pointer(window.mouse_position(), window, cx);
        }
        match action {
            Action::ToggleGrid => self.toggle_grid(window, cx),
            Action::ToggleSidebar => {
                let from = self.animated_sidebar_width(window, cx);
                let collapsed = !self.navigation_collapsed(window, cx);
                self.navigation_width_override =
                    (!collapsed).then_some(Self::logical_window_width(window));
                let to = if collapsed {
                    SIDEBAR_COLLAPSED_W
                } else {
                    self.context_width(window, cx)
                };
                self.navigation_transition =
                    (!cx.reduce_motion()).then_some((Instant::now(), from, to));
                self.drag = None;
                self.menu = None;
                self.menu_return_focus = None;
                self.menu_opened_by_hover = false;
                SettingsState::update(cx, |store| store.sidebar_collapsed = collapsed);
                self.focus.focus(window, cx);
                cx.notify();
            }
            Action::GroupSessions(grouping) => {
                SettingsState::update(cx, |store| {
                    store.sidebar_group_by_project = false;
                    store.sidebar_group_by_worktree =
                        grouping == global_navigation::SessionGrouping::Worktrees;
                });
                cx.notify();
            }
            Action::RefreshSessionHistory => {
                self.runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, cx| {
                        let _ = registry.retry_cli_history();
                        cx.notify();
                    });
                self.load_session_history(cx);
            }
            Action::HistoryFilterMenu(project) => self.open_history_filter(project, window, cx),
            Action::ClearHistorySearch => {
                self.history_search.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus_handle(cx).focus(window, cx);
                });
                cx.notify();
            }
            Action::ClearHistoryFilters => {
                self.history_project_filter = None;
                self.history_provider_filter = None;
                self.history_search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            }
            Action::PreviewSessionHistory(key) => {
                self.hydrate_history_session(&key, false, window, cx);
            }
            Action::PreviewCliHistory(key) => {
                if self.cli_session_history.iter().any(|s| s.key == key) {
                    self.session_history_selected =
                        Some(session_history::HistorySelection::Cli(key));
                    self.session_history_view = None;
                }
                cx.notify();
            }
            Action::ReopenSessionHistory(key) => self.reopen_history_session(&key, window, cx),
            Action::OpenWorkspaces => {
                if let Some(selector) = self.workspace_selector.clone() {
                    let return_focus = self.compact_workspace_focus.clone();
                    cx.defer_in(window, move |_, window, cx| {
                        selector.update(cx, |selector, cx| {
                            selector.open_compact_menu(return_focus, window, cx);
                        });
                    });
                }
                cx.notify();
            }
            Action::AddProject => self.add_project(window, cx),
            #[cfg(test)]
            Action::ToggleProject(path) => {
                if self.project_path_is_active(&path, cx) {
                    let leaving_project_panel = self.project_panel.is_some();
                    self.dismiss_project_panel_for_navigation();
                    if !self.collapsed_projects.insert(path.clone()) {
                        self.collapsed_projects.remove(&path);
                    }
                    cx.notify();
                    if leaving_project_panel && self.project_panel.is_none() {
                        self.focus.focus(window, cx);
                    }
                }
            }
            #[cfg(test)]
            Action::ArchivedProjects => {
                self.open_project_panel(projects::Page::Archived, window, cx);
            }
            #[cfg(test)]
            Action::NewMultiProjectSession => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::NewMultiProjectSession);
                }
            }
            Action::UpdateApp => {
                if self.navigation_available() {
                    let upgrade = self.runtime.read(cx).upgrade.clone();
                    if upgrade.read(cx).can_update() {
                        if let Some(settings) = &self.settings_panel {
                            settings.update(cx, |settings, cx| settings.open_updates(window, cx));
                        }
                        upgrade.update(cx, crate::entities::upgrade::Upgrade::start_update);
                    }
                }
            }
            Action::Destination(destination) => self.open_destination(destination, window, cx),
            Action::ToggleUsage => self.toggle_usage(window, cx),
            Action::OpenSettings => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::Settings);
                }
            }
            #[cfg(test)]
            Action::EditProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
            }
            Action::EditProjectScripts(path, add) => {
                if !self.project_path_is_active(&path, cx)
                    && !self
                        .checkout_targets(cx)
                        .iter()
                        .any(|(_, target)| target == &path)
                {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
                if let Some(panel) = &self.project_panel {
                    panel.update(cx, |panel, cx| panel.open_scripts(add, window, cx));
                }
            }
            #[cfg(test)]
            Action::MoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Move(path), window, cx);
            }
            Action::Select(s) => {
                let leaving_project_panel = self.project_panel.is_some();
                if let Selection::Session(id) = s {
                    self.navigate_to_session(id, window, cx);
                    return;
                }
                if matches!(s, Selection::Home(_))
                    && !self.close_settings_for_session_navigation(window, cx)
                {
                    return;
                }
                self.select(s.clone(), cx);
                let view = match s {
                    Selection::Home(id) => self.home_terminal_views.get(&id),
                    _ => None,
                };
                if let Some(view) = view {
                    view.focus_handle(cx).focus(window, cx);
                } else if leaving_project_panel || was_project_flyout {
                    self.focus.focus(window, cx);
                }
            }
            Action::Mode(mode) => {
                let leaving_project_panel = self.project_panel.is_some();
                self.dismiss_project_panel_for_navigation();
                change_mode(&mut self.mode, &mut self.last_mode, mode);
                self.destination = if mode == ViewMode::List {
                    global_navigation::Destination::Sessions
                } else {
                    global_navigation::Destination::Projects
                };
                self.dashboard_open = false;
                self.menu = None;
                if leaving_project_panel {
                    self.focus.focus(window, cx);
                }
            }
            #[cfg(test)]
            Action::Menu(i) => {
                // Masking keeps the document alive, including its native responder.
                // GPUI focus alone cannot take keys back from that responder.
                crate::platform::inline_html::NativeDocument::focus_host(window);
                if self.menu == Some(i) {
                    if self.menu_opened_by_hover {
                        self.menu_opened_by_hover = false;
                        self.menu_return_focus = self.project_menu_focus.get(&i).cloned();
                        self.menu_focus.focus(window, cx);
                    } else {
                        self.close_menu(window, cx);
                    }
                } else {
                    self.menu = Some(i);
                    self.menu_opened_by_hover = false;
                    if let Some(bounds) = self.project_menu_bounds.get(&i) {
                        self.menu_trigger_bounds = bounds.clone();
                    }
                    self.menu_index = 0;
                    self.menu_return_focus = if was_project_flyout {
                        self.project_paths
                            .get(&i)
                            .and_then(|path| self.project_toggle_focus.get(path))
                            .cloned()
                    } else {
                        self.project_menu_focus.get(&i).cloned()
                    };
                    self.menu_focus.focus(window, cx);
                }
            }
            Action::NewWorktree(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                let Some(i) = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .iter()
                    .position(|p| p.path == path)
                else {
                    return;
                };
                self.menu = None;
                if let Some(focus) = self.menu_return_focus.take() {
                    focus.focus(window, cx);
                }
                self.begin_worktree(i, window, cx);
            }
            Action::InitializeGit(path) => {
                if !self.project_path_is_active(&path, cx) || self.git_init_pending.is_some() {
                    return;
                }
                self.menu = None;
                self.git_init_pending = Some(path.clone());
                self.content_error = None;
                cx.notify();
                cx.spawn(async move |this, cx| {
                    let initialized_path = path.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            grove_core::git::init_if_needed(&path)
                                .map_err(|error| error.to_string())
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.git_init_pending = None;
                        match result {
                            Ok(()) => {
                                this.worktree_readiness.remove(&initialized_path);
                                let service = this.runtime.read(cx).projects.clone();
                                service.update(cx, |service, cx| {
                                    service.project_git_initialized(cx);
                                });
                                this.content_error = None;
                            }
                            Err(error) => {
                                this.content_error =
                                    Some(format!("Could not initialize Git: {error}"));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Action::RefreshGit(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.menu = None;
                self.worktree_readiness.remove(&path);
                self.queue_worktree_readiness(path, cx);
                cx.notify();
            }
            Action::Reveal(path) => {
                cx.reveal_path(std::path::Path::new(&path));
                self.close_menu(window, cx);
            }
            #[cfg(test)]
            Action::RemoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Remove(path), window, cx);
            }
            #[cfg(test)]
            Action::ConfirmRemove(i) => {
                self.runtime
                    .read(cx)
                    .projects
                    .clone()
                    .update(cx, |p, cx| p.remove_project(i, false, cx));
                self.pending_remove = None;
            }
            #[cfg(test)]
            Action::RemoveWorktree(idx, path) => {
                let Some(project) = self.snapshot.projects.iter().find(|p| p.idx == idx) else {
                    return;
                };
                let Some(worktree) = project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.path == path && !worktree.is_main)
                else {
                    return;
                };
                let project_path = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|project| project.path.clone());
                let Some(project_path) = project_path else {
                    return;
                };
                self.pending_worktree_removal = Some(PendingWorktreeRemoval {
                    project_path,
                    path: path.clone(),
                    name: worktree.name.clone(),
                    started: false,
                    error: None,
                });
                self.worktree_removal_return_focus = self.worktree_delete_focus.get(&path).cloned();
                self.mode = ViewMode::Project;
                self.menu = None;
                self.worktree_removal_focus.focus(window, cx);
            }
            Action::ConfirmWorktreeRemoval => {
                let Some(removal) = self.pending_worktree_removal.as_mut() else {
                    return;
                };
                if removal.started {
                    return;
                }
                removal.started = true;
                let (project_path, path) = (removal.project_path.clone(), removal.path.clone());
                let service = self.runtime.read(cx).projects.clone();
                if let Err(error) = service.update(cx, |service, cx| {
                    service.remove_worktree_by_path(&project_path, &path, cx)
                }) {
                    if let Some(removal) = self.pending_worktree_removal.as_mut() {
                        removal.error = Some(error);
                    }
                }
                self.worktree_removal_focus.focus(window, cx);
            }
            Action::SkipWorktreeTeardown => {
                if let Some(removal) = &self.pending_worktree_removal {
                    if self
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .is_some_and(|status| status.stage == WorktreeRemovalStage::RunningScript)
                    {
                        self.runtime.read(cx).projects.clone().update(
                            cx,
                            crate::project_service::ProjectService::skip_worktree_teardown,
                        );
                        self.worktree_removal_focus.focus(window, cx);
                    }
                }
            }
            Action::DismissWorktreeRemoval => {
                self.pending_worktree_removal = None;
                if let Some(focus) = self.worktree_removal_return_focus.take() {
                    focus.focus(window, cx);
                } else {
                    self.focus.focus(window, cx);
                }
            }
            #[cfg(test)]
            Action::Launch(i, path, agent) => {
                if self.launch(i, path, agent, cx) {
                    if was_project_flyout {
                        self.dismiss_project_context(cx);
                    }
                    let native = self
                        .runtime
                        .read(cx)
                        .state
                        .read(cx)
                        .active_session()
                        .is_some_and(|id| {
                            self.runtime.read(cx).registry.read(cx).codex(id).is_some()
                        });
                    if was_project_flyout || native {
                        self.focus_active_session_after_palette(window, cx);
                    }
                }
            }
            Action::RunScript(project_path, path) => {
                self.choose_project_script(project_path, path, window, cx);
            }
            Action::RunNamedScript(project_path, path, id) => {
                let entry = cx
                    .global::<SettingsState>()
                    .store
                    .script_project(&project_path)
                    .scripts
                    .effective_entries()
                    .into_iter()
                    .find(|entry| entry.id == id);

                if let Some(grove_core::storage::ProjectAction {
                    kind: grove_core::storage::ProjectActionKind::Prompt { template },
                    ..
                }) = entry
                {
                    let feedback_action = format!("script-{id}");
                    let token =
                        self.begin_panel_feedback(&path, &feedback_action, "Sending prompt…", cx);
                    let result = match self.selection {
                        Some(Selection::Session(session_id)) => self
                            .runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .codex(session_id)
                            .cloned()
                            .ok_or_else(|| {
                                "Open a native Codex or Claude chat in this checkout first"
                                    .to_owned()
                            })
                            .and_then(|session| {
                                if session.read(cx).state.saved.cwd != path {
                                    return Err(
                                        "The current chat belongs to another checkout".into()
                                    );
                                }
                                session
                                    .update(cx, |session, cx| session.project_prompt(template, cx))
                                    .map(|_| ())
                            }),
                        _ => {
                            Err("Open a native Codex or Claude chat in this checkout first".into())
                        }
                    };
                    self.finish_panel_feedback(
                        &path,
                        &feedback_action,
                        token,
                        result.is_ok().then_some("Prompt queued"),
                        cx,
                    );
                    let error_key = (path.clone(), id.clone());
                    if let Err(error) = &result {
                        self.checkout_panel
                            .script_errors
                            .insert(error_key, error.clone());
                    } else {
                        self.checkout_panel.script_errors.remove(&error_key);
                    }
                    self.content_error = result.err();
                    cx.notify();
                } else if self.runtime.update(cx, |runtime, cx| {
                    runtime.spawn_project_script(&project_path, &path, &id, cx)
                }) {
                    let run = self.runtime.read(cx).script_runs.last().cloned();
                    if let Some(run) = run {
                        self.watch_script_feedback(&run, cx);
                    }
                    if self.mode == ViewMode::Grid {
                        self.mode = self.last_mode;
                    }
                    self.select_active_session(cx);
                    cx.notify();
                }
            }

            Action::Close(id) => {
                if self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject) {
                    self.project_flyout_hide_task = None;
                }
                self.pending_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::MarkReviewed(id, work_period) => {
                let keyboard = window.last_input_was_keyboard();
                let workspace = self.active_workspace;
                let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
                if let Some(session) = session {
                    let state = &session.read(cx).state;
                    if state.session_status() == "Review"
                        && state.review_completion() == Some(work_period)
                    {
                        let review = &state.saved.session_review;
                        let end = review
                            .pending
                            .as_ref()
                            .and_then(|receipt| state.saved.work_periods.get(receipt.period))
                            .and_then(|period| period.end_item);
                        let epoch = state.interaction_epoch;
                        let error = state.error.clone();
                        self.dashboard_timelines
                            .review_holds
                            .begin(workspace, id, keyboard);
                        self.dashboard_timelines.review_advance = (keyboard && self.dashboard_open)
                            .then(|| {
                                let row_focus =
                                    self.dashboard_timelines.session_focus(workspace, id, cx);
                                cx.observe_in(&session, window, move |this, session, window, cx| {
                                    let state = &session.read(cx).state;
                                    let review = &state.saved.session_review;
                                    if !this.dashboard_open
                                        || this.active_workspace != workspace
                                        || state.interaction_epoch != epoch
                                        || !row_focus.is_focused(window)
                                    {
                                        this.dashboard_timelines.review_advance = None;
                                        return;
                                    }
                                    if state.review_completion() == Some(work_period) {
                                        if state.error != error {
                                            this.dashboard_timelines.review_advance = None;
                                        }
                                        return;
                                    }
                                    this.dashboard_timelines.review_advance = None;
                                    if review.pending.is_none()
                                        && end == Some(review.reviewed_through)
                                        && state.session_status() == "Ready"
                                    {
                                        this.focus_next_dashboard_review(id, window, cx);
                                    }
                                })
                            });
                        session.update(cx, |session, cx| {
                            session
                                .send(grove_core::codex::Action::MarkReviewed { work_period }, cx);
                        });
                    }
                }
                self.dashboard_timelines.restore_review_focus(
                    self.active_workspace,
                    id,
                    window,
                    cx,
                );
                cx.notify();
            }
            Action::MarkTerminalReviewed(id, completion) => {
                self.dashboard_timelines.review_holds.begin(
                    self.active_workspace,
                    id,
                    window.last_input_was_keyboard(),
                );
                self.runtime
                    .read(cx)
                    .activity
                    .clone()
                    .update(cx, |activity, cx| {
                        activity.mark_terminal_reviewed(id, completion, cx);
                    });
                self.dashboard_timelines.restore_review_focus(
                    self.active_workspace,
                    id,
                    window,
                    cx,
                );
                cx.notify();
            }
            Action::MarkActivityDone(id) => {
                // Recheck the live state: a row may have changed since it was painted.
                let ready = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .is_some_and(|session| session.read(cx).state.session_status() == "Ready");
                if ready {
                    self.act(Action::FinishSession(id), window, cx);
                }
            }
            Action::FinishSession(id) => {
                let selected = self.selection == Some(Selection::Session(id));
                self.runtime
                    .update(cx, |runtime, cx| runtime.kill_session(id, cx));
                if selected {
                    self.open_landing_page(window, cx);
                }
                cx.notify();
            }
            Action::ConfirmClose(id) => {
                self.canvas_close_anchor = None;
                self.runtime.update(cx, |r, cx| r.kill_session(id, cx));
                self.pending_close = None;
                self.open_landing_page(window, cx);
            }
            Action::Retry(id) => {
                if let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() {
                    session.update(cx, |session, cx| {
                        session.send(grove_core::codex::Action::Reconnect, cx);
                    });
                    return;
                }
                let meta = self.runtime.read(cx).registry.read(cx).meta(id).cloned();
                if let Some(meta) = meta {
                    let extra_roots: Vec<_> = meta
                        .context_roots
                        .iter()
                        .filter(|root| root.wt_path != meta.wt_path)
                        .map(|root| root.wt_path.clone())
                        .collect();
                    let store = &cx.global::<SettingsState>().store;
                    let args = meta
                        .agent
                        .multi_root_launch_args_for_backend(
                            crate::runtime::native_codex_enabled(store, None),
                            store.yolo_enabled,
                            store.chrome_enabled.unwrap_or(false),
                            &extra_roots,
                        )
                        .unwrap_or_else(|| {
                            meta.agent.launch_args_for_backend(
                                crate::runtime::native_codex_enabled(store, None),
                                store.yolo_enabled,
                                store.chrome_enabled.unwrap_or(false),
                            )
                        });
                    let bundle = if !extra_roots.is_empty()
                        && matches!(meta.agent, Agent::OpenCode | Agent::Terminal)
                    {
                        match grove_core::multi_root::SymlinkBundle::create(&extra_roots) {
                            Ok(bundle) => Some(bundle.into_path().to_string_lossy().into_owned()),
                            Err(error) => {
                                self.content_error =
                                    Some(format!("Could not prepare session context: {error}"));
                                cx.notify();
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    self.runtime.update(cx, |r, cx| {
                        r.kill_session(id, cx);
                        r.spawn_session_in_with_context(
                            meta.project,
                            meta.wt_path,
                            meta.agent,
                            args,
                            meta.context_roots,
                            bundle,
                            cx,
                        );
                    });
                    let active = self.runtime.read(cx).state.read(cx).active_session();
                    if let Some(id) = active {
                        self.select(Selection::Session(id), cx);
                    }
                }
            }
            Action::OpenDiff(id) => {
                let Some(path) = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(id)
                    .map(|meta| meta.wt_path.clone())
                else {
                    return;
                };
                self.act(Action::OpenWorktreeDiff(path), window, cx);
            }
            Action::OpenBranchHistory(path, selection, identity) => {
                if self.selection == selection
                    && self.branch_entry.as_ref() == Some(&(path.clone(), identity))
                {
                    self.branch_entry_focus.focus(window, cx);
                    self.act(Action::OpenHistory(path), window, cx);
                }
            }
            Action::RefreshBranchPreview => {
                if let Some(cache) = &self.branch_peek_cache {
                    cache.data.update(cx, BranchHistoryState::reload);
                }
            }
            Action::OpenHistory(path) => {
                self.history_hover = branch_history::HistoryHover::default();
                self.dismiss_branch_peek(window, cx);
                self.dismiss_agent_peek(window, cx);
                let path = crate::paths::normalize_wt_path(&path).to_string();
                let history = cx.new(|cx| BranchHistoryState::new(path, cx));
                self.history_observer = Some(cx.observe(&history, |_, _, cx| cx.notify()));
                self.history = Some(history);
                self.history_selection = self.selection.clone();
                self.history_return_focus = window.focused(cx);
                self.history_focus.focus(window, cx);
            }
            Action::CloseHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                self.history = None;
                self.history_observer = None;
                self.history_selection = None;
                if let Some(focus) = self.history_return_focus.take() {
                    focus.focus(window, cx);
                }
            }
            Action::HistoryGraph => {
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::collapse);
                    self.history_focus.focus(window, cx);
                }
            }
            Action::RefreshHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::reload);
                }
            }
            Action::MoreHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::more);
                }
            }
            Action::SelectCommit(id) => {
                self.pin_history_card();
                if let Some(history) = &self.history {
                    history.update(cx, |history, cx| history.select(id, cx));
                }
            }
            Action::ExpandCommit => {
                self.pin_history_card();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::expand);
                }
            }
            Action::HistoryDiffMode(mode) => {
                if let Some(history) = &self.history {
                    history.update(cx, |history, cx| history.set_diff_mode(mode, cx));
                }
            }
            Action::ToggleHistoryMetadata => {
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::toggle_metadata);
                }
            }
            Action::CopyHistoryCommit => {
                if let Some(id) = self
                    .history
                    .as_ref()
                    .and_then(|history| history.read(cx).selected.clone())
                {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(id));
                }
            }
            Action::SelectHistoryFile(path) => {
                if let Some(history) = &self.history {
                    let file = history
                        .read(cx)
                        .details
                        .as_ref()
                        .and_then(|details| details.files.iter().find(|f| f.path == path))
                        .cloned();
                    if let Some(file) = file {
                        history.update(cx, |history, cx| history.file(file, cx));
                    }
                }
            }
            Action::OpenWorktreeDiff(path) => {
                let path = crate::paths::normalize_wt_path(&path).to_string();
                let mode = cx.global::<SettingsState>().store.diff_mode;
                let viewer = cx.new(|cx| DiffViewerState::new(path, mode, cx));
                self.diff_observer = Some(cx.observe(&viewer, |_, _, cx| cx.notify()));
                self.diff_viewer = Some(viewer);
                self.diff_return_focus = window.focused(cx);
                self.diff_focus.focus(window, cx);
            }
            Action::CloseDiff => {
                self.diff_viewer = None;
                self.diff_observer = None;
                if let Some(focus) = self.diff_return_focus.take() {
                    focus.focus(window, cx);
                } else {
                    self.focus.focus(window, cx);
                }
            }
            Action::SelectDiffFile(path) => {
                if let Some(viewer) = &self.diff_viewer {
                    viewer.update(cx, |viewer, cx| viewer.select(path, cx));
                }
            }
            Action::RefreshDiff => {
                if let Some(viewer) = &self.diff_viewer {
                    viewer.update(cx, DiffViewerState::load_files);
                }
            }
            Action::AddTerminal => {
                if self.mode == ViewMode::Grid {
                    self.mode = self.last_mode;
                }
                let previous_count = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal_count();
                self.runtime.update(cx, Runtime::new_home_terminal);
                let new_id = {
                    let registry = self.runtime.read(cx).registry.read(cx);
                    (registry.home_terminal_count() > previous_count)
                        .then(|| registry.home_terminals().last().map(|meta| meta.id))
                        .flatten()
                };
                if let Some(id) = new_id {
                    self.terminal_owners.insert(id, self.active_workspace);
                    if !self.close_settings_for_session_navigation(window, cx) {
                        return;
                    }
                    self.select(Selection::Home(id), cx);
                    self.mode = ViewMode::Project;
                    SettingsState::update(cx, |store| store.sidebar_collapsed = false);
                    self.pending_canvas_focus = Some(id);
                }
            }
            Action::CloseHome(id) => {
                self.pending_home_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmHome(id) => {
                let from_canvas = self.canvas_close_anchor.take().is_some();
                let was_selected = self.selection == Some(Selection::Home(id));
                let index = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminals()
                    .iter()
                    .position(|m| m.id == id);
                if let Some(i) = index {
                    self.runtime
                        .update(cx, |r, cx| r.close_home_terminal(i, cx));
                }
                self.pending_home_close = None;
                self.terminal_owners.remove(&id);
                if was_selected {
                    self.selection = None;
                }
                if from_canvas || was_selected {
                    self.focus_remaining_canvas(window, cx);
                }
            }
            Action::Cancel => {
                if self.pending_worktree_removal.take().is_some() {
                    if let Some(focus) = self.worktree_removal_return_focus.take() {
                        focus.focus(window, cx);
                    } else {
                        self.focus.focus(window, cx);
                    }
                    cx.notify();
                    return;
                }
                self.canvas_close_anchor = None;
                if self.menu.is_some() {
                    self.close_menu(window, cx);
                }
                if let Some(focus) = self.confirmation_return_focus.take() {
                    focus.focus(window, cx);
                }
                self.pending_close = None;
                self.pending_home_close = None;
                self.pending_remove = None;
                self.menu = None;
            }
        }
        cx.notify();
    }

    pub(super) fn dismiss_project_panel_for_navigation(&mut self) {
        if self.project_decision {
            return;
        }
        self.project_panel = None;
        self.project_return_focus = None;
        self.project_return_path = None;
        self.project_return_navigation = None;
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn menu_state(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> crate::app_menus::MenuState {
        crate::app_menus::MenuState {
            window: true,
            fullscreen: window.is_fullscreen(),
            navigation: self.navigation_available() && !self.settings_navigation_blocked(cx),
            worktree: self.selected_worktree().is_some(),
            close_session: matches!(
                self.selection,
                Some(Selection::Session(_) | Selection::Home(_))
            ),
            sessions: !self.visible_session_order(cx).is_empty(),
            waiting: self.waiting_session(cx).is_some(),
            terminal_edit: self
                .terminal_views
                .values()
                .chain(self.home_terminal_views.values())
                .chain(self.worktree_terminal.views.values())
                .find_map(|view| view.read(cx).terminal_menu_state(window, cx)),
            grid: self.is_grid(),
            zen: self.is_zen(),
            collapsed: self.is_collapsed(cx),
            list: self.mode == ViewMode::List,
            ..Default::default()
        }
    }
}
