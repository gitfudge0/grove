//! Tree/project/worktree click handling, session lifecycle (spawn/select/
//! kill), file drops, and the visible-order cycling helpers used by global
//! shortcuts.

use super::shortcuts::grid_focus_after_kill;
use crate::app::{Modal, OnboardStep};
use crate::gui::state::{FocusedPane, Grove, Msg};
use grove_core::agent::Agent;
use grove_core::session::Session;
use iced::Task;
use std::sync::Arc;

impl Grove {
    pub(super) fn on_toggle_collapse_all(&mut self) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.tree_expand = self.tree_expand.next();
        self.apply_tree_expand();
    }

    pub(super) fn on_project_clicked(&mut self, i: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        if self.collapsed.contains(&i) {
            self.collapsed.remove(&i);
        } else {
            self.collapsed.insert(i);
        }
        self.switch_active_project(i);
        self.ensure_wt_cached(i);
    }

    pub(super) fn on_worktree_clicked(&mut self, proj: usize, wt: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.switch_active_project(proj);
        self.app.wt_idx = wt;
        let key = (proj, wt);
        if self.collapsed_wt.contains(&key) {
            self.collapsed_wt.remove(&key);
        } else {
            self.collapsed_wt.insert(key);
        }
        // Keep the workspace in sync: switch to the first session
        // belonging to this worktree, or clear if there are none.
        self.sync_session_to_wt(proj, wt);
    }

    pub(super) fn on_start_session(&mut self, proj: usize, wt: usize, agent: Agent) {
        self.open_agent_menu = None;
        self.spawn(proj, wt, agent);
    }

    pub(super) fn on_start_terminal(&mut self, proj: usize, wt: usize) {
        self.open_agent_menu = None;
        self.spawn(proj, wt, Agent::Terminal);
    }

    pub(in crate::gui) fn on_toggle_term_panel(&mut self) -> Task<Msg> {
        self.open_agent_menu = None;
        // Opening only makes sense with an active session to anchor the
        // panel to a worktree; bail before flipping the flag if there
        // is none.
        if !self.term_panel_open && self.active_wt_path().is_none() {
            return Task::none();
        }
        self.term_panel_open = !self.term_panel_open;
        // Recompute the split dims (resizes the agent + panel shells to
        // their new 65/35 — or full — widths) before spawning anything.
        self.refresh_pty_viewport();
        if self.term_panel_open {
            if let Some(wt) = self.active_wt_path() {
                self.app
                    .ensure_wt_terminal(&wt, self.pty_layout.rows, self.pty_layout.panel_cols);
            }
            // Focusing the just-opened panel is the natural default —
            // that's why the user opened it. Click the agent to switch.
            self.focused_pane = FocusedPane::Panel;
        } else {
            // Panel gone: the only interactive PTY is the agent again.
            self.focused_pane = FocusedPane::Agent;
        }
        self.pty_selection = None;
        Task::none()
    }

    pub(super) fn on_select_home_terminal(&mut self, i: usize) {
        if i < self.app.home_terminals.len() {
            self.app.active_terminal = Some(i);
            self.app.home_terminals[i].resize(self.pty_layout.rows, self.pty_layout.cols);
            self.terminal_focused = true;
            self.pty_selection = None;
            // Focus moved to a terminal: both confirm-to-kill arms are stale.
            self.pending_kill = None;
            self.pending_kill_terminal = None;
            // Symmetry with new/close/restart: don't rely on `resize`
            // happening to dirty the target to surface the right frame.
            self.invalidate_pty_render_cache();
        }
    }

    pub(super) fn on_close_home_terminal(&mut self, i: usize) {
        // Shift any pending confirmation index across the removal so
        // it can't end up pointing at a different terminal (mirrors
        // `KillSession`'s handling of `pending_kill`).
        self.pending_kill_terminal = match self.pending_kill_terminal {
            Some(p) if p == i => None,
            Some(p) if p > i => Some(p - 1),
            other => other,
        };
        self.app.close_home_terminal(i);
        // Nothing left to show on the terminal tab — staying focused there
        // would swallow every keystroke with no PTY to send it to.
        if self.app.active_terminal.is_none() {
            self.leave_terminal_tab();
        }
        self.pty_selection = None;
        self.invalidate_pty_render_cache();
    }

    pub(in crate::gui) fn on_add_worktree(&mut self, proj: usize) -> Task<Msg> {
        self.open_agent_menu = None;
        self.switch_active_project(proj);
        self.app.focus_pane(crate::app::Pane::Worktrees);
        self.app.start_add();
        super::focus(crate::gui::view::modal_input_id())
    }

    pub(in crate::gui) fn on_delete_worktree(&mut self, proj: usize, wt: usize) {
        self.open_agent_menu = None;
        self.switch_active_project(proj);
        self.app.wt_idx = wt;
        self.app.focus_pane(crate::app::Pane::Worktrees);
        self.app.start_delete();
    }

    pub(super) fn on_remove_project(&mut self, proj: usize) {
        self.open_agent_menu = None;
        self.switch_active_project(proj);
        self.app.focus_pane(crate::app::Pane::Projects);
        self.app.open_remove_project_modal(proj);
    }

    pub(super) fn on_run_script(&mut self, proj: usize, wt: usize) {
        self.open_agent_menu = None;
        self.switch_active_project(proj);
        self.app.wt_idx = wt;
        if let Some(w) = self.app.worktrees.get(wt).cloned() {
            let before = self.session_keys();
            if self.grid_view {
                self.app.run_worktree_script(
                    &w.path,
                    self.pty_layout.rows,
                    self.pty_layout.panel_cols,
                );
                if self.app.sessions.len() > before.len() {
                    self.tile_order.push(self.app.sessions.len() - 1);
                    self.persist_grid_order();
                }
                self.refresh_pty_viewport();
            } else {
                self.term_panel_open = true;
                self.refresh_pty_viewport();
                self.app.run_worktree_script(
                    &w.path,
                    self.pty_layout.rows,
                    self.pty_layout.panel_cols,
                );
                self.focused_pane = FocusedPane::Panel;
                self.pty_selection = None;
            }
            self.collapsed_wt.remove(&(proj, wt));
        }
    }

    pub(in crate::gui) fn on_new_wt_terminal(&mut self) {
        if let Some(wt) = self.active_wt_path() {
            self.app
                .new_wt_terminal(&wt, self.pty_layout.rows, self.pty_layout.panel_cols);
            self.pty_selection = None;
            self.invalidate_pty_render_cache();
        }
    }

    pub(super) fn on_select_wt_terminal(&mut self, i: usize) {
        if let Some(wt) = self.active_wt_path() {
            self.app
                .select_wt_terminal(&wt, i, self.pty_layout.rows, self.pty_layout.panel_cols);
            self.pty_selection = None;
            self.invalidate_pty_render_cache();
        }
    }

    pub(super) fn on_close_wt_terminal(&mut self, i: usize) {
        if let Some(wt) = self.active_wt_path() {
            // Evict just this shell's render-cache entry (mirroring
            // KillSession), capturing its key before the session drops.
            if let Some(s) = self.app.wt_terminals.get(&wt).and_then(|v| v.get(i)) {
                let key = Arc::as_ptr(&s.dirty) as usize;
                self.pty_cache.borrow_mut().remove(&key);
            }
            self.app.close_wt_terminal(&wt, i);
            self.pty_selection = None;
        }
    }

    pub(super) fn on_jump_to_waiting_session(&mut self) -> Task<Msg> {
        if let Some(&first) = self.waiting_sessions().first() {
            // Jumping here is meant to show the live prompt that's
            // waiting on the user, not wherever the view happened to
            // be scrolled — unlike a manual mod+j/k switch, which
            // should preserve a scroll position left on purpose.
            if let Some(s) = self.app.sessions.get_mut(first) {
                s.snap_to_bottom();
            }
            self.on_select_session(first);
            return Task::none();
        }
        Task::none()
    }

    pub(in crate::gui) fn on_select_session(&mut self, i: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.attention_open = false;
        if i < self.app.sessions.len() {
            self.app.active_session = Some(i);
            self.sync_grid_focus();
            self.leave_terminal_tab();
            self.acknowledge_session(i);
            self.app.sessions[i].resize(self.pty_layout.rows, self.pty_layout.sess_cols);
            // The panel re-anchors to the new session's worktree; reset
            // focus so a stale agent/panel choice doesn't carry over.
            self.reset_focused_pane();
            // Keep the sidebar worktree highlight in sync with the
            // session that is now visible in the workspace.
            let proj_name = self.app.sessions[i].project.clone();
            let wt_path = self.app.sessions[i].wt_path.clone();
            self.sync_wt_to_session(&proj_name, &wt_path);
        }
    }

    pub(super) fn on_kill_session(&mut self, i: usize) {
        // Shift any pending confirmation index across the removal so
        // it can't end up pointing at a different session.
        self.pending_kill = match self.pending_kill {
            Some(p) if p == i => None,
            Some(p) if p > i => Some(p - 1),
            other => other,
        };
        if i < self.app.sessions.len() {
            let key = Arc::as_ptr(&self.app.sessions[i].dirty) as usize;
            self.pty_cache.borrow_mut().remove(&key);
            {
                let s = &self.app.sessions[i];
                let mins = s.created_at.elapsed().as_secs() / 60;
                crate::telemetry::track(
                    "session_ended",
                    vec![
                        ("agent", s.agent.label().into()),
                        ("duration_min", mins.into()),
                        ("tmux", s.tmux_name().is_some().into()),
                    ],
                );
            }
            self.app.sessions[i].kill();
            self.app.sessions.remove(i);
            if let Some(a) = self.app.active_session {
                if a == i {
                    self.app.active_session = None;
                } else if a > i {
                    self.app.active_session = Some(a - 1);
                }
            }
            if self.grid_view || self.grid_view_before_zen {
                // Capture the killed tile's slot before it's dropped from
                // tile_order, so we can refocus whatever now sits there.
                let killed_pos = self.tile_order.iter().position(|&x| x == i);
                let was_focused = self.grid_focused == Some(i);
                // Remove the killed session index from tile_order; shift
                // higher indices down to match the new sessions array.
                self.tile_order.retain_mut(|si| {
                    if *si == i {
                        return false;
                    }
                    if *si > i {
                        *si -= 1;
                    }
                    true
                });
                self.grid_focused = match self.grid_focused {
                    Some(si) if si == i => None,
                    Some(si) if si > i => Some(si - 1),
                    other => other,
                };
                // If the killed session was the focused tile, focus whatever
                // now occupies its slot instead of leaving nothing focused.
                if was_focused && !self.tile_order.is_empty() {
                    if let Some(pos) = grid_focus_after_kill(killed_pos, self.tile_order.len()) {
                        let si = self.tile_order[pos];
                        self.app.active_session = Some(si);
                        self.set_grid_focus(Some(si));
                        self.acknowledge_session(si);
                    }
                }
                if self.grid_view {
                    if self.tile_order.is_empty() {
                        // Auto-exit grid when all sessions are gone.
                        self.grid_view = false;
                    }
                    self.refresh_pty_viewport();
                }
            }
        }
    }

    pub(super) fn on_file_dropped(&mut self, path: std::path::PathBuf) -> Task<Msg> {
        match &self.app.modal {
            // A folder dropped while the add-project modal is open
            // chooses it (on either step — re-choosing is a cheap undo).
            Modal::AddProject => return self.choose_add_project_folder(path),
            // Same affordance on the onboarding project step.
            Modal::Onboarding {
                step: OnboardStep::Project,
                ..
            } => {
                if path.is_dir() {
                    self.app.onboard_set_path(format!("{}/", path.display()));
                }
            }
            // No modal: paste the path into the focused terminal.
            Modal::None => {
                if let Some(sess) = self.focused_session_mut() {
                    sess.send(crate::gui::drop::dropped_path_text(&path).as_bytes());
                    self.pty_selection = None;
                }
            }
            // Any other modal: ignore — dropped text could land in an
            // unexpected place otherwise.
            _ => {}
        }
        Task::none()
    }

    /// Cycle the focused session in visible order: `tile_order` while the
    /// grid is open, the sessions list otherwise.
    pub(super) fn cycle_session(&mut self, delta: i32) -> Task<Msg> {
        if self.grid_view {
            if self.tile_order.is_empty() {
                return Task::none();
            }
            let cur = self
                .grid_focused
                .and_then(|si| self.tile_order.iter().position(|&x| x == si));
            let pos = match cur {
                Some(p) => crate::app::cycle(p, delta, self.tile_order.len()),
                None if delta > 0 => 0,
                None => self.tile_order.len() - 1,
            };
            let si = self.tile_order[pos];
            self.app.active_session = Some(si);
            self.leave_terminal_tab();
            self.sync_grid_focus();
            self.acknowledge_session(si);
            return Task::none();
        }
        if self.app.sessions.is_empty() {
            return Task::none();
        }
        // Coming back from the terminal tab, the first press just reveals the
        // session that was already active — advancing off a session the user
        // can't see is disorienting.
        if self.terminal_focused {
            if let Some(cur) = self.app.active_session {
                self.on_select_session(cur);
                return Task::none();
            }
            self.leave_terminal_tab();
        }
        let next = match self.app.active_session {
            Some(cur) => crate::app::cycle(cur, delta, self.app.sessions.len()),
            None if delta > 0 => 0,
            None => self.app.sessions.len() - 1,
        };
        // Reuse the SelectSession handler so resize / acknowledge / sidebar
        // sync all apply.
        self.on_select_session(next);
        Task::none()
    }

    /// Select the Nth session in visible order (mod+1..9).
    pub(super) fn select_visible_session(&mut self, n: usize) -> Task<Msg> {
        if self.grid_view {
            if let Some(&si) = self.tile_order.get(n) {
                self.app.active_session = Some(si);
                self.leave_terminal_tab();
                self.sync_grid_focus();
                self.acknowledge_session(si);
            }
            return Task::none();
        }
        // Outside the agent grid, `mod+1..9` follows the sidebar's on-screen
        // tree layout rather than raw session index, so the number the user
        // sees is the session they get.
        if let Some(&si) = self.visible_session_order().get(n) {
            self.on_select_session(si);
        }
        Task::none()
    }

    pub(super) fn spawn(&mut self, proj: usize, wt: usize, agent: Agent) {
        self.switch_active_project(proj);
        self.app.wt_idx = wt;
        let pname = match self.app.store.projects.get(proj) {
            Some(p) => p.name.clone(),
            None => return,
        };
        let Some(w) = self.app.worktrees.get(wt).cloned() else {
            return;
        };
        let label = if w.is_main {
            pname.clone()
        } else {
            crate::app::path_basename(&w.path)
        };
        let args = agent.launch_args(
            self.app.skip_permissions_enabled(),
            self.app.chrome_enabled(),
        );
        let use_tmux = self.app.use_tmux();
        match Session::spawn(
            label,
            pname.clone(),
            w.path.clone(),
            agent,
            &args,
            &w.path,
            use_tmux,
        ) {
            Ok(mut s) => {
                s.resize(self.pty_layout.rows, self.pty_layout.sess_cols);
                self.app.sessions.push(s);
                crate::gui::launcher::push_recent_launch(
                    &mut self.app.store.recent_launches,
                    grove_core::storage::RecentLaunch {
                        project: pname,
                        wt_path: w.path.clone(),
                        agent,
                    },
                );
                grove_core::storage::persist(&self.app.store);
                let open = self.app.sessions.len();
                let native = self
                    .app
                    .sessions
                    .iter()
                    .filter(|s| matches!(s.backend, grove_core::session::SessionBackend::Native))
                    .count();
                crate::telemetry::track(
                    "session_created",
                    vec![
                        ("agent", agent.label().into()),
                        ("tmux", use_tmux.into()),
                        ("open_sessions", (open as u64).into()),
                        ("open_native", (native as u64).into()),
                        ("open_tmux", ((open - native) as u64).into()),
                    ],
                );
                self.app.active_session = Some(self.app.sessions.len() - 1);
                self.leave_terminal_tab();
                // Reveal the freshly spawned session if its worktree was
                // collapsed in the tree.
                self.collapsed_wt.remove(&(proj, wt));
            }
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "spawn_failed".into())]);
                self.app
                    .set_error_toast(format!("failed to start session: {e}"));
            }
        }
    }
}
