//! Workspace snapshots, readiness polling and title and age subscriptions.

use super::{
    branch_history, remap_selection, Duration, HashMap, HashSet, Instant, Navigation, Selection,
    SettingsState, Sidebar, SidebarContext, ViewMode,
};
use gpui::{App, Context, Window};

pub(super) const SESSION_AGE_REFRESH: Duration = Duration::from_secs(1);
const WORKTREE_READINESS_REFRESH: Duration = Duration::from_secs(5);

impl Sidebar {
    pub(super) fn queue_worktree_readiness(&mut self, path: String, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .worktree_readiness
            .get(&path)
            .is_some_and(|(checked, _)| now.duration_since(*checked) < WORKTREE_READINESS_REFRESH)
        {
            return;
        }
        self.worktree_readiness.insert(path.clone(), (now, None));
        cx.spawn(async move |this, cx| {
            let checked_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::project_service::worktree_readiness(&checked_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .worktree_readiness
                    .get(&path)
                    .is_some_and(|(started, state)| *started == now && state.is_none())
                {
                    this.worktree_readiness
                        .insert(path, (Instant::now(), Some(result)));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_dirty = false;
        self.registry_sync_revision = self.runtime.read(cx).registry.read(cx).revision();
        self.list_groups_cache.borrow_mut().take();
        if self.git_refresh_timer.is_none() {
            self.git_refresh_timer = Some(cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.git_paths.is_empty() {
                            return;
                        }
                        let paths = this.git_paths.clone();
                        this.runtime.read(cx).tree.clone().update(cx, |tree, cx| {
                            tree.maybe_poll_git_state(paths, window.is_window_active(), cx);
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }));
        }
        if self.mode != ViewMode::Grid || self.is_zen() || !self.navigation_available() {
            self.grid_session_drag = None;
        }
        let paths: HashMap<usize, String> = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(idx, p)| (idx, p.path.clone()))
            .collect();
        let new_indices: HashMap<_, _> = paths.iter().map(|(index, path)| (path, *index)).collect();
        let mut mapping: HashMap<usize, usize> = self
            .project_paths
            .iter()
            .filter_map(|(old, path)| new_indices.get(path).map(|new| (*old, *new)))
            .collect();
        for idx in paths.keys() {
            if !self.project_paths.contains_key(idx) {
                mapping.insert(*idx, *idx);
            }
        }
        if self.project_paths != paths {
            remap_selection(&mut self.selection, &mapping);
            for saved in self.saved.values_mut() {
                remap_selection(&mut saved.selection, &mapping);
            }
            self.menu = self.menu.and_then(|i| mapping.get(&i).copied());
            self.pending_remove = self.pending_remove.and_then(|i| mapping.get(&i).copied());
            self.pending_new_worktree = self
                .pending_new_worktree
                .and_then(|i| mapping.get(&i).copied());
            self.project_paths = paths;
        }
        self.collapsed_projects
            .retain(|path| self.project_paths.values().any(|project| project == path));
        self.project_toggle_focus
            .retain(|path, _| self.project_paths.values().any(|project| project == path));
        for path in self.project_paths.values() {
            self.project_toggle_focus
                .entry(path.clone())
                .or_insert_with(|| cx.focus_handle());
        }
        let active = cx.global::<SettingsState>().store.workspaces.active;
        let switched = active != self.active_workspace;
        if self
            .project_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).workspace != active)
        {
            self.project_panel = None;
            self.project_decision = false;
            self.project_return_focus = None;
            self.project_return_path = None;
            self.project_return_navigation = None;
            self.focus.focus(window, cx);
        }
        if active != self.active_workspace {
            self.session_history_selected = None;
            self.session_history_view = None;
            self.history_project_filter = None;
            self.history_provider_filter = None;
            self.history_search
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.history_filter_popup = None;
            self.history_filter_subscription = None;
            self.session_history.clear();
            self.dashboard_open = false;
            self.branch_peek = None;
            self.branch_peek_task = None;
            self.branch_entry = None;
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
            self.zen_return = None;
            self.saved.insert(
                self.active_workspace,
                Navigation {
                    selection: self.selection.take(),
                    scroll: self.scroll.clone(),
                },
            );
            let next = self.saved.remove(&active).unwrap_or_default();
            self.selection = next.selection;
            self.scroll = next.scroll;
            self.active_workspace = active;
            self.menu = None;
            self.pending_close = None;
            self.pending_home_close = None;
            self.canvas_close_anchor = None;
            self.dismiss_header_tasks_for_navigation(window, cx);
            self.grid_drag = None;
            self.grid_session_drag = None;
            self.pending_remove = None;
            self.pending_new_worktree = None;
            self.content_error = None;
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
        }
        let existing: HashSet<_> = cx
            .global::<SettingsState>()
            .store
            .workspaces
            .rows
            .iter()
            .map(|w| w.id)
            .collect();
        for owner in self.terminal_owners.values_mut() {
            if !existing.contains(owner) {
                *owner = active;
            }
        }
        let rt = self.runtime.read(cx);
        let tree = rt.tree.clone();
        let ids: Vec<_> = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(active)
            .map(|(i, _)| i)
            .collect();
        let generation = tree.read(cx).generation();
        let active_project = rt.state.read(cx).proj_idx();
        let warm_active = self.cache_warm.is_none()
            && !tree
                .read(cx)
                .worktrees_for_project(active_project, active_project)
                .is_empty();
        if self.cache_warm != Some((active, generation)) {
            let targets = cx
                .global::<SettingsState>()
                .store
                .workspace_projects(active)
                .filter(|(index, _)| !warm_active || *index != active_project)
                .map(|(i, p)| (i, p.path.clone()))
                .collect();
            tree.update(cx, |tree, cx| tree.sweep_wt_cache(targets, cx));
            self.cache_warm = Some((active, generation));
        }
        self.snapshot = self.runtime.update(cx, |r, cx| r.snapshot(cx));
        self.snapshot.projects.retain(|p| ids.contains(&p.idx));
        let stale_project_flyout = switched
            || !self.is_collapsed(cx)
            || self.mode != ViewMode::Project
            || self.zen_return.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            || self.project_flyout.as_ref().is_some_and(|target| {
                if *target == SidebarContext::MultiProject {
                    return false;
                }
                let Some(path) = target.project_path() else {
                    return true;
                };
                !self
                    .snapshot
                    .projects
                    .iter()
                    .any(|p| self.project_paths.get(&p.idx) == Some(path))
            });
        if stale_project_flyout {
            let restore_focus = self.project_flyout.is_some()
                && self.project_flyout_focus.contains_focused(window, cx);
            self.project_flyout = None;
            self.project_flyout_launch_path = None;
            self.project_flyout_launch_error = None;
            self.project_flyout_hover_open = false;
            self.project_flyout_hide_task = None;
            if restore_focus {
                self.focus.focus(window, cx);
            }
        }
        self.git_paths = {
            let registry = self.runtime.read(cx).registry.read(cx);
            let mut seen = HashSet::new();
            self.snapshot
                .projects
                .iter()
                .flat_map(|project| project.sessions.iter())
                .filter_map(|id| registry.meta(*id))
                .map(|meta| crate::paths::normalize_wt_path(&meta.wt_path).to_string())
                .filter(|path| !path.is_empty() && seen.insert(path.clone()))
                .collect::<Vec<_>>()
        };
        tree.update(cx, |tree, cx| {
            tree.maybe_poll_git_state(self.git_paths.clone(), window.is_window_active(), cx);
        });
        for project in &self.snapshot.projects {
            self.project_menu_bounds.entry(project.idx).or_default();
            self.project_group_bounds.entry(project.idx).or_default();
            self.project_menu_focus
                .entry(project.idx)
                .or_insert_with(|| cx.focus_handle());
        }
        for id in self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.sessions)
        {
            self.session_close_bounds.entry(*id).or_default();
            if !self.session_close_focus.contains_key(id) {
                let handle = cx.focus_handle().tab_stop(true);
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.session_close_focus.insert(*id, handle);
            }
        }
        for meta in self.runtime.read(cx).registry.read(cx).home_terminals() {
            self.home_close_bounds.entry(meta.id).or_default();
        }
        for path in self
            .snapshot
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter().map(|w| w.path.clone()))
        {
            if !self.worktree_diff_focus.contains_key(&path) {
                let handle = cx.focus_handle().tab_stop(true);
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.worktree_diff_focus.insert(path.clone(), handle);
            }
            if !self.worktree_focus.contains_key(&path) {
                let handle = cx.focus_handle().tab_stop(true);
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.worktree_focus.insert(path, handle);
            }
        }
        for path in self.snapshot.projects.iter().flat_map(|p| {
            p.worktrees
                .iter()
                .filter(|worktree| !worktree.is_main)
                .map(|worktree| worktree.path.clone())
        }) {
            self.worktree_delete_focus
                .entry(path)
                .or_insert_with(|| cx.focus_handle());
        }
        let visible_session_ids: HashSet<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .flat_map(|worktree| worktree.sessions.iter().copied())
            .collect();
        self.session_diff_focus
            .retain(|id, _| visible_session_ids.contains(id));
        self.session_diff_observers
            .retain(|id, _| visible_session_ids.contains(id));
        for id in visible_session_ids {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.session_diff_focus.entry(id)
            {
                let handle = cx.focus_handle();
                let focus_in = cx.on_focus_in(&handle, window, |_, _, cx| cx.notify());
                let focus_out = cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify());
                entry.insert(handle);
                self.session_diff_observers
                    .insert(id, (focus_in, focus_out));
            }
        }
        let registry = self.runtime.read(cx).registry.read(cx);
        let live: HashSet<_> = registry
            .all()
            .iter()
            .chain(registry.home_terminals().iter())
            .map(|meta| meta.id)
            .collect();
        self.canvas_bounds.retain(|id, _| live.contains(id));
        self.title_edit_focus.retain(|id, _| live.contains(id));
        if self
            .title_edit
            .as_ref()
            .is_some_and(|edit| !live.contains(&edit.session))
        {
            self.title_edit = None;
        }
        self.canvas_close_focus
            .retain(|(id, _), _| live.contains(id));
        self.canvas_breadcrumb_focus
            .retain(|(id, _), _| live.contains(id));
        self.canvas_observers.retain(|id, _| live.contains(id));
        self.canvas_focus_observers
            .retain(|id, _| live.contains(id));
        self.canvas_signatures.retain(|id, _| live.contains(id));
        self.codex_views
            .retain(|id, _| registry.meta(*id).is_some());
        self.terminal_views
            .retain(|id, _| registry.meta(*id).is_some());
        self.home_terminal_views
            .retain(|id, _| registry.home_terminals().iter().any(|m| m.id == *id));
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| match selection {
                Selection::Session(id) => {
                    !registry
                        .meta(*id)
                        .is_some_and(|meta| meta.project.is_empty())
                        && !self
                            .snapshot
                            .projects
                            .iter()
                            .any(|p| p.sessions.contains(id))
                }
                Selection::Home(id) => !registry.home_terminals().iter().any(|m| m.id == *id),
                Selection::Project(idx) | Selection::Worktree(idx, _) => !ids.contains(idx),
            })
        {
            self.selection = None;
        }
        let show_dashboard_on_startup = self.initial_selection_pending;
        if self.selection.is_none() && (switched || self.initial_selection_pending) {
            let next = self
                .visible_session_order(cx)
                .first()
                .copied()
                .map(Selection::Session)
                .or_else(|| {
                    self.active_canvas_sessions(cx)
                        .into_iter()
                        .find_map(|(id, home)| home.then_some(Selection::Home(id)))
                })
                .or_else(|| {
                    self.snapshot
                        .projects
                        .first()
                        .map(|project| Selection::Project(project.idx))
                });
            if let Some(selection) = next {
                let (mode, last_mode) = (self.mode, self.last_mode);
                self.select(selection, cx);
                self.mode = mode;
                self.last_mode = last_mode;
                self.pending_canvas_focus = match self.selection {
                    Some(Selection::Session(id) | Selection::Home(id)) => Some(id),
                    _ => None,
                };
            }
        }
        if switched {
            self.runtime.read(cx).state.clone().update(cx, |state, cx| {
                state.clear_canvas_selection();
                cx.notify();
            });
            if self.mode == ViewMode::Grid && !self.is_zen() {
                self.selection = self.active_canvas_sessions(cx).first().map(|(id, home)| {
                    if *home {
                        Selection::Home(*id)
                    } else {
                        Selection::Session(*id)
                    }
                });
            }
            if let Some(selection) = self.selection.clone() {
                let (mode, last_mode) = (self.mode, self.last_mode);
                self.select(selection, cx);
                self.mode = mode;
                self.last_mode = last_mode;
            }
            self.pending_canvas_focus = match self.selection {
                Some(Selection::Session(id) | Selection::Home(id)) => Some(id),
                _ => None,
            };
            if self.mode == ViewMode::Grid && !self.is_zen() {
                self.pending_grid_workspace_focus = self.pending_canvas_focus;
            }
            cx.notify();
        }
        if show_dashboard_on_startup || switched {
            self.initial_selection_pending = false;
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            self.open_landing_page(window, cx);
        }
        let checked_indices = [
            match &self.selection {
                Some(Selection::Project(idx) | Selection::Worktree(idx, _)) => Some(*idx),
                _ => None,
            },
            self.menu,
        ];
        let checked_paths = checked_indices
            .into_iter()
            .flatten()
            .filter_map(|idx| {
                cx.global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|p| p.path.clone())
            })
            .collect::<Vec<_>>();
        for path in checked_paths {
            self.queue_worktree_readiness(path, cx);
        }
        self.sync_title_observers(cx);
        self.sync_task_glances(cx);
    }

    fn sync_title_observers(&mut self, cx: &mut Context<Self>) {
        let registry = self.runtime.read(cx).registry.read(cx);
        let mut observed = Vec::new();
        let mut seen = HashSet::new();
        for id in self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter().copied())
        {
            if seen.insert(id) {
                if let Some(session) = registry.session(id) {
                    observed.push((id, session.clone()));
                }
            }
        }
        for (index, meta) in registry.home_terminals().iter().enumerate() {
            if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) == self.active_workspace {
                if let Some(session) = registry.home_terminal(index) {
                    observed.push((meta.id, session.clone()));
                }
            }
        }
        let observed_ids: HashSet<_> = observed.iter().map(|(id, _)| *id).collect();
        self.title_observers
            .retain(|id, _| observed_ids.contains(id));
        self.observed_titles
            .retain(|id, _| observed_ids.contains(id));
        for (id, session) in observed {
            if self
                .title_observers
                .get(&id)
                .is_some_and(|(existing, _)| existing == &session)
            {
                continue;
            }
            self.observed_titles.insert(id, session.read(cx).title());
            let observer = cx.observe(&session, move |this, session, cx| {
                let title = session.read(cx).title();
                if this.observed_titles.get(&id) != Some(&title) {
                    this.observed_titles.insert(id, title);
                    cx.notify();
                }
            });
            self.title_observers.insert(id, (session, observer));
        }
    }

    fn age_visible(&self) -> bool {
        (self.mode == ViewMode::List || self.dashboard_open)
            && self
                .snapshot
                .projects
                .iter()
                .any(|project| !project.sessions.is_empty())
    }

    fn age_refresh_delay(&self, cx: &App) -> Duration {
        if !self.window_active {
            return Duration::from_mins(1);
        }
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        let activity = runtime.activity.read(cx);
        let now = Instant::now();
        registry
            .all()
            .iter()
            .map(|meta| {
                let since = activity.since_of(meta.id).unwrap_or(meta.spawned_at);
                let age = now.saturating_duration_since(since);
                if age < Duration::from_mins(1) {
                    SESSION_AGE_REFRESH
                } else {
                    Duration::from_secs(60 - age.as_secs() % 60)
                }
            })
            .min()
            .unwrap_or(Duration::from_mins(1))
    }

    pub(super) fn sync_age_timer(&mut self, cx: &mut Context<Self>) {
        if !self.age_visible() {
            self.age_timer = None;
        } else if self.age_timer.is_none() {
            self.age_timer = Some(
                cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
                    let delay = this
                        .update(cx, |this, cx| this.age_refresh_delay(cx))
                        .unwrap_or(Duration::from_mins(1));
                    cx.background_executor().timer(delay).await;
                    let visible = this
                        .update(cx, |this, cx| {
                            if this.age_visible() {
                                #[cfg(test)]
                                {
                                    this.age_ticks += 1;
                                }
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !visible {
                        break;
                    }
                }),
            );
        }
    }
}
