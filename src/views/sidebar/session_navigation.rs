//! Session destination selection and focus routing from the Sidebar.

use super::*;

impl Sidebar {
    pub(super) fn list_session_groups(&self, cx: &App) -> Vec<(&'static str, Vec<SessionId>)> {
        if self.registry_sync_revision == self.runtime.read(cx).registry.read(cx).revision() {
            if let Some(groups) = self.list_groups_cache.borrow().as_ref() {
                return groups.clone();
            }
        }
        let visible: HashSet<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter().copied())
            .collect();
        let registry = self.runtime.read(cx).registry.read(cx);
        let activity = self.runtime.read(cx).activity.read(cx);
        let entries: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| meta.project.is_empty() || visible.contains(&meta.id))
            .map(|meta| SessionListEntry {
                id: meta.id,
                state: activity.state_of(meta.id),
                status: self
                    .session_placement
                    .status(meta.id, self.status(meta, cx).0),
                since: activity.since_of(meta.id),
                active: activity.active_of(meta.id),
            })
            .collect();
        let groups: Vec<_> = group_sessions_for_list(&entries, Instant::now())
            .into_iter()
            .map(|(heading, indices)| {
                let mut ids: Vec<_> = indices.into_iter().map(|index| entries[index].id).collect();
                self.session_placement.sort(&mut ids);
                (heading, ids)
            })
            .collect();
        *self.list_groups_cache.borrow_mut() = Some(groups.clone());
        groups
    }

    pub(super) fn visible_session_order(&self, cx: &App) -> Vec<SessionId> {
        if self.mode == ViewMode::Grid {
            return self
                .active_canvas_sessions(cx)
                .into_iter()
                .filter_map(|(id, home)| (!home).then_some(id))
                .collect();
        }
        // Shortcut/switcher order follows the always-mounted Sessions section,
        // independently of which main-content destination is currently open.
        self.sidebar_session_groups(cx)
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .collect()
    }

    pub(crate) fn visible_session_targets(&self, cx: &App) -> Vec<(SessionId, String)> {
        let registry = self.runtime.read(cx).registry.read(cx);
        self.visible_session_order(cx)
            .into_iter()
            .filter_map(|id| {
                let meta = registry.meta(id)?;
                let title = session_display_title(
                    meta,
                    registry
                        .session(id)
                        .and_then(|session| session.read(cx).title()),
                );
                Some((id, format!("{title} · {}", meta.project)))
            })
            .collect()
    }

    pub(crate) fn select_session_id(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        self.sync(window, cx);
        if self.visible_session_order(cx).contains(&id) {
            self.navigate_to_session(id, window, cx);
        }
    }

    pub(super) fn settings_navigation_blocked(&self, cx: &App) -> bool {
        self.settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).project_operation_pending(cx))
    }

    pub(super) fn close_settings_for_session_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel) = &self.settings_panel else {
            return true;
        };
        panel.update(cx, |panel, cx| {
            panel.close(window, cx);
            !panel.is_open()
        })
    }

    pub(super) fn navigate_to_session(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Close first: settings restores its prior focus, then navigation focuses the target.
        if !self.close_settings_for_session_navigation(window, cx) {
            return;
        }
        self.select(Selection::Session(id), cx);
        if let Some(view) = self.codex_views.get(&id) {
            view.focus_handle(cx).focus(window, cx);
        } else if let Some(view) = self.terminal_views.get(&id) {
            view.focus_handle(cx).focus(window, cx);
        } else {
            self.pending_canvas_focus = Some(id);
            self.pending_grid_workspace_focus = (self.mode == ViewMode::Grid).then_some(id);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn select_next_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_relative_session(1, window, cx);
    }

    pub(crate) fn select_previous_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_relative_session(-1, window, cx);
    }

    fn select_relative_session(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        self.sync(window, cx);
        let selected = self
            .selection
            .as_ref()
            .and_then(|selection| match selection {
                Selection::Session(id) => Some(*id),
                _ => None,
            });
        if let Some(id) = step_session(&self.visible_session_order(cx), selected, delta > 0) {
            self.navigate_to_session(id, window, cx);
        }
    }

    /// `number` is one based and matches the badges shown beside visible rows.
    pub(crate) fn select_numbered_session(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) || number == 0 {
            return;
        }
        self.sync(window, cx);
        if let Some(id) = self.visible_session_order(cx).get(number - 1).copied() {
            self.navigate_to_session(id, window, cx);
        }
    }

    pub(super) fn waiting_session(&self, cx: &App) -> Option<SessionId> {
        let workspace_ids: HashSet<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter().copied())
            .collect();
        {
            let activity = self.runtime.read(cx).activity.read(cx);
            activity
                .waiting_sessions()
                .iter()
                .copied()
                .find(|id| workspace_ids.contains(id))
                .or_else(|| {
                    self.multi_project_sessions(cx)
                        .into_iter()
                        .map(|meta| meta.id)
                        .chain(
                            self.list_session_groups(cx)
                                .into_iter()
                                .flat_map(|(_, ids)| ids),
                        )
                        .find(|id| {
                            workspace_ids.contains(id)
                                && activity.state_of(*id) == ActivityState::WaitingForInput
                        })
                })
        }
    }

    pub(crate) fn select_waiting_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        self.sync(window, cx);
        let target = self.waiting_session(cx);
        if let Some(id) = target {
            let session = self.runtime.read(cx).registry.read(cx).session(id).cloned();
            if let Some(session) = session {
                session.update(cx, |terminal, _| terminal.snap_to_bottom());
            }
            self.navigate_to_session(id, window, cx);
        }
    }
}
