//! `Grove` lifecycle: construction, subscriptions, and all `Msg` handling.

use super::keys::key_to_bytes;
use super::metrics::{
    compute_pty_dims, pty_metrics, PTY_ZOOM_DEFAULT, PTY_ZOOM_MAX, PTY_ZOOM_MIN, PTY_ZOOM_STEP,
};
use super::pty::normalize_selection;
use super::state::{Grove, Msg, PtyCell};
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
        let ui_zoom = PTY_ZOOM_DEFAULT;
        let (pty_rows, pty_cols) =
            compute_pty_dims(window_size.width, window_size.height, ui_zoom, true);
        let mut app = App::new().expect("init app");
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
            ui_zoom,
            window_size,
            open_agent_menu: None,
            pty_selection: None,
            blink_tick: 0,
            pending_kill: None,
            hovered_wt: None,
        };
        // Prime the per-project worktree cache so `view()` never has to shell
        // out to `git worktree list` (it runs on every 33ms tick).
        let n = g.app.store.projects.len();
        for i in 0..n {
            g.ensure_wt_cached(i);
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
                    modified_key,
                    modifiers,
                    ..
                }) => Some(Msg::KeyPress(modified_key, modifiers)),
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
                // `dirty` flags are consumed lazily by `pty()` when
                // it rebuilds a session's cached snapshot.
            }
            Msg::WindowResized(size) => {
                self.window_size = size;
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
            }
            Msg::HoverWorktree(target) => {
                self.hovered_wt = target;
            }
            Msg::StartSession { proj, wt, agent } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, agent);
            }
            Msg::StartTerminal { proj, wt } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, Agent::Terminal);
            }
            Msg::CloseAgentMenu => {
                self.open_agent_menu = None;
            }
            Msg::SelectSession(i) => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                if i < self.app.sessions.len() {
                    self.app.active_session = Some(i);
                    self.app.sessions[i].resize(self.pty_rows, self.pty_cols);
                }
            }
            Msg::RequestKillSession(i) => {
                self.open_agent_menu = None;
                self.pending_kill = Some(i);
            }
            Msg::KillSession(i) => {
                self.pending_kill = None;
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
            Msg::KeyPress(key, mods) => {
                let was_theme_picker = matches!(self.app.modal, Modal::ThemePicker { .. });
                self.handle_key(key, mods);
                if was_theme_picker && matches!(self.app.modal, Modal::ThemePicker { .. }) {
                    return self.scroll_theme_picker_to_selection();
                }
            }
            Msg::PtyMouseDown(x, y) => {
                self.pending_kill = None;
                let cell = pixel_to_cell(x, y);
                self.pty_selection = Some((cell, cell));
            }
            Msg::PtyMouseDrag(x, y) => {
                let cell = pixel_to_cell(x, y);
                if let Some((a, _)) = self.pty_selection {
                    self.pty_selection = Some((a, cell));
                }
            }
            Msg::PtyScroll { up, x, y } => {
                let cell = pixel_to_cell(x, y);
                if let Some(s) = self.app.active_session_mut() {
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

    fn invalidate_pty_render_cache(&mut self) {
        self.pty_cache.borrow_mut().clear();
        for s in &self.app.sessions {
            s.dirty.store(true, Ordering::Relaxed);
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
        for s in &mut self.app.sessions {
            s.resize(rows, cols);
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
    }

    fn agent_picker_select(&mut self, index: usize) {
        if index >= Agent::ALL.len() {
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

    fn handle_key(&mut self, key: Key, mods: Modifiers) {
        if !matches!(self.app.modal, Modal::None) {
            self.handle_modal_key(key, mods);
            return;
        }
        if mods.control() {
            if let Key::Character(s) = &key {
                let ctrl_shift_g = s == "G" || (mods.shift() && s.eq_ignore_ascii_case("g"));
                if ctrl_shift_g {
                    self.app.chrome_visible = !self.app.chrome_visible;
                    self.refresh_pty_viewport();
                    return;
                }
                if s.eq_ignore_ascii_case("g") && !mods.shift() && !mods.alt() {
                    self.app.chrome_visible = true;
                    self.refresh_pty_viewport();
                    return;
                }
            }
        }
        // Ctrl+Shift+C copies the current PTY selection (if any) and does
        // NOT forward to the agent — standard terminal copy shortcut.
        if mods.control() && mods.shift() {
            if let Key::Character(s) = &key {
                if s.eq_ignore_ascii_case("c") {
                    if let Some(text) = self.selection_text() {
                        crate::clipboard::copy(&text);
                    }
                    return;
                }
            }
        }
        if mods.control() && !mods.shift() && !mods.alt() {
            if let Key::Character(s) = &key {
                if s.eq_ignore_ascii_case("t") {
                    self.app.open_theme_picker();
                    return;
                }
            }
        }
        if let Some(bytes) = key_to_bytes(&key, mods) {
            if let Some(i) = self.app.active_session {
                if let Some(s) = self.app.sessions.get_mut(i) {
                    s.send(&bytes);
                }
            }
            self.pty_selection = None;
        }
    }

    fn handle_modal_key(&mut self, key: Key, mods: Modifiers) {
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
                    } else if !mods.alt() {
                        self.app.input_buffer_edit(|b| b.push_str(&s));
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
                s.resize(self.pty_rows, self.pty_cols);
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
                s.resize(self.pty_rows, self.pty_cols);
                self.app.sessions.push(s);
                self.app.active_session = Some(self.app.sessions.len() - 1);
            }
            Err(e) => {
                self.app.status = format!("failed to start session: {e}");
            }
        }
    }

    /// Extract text inside the current PTY selection from the cached styled
    /// rows of the active session.
    pub(super) fn selection_text(&self) -> Option<String> {
        let (a, h) = self.pty_selection?;
        let i = self.app.active_session?;
        let s = self.app.sessions.get(i)?;
        let key = Arc::as_ptr(&s.dirty) as usize;
        let map = self.pty_cache.borrow();
        let entry = map.get(&key)?;
        let rows = &entry.rows;
        if rows.is_empty() {
            return None;
        }
        let (r1, c1, r2, c2) = normalize_selection(a, h);
        let r1 = r1.min(rows.len() - 1);
        let r2 = r2.min(rows.len() - 1);
        let mut out = String::new();
        for r in r1..=r2 {
            let row = &rows[r];
            let row_text: String = row.iter().flat_map(|run| run.text.chars()).collect();
            let row_len = row_text.chars().count();
            let start = if r == r1 { c1 } else { 0 };
            let end = if r == r2 { c2.min(row_len) } else { row_len };
            let slice: String = row_text
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect();
            out.push_str(slice.trim_end());
            if r < r2 {
                out.push('\n');
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

fn pixel_to_cell(x: f32, y: f32) -> PtyCell {
    let metrics = pty_metrics(1.0);
    PtyCell {
        row: (y / metrics.cell_h).max(0.0) as usize,
        col: (x / metrics.cell_w).max(0.0) as usize,
    }
}
