//! `Grove` lifecycle: construction, subscriptions, and all `Msg` handling.

use super::keys::key_to_bytes;
use super::metrics::{
    compute_pty_dims, pty_cols_for_fraction, pty_metrics, PTY_ZOOM_DEFAULT, PTY_ZOOM_MAX,
    PTY_ZOOM_MIN, PTY_ZOOM_STEP, TERM_PANEL_PORTION, TERM_PANEL_PORTION_MAX,
    TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_STEP,
};
use super::state::{AbsCell, FocusedPane, Grove, Msg, PtyCell, PtyDrag, PtyPane, SidebarView};
use crate::agent::Agent;
use crate::app::{App, InputKind, Modal, Pane};
use crate::session::Session;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::{event, keyboard, Event, Subscription, Task};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

impl Grove {
    pub fn new() -> Self {
        // Compute initial PTY dimensions from the default window size (1280×800).
        // Corrected on the first `WindowResized` event after startup.
        let window_size = iced::Size::new(1280.0, 800.0);
        let mut app = App::new().expect("init app");
        let ui_zoom = app
            .store
            .ui_zoom
            .unwrap_or(PTY_ZOOM_DEFAULT)
            .clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        let (pty_rows, pty_cols) =
            compute_pty_dims(window_size.width, window_size.height, ui_zoom, true);
        // Resize any sessions discovered from a previous grove run so tmux
        // reports the correct terminal size immediately.
        for s in &mut app.sessions {
            s.resize(pty_rows, pty_cols);
        }
        let mut g = Self {
            app,
            collapsed: Default::default(),
            collapsed_wt: Default::default(),
            wt_cache: Default::default(),
            pty_cache: Default::default(),
            pty_rows,
            pty_cols,
            pty_sess_cols: pty_cols,
            pty_panel_cols: pty_cols,
            ui_zoom,
            window_size,
            open_agent_menu: None,
            pty_selection: None,
            pty_drag: None,
            blink_tick: 0,
            pending_kill: None,
            hovered_wt: None,
            hovered_activity_row: None,
            sidebar_view: SidebarView::Activity,
            activity_no_sessions_expanded: None,
            term_panel_open: false,
            term_panel_portion: TERM_PANEL_PORTION,
            focused_pane: FocusedPane::Agent,
            dir_cache: Default::default(),
            activity: Default::default(),
            window_focused: true,
            last_badge: 0,
        };
        // Prime the per-project worktree cache so `view()` never has to shell
        // out to `git worktree list` (it runs on every 33ms tick).
        let n = g.app.store.projects.len();
        for i in 0..n {
            g.ensure_wt_cached(i);
        }
        // Default tree state: collapse projects/worktrees with no live sessions,
        // matching the "collapse all" toggle's terminal state.
        for pi in 0..n {
            if !g.project_has_sessionful_worktree(pi) {
                g.collapsed.insert(pi);
            }
            for wi in 0..g.worktrees_for_project(pi).len() {
                if !g.worktree_has_sessions(pi, wi) {
                    g.collapsed_wt.insert((pi, wi));
                }
            }
        }
        g
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        let tick = iced::time::every(Duration::from_millis(60)).map(|_| Msg::Tick);
        // Only forward un-captured keys; widgets (search input) handle their own first.
        let keys = event::listen_with(|ev, status, _| {
            if status == event::Status::Captured {
                return None;
            }
            match ev {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    modified_key,
                    modifiers,
                    ..
                }) => Some(Msg::KeyPress(key, modified_key, modifiers)),
                Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(Msg::FileDropped(path))
                }
                Event::Window(iced::window::Event::Focused) => {
                    Some(Msg::WindowFocusChanged(true))
                }
                Event::Window(iced::window::Event::Unfocused) => {
                    Some(Msg::WindowFocusChanged(false))
                }
                _ => None,
            }
        });
        let resize = iced::window::resize_events().map(|(_id, size)| Msg::WindowResized(size));
        Subscription::batch([tick, keys, resize])
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Tick => {
                // Advance the blink counter (~30 Hz at 60 ms tick interval).
                self.blink_tick = self.blink_tick.wrapping_add(1);
                self.tick_drag_autoscroll();
                // Surface results from background jobs (.worktreeinclude
                // generation runs off-thread).
                let bg = self.app.bg_status.lock().ok().and_then(|mut g| g.take());
                if let Some(msg) = bg {
                    self.app.status = msg;
                    self.app.refresh_worktrees();
                }
                // Re-classify session activity every 8th tick (~480ms at 60ms).
                if self.blink_tick.is_multiple_of(8) {
                    self.refresh_activity();
                }
            }
            Msg::WindowFocusChanged(f) => {
                self.window_focused = f;
                // Regaining focus acknowledges the visible session.
                if f {
                    if let Some(i) = self.app.active_session {
                        self.acknowledge_session(i);
                    }
                }
            }
            Msg::SidebarSetView(v) => {
                self.sidebar_view = v;
                if matches!(v, SidebarView::Terminal) {
                    self.app.ensure_home_terminal(self.pty_rows, self.pty_cols);
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::RestartHomeTerminal => {
                self.app
                    .restart_active_terminal(self.pty_rows, self.pty_cols);
                self.invalidate_pty_render_cache();
            }
            Msg::NewHomeTerminal => {
                self.app.new_home_terminal(self.pty_rows, self.pty_cols);
                self.pty_selection = None;
                self.invalidate_pty_render_cache();
            }
            Msg::SelectHomeTerminal(i) => {
                if i < self.app.home_terminals.len() {
                    self.app.active_terminal = Some(i);
                    self.app.home_terminals[i].resize(self.pty_rows, self.pty_cols);
                    self.pty_selection = None;
                    // Symmetry with new/close/restart: don't rely on `resize`
                    // happening to dirty the target to surface the right frame.
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::CloseHomeTerminal(i) => {
                self.app
                    .close_home_terminal(i, self.pty_rows, self.pty_cols);
                self.pty_selection = None;
                self.invalidate_pty_render_cache();
            }
            Msg::ToggleActivityNoSessionsGroup => {
                let cur = self.activity_no_sessions_expanded.unwrap_or(false);
                self.activity_no_sessions_expanded = Some(!cur);
            }
            Msg::WindowResized(size) => {
                self.window_size =
                    iced::Size::new(size.width * self.ui_zoom, size.height * self.ui_zoom);
                self.refresh_pty_viewport();
            }
            Msg::BackendNative => {
                let _ = self.app.set_tmux_enabled(false);
            }
            Msg::BackendTmux => {
                let _ = self.app.set_tmux_enabled(true);
            }
            Msg::ChooseTmux(enabled) => {
                if let Err(e) = self.app.choose_tmux_enabled(enabled) {
                    self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                }
            }
            Msg::AgentPickerSelect(i) => self.agent_picker_select(i),
            Msg::AgentPickerToggleDefault => self.agent_picker_toggle_default(),
            Msg::AgentPickerSubmit => self.submit_agent_picker(),
            Msg::ToggleCollapseAll => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                if self.is_collapsed_to_sessionful_worktrees() {
                    // Expand everything.
                    self.collapsed.clear();
                    self.collapsed_wt.clear();
                } else {
                    // Collapse everything except worktrees that already have
                    // at least one session running in them.
                    self.collapsed.clear();
                    self.collapsed_wt.clear();
                    for pi in 0..self.app.store.projects.len() {
                        let has_sessions = self.project_has_sessionful_worktree(pi);
                        if !has_sessions {
                            self.collapsed.insert(pi);
                        }
                        for wi in 0..self.worktrees_for_project(pi).len() {
                            if !self.worktree_has_sessions(pi, wi) {
                                self.collapsed_wt.insert((pi, wi));
                            }
                        }
                    }
                }
            }
            Msg::ProjectClicked(i) => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                if self.collapsed.contains(&i) {
                    self.collapsed.remove(&i);
                } else {
                    self.collapsed.insert(i);
                }
                self.switch_active_project(i);
                self.ensure_wt_cached(i);
            }
            Msg::WorktreeClicked { proj, wt } => {
                self.open_agent_menu = None;
                self.pending_kill = None;
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
            Msg::HoverWorktree(target) => {
                self.hovered_wt = target;
            }
            Msg::HoverActivityRow(target) => {
                self.hovered_activity_row = target;
            }
            Msg::StartSession { proj, wt, agent } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, agent);
            }
            Msg::StartTerminal { proj, wt } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, Agent::Terminal);
            }
            Msg::ToggleTermPanel => {
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
                            .ensure_wt_terminal(&wt, self.pty_rows, self.pty_panel_cols);
                    }
                    // Focusing the just-opened panel is the natural default —
                    // that's why the user opened it. Click the agent to switch.
                    self.focused_pane = FocusedPane::Panel;
                } else {
                    // Panel gone: the only interactive PTY is the agent again.
                    self.focused_pane = FocusedPane::Agent;
                }
                self.pty_selection = None;
            }
            Msg::NewWtTerminal => {
                if let Some(wt) = self.active_wt_path() {
                    self.app
                        .new_wt_terminal(&wt, self.pty_rows, self.pty_panel_cols);
                    self.pty_selection = None;
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::SelectWtTerminal(i) => {
                if let Some(wt) = self.active_wt_path() {
                    self.app
                        .select_wt_terminal(&wt, i, self.pty_rows, self.pty_panel_cols);
                    self.pty_selection = None;
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::CloseWtTerminal(i) => {
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
            Msg::CloseAgentMenu => {
                self.open_agent_menu = None;
            }
            Msg::SelectSession(i) => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                if i < self.app.sessions.len() {
                    self.app.active_session = Some(i);
                    self.acknowledge_session(i);
                    self.app.sessions[i].resize(self.pty_rows, self.pty_sess_cols);
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
            Msg::RequestKillSession(i) => {
                self.open_agent_menu = None;
                self.pending_kill = Some(i);
            }
            Msg::KillSession(i) => {
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
                    self.app.sessions[i].kill();
                    self.app.sessions.remove(i);
                    if let Some(a) = self.app.active_session {
                        if a == i {
                            self.app.active_session = None;
                        } else if a > i {
                            self.app.active_session = Some(a - 1);
                        }
                    }
                }
            }
            Msg::KeyPress(key, modified_key, mods) => {
                if let Modal::RemoveProject { in_progress, .. } = &self.app.modal {
                    let busy = *in_progress;
                    return self.handle_remove_project_key(key, busy);
                }
                let was_theme_picker = matches!(self.app.modal, Modal::ThemePicker { .. });
                self.handle_key(key, modified_key, mods);
                if was_theme_picker && matches!(self.app.modal, Modal::ThemePicker { .. }) {
                    return self.scroll_theme_picker_to_selection();
                }
            }
            Msg::FileDropped(path) => {
                // Ignored when a modal is up — dropped text could land in an
                // unexpected place otherwise.
                if matches!(self.app.modal, Modal::None) {
                    if let Some(sess) = self.focused_session_mut() {
                        sess.send(super::drop::dropped_path_text(&path).as_bytes());
                        self.pty_selection = None;
                    }
                }
            }
            Msg::PtyMouseDown(pane, x, y) => {
                self.pending_kill = None;
                // Clicking a PTY focuses its pane (so subsequent keystrokes,
                // scroll, and this very selection route there). Honored only
                // while the panel is open; otherwise the agent always owns input.
                self.focus_pane(pane);
                // A focus switch invalidates any in-progress selection on the
                // previously focused PTY — it was anchored to a different grid.
                self.pty_selection = None;
                if let (Some(cell), Some((h, _))) = (self.pixel_to_abs(x, y), self.pty_view_geom())
                {
                    self.pty_selection = Some((cell, cell));
                    self.pty_drag = Some(PtyDrag {
                        last_x: x,
                        last_y: y,
                        view_h_px: h as f32 * pty_metrics(1.0).cell_h,
                    });
                }
            }
            Msg::PtyMouseDrag(pane, x, y) => {
                // Ignore drags from the pane that doesn't own the active
                // selection (the canvas captures the drag, but focus — and thus
                // the geometry helpers — belong to the pane the press landed in).
                if self.focused_input_pane() != pane {
                    return Task::none();
                }
                if let Some(d) = self.pty_drag.as_mut() {
                    d.last_x = x;
                    d.last_y = y;
                }
                if let (Some(cell), Some((a, _))) = (self.pixel_to_abs(x, y), self.pty_selection) {
                    self.pty_selection = Some((a, cell));
                }
            }
            Msg::PtyScroll { pane, up, x, y } => {
                // Scrolling over a PTY focuses it too, so the wheel always
                // drives the terminal under the cursor — but don't hand focus
                // to a panel with no shell: input routed there would fall back
                // to the agent while keystrokes stayed stuck on the panel.
                let panel_has_shell = self
                    .active_wt_path()
                    .is_some_and(|wt| self.app.active_wt_terminal(&wt).is_some());
                if !matches!(pane, PtyPane::Panel) || panel_has_shell {
                    self.focus_pane(pane);
                }
                let cell = pixel_to_cell(x, y);
                if let Some(s) = self.focused_session_mut() {
                    s.scroll(up, cell.col as u16, cell.row as u16);
                }
            }
            Msg::ToggleZen => {
                self.app.chrome_visible = !self.app.chrome_visible;
                self.refresh_pty_viewport();
            }
            Msg::ZoomIn => self.adjust_ui_zoom(PTY_ZOOM_STEP),
            Msg::ZoomOut => self.adjust_ui_zoom(-PTY_ZOOM_STEP),
            Msg::ZoomReset => self.set_ui_zoom(PTY_ZOOM_DEFAULT),
            Msg::PtyMouseUp => {
                self.pty_drag = None;
                if let Some((a, h)) = self.pty_selection {
                    if a == h {
                        self.pty_selection = None;
                    }
                }
            }
            Msg::AddProject => {
                self.open_agent_menu = None;
                self.app.focus_pane(Pane::Projects);
                self.app.start_add();
            }
            Msg::AddWorktree { proj } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.focus_pane(Pane::Worktrees);
                self.app.start_add();
            }
            Msg::DeleteWorktree { proj, wt } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.wt_idx = wt;
                self.app.focus_pane(Pane::Worktrees);
                self.app.start_delete();
            }
            Msg::RemoveProject { proj } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.focus_pane(Pane::Projects);
                self.app.open_remove_project_modal(proj);
            }
            Msg::ToggleRemoveWorktrees(v) => {
                if let Modal::RemoveProject {
                    also_remove_worktrees,
                    in_progress,
                    ..
                } = &mut self.app.modal
                {
                    if !*in_progress {
                        *also_remove_worktrees = v;
                    }
                }
            }
            Msg::ConfirmRemoveProject => {
                return self.kick_off_remove_project();
            }
            Msg::WorktreeRemovedStep {
                path,
                error,
                remaining,
            } => {
                return self.advance_remove_project(path, error, remaining);
            }
            Msg::ModalSubmit => self.submit_modal_input(),
            Msg::ModalCancel => self.cancel_modal(),
            Msg::ModalConfirm(yes) => self.submit_modal_confirm(yes),
            Msg::ModalPickDir(path) => {
                if let Modal::Input {
                    buffer,
                    kind,
                    dir_sel,
                    ..
                } = &mut self.app.modal
                {
                    if matches!(kind, InputKind::AddProjectPath) {
                        *buffer = format!("{path}/");
                        *dir_sel = 0;
                    }
                }
            }
            Msg::OpenThemePicker => {
                self.app.open_theme_picker();
                return self.scroll_theme_picker_to_selection();
            }
            Msg::ThemePickerSwitchTab => {
                self.theme_picker_switch_tab();
                return self.scroll_theme_picker_to_selection();
            }
            Msg::ThemePickerSelect(i) => {
                self.theme_picker_select(i);
                return self.scroll_theme_picker_to_selection();
            }
            Msg::ThemePickerSubmit => self.theme_picker_submit(),
            Msg::ThemePickerCancel => self.theme_picker_cancel(),
        }
        Task::none()
    }

    fn scroll_theme_picker_to_selection(&self) -> Task<Msg> {
        use super::metrics::ROW_H;
        use crate::app::Modal;
        use iced::widget::scrollable::{self, AbsoluteOffset};
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        let sel = match tab {
            crate::theme::ThemeKind::Dark => *sel_dark,
            crate::theme::ThemeKind::Light => *sel_light,
        };
        let total = crate::theme::themes_of(*tab).len();
        let viewport_rows = total.min(12) as f32;
        let viewport_h = viewport_rows * ROW_H;
        let sel_y = sel as f32 * ROW_H;
        // Center the selection in the viewport, clamped to valid range.
        let max_y = (total as f32 * ROW_H - viewport_h).max(0.0);
        let y = (sel_y - (viewport_h - ROW_H) / 2.0).clamp(0.0, max_y);
        scrollable::scroll_to(
            super::view::theme_picker_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    fn theme_picker_select(&mut self, index: usize) {
        use crate::app::Modal;
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &mut self.app.modal
        else {
            return;
        };
        let themes = crate::theme::themes_of(*tab);
        if index >= themes.len() {
            return;
        }
        match tab {
            crate::theme::ThemeKind::Dark => *sel_dark = index,
            crate::theme::ThemeKind::Light => *sel_light = index,
        }
        crate::theme::set(themes[index]);
        self.invalidate_pty_render_cache();
    }

    fn theme_picker_move(&mut self, delta: i32) {
        self.app.theme_picker_move(delta);
        self.invalidate_pty_render_cache();
    }

    fn theme_picker_switch_tab(&mut self) {
        self.app.theme_picker_switch_tab();
        self.invalidate_pty_render_cache();
    }

    fn theme_picker_submit(&mut self) {
        if let Err(e) = self.app.theme_picker_submit() {
            self.app.modal = crate::app::Modal::Message(format!("theme failed: {e}"));
        }
        self.invalidate_pty_render_cache();
    }

    fn theme_picker_cancel(&mut self) {
        self.app.theme_picker_cancel();
        self.invalidate_pty_render_cache();
    }

    /// Recompute every session's `ActivityState` from its live signals.
    /// Runs every ~480ms; also prunes trackers for sessions that no longer
    /// exist and pushes dock badge/bounce updates on transitions.
    fn refresh_activity(&mut self) {
        use super::activity::{classify, ActivityState, Signals};
        let now = std::time::Instant::now();
        let mut live_keys: Vec<usize> = Vec::with_capacity(self.app.sessions.len());
        let mut newly_waiting = false;

        for (i, s) in self.app.sessions.iter().enumerate() {
            let key = Arc::as_ptr(&s.dirty) as usize;
            live_keys.push(key);
            let focused = self.app.active_session == Some(i) && self.window_focused;
            let tracker = self.activity.entry(key).or_default();

            // Consume new bells: pending only when they ring unfocused.
            let bells = s.bell_count();
            if bells > tracker.bell_seen {
                tracker.bell_seen = bells;
                if !focused {
                    tracker.bell_pending = true;
                }
            }

            let alive = matches!(s.status(), crate::session::SessionStatus::Running);
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            let output_age = now.saturating_duration_since(t);
            // Skip the parser lock for sessions that can't need it.
            let tail = if alive { s.tail_contents(15) } else { String::new() };

            let scrolling = s
                .scroll_age()
                .is_some_and(|a| a < super::activity::SCROLL_QUIET);
            let sig = Signals {
                alive,
                output_age,
                bell_pending: tracker.bell_pending,
                was_working: tracker.was_working,
                focused,
                scrolling,
            };
            let new_state = classify(s.agent, &tail, &sig);
            if new_state == ActivityState::Working {
                tracker.was_working = true;
            }
            if !alive {
                tracker.was_working = false;
                tracker.bell_pending = false;
            }
            if focused {
                // Watching it = continuously acknowledged.
                tracker.bell_pending = false;
            }
            if new_state == ActivityState::WaitingForInput
                && tracker.state != ActivityState::WaitingForInput
            {
                newly_waiting = true;
            }
            tracker.state = new_state;
        }

        self.activity.retain(|k, _| live_keys.contains(k));

        // Dock: badge = waiting count; one bounce per enter-while-unfocused.
        let waiting = self
            .activity
            .values()
            .filter(|t| t.state == ActivityState::WaitingForInput)
            .count();
        if waiting != self.last_badge {
            super::dock::set_badge(waiting);
            self.last_badge = waiting;
        }
        if newly_waiting && !self.window_focused {
            super::dock::request_attention();
        }
    }

    /// Acknowledge the given session's tracker (user focused it).
    fn acknowledge_session(&mut self, i: usize) {
        if let Some(s) = self.app.sessions.get(i) {
            let key = Arc::as_ptr(&s.dirty) as usize;
            if let Some(t) = self.activity.get_mut(&key) {
                t.acknowledge();
            }
        }
    }

    /// Read-only state lookup for the view layer. Unknown sessions render
    /// Idle until the first classification tick.
    pub(super) fn activity_state(&self, s: &Session) -> super::activity::ActivityState {
        let key = Arc::as_ptr(&s.dirty) as usize;
        self.activity
            .get(&key)
            .map(|t| t.state)
            .unwrap_or(super::activity::ActivityState::Idle)
    }

    fn invalidate_pty_render_cache(&mut self) {
        self.pty_cache.borrow_mut().clear();
        for s in &self.app.sessions {
            s.dirty.store(true, Ordering::Relaxed);
        }
        for s in &self.app.home_terminals {
            s.dirty.store(true, Ordering::Relaxed);
        }
        for v in self.app.wt_terminals.values() {
            for s in v {
                s.dirty.store(true, Ordering::Relaxed);
            }
        }
    }

    fn refresh_pty_viewport(&mut self) {
        let (rows, cols) = compute_pty_dims(
            self.window_size.width,
            self.window_size.height,
            self.ui_zoom,
            self.app.chrome_visible,
        );
        self.pty_rows = rows;
        self.pty_cols = cols;
        // When the slide-over panel is open the workspace splits 65/35, so the
        // agent PTY and the panel PTY each see a narrower width than the full
        // workspace. Compute both so every shell wraps at its rendered width.
        let (sess_cols, panel_cols) = if self.term_panel_open {
            let panel = self.term_panel_portion as f32 / 100.0;
            (
                pty_cols_for_fraction(
                    self.window_size.width,
                    self.ui_zoom,
                    self.app.chrome_visible,
                    1.0 - panel,
                ),
                pty_cols_for_fraction(
                    self.window_size.width,
                    self.ui_zoom,
                    self.app.chrome_visible,
                    panel,
                ),
            )
        } else {
            (cols, cols)
        };
        self.pty_sess_cols = sess_cols;
        self.pty_panel_cols = panel_cols;
        for s in &mut self.app.sessions {
            s.resize(rows, sess_cols);
        }
        // Home terminals live on their own full-width tab, never beside the panel.
        for s in &mut self.app.home_terminals {
            s.resize(rows, cols);
        }
        for v in self.app.wt_terminals.values_mut() {
            for s in v {
                s.resize(rows, panel_cols);
            }
        }
        self.invalidate_pty_render_cache();
    }

    fn adjust_ui_zoom(&mut self, delta: f32) {
        self.set_ui_zoom(self.ui_zoom + delta);
    }

    fn set_ui_zoom(&mut self, zoom: f32) {
        let clamped = zoom.clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        let snapped = ((clamped * 10.0).round() / 10.0).clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        if (snapped - self.ui_zoom).abs() < f32::EPSILON {
            return;
        }
        self.ui_zoom = snapped;
        self.refresh_pty_viewport();
        self.app.store.ui_zoom = Some(snapped);
        let _ = crate::storage::save(&self.app.store);
    }

    fn agent_picker_select(&mut self, index: usize) {
        if index >= self.app.available_agents.len() {
            return;
        }
        if let Modal::AgentPicker { sel, .. } = &mut self.app.modal {
            *sel = index;
        }
    }

    fn agent_picker_toggle_default(&mut self) {
        if let Err(e) = self.app.picker_toggle_default() {
            self.app.modal = Modal::Message(format!("default agent failed: {e}"));
        }
    }

    fn submit_agent_picker(&mut self) {
        let before = self.session_keys();
        self.app.picker_submit();
        self.resize_new_sessions(&before);
        self.rebuild_wt_cache();
    }

    fn handle_key(&mut self, key: Key, modified_key: Key, mods: Modifiers) {
        if !matches!(self.app.modal, Modal::None) {
            self.handle_modal_key(key, modified_key, mods);
            return;
        }
        // Shortcuts match the modifier-independent `key`: on Linux a Ctrl
        // combo turns `modified_key` into a control char (e.g. Ctrl+V -> \u16).
        // Copy PTY selection with the OS copy shortcut.
        // macOS: Cmd+C  |  others: Ctrl+Shift+C
        if let Key::Character(s) = &key {
            if is_copy_shortcut(mods, s) {
                if let Some(text) = self.selection_text() {
                    crate::clipboard::copy(&text);
                }
                return;
            }
            if is_paste_shortcut(mods, s) {
                if let Some(text) = crate::clipboard::paste() {
                    if let Some(sess) = self.focused_session_mut() {
                        let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
                        let mut bytes = Vec::with_capacity(normalized.len() + 12);
                        bytes.extend_from_slice(b"\x1b[200~");
                        bytes.extend_from_slice(normalized.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                        sess.send(&bytes);
                    }
                }
                self.pty_selection = None;
                return;
            }
        }
        // Resize the terminal panel with Ctrl+Shift+Left/Right while it is open.
        // Intercepted before `key_to_bytes` so the arrows don't reach the PTY.
        if self.term_panel_open && mods.control() && mods.shift() {
            match key {
                Key::Named(Named::ArrowRight) => {
                    self.adjust_term_panel_portion(TERM_PANEL_PORTION_STEP as i16);
                    return;
                }
                Key::Named(Named::ArrowLeft) => {
                    self.adjust_term_panel_portion(-(TERM_PANEL_PORTION_STEP as i16));
                    return;
                }
                _ => {}
            }
        }
        // Feed the PTY the modifier-independent `key` for Ctrl combos (so the
        // control-byte math sees the base letter), and `modified_key` otherwise
        // so Shift/AltGr text is preserved.
        let pty_key = if mods.control() { &key } else { &modified_key };
        if let Some(bytes) = key_to_bytes(pty_key, mods) {
            if let Some(s) = self.focused_session_mut() {
                s.send(&bytes);
            }
            self.pty_selection = None;
        }
    }

    /// Grow (`delta > 0`) or shrink the terminal panel by `delta` percent of the
    /// workspace, clamped to `[TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX]`,
    /// then reflow every PTY to its new width.
    fn adjust_term_panel_portion(&mut self, delta: i16) {
        let next = (self.term_panel_portion as i16 + delta)
            .clamp(TERM_PANEL_PORTION_MIN as i16, TERM_PANEL_PORTION_MAX as i16)
            as u16;
        if next == self.term_panel_portion {
            return;
        }
        self.term_panel_portion = next;
        self.refresh_pty_viewport();
    }

    /// Keyboard handling for the remove-project modal, mirroring the plain
    /// confirm modal (Esc/n cancel, Enter/y confirm) plus Space to toggle the
    /// delete-worktrees checkbox. Ignored while removal is in flight.
    fn handle_remove_project_key(&mut self, key: Key, busy: bool) -> Task<Msg> {
        if busy {
            return Task::none();
        }
        match key {
            Key::Named(Named::Escape) => self.cancel_modal(),
            Key::Named(Named::Enter) => return self.kick_off_remove_project(),
            Key::Named(Named::Space) => {
                if let Modal::RemoveProject {
                    also_remove_worktrees,
                    ..
                } = &mut self.app.modal
                {
                    *also_remove_worktrees = !*also_remove_worktrees;
                }
            }
            Key::Character(s) => match s.as_str() {
                "y" | "Y" => return self.kick_off_remove_project(),
                "n" | "N" => self.cancel_modal(),
                _ => {}
            },
            _ => {}
        }
        Task::none()
    }

    fn handle_modal_key(&mut self, key: Key, modified_key: Key, mods: Modifiers) {
        match &self.app.modal {
            Modal::Input { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_modal_input(),
                Key::Named(Named::ArrowDown) => self.app.input_dir_move(1),
                Key::Named(Named::ArrowUp) => self.app.input_dir_move(-1),
                Key::Named(Named::Tab) | Key::Named(Named::ArrowRight) => self.app.input_dir_pick(),
                Key::Named(Named::Backspace) => self.app.input_buffer_edit(|b| {
                    b.pop();
                }),
                Key::Named(Named::Space) if !mods.control() && !mods.alt() => {
                    self.app.input_buffer_edit(|b| b.push(' '));
                }
                Key::Character(s) => {
                    if mods.control() {
                        match s.as_str() {
                            "u" | "U" => self.app.input_buffer_edit(|b| b.clear()),
                            "c" | "C" => self.cancel_modal(),
                            _ => {}
                        }
                    } else if is_paste_shortcut(mods, &s) {
                        if let Some(text) = crate::clipboard::paste() {
                            self.app.input_buffer_edit(|b| b.push_str(&text));
                        }
                    } else if !mods.alt() {
                        // Insert the `modified_key` text so Shift/AltGr produce
                        // the right glyph; fall back to the base key.
                        let text = match &modified_key {
                            Key::Character(m) => m.clone(),
                            _ => s,
                        };
                        self.app.input_buffer_edit(|b| b.push_str(&text));
                    }
                }
                _ => {}
            },
            Modal::Confirm { .. } => match key {
                Key::Named(Named::Escape) => self.submit_modal_confirm(false),
                Key::Named(Named::Enter) => self.submit_modal_confirm(true),
                Key::Character(s) => match s.as_str() {
                    "y" | "Y" => self.submit_modal_confirm(true),
                    "n" | "N" => self.submit_modal_confirm(false),
                    _ => {}
                },
                _ => {}
            },
            Modal::Message(_) => match key {
                Key::Named(Named::Escape) | Key::Named(Named::Enter) => self.cancel_modal(),
                Key::Character(s) if matches!(s.as_str(), "q" | "Q") => self.cancel_modal(),
                _ => {}
            },
            Modal::ThemePicker { .. } => match key {
                Key::Named(Named::Escape) => self.theme_picker_cancel(),
                Key::Named(Named::Enter) => self.theme_picker_submit(),
                Key::Named(Named::ArrowDown) => self.theme_picker_move(1),
                Key::Named(Named::ArrowUp) => self.theme_picker_move(-1),
                Key::Named(Named::Tab) => self.theme_picker_switch_tab(),
                Key::Character(s) => match s.as_str() {
                    "j" | "J" => self.theme_picker_move(1),
                    "k" | "K" => self.theme_picker_move(-1),
                    "h" | "H" | "l" | "L" => self.theme_picker_switch_tab(),
                    _ => {}
                },
                _ => {}
            },
            Modal::AgentPicker { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_agent_picker(),
                Key::Named(Named::ArrowDown) => self.app.picker_move(1),
                Key::Named(Named::ArrowUp) => self.app.picker_move(-1),
                Key::Named(Named::Space) => self.agent_picker_toggle_default(),
                Key::Character(s) => match s.as_str() {
                    "j" | "J" => self.app.picker_move(1),
                    "k" | "K" => self.app.picker_move(-1),
                    _ => {}
                },
                _ => {}
            },
            Modal::TmuxChoice => match key {
                Key::Named(Named::Enter) => self.choose_tmux(true),
                Key::Named(Named::Escape) => self.choose_tmux(false),
                Key::Character(s) => match s.as_str() {
                    "t" | "T" | "y" | "Y" => self.choose_tmux(true),
                    "n" | "N" => self.choose_tmux(false),
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

    fn choose_tmux(&mut self, enabled: bool) {
        if let Err(e) = self.app.choose_tmux_enabled(enabled) {
            self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
        }
    }

    fn submit_modal_input(&mut self) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_input() {
            self.app.modal = Modal::Message(format!("input failed: {e}"));
        }
        self.resize_new_sessions(&before);
        self.rebuild_wt_cache();
    }

    fn submit_modal_confirm(&mut self, yes: bool) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_confirm(yes) {
            self.app.modal = Modal::Message(format!("action failed: {e}"));
        }
        self.resize_new_sessions(&before);
        self.rebuild_wt_cache();
    }

    /// Resize any sessions spawned during this update to the current PTY
    /// viewport. Sessions created indirectly (e.g. auto-spawned when a new
    /// worktree is added) otherwise stay at the 80x24 PTY default and don't
    /// fill the workspace width.
    fn resize_new_sessions(&mut self, before: &[usize]) {
        for s in &mut self.app.sessions {
            let key = Arc::as_ptr(&s.dirty) as usize;
            if !before.contains(&key) {
                s.resize(self.pty_rows, self.pty_sess_cols);
            }
        }
    }

    fn session_keys(&self) -> Vec<usize> {
        self.app
            .sessions
            .iter()
            .map(|s| Arc::as_ptr(&s.dirty) as usize)
            .collect()
    }

    fn cancel_modal(&mut self) {
        self.app.modal = Modal::None;
    }

    /// Begin executing a confirmed remove-project action. If the user opted
    /// to delete worktrees on disk, kick off the recursive teardown task;
    /// otherwise finalize inline and close the modal.
    fn kick_off_remove_project(&mut self) -> Task<Msg> {
        let (idx, also, project_path, mut queue) = match &self.app.modal {
            Modal::RemoveProject {
                idx,
                also_remove_worktrees,
                project_path,
                worktrees,
                in_progress,
                ..
            } if !*in_progress => (
                *idx,
                *also_remove_worktrees,
                project_path.clone(),
                worktrees.clone(),
            ),
            _ => return Task::none(),
        };

        if !also || queue.is_empty() {
            match self.app.finalize_remove_project(idx) {
                Ok(msg) if !msg.is_empty() => self.app.status = msg,
                Err(e) => self.app.status = format!("err: {e}"),
                _ => {}
            }
            self.app.modal = Modal::None;
            self.rebuild_wt_cache();
            return Task::none();
        }

        // Kill any sessions tied to these worktrees up front so the
        // PTY handles are released before `git worktree remove --force`
        // touches the filesystem.
        for wt in &queue {
            self.app.kill_sessions_for_wt(wt);
        }

        if let Modal::RemoveProject {
            in_progress,
            done,
            current,
            errors,
            ..
        } = &mut self.app.modal
        {
            *in_progress = true;
            *done = 0;
            *errors = Vec::new();
            *current = queue.first().cloned().unwrap_or_default();
        }

        let first = queue.remove(0);
        remove_worktree_task(project_path, first, queue)
    }

    /// Process the result of one worktree removal and either dispatch the
    /// next one or finalize the project removal when the queue is empty.
    fn advance_remove_project(
        &mut self,
        path: String,
        error: Option<String>,
        remaining: Vec<String>,
    ) -> Task<Msg> {
        let (idx, project_path) = match &mut self.app.modal {
            Modal::RemoveProject {
                idx,
                project_path,
                done,
                current,
                errors,
                ..
            } => {
                *done += 1;
                if let Some(e) = error {
                    errors.push(format!("{path}: {e}"));
                }
                *current = remaining.first().cloned().unwrap_or_default();
                (*idx, project_path.clone())
            }
            _ => return Task::none(),
        };

        if let Some(next) = remaining.first().cloned() {
            let rest: Vec<String> = remaining.into_iter().skip(1).collect();
            return remove_worktree_task(project_path, next, rest);
        }

        // Done — finalize.
        let errors = match &self.app.modal {
            Modal::RemoveProject { errors, .. } => errors.clone(),
            _ => Vec::new(),
        };
        match self.app.finalize_remove_project(idx) {
            Ok(msg) if !msg.is_empty() => self.app.status = msg,
            Err(e) => self.app.status = format!("err: {e}"),
            _ => {}
        }
        if !errors.is_empty() {
            self.app.status = format!("{} ({} worktree errors)", self.app.status, errors.len());
        }
        self.app.modal = Modal::None;
        self.rebuild_wt_cache();
        Task::none()
    }

    /// Switch the active project, saving the outgoing project's worktrees
    /// into `wt_cache` first so `tree_view` can still render its children
    /// while a different project is active.
    pub(super) fn switch_active_project(&mut self, new_proj: usize) {
        if self.app.proj_idx != new_proj {
            let old = self.app.proj_idx;
            let wts = self.app.worktrees.clone();
            self.wt_cache.insert(old, wts);
            self.app.proj_idx = new_proj;
            self.app.refresh_worktrees();
            self.wt_cache.remove(&new_proj);
        }
    }

    /// Sync the worktree highlight (`proj_idx` / `wt_idx`) to whichever
    /// project+worktree owns the given session path.  Called after
    /// `active_session` changes so the sidebar cyan-rail always agrees with
    /// what is displayed in the workspace.
    fn sync_wt_to_session(&mut self, proj_name: &str, wt_path: &str) {
        let proj_idx = self
            .app
            .store
            .projects
            .iter()
            .position(|p| p.name == proj_name);
        if let Some(pi) = proj_idx {
            self.switch_active_project(pi);
            if let Some(wi) = self.app.worktrees.iter().position(|w| w.path == wt_path) {
                self.app.wt_idx = wi;
            }
        }
    }

    /// Sync `active_session` to the first session that lives inside the
    /// worktree identified by `(proj, wt)`.  If the currently-active session
    /// already belongs to that worktree it is left unchanged; if there are no
    /// sessions in that worktree `active_session` is cleared to `None`.
    /// Called after `wt_idx` changes so the workspace stays in sync with the
    /// sidebar highlight.
    fn sync_session_to_wt(&mut self, proj: usize, wt: usize) {
        // Grab the worktree path without holding a borrow into self.
        let wt_path = self
            .worktrees_for_project(proj)
            .get(wt)
            .map(|w| w.path.clone());
        let Some(path) = wt_path else { return };

        // If the active session is already in this worktree, do nothing.
        let already_here = self
            .app
            .active_session
            .and_then(|i| self.app.sessions.get(i))
            .map(|s| s.wt_path == path)
            .unwrap_or(false);
        if already_here {
            return;
        }

        self.app.active_session = self.app.sessions.iter().position(|s| s.wt_path == path);
    }

    fn worktrees_for_project(&self, proj: usize) -> &[crate::git::Worktree] {
        if proj == self.app.proj_idx {
            &self.app.worktrees
        } else {
            self.wt_cache
                .get(&proj)
                .map(|v| v.as_slice())
                .unwrap_or(&[])
        }
    }

    fn worktree_has_sessions(&self, proj: usize, wt: usize) -> bool {
        let Some(worktree) = self.worktrees_for_project(proj).get(wt) else {
            return false;
        };
        self.app.sessions.iter().any(|s| s.wt_path == worktree.path)
    }

    fn project_has_sessionful_worktree(&self, proj: usize) -> bool {
        self.worktrees_for_project(proj)
            .iter()
            .any(|w| self.app.sessions.iter().any(|s| s.wt_path == w.path))
    }

    /// True when every project without sessionful worktrees is collapsed, and
    /// every worktree without sessions is collapsed. Drives the sidebar's
    /// expand/collapse toggle icon.
    pub(super) fn is_collapsed_to_sessionful_worktrees(&self) -> bool {
        let n_proj = self.app.store.projects.len();
        if n_proj == 0 {
            return false;
        }
        for pi in 0..n_proj {
            let should_be_collapsed = !self.project_has_sessionful_worktree(pi);
            if should_be_collapsed && !self.collapsed.contains(&pi) {
                return false;
            }
            if !should_be_collapsed && self.collapsed.contains(&pi) {
                return false;
            }
            for (wi, _) in self.worktrees_for_project(pi).iter().enumerate() {
                let should_be_collapsed = !self.worktree_has_sessions(pi, wi);
                let is_collapsed = self.collapsed_wt.contains(&(pi, wi));
                if should_be_collapsed != is_collapsed {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn ensure_wt_cached(&mut self, proj: usize) {
        if proj == self.app.proj_idx || self.wt_cache.contains_key(&proj) {
            return;
        }
        if let Some(p) = self.app.store.projects.get(proj) {
            let wts = crate::git::list_worktrees(&p.path);
            self.wt_cache.insert(proj, wts);
        }
    }

    fn rebuild_wt_cache(&mut self) {
        self.wt_cache.clear();
        let n = self.app.store.projects.len();
        if self.app.proj_idx >= n {
            self.app.proj_idx = n.saturating_sub(1);
        }
        self.app.refresh_worktrees();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
    }

    fn spawn(&mut self, proj: usize, wt: usize, agent: Agent) {
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
        let args = agent.launch_args();
        let use_tmux = self.app.use_tmux();
        match Session::spawn(
            label,
            pname,
            w.path.clone(),
            agent,
            &args,
            &w.path,
            use_tmux,
        ) {
            Ok(mut s) => {
                s.resize(self.pty_rows, self.pty_sess_cols);
                self.app.sessions.push(s);
                self.app.active_session = Some(self.app.sessions.len() - 1);
                // Reveal the freshly spawned session if its worktree was
                // collapsed in the tree.
                self.collapsed_wt.remove(&(proj, wt));
            }
            Err(e) => {
                self.app.status = format!("failed to start session: {e}");
            }
        }
    }

    /// Extract text inside the current PTY selection. The selection is stored
    /// in scrollback-stable absolute rows, so this may span content that is not
    /// currently visible — extraction walks the session's scrollback to read it.
    /// Whether the persistent home-terminal tab is the active sidebar view.
    pub(super) fn terminal_tab(&self) -> bool {
        matches!(self.sidebar_view, SidebarView::Terminal)
    }

    /// Reset the input-focus target after the active session (and hence the
    /// panel's worktree) changes: focus the panel when it's open (the just
    /// re-anchored terminal), otherwise the agent.
    fn reset_focused_pane(&mut self) {
        self.focused_pane = if self.term_panel_open {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
    }

    /// Whether input currently routes to the panel PTY. Only true while the
    /// panel is open *and* the panel pane holds focus.
    fn panel_focused(&self) -> bool {
        matches!(self.focused_input_pane(), PtyPane::Panel)
    }

    /// Apply a click/scroll's origin pane to the input-focus target. A `Panel`
    /// click only takes effect while the panel is open; an `Agent` click always
    /// returns focus to the agent (it's only reachable as a click target when
    /// the split is showing both PTYs).
    fn focus_pane(&mut self, pane: PtyPane) {
        if !self.term_panel_open {
            return;
        }
        self.focused_pane = match pane {
            PtyPane::Agent => FocusedPane::Agent,
            PtyPane::Panel => FocusedPane::Panel,
        };
    }

    /// The session the workspace PTY is currently showing — and that keystrokes,
    /// scrolling, and selection target. The home terminal when the terminal tab
    /// is active, otherwise the active worktree session.
    pub(super) fn focused_session(&self) -> Option<&Session> {
        if self.terminal_tab() {
            self.app.active_home_terminal()
        } else if self.panel_focused() {
            // Panel terminal when this worktree has one; otherwise fall back to
            // the agent so a worktree with no shell doesn't silently swallow
            // keystrokes.
            self.active_wt_path()
                .and_then(|wt| self.app.active_wt_terminal(&wt))
                .or_else(|| {
                    self.app
                        .active_session
                        .and_then(|i| self.app.sessions.get(i))
                })
        } else {
            self.app
                .active_session
                .and_then(|i| self.app.sessions.get(i))
        }
    }

    pub(super) fn focused_session_mut(&mut self) -> Option<&mut Session> {
        if self.terminal_tab() {
            self.app
                .active_terminal
                .and_then(move |i| self.app.home_terminals.get_mut(i))
        } else if self.panel_focused() {
            if let Some(wt) = self.active_wt_path() {
                if let Some(idx) = self.app.active_wt_terminal_idx(&wt) {
                    return self
                        .app
                        .wt_terminals
                        .get_mut(&wt)
                        .and_then(|v| v.get_mut(idx));
                }
            }
            // No panel shell for this worktree — route to the agent instead.
            self.app
                .active_session
                .and_then(move |i| self.app.sessions.get_mut(i))
        } else {
            self.app
                .active_session
                .and_then(move |i| self.app.sessions.get_mut(i))
        }
    }

    /// Absolute worktree path of the active session — the scope of the terminal
    /// slide-over panel. `None` when no session is active.
    pub(super) fn active_wt_path(&self) -> Option<String> {
        self.app
            .active_session
            .and_then(|i| self.app.sessions.get(i))
            .map(|s| s.wt_path.clone())
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let (a, h) = self.pty_selection?;
        let s = self.focused_session()?;
        s.selection_text_abs((a.a_row, a.col), (h.a_row, h.col))
    }

    /// Visible grid height (rows) and current scrollback offset of the focused
    /// session, used to convert between viewport and absolute selection rows.
    fn pty_view_geom(&self) -> Option<(usize, usize)> {
        let s = self.focused_session()?;
        let p = s.parser.lock().ok()?;
        let (h, _) = p.screen().size();
        Some((h as usize, p.screen().scrollback()))
    }

    /// Convert unzoomed canvas pixels to an absolute selection cell, clamping
    /// the row into the currently-visible window `[S, S + h - 1]`.
    fn pixel_to_abs(&self, x: f32, y: f32) -> Option<AbsCell> {
        let (h, sb) = self.pty_view_geom()?;
        if h == 0 {
            return None;
        }
        let m = pty_metrics(1.0);
        let r = ((y / m.cell_h).max(0.0) as usize).min(h - 1);
        let col = (x / m.cell_w).max(0.0) as usize;
        Some(AbsCell {
            a_row: sb + (h - 1 - r),
            col,
        })
    }

    /// Called each `Msg::Tick`. While a selection drag is held with the cursor
    /// in the top/bottom edge zone, scroll grove's scrollback one step in that
    /// direction and extend the selection head over the revealed line.
    fn tick_drag_autoscroll(&mut self) {
        let Some(d) = self.pty_drag else { return };
        let margin = pty_metrics(1.0).cell_h;
        let up = if d.last_y <= margin {
            true
        } else if d.last_y >= d.view_h_px - margin {
            false
        } else {
            return;
        };
        // Drive grove's own scrollback (no-op if the inner app grabs the mouse).
        let before = self.pty_view_geom().map(|(_, s)| s);
        if let Some(s) = self.focused_session_mut() {
            s.scroll(up, 0, 0);
        }
        // Only extend if the scroll actually moved the view.
        if self.pty_view_geom().map(|(_, s)| s) == before {
            return;
        }
        if let (Some(cell), Some((anchor, _))) =
            (self.pixel_to_abs(d.last_x, d.last_y), self.pty_selection)
        {
            self.pty_selection = Some((anchor, cell));
        }
    }
}

/// Spawn a tokio blocking task that runs `git worktree remove --force` for
/// `path` inside `project_path`, then emits `Msg::WorktreeRemovedStep` with
/// the outcome and the still-unprocessed `remaining` queue.
fn remove_worktree_task(project_path: String, path: String, remaining: Vec<String>) -> Task<Msg> {
    Task::perform(
        async move {
            // `git worktree remove` is a short subprocess; run it inline on
            // the iced/tokio executor. The UI thread keeps rendering.
            let res = crate::git::remove_worktree(&project_path, &path);
            (path, res, remaining)
        },
        |(path, res, remaining)| Msg::WorktreeRemovedStep {
            path,
            error: res.err().map(|e| e.to_string()),
            remaining,
        },
    )
}

fn pixel_to_cell(x: f32, y: f32) -> PtyCell {
    let metrics = pty_metrics(1.0);
    PtyCell {
        row: (y / metrics.cell_h).max(0.0) as usize,
        col: (x / metrics.cell_w).max(0.0) as usize,
    }
}

/// Returns true when the key event matches the OS copy shortcut.
/// macOS: Cmd+C (logo, no ctrl, no shift)
/// Others: Ctrl+Shift+C
fn is_copy_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("c") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

/// Returns true when the key event matches the OS paste shortcut.
/// macOS: Cmd+V (logo, no ctrl)
/// Others: Ctrl+V (no shift)
fn is_paste_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("v") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && !mods.shift();
}
