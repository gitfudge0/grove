//! Session selection, canvas focus and navigation mode transitions.

use super::{
    branch_history, global_navigation, grid, zen_target, Action, HashSet, Selection, SessionId,
    SettingsState, Sidebar, ViewMode,
};
#[cfg(test)]
use gpui::FocusHandle;
use gpui::{App, Context, Focusable, Window};

impl Sidebar {
    pub(crate) fn active_canvas_sessions(&self, cx: &App) -> Vec<(SessionId, bool)> {
        let store = &cx.global::<SettingsState>().store;
        let workspace = store.workspaces.active;
        let projects: HashSet<_> = store
            .workspace_projects(workspace)
            .map(|(_, project)| project.name.as_str())
            .collect();
        let registry = self.runtime.read(cx).registry.read(cx);
        let mut sessions: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| meta.project.is_empty() || projects.contains(meta.project.as_str()))
            .map(|meta| (meta.id, false))
            .collect();
        sessions.extend(
            registry
                .home_terminals()
                .iter()
                .filter(|meta| {
                    let owner = self.terminal_owners.get(&meta.id).copied().unwrap_or(1);
                    if store.workspaces.rows.iter().any(|row| row.id == owner) {
                        owner == workspace
                    } else {
                        true
                    }
                })
                .map(|meta| (meta.id, true)),
        );
        if let Some(grid) = self.grid_layouts.get(&workspace) {
            grid::ordered_sessions(sessions, &grid.order)
        } else {
            sessions
        }
    }

    /// Called after the switcher closes so focus never moves behind its overlay.
    pub(crate) fn focus_grid_workspace_after_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync(window, cx);
        if self.dashboard_open {
            self.open_destination(self.destination, window, cx);
            return;
        }
        if self.mode != ViewMode::Grid || self.is_zen() {
            return;
        }
        let Some((id, home)) = self.active_canvas_sessions(cx).first().copied() else {
            self.pending_grid_workspace_focus = None;
            self.focus.focus(window, cx);
            return;
        };
        self.select(
            if home {
                Selection::Home(id)
            } else {
                Selection::Session(id)
            },
            cx,
        );
        let view = if home {
            self.home_terminal_views.get(&id)
        } else {
            self.terminal_views.get(&id)
        };
        if let Some(view) = view {
            view.focus_handle(cx).focus(window, cx);
        } else {
            self.pending_grid_workspace_focus = Some(id);
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn canvas_terminal_focus(
        &self,
        id: SessionId,
        home: bool,
        cx: &App,
    ) -> Option<FocusHandle> {
        let view = if home {
            self.home_terminal_views.get(&id)
        } else {
            self.terminal_views.get(&id)
        }?;
        Some(view.focus_handle(cx))
    }

    pub(in crate::views) fn request_canvas_close(
        &mut self,
        id: SessionId,
        home: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.canvas_close_anchor = Some(id);
        self.act(
            if home {
                Action::CloseHome(id)
            } else {
                Action::Close(id)
            },
            window,
            cx,
        );
    }

    pub(super) fn focus_remaining_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self
            .active_canvas_sessions(cx)
            .into_iter()
            .find(|(_, home)| {
                self.destination != global_navigation::Destination::Terminals
                    || self.mode == ViewMode::Grid
                    || self.is_zen()
                    || *home
            });
        if let Some((id, home)) = next {
            self.select(
                if home {
                    Selection::Home(id)
                } else {
                    Selection::Session(id)
                },
                cx,
            );
            let view = if home {
                self.home_terminal_views.get(&id)
            } else {
                self.terminal_views.get(&id)
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        } else {
            self.focus.focus(window, cx);
        }
    }

    pub fn is_grid(&self) -> bool {
        self.mode == ViewMode::Grid
    }

    pub(crate) fn is_zen(&self) -> bool {
        self.zen_return.is_some()
    }

    pub(crate) fn toggle_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        self.sync(window, cx);
        if let Some((mode, selection)) = self.zen_return.take() {
            self.mode = mode;
            self.selection = selection;
            let view = match self.selection {
                Some(Selection::Session(id)) => self.terminal_views.get(&id),
                Some(Selection::Home(id)) => self.home_terminal_views.get(&id),
                _ => None,
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
            cx.notify();
            return;
        }
        let sessions = self.active_canvas_sessions(cx);
        let target = zen_target(self.mode, self.selection.as_ref(), &sessions);
        self.zen_return = Some((self.mode, self.selection.clone()));
        if let Some((id, home)) = target {
            self.selection = Some(if home {
                Selection::Home(id)
            } else {
                Selection::Session(id)
            });
            let view = if home {
                self.home_terminal_views.get(&id)
            } else {
                self.terminal_views.get(&id)
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.pending_canvas_focus = Some(id);
                self.focus.focus(window, cx);
            }
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn dashboard_draft_for_test(&self, cx: &App) -> Option<String> {
        self.dashboard
            .as_ref()
            .map(|dashboard| dashboard.session.read(cx).state.saved.draft.clone())
    }

    #[cfg(test)]
    pub(crate) fn select_project_canvas_for_test(&mut self, cx: &mut Context<Self>) {
        self.select(Selection::Project(0), cx);
    }

    pub(super) fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.checkout_panel.change_tools.dismiss();
        self.dashboard_open = false;
        if self.destination == global_navigation::Destination::History {
            self.destination = global_navigation::Destination::Sessions;
            self.session_history_selected = None;
            self.session_history_view = None;
        }
        match selection {
            Selection::Project(_) | Selection::Worktree(..) => {
                self.destination = global_navigation::Destination::Projects;
                self.mode = ViewMode::Project;
                self.last_mode = ViewMode::Project;
            }
            Selection::Home(_) => self.destination = global_navigation::Destination::Terminals,
            Selection::Session(_)
                if self.destination == global_navigation::Destination::Home
                    || self.destination == global_navigation::Destination::Terminals =>
            {
                if self.mode == ViewMode::Grid {
                    self.destination = global_navigation::Destination::Projects;
                } else {
                    self.destination = global_navigation::Destination::Sessions;
                    self.mode = ViewMode::List;
                }
            }
            Selection::Session(_) => {}
        }
        if self
            .history_selection
            .as_ref()
            .is_some_and(|owner| owner != &selection)
        {
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
        }
        self.dismiss_project_context(cx);
        self.initial_selection_pending = false;
        self.pending_canvas_focus = None;
        self.pending_grid_workspace_focus = None;
        self.dismiss_project_panel_for_navigation();
        self.runtime
            .read(cx)
            .state
            .clone()
            .update(cx, |state, _| state.clear_canvas_selection());
        if let Selection::Session(id) = selection {
            let snap = self.snapshot.clone();
            self.runtime.read(cx).state.clone().update(cx, |s, cx| {
                s.select_session(id, &snap);
                cx.notify();
            });
        }
        if let Selection::Home(id) = selection {
            let registry = self.runtime.read(cx).registry.read(cx);
            let index = registry.home_terminals().iter().position(|m| m.id == id);
            let count = registry.home_terminal_count();
            if let Some(index) = index {
                self.runtime.read(cx).state.clone().update(cx, |s, cx| {
                    s.select_home_terminal(index, count);
                    cx.notify();
                });
            }
        }
        self.selection = Some(selection);
        cx.notify();
    }

    pub(crate) fn select_active_session(&mut self, cx: &mut Context<Self>) -> Option<SessionId> {
        if self.settings_navigation_blocked(cx) {
            return None;
        }
        let id = self.runtime.read(cx).state.read(cx).active_session()?;
        self.select(Selection::Session(id), cx);
        Some(id)
    }

    /// Hand focus to the session after the command palette has restored its prior focus.
    pub(crate) fn focus_active_session_after_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.select_active_session(cx) else {
            return;
        };
        if let Some(view) = self.codex_views.get(&id) {
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            view.focus_handle(cx).focus(window, cx);
        } else if let Some(view) = self.terminal_views.get(&id) {
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            view.focus_handle(cx).focus(window, cx);
        } else {
            self.pending_canvas_focus = Some(id);
            self.pending_grid_workspace_focus = (self.mode == ViewMode::Grid).then_some(id);
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn request_close_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        let canvas_visible = self.mode == ViewMode::Grid || self.is_zen();
        match self.selection {
            Some(Selection::Session(id)) => {
                if canvas_visible {
                    self.request_canvas_close(id, false, window, cx);
                } else {
                    if let Some(project) = self.snapshot.projects.iter().find(|project| {
                        project
                            .worktrees
                            .iter()
                            .any(|worktree| worktree.sessions.contains(&id))
                    }) {
                        if let Some(stored) =
                            cx.global::<SettingsState>().store.projects.get(project.idx)
                        {
                            self.collapsed_projects.remove(&stored.path);
                        }
                    }
                    self.act(Action::Close(id), window, cx);
                }
            }
            Some(Selection::Home(id)) => {
                if canvas_visible {
                    self.request_canvas_close(id, true, window, cx);
                } else {
                    self.act(Action::CloseHome(id), window, cx);
                }
            }
            Some(Selection::Project(_) | Selection::Worktree(_, _)) | None => {}
        }
    }

    pub(in crate::views) fn html_owner(&self, cx: &App) -> Option<gpui::EntityId> {
        let Some(Selection::Session(id)) = self.selection else {
            return None;
        };
        self.codex_views
            .get(&id)
            .map(|view| view.read(cx).html.entity_id())
    }

    pub(in crate::views) fn html_overlay_open(&self) -> bool {
        self.usage_panel.is_some()
            || self.confirmation_open()
            || self.header_tasks.open.is_some()
            || self.history.is_some()
            || self.diff_viewer.is_some()
            || self.project_panel.is_some()
            || self.project_flyout.is_some()
    }

    pub(crate) fn highlights_blocked(&self) -> bool {
        !self.navigation_available()
    }

    pub(crate) fn navigation_available(&self) -> bool {
        !self.confirmation_open()
            && self.header_tasks.open.is_none()
            && self.history.is_none()
            && self.diff_viewer.is_none()
            && self.project_panel.is_none()
            && self.menu.is_none()
    }

    pub(super) fn canvas_focus_allowed(&self, window: &Window, cx: &App) -> bool {
        self.canvas_focus_allowed_for_mode(window, cx, false)
    }

    pub(super) fn grid_workspace_focus_allowed(&self, window: &Window, cx: &App) -> bool {
        self.canvas_focus_allowed_for_mode(window, cx, true)
    }

    fn canvas_focus_allowed_for_mode(&self, window: &Window, cx: &App, allow_grid: bool) -> bool {
        if !self.navigation_available()
            || self.settings_navigation_blocked(cx)
            || (self.mode == ViewMode::Grid && !self.is_zen() && !allow_grid)
            || self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.read(cx).is_open())
        {
            return false;
        }
        window.focused(cx).is_none()
            || self.focus.contains_focused(window, cx)
            || self
                .shell_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window))
            || self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.focus_handle(cx).is_focused(window))
    }

    pub(crate) fn toggle_tree_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        let target = match self.mode {
            ViewMode::Grid => self.last_mode,
            ViewMode::Project => ViewMode::List,
            ViewMode::List => ViewMode::Project,
        };
        self.act(Action::Mode(target), window, cx);
    }

    /// Preserve Shell's modal/Settings gate after moving Grid into the sidebar.
    /// Other destinations retain their existing independent navigation rules.
    pub(crate) fn set_grid_blocked_by_shell(&mut self, blocked: bool, cx: &mut Context<Self>) {
        if self.grid_blocked_by_shell != blocked {
            self.grid_blocked_by_shell = blocked;
            cx.notify();
        }
    }

    pub(super) fn grid_navigation_available(&self) -> bool {
        self.navigation_available() && !self.grid_blocked_by_shell
    }

    pub(crate) fn toggle_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.grid_navigation_available() {
            return;
        }
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else {
            ViewMode::Grid
        };
        self.act(Action::Mode(target), window, cx);
    }

    pub(crate) fn add_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() && !self.settings_navigation_blocked(cx) {
            self.act(Action::AddTerminal, window, cx);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.act(Action::ToggleSidebar, window, cx);
    }
}
