//! `Grove` lifecycle: construction, subscriptions, and all `Msg` handling.

use super::keys::key_to_bytes;
use super::metrics::{
    clamp_sidebar_width, compute_pty_dims, pty_cols_for_fraction, pty_metrics,
    term_portion_for_cursor, PTY_ZOOM_DEFAULT, PTY_ZOOM_MAX, PTY_ZOOM_MIN, PTY_ZOOM_STEP, RAIL_W,
    TERM_PANEL_PORTION, TERM_PANEL_PORTION_MAX, TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_STEP,
};
use super::state::{
    AbsCell, ChangelogState, FocusedPane, GridDrag, GridSlide, Grove, Msg, PtyCell, PtyDrag,
    PtyPane, ScriptField, ScriptsEditorState, SidebarDrag, SidebarView, ToolStatus, UpgradeState,
};
use crate::agent::Agent;
use crate::app::{AddProjectStep, App, ConfirmKind, Modal, OnboardStep, Pane};
use crate::session::Session;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::Id;
use iced::{event, keyboard, Event, Subscription, Task};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Focuses the widget with the given [`Id`]. Replaces the convenience
/// `text_input::focus` helper removed in iced 0.14 — 0.14 only exposes the
/// lower-level `widget::operation` primitives, so we build the equivalent
/// `Task` ourselves.
fn focus(id: Id) -> Task<Msg> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::focus::<()>(
        id,
    ))
    .discard()
}

/// Moves the text-input cursor with the given [`Id`] to the end of its
/// content. See [`focus`] for why this wrapper exists.
fn move_cursor_to_end(id: Id) -> Task<Msg> {
    iced::advanced::widget::operate(
        iced::advanced::widget::operation::text_input::move_cursor_to_end::<()>(id),
    )
    .discard()
}

/// Scrolls the scrollable with the given [`Id`] to an absolute offset. See
/// [`focus`] for why this wrapper exists.
fn scroll_to(id: Id, offset: iced::widget::scrollable::AbsoluteOffset) -> Task<Msg> {
    iced::advanced::widget::operate(
        iced::advanced::widget::operation::scrollable::scroll_to::<()>(id, offset.into()),
    )
    .discard()
}

impl Grove {
    pub fn new() -> Self {
        // Attention files are only meaningful within the run that created
        // them (see `attention::cleanup_stale_files`) — clear leftovers from
        // a previous run before any session is spawned, so a reused id
        // can't read a stale state file.
        crate::attention::cleanup_stale_files();
        // Compute initial PTY dimensions from the default window size (1280×800).
        // Corrected on the first `WindowResized` event after startup.
        let window_size = iced::Size::new(1280.0, 800.0);
        let mut app = App::new().expect("init app");
        crate::telemetry::set_enabled(app.telemetry_enabled());
        crate::telemetry::track(
            "app_launched",
            vec![
                (
                    "theme",
                    app.store
                        .theme
                        .clone()
                        .unwrap_or_else(|| "default".to_string())
                        .into(),
                ),
                ("project_count", (app.store.projects.len() as u64).into()),
                ("tmux_enabled", app.use_tmux().into()),
            ],
        );
        crate::telemetry::start_heartbeat();
        let ui_zoom = app
            .store
            .ui_zoom
            .unwrap_or(PTY_ZOOM_DEFAULT)
            .clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        let sidebar_width = clamp_sidebar_width(
            app.store.sidebar_width.unwrap_or(RAIL_W),
            window_size.width / ui_zoom,
        );
        let (pty_rows, pty_cols) = compute_pty_dims(
            window_size.width,
            window_size.height,
            ui_zoom,
            true,
            sidebar_width,
        );
        // Resize any sessions discovered from a previous grove run so tmux
        // reports the correct terminal size immediately.
        for s in &mut app.sessions {
            s.resize(pty_rows, pty_cols);
        }
        let mut g = Self {
            app,
            collapsed: Default::default(),
            collapsed_wt: Default::default(),
            tree_expand: crate::gui::state::TreeExpand::All,
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
            attention_anim: Self::attention_animation(),
            pending_kill: None,
            hovered_wt: None,
            hovered_activity_row: None,
            sidebar_view: SidebarView::Activity,
            activity_no_sessions_expanded: None,
            term_panel_open: false,
            term_panel_portion: TERM_PANEL_PORTION,
            focused_pane: FocusedPane::Agent,
            dir_cache: Default::default(),
            picker_open: false,
            activity: Default::default(),
            // Assumed focused at launch (iced can't be queried); corrected by
            // the first Focused/Unfocused event. Worst case: one missed dock
            // bounce in the first moments of an unfocused launch.
            window_focused: true,
            last_badge: 0,
            sidebar_width,
            sidebar_drag: None,
            grid_view: false,
            tile_order: Vec::new(),
            grid_focused: None,
            grid_drag: None,
            grid_slide: None,
            pending_launcher_proj: None,
            grid_view_before_zen: false,
            last_divider_press: None,
            term_panel_dragging: false,
            last_term_divider_press: None,
            scripts_editor: None,
            settings_tools: Vec::new(),
            upgrade: UpgradeState::Idle,
            upgrade_method: crate::upgrade::detect(),
            upgrade_progress: std::sync::Arc::new(std::sync::Mutex::new(
                crate::gui::state::UpgradeProgress::default(),
            )),
            changelog: ChangelogState::Idle,
            show_changelog: false,
            git_state: Default::default(),
            last_git_poll: None,
            git_poll_inflight: Default::default(),
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

    /// Idle-state constructor for the attention pulse: parked at `false`
    /// (fully opaque, not animating). `go_mut(true, ..)` starts an endless
    /// auto-reversed 1s fade; clearing attention swaps a fresh idle instance
    /// back in so the pulse (and its frame subscription) fully stops.
    fn attention_animation() -> iced::animation::Animation<bool> {
        iced::animation::Animation::new(false)
            .duration(Duration::from_millis(1000))
            .easing(iced::animation::Easing::EaseInOut)
            .auto_reverse()
            .repeat_forever()
    }

    /// Records a slide animation for the two tiles that just swapped places
    /// in `tile_order`, so `grid_workspace` can translate their drawing back
    /// toward where they came from and ease it out to zero. Must be called
    /// AFTER `swap_tiles`, so `src`/`dst` are the tile-order indices the two
    /// tiles now occupy (post-swap).
    fn begin_grid_slide(&mut self, src: usize, dst: usize) {
        let n = self.tile_order.len();
        let (cols, _) = super::metrics::grid_layout(n);
        let cols = cols.max(1);
        let cell = |i: usize| ((i % cols) as i32, (i / cols) as i32);
        let (src_col, src_row) = cell(src);
        let (dst_col, dst_row) = cell(dst);
        self.grid_slide = Some(GridSlide {
            tiles: [
                (dst, src_col - dst_col, src_row - dst_row),
                (src, dst_col - src_col, dst_row - src_row),
            ],
            start: std::time::Instant::now(),
        });
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        // Only forward un-captured keys; widgets (search input) handle their own first.
        // Exception: a focused text_input captures Escape to blur itself
        // (iced_widget text_input.rs) without telling the app, so Escape would
        // otherwise need a second press to reach the modal's cancel handler.
        // Forward it regardless of status; every other captured key stays dropped.
        let keys = event::listen_with(|ev, status, _| {
            if !should_forward(&ev, status) {
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
                Event::Window(iced::window::Event::Focused) => Some(Msg::WindowFocusChanged(true)),
                Event::Window(iced::window::Event::Unfocused) => {
                    Some(Msg::WindowFocusChanged(false))
                }
                _ => None,
            }
        });
        let resize = iced::window::resize_events().map(|(_id, size)| Msg::WindowResized(size));
        let mut subs = vec![keys, resize];
        // Tick cadence (iced 0.14 only redraws on messages, so the tick both
        // drives periodic work and repaints):
        // - 60ms while any PTY exists (session, home terminal, or worktree
        //   shell) — output streaming, spinner/cursor animation, activity
        //   classification — or while transient work needs polling (drag
        //   autoscroll, teardown, upgrade apply).
        // - 1s fallback while focused — toast TTLs, background-job results,
        //   git-status polling, the 24h update check.
        // - no timer at all for an idle unfocused window.
        let has_ptys = !self.app.sessions.is_empty()
            || !self.app.home_terminals.is_empty()
            || self.app.wt_terminals.values().any(|v| !v.is_empty());
        let busy = self.pty_drag.is_some()
            || self.app.teardown.is_some()
            || matches!(self.upgrade, UpgradeState::Updating(_));
        if has_ptys || busy {
            subs.push(iced::time::every(Duration::from_millis(60)).map(|_| Msg::Tick));
        } else if self.window_focused {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Msg::Tick));
        }
        // Needs-attention pulse: while active, let the animation request its
        // own redraws at frame rate; zero subscriptions otherwise.
        if self.attention_anim.value() {
            subs.push(iced::window::frames().map(|_| Msg::AnimationFrame));
        }
        // Tile-slide reorder animation: same frame-rate redraw trick, active
        // only for the ~150ms window after a grid swap.
        if self.grid_slide.is_some() {
            subs.push(iced::window::frames().map(|_| Msg::AnimationFrame));
        }
        subs.push(iced::window::close_requests().map(Msg::CloseRequested));
        // Always-on: drives "system" theme mode, whether or not it's active,
        // so toggling it on later doesn't need a fresh OS notification first.
        subs.push(iced::system::theme_changes().map(Msg::SystemThemeChanged));
        // While the divider is held, listen globally for cursor motion and the
        // button-release — the 1px handle can't drive `mouse_area::on_move` once
        // the cursor leaves its bounds, so the drag is tracked at the app level.
        if self.sidebar_drag.is_some() {
            let drag = event::listen_with(|ev, _status, _| match ev {
                Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Msg::SidebarDragMove(position.x))
                }
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Msg::SidebarDragEnd)
                }
                _ => None,
            });
            subs.push(drag);
        }
        if self.term_panel_dragging {
            let drag = event::listen_with(|ev, _status, _| match ev {
                Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Msg::TermPanelDragMove(position.x))
                }
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Msg::TermPanelDragEnd)
                }
                _ => None,
            });
            subs.push(drag);
        }
        if self.grid_drag.is_some() {
            let drag = event::listen_with(|ev, _status, _| match ev {
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Msg::GridDragEnd)
                }
                _ => None,
            });
            subs.push(drag);
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Tick => {
                // Advance the blink counter (~30 Hz at 60 ms tick interval).
                self.blink_tick = self.blink_tick.wrapping_add(1);
                self.tick_drag_autoscroll();
                // Auto-dismiss the toast once its kind-dependent TTL elapses.
                if self
                    .app
                    .toast
                    .as_ref()
                    .is_some_and(|t| t.expired_at(std::time::Instant::now()))
                {
                    self.app.toast = None;
                }
                // Surface results from background jobs (.worktreeinclude
                // generation runs off-thread).
                let bg = self.app.bg_status.lock().ok().and_then(|mut g| g.take());
                if let Some(msg) = bg {
                    self.app.set_toast(msg);
                    self.app.refresh_worktrees();
                }
                // Re-classify session activity every 8th tick (~480ms at 60ms).
                if self.blink_tick.is_multiple_of(8) {
                    self.refresh_activity();
                }
                // Throttled background git-status poll for visible worktrees
                // (dirty / ahead / behind sidebar suffix).
                self.maybe_poll_git_state();
                // Advance an in-progress worktree teardown (script exit → git
                // removal). Cheap no-op when none is running.
                if self.app.teardown.is_some() {
                    let had_session = self
                        .app
                        .teardown
                        .as_ref()
                        .and_then(|t| t.session.as_ref())
                        .map(|s| Arc::as_ptr(&s.dirty) as usize);
                    self.app.poll_teardown();
                    // The teardown PTY was dropped during removal — evict its
                    // render-cache entry so a future session can't alias its
                    // (now reusable) dirty-Arc address.
                    let still = self
                        .app
                        .teardown
                        .as_ref()
                        .and_then(|t| t.session.as_ref())
                        .is_some();
                    if let (Some(key), false) = (had_session, still) {
                        self.pty_cache.borrow_mut().remove(&key);
                    }
                }
                // Drain apply progress (set by the background apply thread).
                {
                    let drained = if let Ok(mut g) = self.upgrade_progress.lock() {
                        let stage = g.stage.take();
                        let finished = g.finished.take();
                        (stage, finished)
                    } else {
                        (None, None)
                    };
                    if let Some(stage) = drained.0 {
                        self.upgrade = UpgradeState::Updating(stage);
                    }
                    if let Some(result) = drained.1 {
                        self.upgrade = match result {
                            Ok(()) => UpgradeState::Updated,
                            Err(e) => UpgradeState::UpdateFailed(e),
                        };
                    }
                }
                // Periodic update check: at most once per 24h while running.
                if let Some(task) = self.maybe_check_updates_due() {
                    return task;
                }
            }
            // No state to mutate for the attention pulse — the message exists
            // to trigger a redraw so it can interpolate against a fresh
            // Instant. The tile-slide animation, if active, self-clears once
            // its 150ms window elapses so its frame subscription stops.
            Msg::AnimationFrame => {
                if let Some(slide) = &self.grid_slide {
                    if slide_progress(slide.start, std::time::Instant::now()) >= 1.0 {
                        self.grid_slide = None;
                    }
                }
            }
            Msg::WindowFocusChanged(f) => {
                self.window_focused = f;
                // Regaining focus acknowledges the visible session.
                if f {
                    if let Some(i) = self.app.active_session {
                        self.acknowledge_session(i);
                    }
                    // A window that stays idle+unfocused stops ticking (see
                    // `subscription`), so a due update check would otherwise
                    // stall until other activity resumes. Evaluate it here
                    // too so refocus fires it promptly.
                    if let Some(task) = self.maybe_check_updates_due() {
                        return task;
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
                // Keep the sidebar inside the window's bounds (it may now be too
                // wide for a shrunken window). `size` is already logical.
                self.sidebar_width = clamp_sidebar_width(self.sidebar_width, size.width);
                self.refresh_pty_viewport();
            }
            Msg::CloseRequested(id) => {
                // tmux-backed sessions survive grove; only running native
                // sessions die with the window.
                let native_running = self.app.native_sessions_running();
                if native_running == 0 {
                    return iced::window::close(id);
                }
                let noun = if native_running == 1 {
                    "session"
                } else {
                    "sessions"
                };
                // Known gap: grove is one-modal-deep, so the quit confirm
                // replaces any open modal and cancelling does not restore it.
                // Acceptable for now; a modal stack would be needed to do
                // better.
                self.app.modal = Modal::Confirm {
                    title: "quit grove?".into(),
                    prompt: format!("{native_running} running {noun} will end. quit anyway?"),
                    destructive: true,
                    kind: ConfirmKind::Quit,
                };
            }
            Msg::BackendNative => {
                let _ = self.app.set_tmux_enabled(false);
            }
            Msg::BackendTmux => {
                let _ = self.app.set_tmux_enabled(true);
            }
            Msg::SkipPermissionsEnable => {
                let _ = self.app.set_skip_permissions_enabled(true);
            }
            Msg::SkipPermissionsDisable => {
                let _ = self.app.set_skip_permissions_enabled(false);
            }
            Msg::TelemetryToggle(v) => {
                let _ = self.app.set_telemetry_enabled(v);
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
                self.tree_expand = self.tree_expand.next();
                self.apply_tree_expand();
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
                    self.sync_grid_focus();
                    self.leave_terminal_tab();
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
            Msg::KeyPress(key, modified_key, mods) => {
                if let Modal::RemoveProject { in_progress, .. } = &self.app.modal {
                    let busy = *in_progress;
                    return self.handle_remove_project_key(key, busy);
                }
                let was_theme_picker = matches!(self.app.modal, Modal::ThemePicker { .. });
                let task = self.handle_key(key, modified_key, mods);
                if was_theme_picker && matches!(self.app.modal, Modal::ThemePicker { .. }) {
                    return self.scroll_theme_picker_to_selection();
                }
                return task;
            }
            Msg::FileDropped(path) => {
                match &self.app.modal {
                    // A folder dropped while the add-project modal is open
                    // chooses it (on either step — re-choosing is a cheap undo).
                    Modal::AddProject { .. } => {
                        self.app.add_project_choose(path);
                        return self.focus_add_project_field();
                    }
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
                            sess.send(super::drop::dropped_path_text(&path).as_bytes());
                            self.pty_selection = None;
                        }
                    }
                    // Any other modal: ignore — dropped text could land in an
                    // unexpected place otherwise.
                    _ => {}
                }
            }
            Msg::PtyMouseDown(pane, x, y) => {
                if let PtyPane::Tile(si) = pane {
                    // Focus this tile, then anchor a selection the same way
                    // the Agent/Panel path below does — `grid_focused` is
                    // already updated, so `pixel_to_abs`/`pty_view_geom`
                    // (which resolve via `focused_session`) target this
                    // tile's session.
                    self.grid_focused = Some(si);
                    self.app.active_session = Some(si);
                    self.acknowledge_session(si);
                    self.pty_selection = None;
                    if let (Some(cell), Some((h, _))) =
                        (self.pixel_to_abs(x, y), self.pty_view_geom())
                    {
                        self.pty_selection = Some((cell, cell));
                        self.pty_drag = Some(PtyDrag {
                            last_x: x,
                            last_y: y,
                            view_h_px: h as f32 * pty_metrics(1.0).cell_h,
                        });
                    }
                    return Task::none();
                }
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
                // `selection_pane` covers grid tiles too: it resolves to the
                // focused tile while in grid view.
                if self.selection_pane() != pane {
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
                if let PtyPane::Tile(si) = pane {
                    // Scroll the specific tile under the cursor, not just the focused one.
                    if let Some(s) = self.app.sessions.get_mut(si) {
                        let cell = pixel_to_cell(x, y);
                        s.scroll(up, cell.col as u16, cell.row as u16);
                    }
                    return Task::none();
                }
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
                if !self.app.chrome_visible {
                    // Exiting zen.
                    self.app.chrome_visible = true;
                    if self.grid_view_before_zen {
                        // Zen was entered from grid view: restore grid.
                        self.grid_view = true;
                        self.grid_view_before_zen = false;
                    }
                    self.refresh_pty_viewport();
                } else if self.grid_view {
                    // Entering zen from the grid: focus the selected tile so zen
                    // shows that one session, matching the tile's zen button.
                    if let Some(si) = self
                        .grid_focused
                        .or(self.app.active_session)
                        .or_else(|| self.tile_order.first().copied())
                    {
                        return self.update(Msg::GridTileZen(si));
                    }
                    self.app.chrome_visible = false;
                    self.refresh_pty_viewport();
                } else {
                    // Entering zen from the single-session workspace: the active
                    // session is already focused, just hide the chrome.
                    self.app.chrome_visible = false;
                    self.refresh_pty_viewport();
                }
            }
            Msg::ZoomIn => {
                crate::telemetry::track("zoom_changed", vec![]);
                self.adjust_ui_zoom(PTY_ZOOM_STEP);
            }
            Msg::ZoomOut => {
                crate::telemetry::track("zoom_changed", vec![]);
                self.adjust_ui_zoom(-PTY_ZOOM_STEP);
            }
            Msg::ZoomReset => {
                crate::telemetry::track("zoom_changed", vec![]);
                self.set_ui_zoom(PTY_ZOOM_DEFAULT);
            }
            Msg::PtyMouseUp => {
                self.pty_drag = None;
                if let Some((a, h)) = self.pty_selection {
                    if a == h {
                        self.pty_selection = None;
                    }
                }
            }
            Msg::SidebarDragStart => {
                // Double-click (two presses within 350ms) resets to the default.
                let now = std::time::Instant::now();
                let double = self
                    .last_divider_press
                    .is_some_and(|t| now.duration_since(t) < Duration::from_millis(350));
                if double {
                    self.sidebar_drag = None;
                    self.last_divider_press = None;
                    let logical_w = self.window_size.width / self.ui_zoom;
                    self.sidebar_width = clamp_sidebar_width(RAIL_W, logical_w);
                    self.refresh_pty_viewport();
                    self.persist_sidebar_width();
                } else {
                    self.last_divider_press = Some(now);
                    self.sidebar_drag = Some(SidebarDrag {
                        grab_offset: None,
                        start_width: self.sidebar_width,
                    });
                }
            }
            Msg::SidebarDragMove(cursor_x) => {
                if let Some(drag) = self.sidebar_drag {
                    // The sidebar's left edge is the window's left edge, so the
                    // cursor x maps directly to width; the grab offset (set on
                    // the first move) absorbs an off-edge press so width doesn't
                    // jump. Both are logical px (iced scale_factor == ui_zoom).
                    let offset = match drag.grab_offset {
                        Some(o) => o,
                        None => {
                            let o = self.sidebar_width - cursor_x;
                            self.sidebar_drag = Some(SidebarDrag {
                                grab_offset: Some(o),
                                start_width: drag.start_width,
                            });
                            o
                        }
                    };
                    let logical_w = self.window_size.width / self.ui_zoom;
                    self.sidebar_width = clamp_sidebar_width(cursor_x + offset, logical_w);
                    // Visual width follows live; PTY grid is recomputed on end.
                }
            }
            Msg::SidebarDragEnd => {
                if let Some(drag) = self.sidebar_drag.take() {
                    // Skip the PTY resize + persist when the width didn't move
                    // (a plain click rather than a drag).
                    if (self.sidebar_width - drag.start_width).abs() >= 0.5 {
                        self.refresh_pty_viewport();
                        self.persist_sidebar_width();
                    }
                }
            }
            Msg::TermPanelDragStart => {
                let now = std::time::Instant::now();
                let double = self
                    .last_term_divider_press
                    .is_some_and(|t| now.duration_since(t) < Duration::from_millis(350));
                if double {
                    self.term_panel_dragging = false;
                    self.last_term_divider_press = None;
                    if self.term_panel_portion != TERM_PANEL_PORTION {
                        self.term_panel_portion = TERM_PANEL_PORTION;
                        self.refresh_pty_viewport();
                    }
                } else {
                    self.last_term_divider_press = Some(now);
                    self.term_panel_dragging = true;
                }
            }
            Msg::TermPanelDragMove(cursor_x) => {
                if self.term_panel_dragging {
                    let logical_w = self.window_size.width / self.ui_zoom;
                    // The split divider sits at the workspace edge, so the cursor
                    // x maps directly to the panel's width share. Live update;
                    // PTY columns are recomputed on release.
                    self.term_panel_portion =
                        term_portion_for_cursor(cursor_x, logical_w, self.sidebar_width);
                }
            }
            Msg::TermPanelDragEnd => {
                if self.term_panel_dragging {
                    self.term_panel_dragging = false;
                    self.refresh_pty_viewport();
                }
            }
            Msg::AddProject => {
                self.open_agent_menu = None;
                self.app.focus_pane(Pane::Projects);
                self.app.start_add();
                return focus(crate::gui::view::modal_input_id());
            }
            Msg::AddWorktree { proj } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.focus_pane(Pane::Worktrees);
                self.app.start_add();
                return focus(crate::gui::view::modal_input_id());
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
            Msg::RunScript { proj, wt } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.wt_idx = wt;
                if let Some(w) = self.app.worktrees.get(wt).cloned() {
                    let before = self.session_keys();
                    if self.grid_view {
                        self.app
                            .run_worktree_script(&w.path, self.pty_rows, self.pty_panel_cols);
                        if self.app.sessions.len() > before.len() {
                            self.tile_order.push(self.app.sessions.len() - 1);
                            self.persist_grid_order();
                        }
                        self.refresh_pty_viewport();
                    } else {
                        self.term_panel_open = true;
                        self.refresh_pty_viewport();
                        self.app
                            .run_worktree_script(&w.path, self.pty_rows, self.pty_panel_cols);
                        self.focused_pane = FocusedPane::Panel;
                        self.pty_selection = None;
                    }
                    self.collapsed_wt.remove(&(proj, wt));
                }
            }
            Msg::EditScripts { proj } => {
                self.open_agent_menu = None;
                self.open_scripts_editor(proj);
            }
            Msg::ScriptsEditorAction(field, action) => {
                if let Some(ed) = self.scripts_editor.as_mut() {
                    match field {
                        ScriptField::Setup => ed.setup.perform(action),
                        ScriptField::Run => ed.run.perform(action),
                        ScriptField::Teardown => ed.teardown.perform(action),
                    }
                }
            }
            Msg::ScriptsEditorSave => self.save_scripts_editor(),
            Msg::ScriptsEditorCancel => self.cancel_modal(),
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
            Msg::InputPathChanged(s) => self.app.set_input_path(s),
            Msg::ModalConfirm(yes) => {
                if matches!(self.app.modal, Modal::Teardown) {
                    // The teardown modal's only confirm action is dismissal,
                    // and only once removal has finished.
                    if matches!(
                        self.app.teardown.as_ref().map(|t| t.stage),
                        Some(crate::app::TeardownStage::Done { .. })
                    ) {
                        self.app.close_teardown();
                    }
                } else {
                    return self.confirm_modal_response(yes);
                }
            }
            Msg::AddProjectBrowse => {
                // One dialog at a time — a second click while the picker is up
                // must not spawn another.
                if self.picker_open {
                    return Task::none();
                }
                if matches!(
                    self.app.modal,
                    Modal::AddProject { .. } | Modal::Onboarding { .. }
                ) {
                    self.picker_open = true;
                    return Task::perform(
                        async {
                            rfd::AsyncFileDialog::new()
                                .set_title("Choose a project folder")
                                .pick_folder()
                                .await
                                .map(|h| h.path().to_path_buf())
                        },
                        Msg::AddProjectPicked,
                    );
                }
            }
            Msg::AddProjectPicked(picked) => {
                self.picker_open = false;
                // A late result after the modal closed (or changed) must not
                // mutate an unrelated modal; `None` = user cancelled.
                if let Some(path) = picked {
                    match &self.app.modal {
                        Modal::AddProject { .. } => {
                            self.app.add_project_choose(path);
                            return self.focus_add_project_field();
                        }
                        Modal::Onboarding {
                            step: OnboardStep::Project,
                            ..
                        } => {
                            self.app.onboard_set_path(format!("{}/", path.display()));
                            return move_cursor_to_end(crate::gui::view::modal_input_id());
                        }
                        _ => {}
                    }
                }
            }
            Msg::AddProjectPathChanged(s) => self.app.add_project_set_path(s),
            Msg::AddProjectChooseTyped => {
                self.app.add_project_choose_typed();
                return self.focus_add_project_field();
            }
            Msg::AddProjectNameChanged(s) => self.app.add_project_set_name(s),
            Msg::AddProjectChangeSource => {
                self.app.add_project_change_source();
                return focus(crate::gui::view::modal_input_id());
            }
            Msg::AddProjectToggleInitGit(v) => {
                if let Modal::AddProject { init_git, .. } = &mut self.app.modal {
                    *init_git = v;
                }
            }
            Msg::AddProjectSubmit => {
                if let Err(e) = self.app.submit_add_project() {
                    self.app.modal = Modal::Message(format!("add project failed: {e}"));
                }
                self.rebuild_wt_cache();
            }
            Msg::ModalPickDir(path) => {
                if matches!(
                    &self.app.modal,
                    Modal::AddProject {
                        step: AddProjectStep::PickSource,
                        ..
                    }
                ) {
                    self.app.add_project_set_path(format!("{path}/"));
                    return move_cursor_to_end(crate::gui::view::modal_input_id());
                }
            }
            Msg::OpenThemePicker => {
                // The only entry point now is the Settings Appearance section,
                // so the picker always returns to Settings when closed.
                self.app.open_theme_picker(true);
                return self.scroll_theme_picker_to_selection();
            }
            Msg::OpenSettings => {
                crate::telemetry::track("settings_opened", vec![]);
                self.app.open_settings();
                return self.detect_tools_task();
            }
            Msg::OpenShortcutOverlay => {
                self.app.modal = Modal::ShortcutOverlay;
            }
            Msg::RefreshTools => return self.detect_tools_task(),
            Msg::SetDefaultAgent(agent) => {
                if let Err(e) = self.app.set_default_agent(agent) {
                    self.app.set_error_toast(e.to_string());
                }
            }
            Msg::ToolVersionsDetected(results) => {
                self.settings_tools = results.into_iter().map(|(_, status)| status).collect();
            }
            Msg::CheckForUpdates { manual } => {
                // Guard: don't fire a duplicate request if a check is already in-flight.
                if matches!(self.upgrade, UpgradeState::Checking) {
                    return Task::none();
                }
                return self.check_updates_task(manual);
            }
            Msg::UpdateCheckResult(result, manual) => {
                // Record the check time regardless of outcome so the periodic trigger backs off.
                self.app.store.last_update_check = Some(now_unix());
                let _ = crate::storage::save(&self.app.store);
                match result {
                    Ok(release) => {
                        let current = env!("CARGO_PKG_VERSION");
                        let skipped = self.app.store.skipped_version.as_deref();
                        if crate::upgrade::update_available(current, &release, skipped) {
                            self.upgrade = UpgradeState::Available(release);
                        } else {
                            self.upgrade = UpgradeState::UpToDate;
                        }
                    }
                    Err(e) => {
                        eprintln!("update check failed: {e}");
                        if manual {
                            // Manual checks surface the error inline so the user knows.
                            self.upgrade = UpgradeState::Error(e);
                        } else {
                            // Launch/periodic checks fail silently (log only; no badge/error shown).
                            self.upgrade = UpgradeState::Idle;
                        }
                    }
                }
            }
            Msg::SkipVersion => {
                if let UpgradeState::Available(release) = &self.upgrade {
                    self.app.store.skipped_version = Some(release.tag.clone());
                    let _ = crate::storage::save(&self.app.store);
                    crate::telemetry::track(
                        "update_declined",
                        vec![("version", release.tag.clone().into())],
                    );
                }
                self.upgrade = UpgradeState::UpToDate;
            }
            Msg::CopyReleaseUrl => {
                if let UpgradeState::Available(r) = &self.upgrade {
                    crate::clipboard::copy(&r.html_url);
                    self.app.set_toast("release url copied");
                }
            }
            Msg::StartUpdate => {
                let UpgradeState::Available(release) = self.upgrade.clone() else {
                    return Task::none();
                };
                let method = self.upgrade_method;
                self.upgrade = UpgradeState::Updating(crate::upgrade::Stage::Downloading);
                self.app.modal = Modal::Updating;

                let handle = self.upgrade_progress.clone();
                std::thread::spawn(move || {
                    let cb_handle = handle.clone();
                    let cb = move |stage: crate::upgrade::Stage| {
                        if let Ok(mut g) = cb_handle.lock() {
                            g.stage = Some(stage);
                        }
                    };
                    let result =
                        crate::upgrade::apply(method, &release, &cb).map_err(|e| e.to_string());
                    if result.is_ok() {
                        crate::telemetry::track(
                            "update_applied",
                            vec![("to_version", release.tag.clone().into())],
                        );
                    }
                    if let Ok(mut g) = handle.lock() {
                        g.finished = Some(result);
                    }
                });
                return Task::none();
            }
            Msg::RestartApp => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).spawn();
                }
                std::process::exit(0);
            }
            Msg::OpenChangelog => {
                self.changelog = ChangelogState::Loading;
                self.show_changelog = true;
                // The changelog modal takes over; close the Settings modal behind it.
                self.app.modal = Modal::None;
                return self.fetch_changelog_task();
            }
            Msg::ChangelogLoaded(result) => {
                self.changelog = match result {
                    Ok(notes) => ChangelogState::Loaded(notes),
                    Err(e) => ChangelogState::Error(e),
                };
            }
            Msg::CloseChangelog => {
                self.show_changelog = false;
                // Return to Settings, where the button lives (mirrors ThemePicker return).
                self.app.modal = Modal::Settings;
            }
            Msg::ThemePickerSwitchTab => {
                self.theme_picker_switch_tab();
                return self.scroll_theme_picker_to_selection();
            }
            Msg::ThemePickerSelect(i) => {
                self.theme_picker_select(i);
                return self.scroll_theme_picker_to_selection();
            }
            Msg::ThemePickerToggleSystem(enabled) => self.theme_picker_toggle_system(enabled),
            Msg::ThemePickerSubmit => self.theme_picker_submit(),
            Msg::ThemePickerCancel => self.theme_picker_cancel(),
            Msg::SystemThemeChanged(mode) => {
                self.app.system_theme_mode = mode;
                // Re-resolve immediately if the persisted setting follows the
                // OS, or if the theme picker is open with the "follow
                // system" checkbox previewed-but-not-yet-submitted — otherwise
                // an OS appearance change mid-preview would silently freeze
                // the preview at the mode captured when the checkbox was
                // ticked.
                let previewing_system = matches!(
                    self.app.modal,
                    crate::app::Modal::ThemePicker {
                        follow_system: true,
                        ..
                    }
                );
                if self.app.theme_follow_system {
                    self.app.apply_system_theme();
                    self.invalidate_pty_render_cache();
                } else if previewing_system {
                    // Not yet submitted, so `apply_system_theme` (gated on
                    // `theme_follow_system`) would no-op — resolve directly.
                    let name = self
                        .app
                        .resolve_system_theme_name(self.app.system_theme_mode)
                        .to_string();
                    crate::theme::set_by_name(&name);
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::OnbNext => return self.onboard_advance(),
            Msg::OnbBack => self.app.onboard_back(),
            Msg::OnbSkip => self.onboard_skip(),
            Msg::OnbPathChanged(s) => self.app.onboard_set_path(s),
            Msg::OnbNameChanged(s) => self.app.onboard_set_name(s),
            Msg::OnbPickDir(p) => {
                self.app.onboard_pick_dir(p);
                return move_cursor_to_end(crate::gui::view::modal_input_id());
            }
            Msg::OnbThemeTab => self.app.onboard_theme_switch_tab(),
            Msg::OnbThemeSelect(i) => self.app.onboard_theme_select(i),
            Msg::OnbAgentSelect(i) => self.app.onboard_agent_select(i),
            Msg::OnbBackendSelect(tmux) => self.app.onboard_set_backend(tmux),
            Msg::OnbPermsSelect(skip) => self.app.onboard_set_perms(skip),
            Msg::ToggleGridView => {
                self.grid_view = !self.grid_view;
                // Entering/leaving grid changes which pane can own a
                // selection — drop any stale one rather than mis-render it.
                self.pty_selection = None;
                if self.grid_view {
                    let live_keys: Vec<String> = self
                        .app
                        .sessions
                        .iter()
                        .map(|s| crate::gui::launcher::session_grid_key(&s.project, &s.wt_path))
                        .collect();
                    self.tile_order = crate::gui::launcher::reconcile_tile_order(
                        &live_keys,
                        &self.app.store.grid_order,
                    );
                    // Open with a focused tile so the directional shortcuts
                    // (mod+hjkl to move focus, mod+alt+hjkl to move the tile)
                    // work on the first keypress. Keep the active session's
                    // tile if it has one — yanking focus elsewhere on entry
                    // would be a surprise — otherwise focus the first tile.
                    let focus = self
                        .app
                        .active_session
                        .filter(|si| self.tile_order.contains(si))
                        .or_else(|| self.tile_order.first().copied());
                    self.grid_focused = focus;
                    if let Some(si) = focus {
                        self.app.active_session = Some(si);
                        self.acknowledge_session(si);
                    }
                    self.grid_drag = None;
                } else {
                    // Carry the focused tile into the normal workspace.
                    if let Some(si) = self.grid_focused {
                        self.app.active_session = Some(si);
                        self.leave_terminal_tab();
                    }
                    self.persist_grid_order();
                    self.tile_order.clear();
                    self.grid_focused = None;
                    self.grid_drag = None;
                }
                self.refresh_pty_viewport();
            }
            Msg::GridDragStart(tile_idx) => {
                if tile_idx >= self.tile_order.len() {
                    return Task::none();
                }
                let si = self.tile_order[tile_idx];
                self.set_grid_focus(Some(si));
                self.app.active_session = Some(si);
                self.acknowledge_session(si);
                self.grid_drag = Some(GridDrag {
                    source_idx: tile_idx,
                    hover_idx: tile_idx,
                });
            }
            Msg::GridDragHover(tile_idx) => {
                if let Some(drag) = &mut self.grid_drag {
                    drag.hover_idx = tile_idx;
                }
                // No-op when no drag is active (on_enter always fires).
            }
            Msg::GridDragEnd => {
                if let Some(drag) = self.grid_drag.take() {
                    let src = drag.source_idx;
                    let dst = drag.hover_idx;
                    if src != dst && src < self.tile_order.len() && dst < self.tile_order.len() {
                        crate::gui::launcher::swap_tiles(&mut self.tile_order, src, dst);
                        self.begin_grid_slide(src, dst);
                        self.persist_grid_order();
                        // Every tile between src and dst may have changed column, so re-size each tile's PTY to its new column height.
                        self.refresh_pty_viewport();
                    }
                }
            }
            Msg::GridTileZen(si) => {
                self.app.active_session = Some(si);
                self.leave_terminal_tab();
                self.grid_focused = Some(si);
                self.acknowledge_session(si);
                // Switching workspace shape invalidates any tile selection.
                self.pty_selection = None;
                // Temporarily exit grid so zen has a single-session workspace.
                self.grid_view = false;
                self.grid_view_before_zen = true;
                self.app.chrome_visible = false;
                self.refresh_pty_viewport();
            }
            Msg::OpenSessionLauncher => {
                crate::telemetry::track("launcher_opened", vec![]);
                self.open_session_launcher();
            }
            Msg::LauncherSelectProject(i) => self.launcher_select_project(i),
            Msg::LauncherSelectWorktree(i) => {
                let proj = match &self.app.modal {
                    crate::app::Modal::SessionLauncher { proj, .. } => Some(*proj),
                    _ => None,
                };
                if let Some(proj) = proj {
                    let max = self.launcher_worktrees(proj).len();
                    if let crate::app::Modal::SessionLauncher { wt, col, .. } = &mut self.app.modal
                    {
                        if i < max {
                            *wt = i;
                            *col = 1;
                        }
                    }
                }
            }
            Msg::LauncherSelectAgent(i) => {
                let max = self.app.available_agents.len();
                if let crate::app::Modal::SessionLauncher { agent, col, .. } = &mut self.app.modal {
                    if i < max {
                        *agent = i;
                        *col = 2;
                    }
                }
            }
            Msg::LauncherNewWorktree => return self.launcher_new_worktree(),
            Msg::LauncherStart => self.launcher_start(),
        }
        Task::none()
    }

    /// On the session step, advance == finish (launch). On any other step, move
    /// forward; if the project step just registered a project, refresh the
    /// worktree cache so the rest of the app sees it.
    fn onboard_advance(&mut self) -> Task<Msg> {
        let on_session = matches!(
            self.app.modal,
            Modal::Onboarding {
                step: crate::app::OnboardStep::Session,
                ..
            }
        );
        if on_session {
            return self.onboard_finish();
        }
        self.app.onboard_next();
        self.rebuild_wt_cache();
        // Keep the project-step path input focused after rendering.
        if matches!(
            self.app.modal,
            Modal::Onboarding {
                step: crate::app::OnboardStep::Project,
                ..
            }
        ) {
            self.app.onboard_reset_project_focus();
            return focus(crate::gui::view::modal_input_id());
        }
        Task::none()
    }

    fn onboard_skip(&mut self) {
        if let Err(e) = self.app.onboard_skip() {
            self.app.modal = Modal::Message(format!("setup failed: {e}"));
            return;
        }
        self.after_onboarding();
    }

    fn onboard_finish(&mut self) -> Task<Msg> {
        match self.app.onboard_finish() {
            Ok(Some((proj, agent))) => {
                let before = self.session_keys();
                self.spawn(proj, 0, agent);
                self.resize_new_sessions(&before);
                // If the grid is open, append the new session index so it appears.
                if self.grid_view && self.app.sessions.len() > before.len() {
                    self.tile_order.push(self.app.sessions.len() - 1);
                    self.persist_grid_order();
                    self.refresh_pty_viewport();
                }
                self.rebuild_wt_cache();
            }
            Ok(None) => {}
            Err(e) => {
                self.app.modal = Modal::Message(format!("setup failed: {e}"));
                return Task::none();
            }
        }
        self.after_onboarding();
        Task::none()
    }

    /// After the wizard closes, surface the one-time tmux/native choice if it's
    /// still pending and nothing else grabbed the modal slot.
    fn after_onboarding(&mut self) {
        if matches!(self.app.modal, Modal::None)
            && crate::app::needs_tmux_choice(self.app.tmux_available, self.app.store.tmux_enabled)
        {
            self.app.modal = Modal::TmuxChoice;
        }
    }

    /// The tools shown in the Settings Tools section, in display order.
    /// `Terminal` is omitted — always available, no version.
    const SETTINGS_TOOLS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

    /// Mark the Tools rows as detecting (drives the spinner) and dispatch the
    /// off-thread availability + version scan, which posts back
    /// `Msg::ToolVersionsDetected`.
    fn detect_tools_task(&mut self) -> Task<Msg> {
        self.settings_tools = Self::SETTINGS_TOOLS
            .iter()
            .map(|&agent| ToolStatus {
                agent,
                installed: false,
                version: None,
                detecting: true,
            })
            .collect();
        Task::perform(
            async {
                // `--version` is a short subprocess; running it on the executor
                // keeps the UI thread free even if a binary is slow.
                Self::SETTINGS_TOOLS
                    .iter()
                    .map(|&agent| {
                        let installed = agent.available();
                        let version = if installed { agent.version() } else { None };
                        (
                            agent,
                            ToolStatus {
                                agent,
                                installed,
                                version,
                                detecting: false,
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            },
            Msg::ToolVersionsDetected,
        )
    }

    /// Returns a `check_updates_task` if the 24h periodic update check is due
    /// and no check/apply is already in flight. Shared by the tick handler
    /// and the focus-regained path, since the idle+unfocused window stops
    /// ticking and would otherwise miss a check that came due while away.
    fn maybe_check_updates_due(&mut self) -> Option<Task<Msg>> {
        let due = match self.app.store.last_update_check {
            Some(ts) => now_unix() - ts >= 24 * 60 * 60,
            None => false, // launch check seeds the timestamp; don't double-fire at boot
        };
        if due && matches!(self.upgrade, UpgradeState::Idle | UpgradeState::UpToDate) {
            Some(self.check_updates_task(false))
        } else {
            None
        }
    }

    /// Set upgrade state to Checking and dispatch an off-thread release fetch,
    /// which posts back `Msg::UpdateCheckResult`. Mirrors `detect_tools_task`.
    /// `manual` is threaded into the result so the handler can apply the correct
    /// error policy (surface inline vs. fail silently).
    fn check_updates_task(&mut self, manual: bool) -> Task<Msg> {
        self.upgrade = UpgradeState::Checking;
        // Mirrors detect_tools_task: short blocking work on the iced/tokio executor.
        Task::perform(
            async move { crate::upgrade::latest().map_err(|e| e.to_string()) },
            move |result| Msg::UpdateCheckResult(result, manual),
        )
    }

    /// Dispatch an off-thread release-notes fetch, posting back `Msg::ChangelogLoaded`.
    /// Mirrors `check_updates_task`.
    fn fetch_changelog_task(&self) -> Task<Msg> {
        // Off-thread, mirroring the update check. 10 most recent releases.
        Task::perform(
            async { crate::upgrade::releases(10).map_err(|e| e.to_string()) },
            Msg::ChangelogLoaded,
        )
    }

    fn scroll_theme_picker_to_selection(&self) -> Task<Msg> {
        use super::metrics::ROW_H;
        use crate::app::Modal;
        use iced::widget::scrollable::AbsoluteOffset;
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
        scroll_to(
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
            follow_system,
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
        // Picking a concrete theme from the list opts back out of "system".
        *follow_system = false;
        crate::theme::set(themes[index]);
        self.invalidate_pty_render_cache();
    }

    /// Toggle the theme picker's "follow system appearance" checkbox and
    /// preview the result immediately: checking it previews the resolved
    /// system theme; unchecking it restores the current tab's selection.
    fn theme_picker_toggle_system(&mut self, enabled: bool) {
        use crate::app::Modal;
        let Modal::ThemePicker { follow_system, .. } = &mut self.app.modal else {
            return;
        };
        *follow_system = enabled;
        if enabled {
            let name = self
                .app
                .resolve_system_theme_name(self.app.system_theme_mode)
                .to_string();
            crate::theme::set_by_name(&name);
        } else if let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &self.app.modal
        {
            let themes = crate::theme::themes_of(*tab);
            let sel = match tab {
                crate::theme::ThemeKind::Dark => *sel_dark,
                crate::theme::ThemeKind::Light => *sel_light,
            };
            if let Some(t) = themes.get(sel) {
                crate::theme::set(*t);
            }
        }
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
        let mut live_keys: Vec<u64> = Vec::with_capacity(self.app.sessions.len());
        let mut newly_waiting = false;

        for (i, s) in self.app.sessions.iter().enumerate() {
            live_keys.push(s.id);
            let focused = self.app.active_session == Some(i) && self.window_focused;
            let tracker = self.activity.entry(s.id).or_default();

            // Consume new bells: pending only when they ring unfocused.
            let bells = s.bell_count();
            if bells < tracker.bell_seen {
                // The counter only goes backwards if the vt100 parser was
                // reset/replaced — resync instead of going silent forever.
                tracker.bell_seen = bells;
            } else if bells > tracker.bell_seen {
                tracker.bell_seen = bells;
                if !focused {
                    tracker.bell_pending = true;
                }
            }

            let alive = matches!(s.status(), crate::session::SessionStatus::Running);
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            let output_age = now.saturating_duration_since(t);
            // Skip the parser lock for sessions that can't need it.
            let tail = if alive {
                s.tail_contents(15)
            } else {
                String::new()
            };

            let scrolling = s
                .scroll_age()
                .is_some_and(|a| a < super::activity::SCROLL_QUIET);
            let interacting = s
                .input_age()
                .is_some_and(|a| a < super::activity::INPUT_QUIET);
            let sig = Signals {
                alive,
                output_age,
                bell_pending: tracker.bell_pending,
                was_working: tracker.was_working,
                focused,
                scrolling,
                interacting,
                // Structured OSC title — primary working signal for agents
                // that emit one; vt100 already tracks it from the PTY stream.
                title: if alive { s.current_title() } else { None },
            };
            // Claude/Codex sessions with a hook/notify state file get a
            // deterministic signal that outranks the screen-scraping
            // heuristic below (but never a dead process — a stale `working`
            // left behind by a killed agent must still show Exited). A
            // `NeedsYou` signal while focused is treated like the user has
            // already seen it (never resurrect the highest-urgency state on
            // the session they're looking at, mirroring
            // `Tracker::acknowledge`'s existing downgrade rule).
            let new_state = match (alive, s.attention_state()) {
                (false, _) => classify(s.agent, &tail, &sig),
                (true, Some(crate::attention::AttentionState::NeedsYou)) if !focused => {
                    ActivityState::WaitingForInput
                }
                (true, Some(crate::attention::AttentionState::NeedsYou)) => ActivityState::Working,
                (true, Some(crate::attention::AttentionState::Done)) => ActivityState::Done,
                (true, Some(crate::attention::AttentionState::Working)) => ActivityState::Working,
                (true, None) => classify(s.agent, &tail, &sig),
            };
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
        // Start/stop the needs-attention pulse to match the waiting set.
        if (waiting > 0) != self.attention_anim.value() {
            if waiting > 0 {
                self.attention_anim.go_mut(true, now);
            } else {
                self.attention_anim = Self::attention_animation();
            }
        }
        if newly_waiting && !self.window_focused {
            super::dock::request_attention();
        }
    }

    /// Acknowledge the given session's tracker (user focused it). Also
    /// clears any deterministic attention-signal file so a stale `needs-you`/
    /// `done` doesn't resurface once the user looks away again.
    fn acknowledge_session(&mut self, i: usize) {
        if let Some(s) = self.app.sessions.get(i) {
            if let Some(t) = self.activity.get_mut(&s.id) {
                t.acknowledge();
            }
            s.acknowledge_attention();
        }
    }

    /// Read-only state lookup for the view layer. Unknown sessions render
    /// Idle until the first classification tick.
    pub(super) fn activity_state(&self, s: &Session) -> super::activity::ActivityState {
        self.activity
            .get(&s.id)
            .map(|t| t.state)
            .unwrap_or(super::activity::ActivityState::Idle)
    }

    /// Current needs-attention pulse phase in `[0, 1]` (0 = fully opaque,
    /// 1 = maximum dim). Constant 0 while no session waits for input, so
    /// callers can interpolate unconditionally.
    pub(super) fn attention_pulse(&self) -> f32 {
        self.attention_anim
            .interpolate(0.0, 1.0, std::time::Instant::now())
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
        if self.grid_view {
            let total = self.tile_order.len();
            let n = total.max(1);
            let (grid_cols, _) = super::metrics::grid_layout(n);
            // All columns are equal width, so the cell width is uniform.
            let tile_cols = super::metrics::grid_tile_cols(self.window_size.width, self.ui_zoom, n);
            // Height is per-column: a tile's PTY rows depend on how many tiles
            // share its column (column `p % grid_cols` for tile-order slot `p`),
            // so the lone tile in a short column fills the full workspace height.
            for (p, &si) in self.tile_order.iter().enumerate() {
                let col = p % grid_cols;
                let tiles_in_col = (total - 1 - col) / grid_cols + 1;
                let tile_rows = super::metrics::grid_tile_rows_for_col(
                    self.window_size.height,
                    self.ui_zoom,
                    tiles_in_col,
                );
                if let Some(s) = self.app.sessions.get_mut(si) {
                    s.resize(tile_rows, tile_cols);
                }
            }
            self.invalidate_pty_render_cache();
            return;
        }
        let (rows, cols) = compute_pty_dims(
            self.window_size.width,
            self.window_size.height,
            self.ui_zoom,
            self.app.chrome_visible,
            self.sidebar_width,
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
                    self.sidebar_width,
                ),
                pty_cols_for_fraction(
                    self.window_size.width,
                    self.ui_zoom,
                    self.app.chrome_visible,
                    panel,
                    self.sidebar_width,
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

    fn persist_sidebar_width(&mut self) {
        self.app.store.sidebar_width = Some(self.sidebar_width);
        let _ = crate::storage::save(&self.app.store);
    }

    /// Save the current `tile_order` to `Store::grid_order` (mapped through
    /// each tile's stable session key) so Agent View reopens in the same
    /// arrangement, including across app restarts.
    fn persist_grid_order(&mut self) {
        self.app.store.grid_order = self
            .tile_order
            .iter()
            .filter_map(|&si| self.app.sessions.get(si))
            .map(|s| crate::gui::launcher::session_grid_key(&s.project, &s.wt_path))
            .collect();
        let _ = crate::storage::save(&self.app.store);
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
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        self.rebuild_wt_cache();
    }

    fn handle_key(&mut self, key: Key, modified_key: Key, mods: Modifiers) -> Task<Msg> {
        // Changelog is a modal overlay; Escape returns to Settings.
        if self.show_changelog {
            if matches!(key, Key::Named(Named::Escape)) {
                self.show_changelog = false;
                self.app.modal = Modal::Settings;
            }
            return Task::none();
        }
        if !matches!(self.app.modal, Modal::None) {
            return self.handle_modal_key(key, mods);
        }
        // No modal open, but a kill-confirmation or split-agent menu can still
        // be armed — today both are cleared only by a mouse message, so a
        // keyboard user had no way out. Escape dismisses them here; with
        // neither armed, Escape falls through to the PTY below (many TUI
        // programs need it, so it must not be swallowed unconditionally).
        // Bare Escape only: Alt+Escape is a real readline chord and must reach
        // the PTY as ESC ESC even while something is armed.
        if matches!(key, Key::Named(Named::Escape))
            && mods.is_empty()
            && escape_should_dismiss(self.pending_kill, self.open_agent_menu)
        {
            self.pending_kill = None;
            self.open_agent_menu = None;
            return Task::none();
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
                return Task::none();
            }
            if is_paste_shortcut(mods, s) {
                // Wayland has no native file drag-and-drop (winit gap), so if the
                // clipboard holds file URIs (e.g. "Copy" from a file manager),
                // type their paths like a drop would. Falls through to text paste
                // otherwise.
                let paths = super::drop::clipboard_paths();
                if !paths.is_empty() {
                    if let Some(sess) = self.focused_session_mut() {
                        for path in &paths {
                            sess.send(super::drop::dropped_path_text(path).as_bytes());
                        }
                    }
                    self.clear_pty_selection();
                    return Task::none();
                }
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
                self.clear_pty_selection();
                return Task::none();
            }
        }
        // Global app shortcuts (Cmd on macOS, Ctrl+Shift elsewhere). Checked
        // after copy/paste so those keep their exact existing semantics, and
        // before key_to_bytes so the chords never leak into the PTY. Screen-
        // scoped so a Grid-only (etc.) chord falls through instead of being
        // swallowed on a screen where it does nothing (see `Scope`).
        if let Some(sc) = match_global_shortcut(&key, mods, self.current_screen()) {
            return self.run_global_shortcut(sc);
        }
        // Resize the terminal panel with Ctrl+Shift+Left/Right — matches the
        // registry's display-only "resize terminal panel" row (Workspace-only),
        // kept out of `match_global_shortcut` because `term_panel_open` is
        // runtime state, not scope: unlike every other registry shortcut, a
        // closed panel must fall through to the PTY rather than being consumed,
        // so the open-check has to gate the fallthrough itself instead of living
        // inside an arm that's already committed to returning `Task::none()`.
        if self.term_panel_open {
            if let Some(delta) = term_panel_resize_delta(&key, mods, self.current_screen()) {
                self.adjust_term_panel_portion(delta);
                return Task::none();
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
            self.clear_pty_selection();
        }
        Task::none()
    }

    /// Clear an in-progress PTY selection together with its drag anchor. A
    /// keypress mid-drag must kill both, or `tick_drag_autoscroll` keeps
    /// scrolling the pane with no visible selection until mouse-up (Bug 8).
    fn clear_pty_selection(&mut self) {
        self.pty_selection = None;
        self.pty_drag = None;
    }

    /// Coarse current screen, derived from existing flags. Zen wins over grid:
    /// while chrome is hidden the user is in zen regardless of `grid_view`.
    pub(crate) fn current_screen(&self) -> Screen {
        screen_from_flags(self.app.chrome_visible, self.grid_view)
    }

    fn run_global_shortcut(&mut self, sc: GlobalShortcut) -> Task<Msg> {
        match sc {
            GlobalShortcut::NewSession => self.update(Msg::OpenSessionLauncher),
            GlobalShortcut::NewSessionInWorktree => {
                let Some(s) = self.focused_session() else {
                    return Task::none();
                };
                let project = s.project.clone();
                let wt_path = s.wt_path.clone();
                let before = self.session_keys();
                if let Some(at) = self.app.launch_or_pick(project, wt_path) {
                    self.resize_new_sessions(&before);
                    self.leave_terminal_tab();
                    if self.grid_view {
                        crate::gui::launcher::insert_into_tile_order(&mut self.tile_order, at);
                        self.persist_grid_order();
                        self.set_grid_focus(Some(at));
                        self.refresh_pty_viewport();
                    }
                    self.rebuild_wt_cache();
                }
                Task::none()
            }
            GlobalShortcut::Settings => self.update(Msg::OpenSettings),
            GlobalShortcut::ToggleZen => self.update(Msg::ToggleZen),
            GlobalShortcut::ToggleGrid => self.update(Msg::ToggleGridView),
            GlobalShortcut::ZoomIn => self.update(Msg::ZoomIn),
            GlobalShortcut::ZoomOut => self.update(Msg::ZoomOut),
            GlobalShortcut::ZoomReset => self.update(Msg::ZoomReset),
            GlobalShortcut::NextSession => {
                self.cycle_session(1);
                Task::none()
            }
            GlobalShortcut::PrevSession => {
                self.cycle_session(-1);
                Task::none()
            }
            GlobalShortcut::SelectSession(n) => {
                self.select_visible_session(n);
                Task::none()
            }
            GlobalShortcut::ShortcutOverlay => self.update(Msg::OpenShortcutOverlay),
            GlobalShortcut::CloseFocusedSession => {
                // Grid: the focused tile. Everywhere else (tree/activity
                // sidebar, zen): the active session, whose row renders the same
                // confirm-to-kill state (`session_row` in rows.rs).
                let target = if self.grid_view {
                    self.grid_focused
                } else {
                    self.app.active_session
                };
                match close_focused_session_decision(target, self.pending_kill) {
                    CloseFocusedDecision::Kill(si) => self.update(Msg::KillSession(si)),
                    CloseFocusedDecision::Request(si) => self.update(Msg::RequestKillSession(si)),
                    CloseFocusedDecision::NoOp => Task::none(),
                }
            }
            GlobalShortcut::GridMove(dx, dy) => {
                crate::telemetry::track("tile_moved", vec![]);
                self.grid_move(dx, dy);
                Task::none()
            }
            GlobalShortcut::GridSwap(dx, dy) => {
                self.grid_swap(dx, dy);
                Task::none()
            }
        }
    }

    /// Keep `grid_focused` pointed at the active session whenever it changes
    /// while the grid is showing, or will show again once zen exits
    /// (`grid_view_before_zen`). Without this, cycling/selecting sessions
    /// while zenned in from a tile leaves the tile pointer stale, so exiting
    /// zen restores focus to the wrong tile (Bug 5).
    fn sync_grid_focus(&mut self) {
        if should_sync_grid_focus(self.grid_view, self.grid_view_before_zen) {
            self.set_grid_focus(self.app.active_session);
        }
    }

    /// Point `grid_focused` at a (possibly different) tile. Any selection
    /// anchored to the previously focused tile is stale once focus moves —
    /// it would paint on, and copy from, the wrong session — so clear it.
    fn set_grid_focus(&mut self, focus: Option<usize>) {
        if self.grid_focused != focus {
            self.pty_selection = None;
        }
        self.grid_focused = focus;
    }

    /// Cycle the focused session in visible order: `tile_order` while the
    /// grid is open, the sessions list otherwise.
    fn cycle_session(&mut self, delta: i32) {
        if self.grid_view {
            if self.tile_order.is_empty() {
                return;
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
            self.sync_grid_focus();
            self.acknowledge_session(si);
            return;
        }
        if self.app.sessions.is_empty() {
            return;
        }
        let next = match self.app.active_session {
            Some(cur) => crate::app::cycle(cur, delta, self.app.sessions.len()),
            None if delta > 0 => 0,
            None => self.app.sessions.len() - 1,
        };
        // Reuse SelectSession so resize / acknowledge / sidebar sync all apply.
        let _ = self.update(Msg::SelectSession(next));
    }

    /// Move keyboard focus between grid tiles directionally (mod+h/j/k/l,
    /// mod+arrows). Grid-only; no-ops if there's nothing to focus or the move
    /// would fall off the edge of the tile layout (see `grid_neighbor`).
    fn grid_move(&mut self, dx: i32, dy: i32) {
        if self.tile_order.is_empty() {
            return;
        }
        let cur = self
            .grid_focused
            .and_then(|si| self.tile_order.iter().position(|&x| x == si));
        let pos = match cur {
            Some(p) => p,
            None => {
                let si = self.tile_order[0];
                self.app.active_session = Some(si);
                self.sync_grid_focus();
                self.acknowledge_session(si);
                return;
            }
        };
        let Some(target) = grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        let si = self.tile_order[target];
        self.app.active_session = Some(si);
        self.sync_grid_focus();
        self.acknowledge_session(si);
    }

    /// Swap the focused tile with its neighbor (mod+alt+h/j/k/l, mod+alt+
    /// arrows). Grid-only; no-ops if there's nothing to focus or the swap
    /// would fall off the edge of the tile layout (see `grid_neighbor`).
    /// Leaves `grid_focused`/`active_session` untouched — both hold a session
    /// index, not a tile-order position, so focus stays on the same session
    /// after its tile moves.
    fn grid_swap(&mut self, dx: i32, dy: i32) {
        let Some(pos) = self
            .grid_focused
            .and_then(|si| self.tile_order.iter().position(|&x| x == si))
        else {
            return;
        };
        let Some(target) = grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        crate::gui::launcher::swap_tiles(&mut self.tile_order, pos, target);
        self.begin_grid_slide(pos, target);
        self.persist_grid_order();
        self.refresh_pty_viewport();
    }

    /// Select the Nth session in visible order (mod+1..9).
    fn select_visible_session(&mut self, n: usize) {
        if self.grid_view {
            if let Some(&si) = self.tile_order.get(n) {
                self.app.active_session = Some(si);
                self.sync_grid_focus();
                self.acknowledge_session(si);
            }
            return;
        }
        // Outside the agent grid, `mod+1..9` follows the sidebar's on-screen
        // order (activity grouping or tree layout) rather than raw session
        // index, so the number the user sees is the session they get.
        if let Some(&si) = self.visible_session_order().get(n) {
            let _ = self.update(Msg::SelectSession(si));
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

    /// Keyboard handling for the remove-project modal: Esc/n cancel, y
    /// confirms (Enter deliberately does not), Space toggles the
    /// delete-worktrees checkbox. Ignored while removal is in flight.
    fn handle_remove_project_key(&mut self, key: Key, busy: bool) -> Task<Msg> {
        if busy {
            return Task::none();
        }
        match key {
            Key::Named(Named::Escape) => self.cancel_modal(),
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

    fn handle_modal_key(&mut self, key: Key, mods: Modifiers) -> Task<Msg> {
        match &self.app.modal {
            // Text entry, caret movement, selection, and paste are owned by the
            // `text_input` widgets. The subscription only drives the directory
            // match list and modal lifecycle.
            Modal::Input { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_modal_input(),
                Key::Character(s) if mods.control() && matches!(s.as_str(), "c" | "C") => {
                    self.cancel_modal()
                }
                _ => {}
            },
            Modal::AddProject { step, .. } => match step {
                AddProjectStep::PickSource => match key {
                    Key::Named(Named::Escape) => self.cancel_modal(),
                    Key::Named(Named::Enter) => {
                        self.app.add_project_choose_typed();
                        return self.focus_add_project_field();
                    }
                    Key::Named(Named::ArrowDown) => self.app.add_project_dir_move(1),
                    Key::Named(Named::ArrowUp) => self.app.add_project_dir_move(-1),
                    Key::Named(Named::Tab) => {
                        // Tab completes the path in the buffer; move the caret to
                        // the end so subsequent typing appends instead of
                        // inserting where the caret happened to sit before
                        // completion.
                        self.app.add_project_dir_pick();
                        return move_cursor_to_end(crate::gui::view::modal_input_id());
                    }
                    Key::Character(s) if mods.control() && matches!(s.as_str(), "c" | "C") => {
                        self.cancel_modal()
                    }
                    _ => {}
                },
                AddProjectStep::Details => match key {
                    // Esc is a cheap undo back to pick-source; a second Esc
                    // (from step 1) cancels the modal outright.
                    Key::Named(Named::Escape) => {
                        self.app.add_project_change_source();
                        return focus(crate::gui::view::modal_input_id());
                    }
                    Key::Named(Named::Enter) => {
                        if let Err(e) = self.app.submit_add_project() {
                            self.app.modal = Modal::Message(format!("add project failed: {e}"));
                        }
                        self.rebuild_wt_cache();
                    }
                    Key::Character(s) if mods.control() && matches!(s.as_str(), "c" | "C") => {
                        self.cancel_modal()
                    }
                    _ => {}
                },
            },
            Modal::Confirm { .. } => match key {
                Key::Named(Named::Escape) => return self.confirm_modal_response(false),
                Key::Named(Named::Enter) => return self.confirm_modal_response(true),
                Key::Character(s) => match s.as_str() {
                    "y" | "Y" => return self.confirm_modal_response(true),
                    "n" | "N" => return self.confirm_modal_response(false),
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
            Modal::SessionLauncher {
                proj,
                wt,
                agent,
                col,
                ..
            } => {
                let (proj, wt, agent, col) = (*proj, *wt, *agent, *col);
                // Vim parity: h/l mirror ←/→, j/k mirror ↓/↑.
                let nav_h: Option<i32> = match &key {
                    Key::Named(Named::ArrowLeft) => Some(-1),
                    Key::Named(Named::ArrowRight) => Some(1),
                    Key::Character(s) if matches!(s.as_str(), "h" | "H") => Some(-1),
                    Key::Character(s) if matches!(s.as_str(), "l" | "L") => Some(1),
                    _ => None,
                };
                let nav_v: Option<i32> = match &key {
                    Key::Named(Named::ArrowDown) => Some(1),
                    Key::Named(Named::ArrowUp) => Some(-1),
                    Key::Character(s) if matches!(s.as_str(), "j" | "J") => Some(1),
                    Key::Character(s) if matches!(s.as_str(), "k" | "K") => Some(-1),
                    _ => None,
                };
                if let Some(delta) = nav_h {
                    if let Modal::SessionLauncher { col, .. } = &mut self.app.modal {
                        *col = crate::gui::launcher::move_column(*col, delta);
                    }
                } else if let Some(delta) = nav_v {
                    let proj_len = self.app.store.projects.len();
                    let wt_len = self.launcher_worktrees(proj).len();
                    let agent_len = self.app.available_agents.len();
                    let (np, nw, na) = crate::gui::launcher::nav_within_column(
                        col, proj, wt, agent, delta, proj_len, wt_len, agent_len,
                    );
                    // A project change reloads that project's worktrees.
                    if col == 0 && np != proj {
                        self.ensure_wt_cached(np);
                    }
                    if let Modal::SessionLauncher {
                        proj, wt, agent, ..
                    } = &mut self.app.modal
                    {
                        *proj = np;
                        *wt = nw;
                        *agent = na;
                    }
                } else {
                    match key {
                        Key::Named(Named::Escape) => self.cancel_modal(),
                        Key::Named(Named::Enter) => self.launcher_start(),
                        Key::Named(Named::Space) => self.launcher_toggle_default(),
                        _ => {}
                    }
                }
            }
            Modal::Settings => {
                if matches!(key, Key::Named(Named::Escape)) {
                    self.app.modal = Modal::None;
                }
            }
            Modal::Updating => {
                if matches!(key, Key::Named(Named::Escape))
                    && !matches!(self.upgrade, UpgradeState::Updating(_))
                {
                    self.app.modal = Modal::None;
                }
            }
            Modal::TmuxChoice => match key {
                Key::Named(Named::Enter) => self.choose_tmux(true),
                // Esc dismisses without persisting, so the choice is re-asked
                // on the next launch. Only explicit picks record a backend.
                Key::Named(Named::Escape) => self.app.modal = Modal::None,
                Key::Character(s) => match s.as_str() {
                    "t" | "T" | "y" | "Y" => self.choose_tmux(true),
                    "n" | "N" => self.choose_tmux(false),
                    _ => {}
                },
                _ => {}
            },
            Modal::Onboarding { step, .. } => {
                let step = *step;
                match key {
                    Key::Named(Named::Escape) => self.onboard_skip(),
                    Key::Named(Named::Enter) => return self.onboard_advance(),
                    Key::Named(Named::ArrowDown) => match step {
                        crate::app::OnboardStep::Project => self.app.onboard_dir_move(1),
                        crate::app::OnboardStep::Theme => self.app.onboard_theme_move(1),
                        _ => {}
                    },
                    Key::Named(Named::ArrowUp) => match step {
                        crate::app::OnboardStep::Project => self.app.onboard_dir_move(-1),
                        crate::app::OnboardStep::Theme => self.app.onboard_theme_move(-1),
                        _ => {}
                    },
                    Key::Named(Named::Tab) => match step {
                        crate::app::OnboardStep::Project => {
                            if self.app.onboard_toggle_project_focus() {
                                return focus(crate::gui::view::modal_name_id());
                            }
                            self.app.onboard_dir_pick();
                            return Task::batch([
                                focus(crate::gui::view::modal_input_id()),
                                move_cursor_to_end(crate::gui::view::modal_input_id()),
                            ]);
                        }
                        crate::app::OnboardStep::Theme => self.app.onboard_theme_switch_tab(),
                        _ => {}
                    },
                    _ => {}
                }
            }
            Modal::ShortcutOverlay => {
                if matches!(key, Key::Named(Named::Escape))
                    || match_global_shortcut(&key, mods, self.current_screen())
                        == Some(GlobalShortcut::ShortcutOverlay)
                {
                    self.app.modal = Modal::None;
                }
            }
            // No handler arm here meant every key, including Escape, was
            // swallowed by the "any modal open" guard with no way to dismiss
            // from the keyboard (Bug 10).
            Modal::ScriptsEditor => {
                if matches!(key, Key::Named(Named::Escape)) {
                    // Same path as the Cancel button (`Msg::ScriptsEditorCancel`),
                    // so unsaved edits are discarded and `scripts_editor` is reset.
                    self.cancel_modal();
                }
            }
            Modal::Teardown => {
                if matches!(key, Key::Named(Named::Escape)) {
                    // `cancel_modal` already gates this by teardown stage: it
                    // skips a still-running script (mirroring "skip & remove"),
                    // dismisses once removal has finished (mirroring "close"),
                    // and is a no-op mid-removal — there's no button for that
                    // stage either, since an in-flight `git worktree remove`
                    // can't be safely interrupted.
                    self.cancel_modal();
                }
            }
            _ => {}
        }
        Task::none()
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
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        self.rebuild_wt_cache();
        // If the worktree-name input was launched from the session launcher,
        // and a new worktree/session was actually created, re-open the launcher.
        if self.pending_launcher_proj.is_some() {
            match &self.app.modal {
                Modal::None => self.reopen_launcher(),
                Modal::Input { .. } => {
                    // Validation note re-showed the input: keep the target
                    // parked so a later successful submit still re-opens the
                    // launcher.
                }
                _ => {
                    // Landed on some other modal (e.g. AgentPicker, or an
                    // init-git confirm prompt): that's no longer a plain
                    // worktree-name round-trip, so don't leave the launcher
                    // parked indefinitely — an unrelated later modal
                    // resolving to `Modal::None` must not spuriously re-open
                    // the launcher.
                    self.pending_launcher_proj = None;
                }
            }
        }
    }

    /// Resolve a Confirm modal. `ConfirmKind::Quit` is handled here (it needs
    /// an iced Task to exit); everything else delegates to the app layer.
    fn confirm_modal_response(&mut self, yes: bool) -> Task<Msg> {
        if matches!(
            self.app.modal,
            Modal::Confirm {
                kind: ConfirmKind::Quit,
                ..
            }
        ) {
            self.app.modal = Modal::None;
            if yes {
                return iced::exit();
            }
            return Task::none();
        }
        self.submit_modal_confirm(yes);
        Task::none()
    }

    fn submit_modal_confirm(&mut self, yes: bool) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_confirm(yes) {
            self.app.modal = Modal::Message(format!("action failed: {e}"));
        }
        self.resize_new_sessions(&before);
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        // The teardown PTY lives outside `app.sessions`, so resize it directly.
        if let Some(s) = self.app.teardown.as_mut().and_then(|t| t.session.as_mut()) {
            s.resize(self.pty_rows, self.pty_sess_cols);
        }
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

    /// Open the per-project lifecycle-scripts editor, seeding the three
    /// `text_editor` buffers from the project's stored scripts.
    fn open_scripts_editor(&mut self, proj: usize) {
        use iced::widget::text_editor::Content;
        let Some(p) = self.app.store.projects.get(proj) else {
            return;
        };
        self.scripts_editor = Some(ScriptsEditorState {
            proj,
            project_name: p.name.clone(),
            setup: Content::with_text(p.scripts.setup.as_deref().unwrap_or("")),
            run: Content::with_text(p.scripts.run.as_deref().unwrap_or("")),
            teardown: Content::with_text(p.scripts.teardown.as_deref().unwrap_or("")),
        });
        self.app.modal = Modal::ScriptsEditor;
    }

    /// Persist the edited scripts back to the project and close the editor. An
    /// empty/whitespace-only buffer clears that script (stored as `None`).
    fn save_scripts_editor(&mut self) {
        let Some(ed) = self.scripts_editor.take() else {
            self.app.modal = Modal::None;
            return;
        };
        let norm = |t: String| {
            let t = t.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };
        if let Some(p) = self.app.store.projects.get_mut(ed.proj) {
            p.scripts.setup = norm(ed.setup.text());
            p.scripts.run = norm(ed.run.text());
            p.scripts.teardown = norm(ed.teardown.text());
        }
        if let Err(e) = crate::storage::save(&self.app.store) {
            self.app.modal = Modal::Message(format!("failed to save scripts: {e}"));
            return;
        }
        self.app.set_toast("saved project scripts");
        self.app.modal = Modal::None;
    }

    /// After a choose-funnel attempt, focus whichever add-project field is now
    /// primary: the name field once the details step is showing, else the
    /// step-1 path input (the funnel rejected the folder).
    fn focus_add_project_field(&self) -> Task<Msg> {
        match &self.app.modal {
            Modal::AddProject {
                step: AddProjectStep::Details,
                ..
            } => focus(crate::gui::view::modal_name_id()),
            Modal::AddProject { .. } => focus(crate::gui::view::modal_input_id()),
            _ => Task::none(),
        }
    }

    fn cancel_modal(&mut self) {
        // The teardown modal repurposes cancel: skip a still-running script
        // (proceed to removal) or dismiss once removal has finished.
        if matches!(self.app.modal, Modal::Teardown) {
            match self.app.teardown.as_ref().map(|t| t.stage) {
                Some(crate::app::TeardownStage::Done { .. }) => self.app.close_teardown(),
                _ => self.app.skip_teardown_script(),
            }
            return;
        }
        self.scripts_editor = None;
        self.app.modal = Modal::None;
        // Cancelling any modal abandons the launcher "+ New worktree…"
        // round-trip if one was in flight; otherwise a later unrelated
        // `submit_modal_input` that lands on `Modal::None` would spuriously
        // re-open the session launcher.
        self.pending_launcher_proj = None;
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
                Ok(msg) if !msg.is_empty() => self.app.set_toast(msg),
                Err(e) => self.app.set_error_toast(format!("err: {e}")),
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
            Ok(msg) if !msg.is_empty() && !errors.is_empty() => {
                self.app
                    .set_error_toast(format!("{} ({} worktree errors)", msg, errors.len()))
            }
            Ok(msg) if !msg.is_empty() => self.app.set_toast(msg),
            Err(e) => self.app.set_error_toast(format!("err: {e}")),
            _ => {}
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
    /// When focusing an agent session, drop out of the terminal tab. `workspace()`
    /// checks `terminal_tab()` *before* `active_session`, so while `sidebar_view`
    /// stays `Terminal` the workspace — and zen mode — keep rendering the home
    /// terminal instead of the session the user just focused.
    fn leave_terminal_tab(&mut self) {
        if self.terminal_tab() {
            self.sidebar_view = SidebarView::Tree;
        }
    }

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
    /// Rewrite `collapsed`/`collapsed_wt` so the tree matches `self.tree_expand`.
    /// Fully overrides any manual per-row toggles.
    pub(super) fn apply_tree_expand(&mut self) {
        use crate::gui::state::TreeExpand;
        self.collapsed.clear();
        self.collapsed_wt.clear();
        match self.tree_expand {
            TreeExpand::All => {}
            TreeExpand::Collapsed => {
                for pi in 0..self.app.store.projects.len() {
                    self.collapsed.insert(pi);
                }
            }
            TreeExpand::SessionsOnly => {
                for pi in 0..self.app.store.projects.len() {
                    if !self.project_has_sessionful_worktree(pi) {
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
    }

    /// Every ~5s, shell out to `git status` (one call per worktree) for the
    /// worktrees currently visible in the tree — i.e. belonging to an
    /// expanded project — off the UI thread, and stash the results in
    /// `git_state` for `tree_view` to read on the next frame. Never runs more
    /// than once per throttle window, never overlaps a still-running poll,
    /// and never blocks `update()`: the actual git calls happen inside the
    /// spawned thread. Skipped entirely unless the tree view is the active
    /// sidebar view, since its suffix is the only consumer of `git_state`.
    fn maybe_poll_git_state(&mut self) {
        if !matches!(self.sidebar_view, SidebarView::Tree) {
            return;
        }
        let now = std::time::Instant::now();
        let due = self
            .last_git_poll
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5));
        if !due {
            return;
        }
        self.last_git_poll = Some(now);

        let paths = self.visible_worktree_paths();
        if paths.is_empty() {
            return;
        }
        if self
            .git_poll_inflight
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            // Previous poll is still running — skip this tick rather than
            // overlap it.
            return;
        }
        let handle = self.git_state.clone();
        let inflight = self.git_poll_inflight.clone();
        std::thread::spawn(move || {
            let mut fresh = std::collections::HashMap::new();
            let mut stale = Vec::new();
            for path in paths {
                match crate::git::worktree_git_state(&path) {
                    Some(state) => {
                        fresh.insert(path, state);
                    }
                    // Any failure (no repo, no upstream, git missing, bad
                    // worktree state) degrades to "no signal" — drop any
                    // previously cached value rather than showing stale data.
                    None => stale.push(path),
                }
            }
            if let Ok(mut g) = handle.lock() {
                g.extend(fresh);
                for path in stale {
                    g.remove(&path);
                }
            }
            inflight.store(false, std::sync::atomic::Ordering::Release);
        });
    }

    /// Paths of every worktree currently rendered in the tree view — i.e.
    /// every worktree belonging to a non-collapsed project. Matches exactly
    /// the set `tree_view` iterates when building worktree rows.
    fn visible_worktree_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for pi in 0..self.app.store.projects.len() {
            if self.collapsed.contains(&pi) {
                continue;
            }
            paths.extend(
                self.worktrees_for_project(pi)
                    .iter()
                    .map(|w| w.path.clone()),
            );
        }
        paths
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

    /// The worktrees backing launcher project `proj`: the live `app.worktrees`
    /// when it is the active project, else the cached list (loaded on demand).
    pub(super) fn launcher_worktrees(&self, proj: usize) -> Vec<crate::git::Worktree> {
        if proj == self.app.proj_idx {
            self.app.worktrees.clone()
        } else {
            self.wt_cache.get(&proj).cloned().unwrap_or_default()
        }
    }

    /// Open the session launcher with a sensible default selection: the active
    /// project + worktree, agent index 0, skip-perms from the global default.
    fn open_session_launcher(&mut self) {
        if self.app.store.projects.is_empty() {
            self.app.set_toast("add a project first");
            return;
        }
        self.app.refresh_available_agents();
        let n = self.app.store.projects.len();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
        let proj = self.app.proj_idx.min(n - 1);
        let wt = self
            .app
            .wt_idx
            .min(self.launcher_worktrees(proj).len().saturating_sub(1));
        self.app.modal = crate::app::Modal::SessionLauncher {
            proj,
            wt,
            agent: 0,
            col: 0,
        };
    }

    /// Select a launcher project: reset the worktree selection and ensure that
    /// project's worktrees are loaded.
    fn launcher_select_project(&mut self, index: usize) {
        let n = self.app.store.projects.len();
        if index >= n {
            return;
        }
        self.ensure_wt_cached(index);
        if let crate::app::Modal::SessionLauncher { proj, wt, col, .. } = &mut self.app.modal {
            *proj = index;
            *wt = 0;
            *col = 0;
        }
    }

    /// "+ New worktree…": remember the launcher's project, switch to it, and
    /// open Grove's standard worktree-name input. After creation,
    /// `submit_modal_input` re-opens the launcher (see `reopen_launcher`).
    fn launcher_new_worktree(&mut self) -> Task<Msg> {
        let crate::app::Modal::SessionLauncher { proj, .. } = self.app.modal else {
            return Task::none();
        };
        if proj >= self.app.store.projects.len() {
            return Task::none();
        }
        self.pending_launcher_proj = Some(proj);
        self.switch_active_project(proj);
        // Mirror the sidebar "add worktree" entry point.
        self.app.focus = crate::app::Pane::Worktrees;
        self.app.start_add();
        focus(crate::gui::view::modal_input_id())
    }

    /// Re-open the launcher after a worktree was created from it. Selects the
    /// newly-created worktree (the last non-main entry) in the stashed project.
    fn reopen_launcher(&mut self) {
        let Some(proj) = self.pending_launcher_proj.take() else {
            return;
        };
        if proj >= self.app.store.projects.len() {
            return;
        }
        self.app.refresh_available_agents();
        self.ensure_wt_cached(proj);
        let worktrees = self.launcher_worktrees(proj);
        // The newest worktree is the last entry (git lists main first).
        let wt = worktrees.len().saturating_sub(1);
        self.app.modal = crate::app::Modal::SessionLauncher {
            proj,
            wt,
            agent: 0,
            col: 1,
        };
    }

    /// Space in the launcher: set (or clear, when re-selecting the current)
    /// the global default agent from the agent-column selection — the same
    /// affordance as Modal::AgentPicker's Space.
    fn launcher_toggle_default(&mut self) {
        let Modal::SessionLauncher { agent, .. } = self.app.modal else {
            return;
        };
        let Some(a) = self.app.available_agents.get(agent).copied() else {
            return;
        };
        if let Err(e) = self.app.set_default_agent(a) {
            self.app.modal = Modal::Message(format!("default agent failed: {e}"));
        }
    }

    /// Start the selected session, then (grid always open here) append it to
    /// `tile_order` and focus it.
    fn launcher_start(&mut self) {
        let crate::app::Modal::SessionLauncher {
            proj, wt, agent, ..
        } = self.app.modal.clone()
        else {
            return;
        };
        let Some(project) = self.app.store.projects.get(proj) else {
            return;
        };
        let pname = project.name.clone();
        let worktrees = self.launcher_worktrees(proj);
        let Some(w) = worktrees.get(wt).cloned() else {
            return;
        };
        let Some(ag) = self.app.available_agents.get(agent).copied() else {
            return;
        };
        let label = crate::gui::launcher::default_label(w.is_main, &pname, &w.path);
        let args = ag.launch_args(self.app.skip_permissions_enabled());
        let before = self.session_keys();
        self.app.modal = crate::app::Modal::None;
        // `at_end = true`: launcher sessions always land last in the sessions
        // vector so they appear at the end of the Agent View grid, even after a
        // tile_order rebuild (entering Agent View resets it to sessions order).
        let inserted =
            self.app
                .spawn_session(label, pname, w.path.clone(), ag, args, &w.path, true);
        self.resize_new_sessions(&before);
        if let Some(at) = inserted {
            self.leave_terminal_tab();
            if self.grid_view {
                crate::gui::launcher::insert_into_tile_order(&mut self.tile_order, at);
                self.persist_grid_order();
                self.set_grid_focus(Some(at));
                self.refresh_pty_viewport();
            }
        }
        self.rebuild_wt_cache();
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
        let args = agent.launch_args(self.app.skip_permissions_enabled());
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
                let open = self.app.sessions.len();
                let native = self
                    .app
                    .sessions
                    .iter()
                    .filter(|s| matches!(s.backend, crate::session::SessionBackend::Native))
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
            PtyPane::Tile(_) => return, // tile focus handled via grid_focused
        };
    }

    /// The session the workspace PTY is currently showing — and that keystrokes,
    /// scrolling, and selection target. The home terminal when the terminal tab
    /// is active, otherwise the active worktree session.
    pub(super) fn focused_session(&self) -> Option<&Session> {
        if self.grid_view {
            return self.grid_focused.and_then(|si| self.app.sessions.get(si));
        }
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
        if self.grid_view {
            return self
                .grid_focused
                .and_then(move |si| self.app.sessions.get_mut(si));
        }
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

/// Returns the current time as seconds since UNIX_EPOCH, or 0 on error.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn pixel_to_cell(x: f32, y: f32) -> PtyCell {
    let metrics = pty_metrics(1.0);
    PtyCell {
        row: (y / metrics.cell_h).max(0.0) as usize,
        col: (x / metrics.cell_w).max(0.0) as usize,
    }
}

/// The platform's global-shortcut modifier: Cmd on macOS (matching the Cmd+C /
/// Cmd+V pair), Ctrl+Shift elsewhere (matching Ctrl+Shift+C / Ctrl+Shift+V, so
/// plain Ctrl chords stay available to the PTY).
fn global_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

/// Modifier for "new session in current worktree": Cmd+Alt (mac) / Ctrl+Alt
/// (elsewhere), independent of [`global_mods`] (which already requires Shift
/// on non-mac and so can't be reused as a base for an Alt chord there).
fn new_session_in_worktree_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.logo() && mods.alt() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.alt() && !mods.shift();
}

/// Human-readable label for the global-shortcut modifier, matching
/// [`global_mods`]. Shown in the status-bar chip and the shortcut overlay so the
/// displayed text can't drift from the actual chord.
pub(crate) fn platform_mod_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl+shift"
    }
}

/// App-level actions reachable from the global keyboard layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalShortcut {
    NewSession,
    NewSessionInWorktree,
    Settings,
    ToggleZen,
    ToggleGrid,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    NextSession,
    PrevSession,
    SelectSession(usize),
    ShortcutOverlay,
    CloseFocusedSession,
    /// Move keyboard focus between grid tiles by `(dx, dy)`. Grid screen only.
    GridMove(i32, i32),
    /// Swap the focused tile with its neighbor by `(dx, dy)`. Grid screen only.
    GridSwap(i32, i32),
}

/// Coarse "which screen am I on" model, derived from existing UI flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    Grid,
    Workspace,
    Zen,
}

impl Screen {
    /// Section header label used in the overlay when >1 scope is visible.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Screen::Grid => "grid",
            Screen::Workspace => "workspace",
            Screen::Zen => "zen",
        }
    }
}

/// Where a shortcut applies. A shortcut may list several scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Screen(Screen),
}

/// One row of the shortcut registry — single source of truth for both
/// `match_global_shortcut` (behavior) and `shortcut_overlay_modal` (display).
pub(crate) struct ShortcutDef {
    /// `None` for the display-only `1–9` row (matcher handles it dynamically).
    pub(crate) action: Option<GlobalShortcut>,
    /// Key chars matched against iced's modifier-independent `key`. Empty for
    /// the display-only row. `Enter` is matched separately (see the matcher).
    pub(crate) triggers: &'static [&'static str],
    /// Key label shown in the overlay; the platform modifier is prepended at
    /// render time (e.g. `"n"` -> `"cmd+n"`).
    pub(crate) display_keys: &'static str,
    pub(crate) description: &'static str,
    pub(crate) scopes: &'static [Scope],
    /// When true, this shortcut layers Alt on top of the platform's global
    /// modifier (e.g. Cmd+Alt+N / Ctrl+Alt+N) rather than using the plain
    /// platform modifier. Rendered with an "+alt+" infix by the overlay.
    pub(crate) requires_alt: bool,
    /// When true, `display_keys` is the complete chord text and the overlay
    /// renders it verbatim instead of prepending the platform modifier. Used
    /// by the one shortcut that is the same literal chord on every platform
    /// (`Ctrl+Shift+Arrow`, unlike `mod`'s Cmd-on-mac / Ctrl+Shift-elsewhere).
    pub(crate) literal: bool,
}

const G: &[Scope] = &[Scope::Global];

/// Single source of truth for behavioral matching and overlay display. Order
/// matches the overlay's reading order. Most entries are `Global`; a few are
/// scoped to a single screen (see each row's `scopes`).
pub(crate) const SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef {
        action: Some(GlobalShortcut::NewSession),
        triggers: &["n", "N"],
        display_keys: "n",
        description: "new session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewSessionInWorktree),
        triggers: &["n", "N"],
        display_keys: "n",
        description: "new session in current worktree",
        scopes: G,
        requires_alt: true,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NextSession),
        triggers: &["j", "J"],
        display_keys: "j",
        description: "next session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::PrevSession),
        triggers: &["k", "K"],
        display_keys: "k",
        description: "previous session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: the matcher handles 1–9 dynamically (see match_global_shortcut).
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "1–9",
        description: "select nth session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleGrid),
        triggers: &["g", "G"],
        display_keys: "g",
        description: "toggle grid view",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleZen),
        triggers: &[],
        display_keys: "enter",
        description: "toggle zen mode",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::Settings),
        triggers: &[","],
        display_keys: ",",
        description: "settings",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomIn),
        triggers: &["=", "+"],
        display_keys: "=",
        description: "zoom in",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomOut),
        triggers: &["-", "_"],
        display_keys: "-",
        description: "zoom out",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomReset),
        triggers: &["0"],
        display_keys: "0",
        description: "reset zoom",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ShortcutOverlay),
        triggers: &["/", "?"],
        display_keys: "/",
        description: "this overlay",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::CloseFocusedSession),
        triggers: &["w", "W"],
        display_keys: "w",
        description: "close focused session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: `match_global_shortcut` handles both of these rows ahead
    // of the registry lookup (dynamic dx/dy per key, Alt picks move vs. swap),
    // scoped to Screen::Grid by hand there — keep the three in sync.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "h j k l / ←↓↑→",
        description: "move focus in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "alt+h j k l / ←↓↑→",
        description: "move tile in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        requires_alt: true,
        literal: false,
    },
    // Display-only: matched by `term_panel_resize_delta`, not
    // `match_global_shortcut` — closing the panel must fall through to the
    // PTY, which a registry-matched shortcut never does (see the guard's
    // comment in `handle_key`). Listed here purely so it's scoped and
    // discoverable in the `mod+/` overlay; `scopes` here must track
    // `term_panel_resize_delta`'s `Screen::Workspace` check by hand.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "ctrl+shift+←/→",
        description: "resize terminal panel",
        scopes: &[Scope::Screen(Screen::Workspace)],
        requires_alt: false,
        literal: true,
    },
];

/// Derive the coarse screen from UI flags. Zen wins over grid: while chrome is
/// hidden the user is in zen regardless of `grid_view`.
pub(crate) fn screen_from_flags(chrome_visible: bool, grid_view: bool) -> Screen {
    if !chrome_visible {
        Screen::Zen
    } else if grid_view {
        Screen::Grid
    } else {
        Screen::Workspace
    }
}

/// True if a shortcut whose registry row lists `scopes` may fire on `screen`:
/// always for `Global`, otherwise only on its matching `Screen(screen)` entry.
/// Shared by the matcher (behavior) and `shortcut_overlay_modal` (display) so
/// the two can never disagree about what's visible/active on a given screen.
pub(crate) fn scope_allows(scopes: &[Scope], screen: Screen) -> bool {
    scopes
        .iter()
        .any(|s| matches!(s, Scope::Global) || *s == Scope::Screen(screen))
}

/// Map a key event to a global shortcut, or `None` if the chord doesn't match
/// or its registry row is out of scope on `screen` — callers must fall
/// through to the PTY on `None` rather than treat it as consumed. Matches
/// iced's modifier-independent `key`, so Shift in the non-mac Ctrl+Shift
/// chords doesn't change the character being compared.
fn match_global_shortcut(key: &Key, mods: Modifiers, screen: Screen) -> Option<GlobalShortcut> {
    // Checked ahead of `global_mods`: on non-mac, `global_mods` already
    // requires Shift, so Ctrl+Alt+N (no Shift) would never reach it. This
    // chord is Cmd+Alt+N (mac) / Ctrl+Alt+N (elsewhere), independent of the
    // platform's base global-shortcut modifier.
    //
    // On mac this early check is technically redundant now that the registry
    // lookup below honors `requires_alt`: `global_mods` there is just
    // `logo() && !control()`, which Cmd+Alt+N already satisfies, so the
    // registry `.find()` alone would resolve it to `NewSessionInWorktree`.
    // It still has to stay because non-mac needs it — `global_mods` there
    // requires Shift, which Ctrl+Alt+N (no Shift) never has, so non-mac
    // can't reach the registry lookup at all for this chord.
    if new_session_in_worktree_mods(mods) {
        if let Key::Character(s) = key {
            if s.eq_ignore_ascii_case("n") {
                // Global today, but scope-checked like everything else below
                // rather than bypassing it, so a future rescoping can't be
                // missed here.
                let scopes = SHORTCUTS
                    .iter()
                    .find(|d| d.action == Some(GlobalShortcut::NewSessionInWorktree))
                    .map(|d| d.scopes)
                    .unwrap_or(G);
                if scope_allows(scopes, screen) {
                    return Some(GlobalShortcut::NewSessionInWorktree);
                }
            }
        }
    }
    if !global_mods(mods) {
        return None;
    }
    // Grid-only directional focus move. Checked ahead of the registry lookup
    // so it shadows the global `mod+j`/`mod+k` NextSession/PrevSession
    // bindings on this screen only — those two rows, and every other screen,
    // are untouched.
    if screen == Screen::Grid {
        let dir = match key {
            Key::Character(s) => match s.as_str() {
                "h" | "H" => Some((-1, 0)),
                "l" | "L" => Some((1, 0)),
                "k" | "K" => Some((0, -1)),
                "j" | "J" => Some((0, 1)),
                _ => None,
            },
            Key::Named(Named::ArrowLeft) => Some((-1, 0)),
            Key::Named(Named::ArrowRight) => Some((1, 0)),
            Key::Named(Named::ArrowUp) => Some((0, -1)),
            Key::Named(Named::ArrowDown) => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = dir {
            return Some(if mods.alt() {
                GlobalShortcut::GridSwap(dx, dy)
            } else {
                GlobalShortcut::GridMove(dx, dy)
            });
        }
    }
    match key {
        // Not registry-`.find()`-driven like the char rows below (it's a
        // `Key::Named`, not a `Key::Character`), but still scope-checked
        // against its row (`G` today) for the same reason as the Alt chord
        // above.
        Key::Named(Named::Enter) => scope_allows(G, screen).then_some(GlobalShortcut::ToggleZen),
        Key::Character(s) => {
            let s = s.as_str();
            // Registry-driven character shortcuts. `requires_alt` must be part
            // of the match, not just display metadata: `NewSession` and
            // `NewSessionInWorktree` share `triggers` and only differ by Alt,
            // so without this the first row in array order would always win
            // (Bug 7) — swapping the two rows would silently swap their
            // meaning with no compiler error and no failing test.
            if let Some(def) = SHORTCUTS.iter().find(|d| {
                d.action.is_some() && d.triggers.contains(&s) && d.requires_alt == mods.alt()
            }) {
                return def.action.filter(|_| scope_allows(def.scopes, screen));
            }
            // SelectNth stays special-cased: dynamic n, display-only in
            // registry (`G` — scope-checked for the same reason as above).
            s.parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n) && scope_allows(G, screen))
                .map(|n| GlobalShortcut::SelectSession(n - 1))
        }
        _ => None,
    }
}

/// Result of [`close_focused_session_decision`]: what `CloseFocusedSession`
/// should do given the current confirm-to-kill state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseFocusedDecision {
    /// No session is focused.
    NoOp,
    /// First press: arm the confirm-to-kill state for this session.
    Request(usize),
    /// Second press while armed for this session: actually kill it.
    Kill(usize),
}

/// Pure decision logic for `GlobalShortcut::CloseFocusedSession`, mirroring the
/// close-button toggle on both the grid tile (`grid_tile`) and the sidebar row
/// (`session_row`). Kept as a free function so it's testable without
/// constructing a full `Grove`. `target` is whichever session the current
/// screen considers focused; the caller resolves it.
fn close_focused_session_decision(
    target: Option<usize>,
    pending_kill: Option<usize>,
) -> CloseFocusedDecision {
    match target {
        Some(si) if pending_kill == Some(si) => CloseFocusedDecision::Kill(si),
        Some(si) => CloseFocusedDecision::Request(si),
        None => CloseFocusedDecision::NoOp,
    }
}

/// New value for `grid_focused` after the active session changes, given
/// whether the grid is showing or will show again once zen exits. `None`
/// means "leave `grid_focused` alone" — outside the grid (and not zenned in
/// from it) there's no tile to track. Kept as a free function so it's
/// testable without constructing a full `Grove` (Bug 5).
fn should_sync_grid_focus(grid_view: bool, grid_view_before_zen: bool) -> bool {
    grid_view || grid_view_before_zen
}

/// Tile index reached by moving `(dx, dy)` from tile `i` in a grid of `n`
/// tiles, or `None` if there's no such tile. Tiles are numbered row-major
/// (`tile_idx = row * cols + col`, see `grid_layout`/`grid_workspace`) but
/// rendered into per-column containers that skip any `tile_idx >= n`, so a
/// short column simply stacks the tiles it has, full height. E.g. n=3 gives
/// cols=2, rows=2: the left column shows tiles 0 (top) and 2 (bottom); the
/// right column shows only tile 1, spanning the full height.
///
/// Vertical moves (`dx == 0`) require the naive target index to exist —
/// there's no "nearest tile in that column" fallback, since the columns
/// don't share a row grid. Horizontal moves (`dy == 0`) instead clamp the row
/// downward to the largest row `<= target_row` that has a tile in the target
/// column, matching what's visually below the cursor's row.
pub(crate) fn grid_neighbor(i: usize, n: usize, dx: i32, dy: i32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (cols, _rows) = crate::gui::metrics::grid_layout(n);
    let cols = cols as i32;
    let row = i as i32 / cols;
    let col = i as i32 % cols;
    let target_col = col + dx;
    if target_col < 0 || target_col >= cols {
        return None;
    }
    if dx == 0 {
        let target_row = row + dy;
        if target_row < 0 {
            return None;
        }
        let idx = target_row * cols + target_col;
        return (idx >= 0 && (idx as usize) < n).then_some(idx as usize);
    }
    // Horizontal move: clamp the row downward to the largest row that still
    // has a tile in the target column.
    let mut r = row;
    loop {
        if r < 0 {
            return None;
        }
        let idx = r * cols + target_col;
        if idx >= 0 && (idx as usize) < n {
            return Some(idx as usize);
        }
        r -= 1;
    }
}

/// Duration of the draw-only tile-slide animation triggered by a grid
/// reorder (drag or keyboard swap).
pub(crate) const GRID_SLIDE: Duration = Duration::from_millis(150);

/// Timing curve of the tile slide. `lilt` (iced's animation crate) exposes
/// its easings as plain `fn(f32) -> f32`, so the curve is a one-word swap —
/// see `iced::animation::Easing` for the full set, incl. `Custom` for a
/// hand-rolled cubic-bezier if a named curve ever stops being enough.
const GRID_SLIDE_EASING: iced::animation::Easing = iced::animation::Easing::EaseOutCubic;

/// Eased progress `[0, 1]` for a `GRID_SLIDE`-duration animation that started
/// at `start`, evaluated at `now`.
pub(crate) fn slide_progress(start: std::time::Instant, now: std::time::Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start);
    if elapsed >= GRID_SLIDE {
        return 1.0;
    }
    GRID_SLIDE_EASING.value(elapsed.as_secs_f32() / GRID_SLIDE.as_secs_f32())
}

/// Whether the event subscription forwards this event to `update()`.
///
/// Captured events belong to the widget that consumed them — except Escape: a
/// focused `text_input` captures it only to blur itself and never tells the
/// app, so without this carve-out cancelling a modal would take two presses.
fn should_forward(ev: &Event, status: event::Status) -> bool {
    if status != event::Status::Captured {
        return true;
    }
    matches!(
        ev,
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        })
    )
}

/// Whether Escape has something to dismiss when no modal is open. `false`
/// means Escape must reach the PTY — many TUI programs need it, and
/// swallowing it unconditionally would regress that. The caller clears both
/// states, so which one is armed doesn't matter.
fn escape_should_dismiss(
    pending_kill: Option<usize>,
    open_agent_menu: Option<(usize, usize)>,
) -> bool {
    pending_kill.is_some() || open_agent_menu.is_some()
}

/// Chord + scope check for the terminal-panel resize (see the registry's
/// display-only "resize terminal panel" row): Ctrl+Shift+Left/Right, Workspace
/// only, on every platform (unlike `global_mods`, this isn't Cmd on macOS).
/// Doesn't know about `term_panel_open` — that's runtime state the caller
/// gates separately so a closed panel falls through to the PTY.
fn term_panel_resize_delta(key: &Key, mods: Modifiers, screen: Screen) -> Option<i16> {
    if screen != Screen::Workspace || !(mods.control() && mods.shift()) {
        return None;
    }
    match key {
        Key::Named(Named::ArrowRight) => Some(TERM_PANEL_PORTION_STEP as i16),
        Key::Named(Named::ArrowLeft) => Some(-(TERM_PANEL_PORTION_STEP as i16)),
        _ => None,
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
/// Others: Ctrl+Shift+V (mirrors the Ctrl+Shift+C copy shortcut; plain
/// Ctrl+V is left for the PTY, e.g. literal insert in vim/readline).
fn is_paste_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("v") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

#[cfg(test)]
mod tests {
    use super::{match_global_shortcut, slide_progress, GlobalShortcut, Screen, GRID_SLIDE};
    use iced::keyboard::{key::Named, Key, Modifiers};
    use smol_str::SmolStr;

    /// The platform's global modifier: Cmd on macOS, Ctrl+Shift elsewhere.
    fn gmods() -> Modifiers {
        #[cfg(target_os = "macos")]
        return Modifiers::LOGO;
        #[cfg(not(target_os = "macos"))]
        return Modifiers::CTRL | Modifiers::SHIFT;
    }

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }

    #[test]
    fn global_shortcuts_map_with_platform_modifier() {
        // All of these are `Global`-scoped, so Workspace is an arbitrary pick —
        // `screen_scoped_shortcuts_respect_scopes` below covers the Grid-only row.
        use GlobalShortcut::*;
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("n"), gmods(), screen),
            Some(NewSession)
        );
        assert_eq!(
            match_global_shortcut(&ch(","), gmods(), screen),
            Some(Settings)
        );
        assert_eq!(
            match_global_shortcut(&ch("g"), gmods(), screen),
            Some(ToggleGrid)
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(NextSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(PrevSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("="), gmods(), screen),
            Some(ZoomIn)
        );
        assert_eq!(
            match_global_shortcut(&ch("-"), gmods(), screen),
            Some(ZoomOut)
        );
        assert_eq!(
            match_global_shortcut(&ch("0"), gmods(), screen),
            Some(ZoomReset)
        );
        assert_eq!(
            match_global_shortcut(&ch("3"), gmods(), screen),
            Some(SelectSession(2))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::Enter), gmods(), screen),
            Some(ToggleZen)
        );
        assert_eq!(
            match_global_shortcut(&ch("/"), gmods(), screen),
            Some(ShortcutOverlay)
        );
        // Registry-driven aliases.
        assert_eq!(
            match_global_shortcut(&ch("+"), gmods(), screen),
            Some(ZoomIn)
        );
        assert_eq!(
            match_global_shortcut(&ch("_"), gmods(), screen),
            Some(ZoomOut)
        );
        assert_eq!(
            match_global_shortcut(&ch("?"), gmods(), screen),
            Some(ShortcutOverlay)
        );
    }

    /// `mod+w` closes the focused session on every screen: the grid tile in
    /// Grid, the active session's sidebar row otherwise. It must never fall
    /// through to the PTY, where `key_to_bytes` would turn Ctrl+Shift+W into
    /// `0x17` (readline delete-word) on Linux and a literal `w` on macOS.
    #[test]
    fn close_focused_session_matches_on_every_screen() {
        use GlobalShortcut::CloseFocusedSession;
        for screen in [Screen::Grid, Screen::Workspace, Screen::Zen] {
            assert_eq!(
                match_global_shortcut(&ch("w"), gmods(), screen),
                Some(CloseFocusedSession)
            );
            assert_eq!(
                match_global_shortcut(&ch("W"), gmods(), screen),
                Some(CloseFocusedSession)
            );
        }
    }

    /// The real "new session in worktree" chord: Cmd+Alt (mac) / Ctrl+Alt
    /// (elsewhere) — independent of `gmods()`, which on non-mac already
    /// includes Shift and would mask a regression back to requiring it.
    fn alt_mods() -> Modifiers {
        #[cfg(target_os = "macos")]
        return Modifiers::LOGO | Modifiers::ALT;
        #[cfg(not(target_os = "macos"))]
        return Modifiers::CTRL | Modifiers::ALT;
    }

    #[test]
    fn alt_n_maps_to_new_session_in_worktree() {
        use GlobalShortcut::*;
        let alt = alt_mods();
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("n"), alt, screen),
            Some(NewSessionInWorktree)
        );
        assert_eq!(
            match_global_shortcut(&ch("N"), alt, screen),
            Some(NewSessionInWorktree)
        );
        // Plain platform modifier (no Alt) still resolves to NewSession —
        // no regression from adding the alt-chord.
        assert_eq!(
            match_global_shortcut(&ch("n"), gmods(), screen),
            Some(NewSession)
        );
        // Alt held on an unclaimed key is *not* a shortcut on either platform:
        // the registry now requires an exact `requires_alt` match (Bug 7's
        // fix), and `ToggleGrid`'s row has `requires_alt: false`, so holding
        // Alt no longer falls through to it even on mac, where `alt_mods()`
        // (Cmd+Alt) still satisfies `global_mods` (Cmd, no Ctrl). On non-mac,
        // `alt_mods()` is Ctrl+Alt with no Shift, which `global_mods`
        // (Ctrl+Shift) rejects outright, so it never even reaches the registry.
        assert_eq!(match_global_shortcut(&ch("g"), alt, screen), None);
    }

    /// Pins Bug 7's fix directly, non-mac only: a chord that carries Shift (so
    /// `new_session_in_worktree_mods`'s `!shift()` fails and the early-check
    /// never fires) but still satisfies `global_mods` (Ctrl+Shift) and holds
    /// Alt must still resolve through the registry alone, proving the registry
    /// lookup — not just the early-check — is `requires_alt`-correct.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn registry_lookup_resolves_worktree_variant_when_early_check_is_bypassed() {
        use GlobalShortcut::*;
        let mods = Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT;
        assert_eq!(
            match_global_shortcut(&ch("n"), mods, Screen::Workspace),
            Some(NewSessionInWorktree)
        );
    }

    #[test]
    fn screen_zen_wins_over_grid() {
        use super::screen_from_flags;
        assert_eq!(screen_from_flags(false, true), Screen::Zen);
        assert_eq!(screen_from_flags(false, false), Screen::Zen);
        assert_eq!(screen_from_flags(true, true), Screen::Grid);
        assert_eq!(screen_from_flags(true, false), Screen::Workspace);
    }

    #[test]
    fn grid_move_shortcuts_scoped_to_grid_screen() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), gmods(), screen),
            Some(GridMove(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(GridMove(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), gmods(), screen),
            Some(GridMove(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), gmods(), screen),
            Some(GridMove(0, 1))
        );
        // Elsewhere, mod+j/mod+k are still NextSession/PrevSession — the Grid
        // shadow must not leak to other screens.
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), Screen::Workspace),
            Some(NextSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), Screen::Workspace),
            Some(PrevSession)
        );
    }

    #[test]
    fn grid_swap_shortcuts_scoped_to_grid_screen() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        let alt = gmods() | Modifiers::ALT;
        assert_eq!(
            match_global_shortcut(&ch("h"), alt, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), alt, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), alt, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), alt, screen),
            Some(GridSwap(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), alt, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), alt, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), alt, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), alt, screen),
            Some(GridSwap(0, 1))
        );
        // Without Alt, the same keys still resolve to GridMove — no regression
        // from layering the Alt dispatch on top.
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(GridMove(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), gmods(), screen),
            Some(GridMove(1, 0))
        );
        // Alt+h/j/k/l elsewhere is not GridSwap — the Grid-only shadow must
        // not leak to other screens (mirrors the GridMove scoping check above).
        assert_eq!(
            match_global_shortcut(&ch("h"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), alt, Screen::Workspace),
            None
        );
    }

    #[test]
    fn unmodified_or_unmapped_keys_are_not_shortcuts() {
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("n"), Modifiers::empty(), screen),
            None
        );
        assert_eq!(match_global_shortcut(&ch("x"), gmods(), screen), None);
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::Tab), gmods(), screen),
            None
        );
    }

    #[test]
    fn slide_progress_eases_out_from_zero_to_one() {
        let start = std::time::Instant::now();
        assert_eq!(slide_progress(start, start), 0.0);
        // Halfway through, the cubic ease-out has already covered more than
        // half the distance (front-loaded motion).
        let half = start + GRID_SLIDE / 2;
        let p = slide_progress(start, half);
        assert!(p > 0.5 && p < 1.0, "expected ease-out progress, got {p}");
        // At and beyond the duration, progress clamps to 1.0.
        assert_eq!(slide_progress(start, start + GRID_SLIDE), 1.0);
        assert_eq!(slide_progress(start, start + GRID_SLIDE * 10), 1.0);
    }

    /// The terminal-panel resize chord (see the registry's display-only
    /// "resize terminal panel" row) is Ctrl+Shift+Left/Right on every
    /// platform, and scoped to `Screen::Workspace` only — matched by
    /// `term_panel_resize_delta`, not `match_global_shortcut`, because it has
    /// an extra runtime gate (`term_panel_open`) that `handle_key` applies
    /// separately.
    mod term_panel_resize {
        use super::super::{term_panel_resize_delta, Screen, TERM_PANEL_PORTION_STEP};
        use iced::keyboard::{key::Named, Key, Modifiers};

        fn ctrl_shift() -> Modifiers {
            Modifiers::CTRL | Modifiers::SHIFT
        }

        #[test]
        fn matches_only_on_workspace() {
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowRight),
                    ctrl_shift(),
                    Screen::Workspace
                ),
                Some(TERM_PANEL_PORTION_STEP as i16)
            );
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowLeft),
                    ctrl_shift(),
                    Screen::Workspace
                ),
                Some(-(TERM_PANEL_PORTION_STEP as i16))
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::ArrowRight), ctrl_shift(), Screen::Grid),
                None
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::ArrowRight), ctrl_shift(), Screen::Zen),
                None
            );
        }

        #[test]
        fn requires_the_literal_ctrl_shift_chord() {
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowRight),
                    Modifiers::CTRL,
                    Screen::Workspace
                ),
                None
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::Tab), ctrl_shift(), Screen::Workspace),
                None
            );
        }
    }

    /// `CloseFocusedSession`'s decision logic is screen-independent — the
    /// caller resolves `target` per screen (grid tile vs active session), so
    /// these exercise the remaining runtime state (whether anything is focused,
    /// confirm-to-kill arming) directly rather than the full `Grove`, which is
    /// expensive to construct for a single match arm.
    mod close_focused_session_decision {
        use super::super::{close_focused_session_decision, CloseFocusedDecision};

        #[test]
        fn no_op_with_nothing_focused() {
            assert_eq!(
                close_focused_session_decision(None, None),
                CloseFocusedDecision::NoOp
            );
        }

        #[test]
        fn requests_kill_when_not_yet_armed() {
            assert_eq!(
                close_focused_session_decision(Some(2), None),
                CloseFocusedDecision::Request(2)
            );
            // Pending kill armed for a *different* session still requests.
            assert_eq!(
                close_focused_session_decision(Some(2), Some(5)),
                CloseFocusedDecision::Request(2)
            );
        }

        #[test]
        fn kills_when_already_armed_for_focused_session() {
            assert_eq!(
                close_focused_session_decision(Some(2), Some(2)),
                CloseFocusedDecision::Kill(2)
            );
        }
    }

    /// `grid_focused` must track the active session whenever the grid is
    /// showing, or will show again once zen exits — otherwise cycling/
    /// selecting sessions while zenned in from a tile leaves the tile
    /// pointer stale for when zen exits (Bug 5).
    mod should_sync_grid_focus {
        use super::super::should_sync_grid_focus;

        #[test]
        fn untouched_outside_grid_and_not_zenned_from_it() {
            assert!(!should_sync_grid_focus(false, false));
        }

        #[test]
        fn syncs_while_grid_is_open() {
            assert!(should_sync_grid_focus(true, false));
        }

        #[test]
        fn syncs_while_zenned_in_from_the_grid() {
            // grid_view is false during zen (it's temporarily suspended), but
            // grid_view_before_zen remembers to restore it on exit — that's
            // exactly the state where the desync used to happen.
            assert!(should_sync_grid_focus(false, true));
        }
    }

    /// Pure tile-index arithmetic for directional grid focus movement — see
    /// `grid_neighbor`'s doc comment for the row-major-but-column-rendered
    /// geometry this covers.
    mod grid_neighbor {
        use super::super::grid_neighbor;

        #[test]
        fn n3_horizontal_and_vertical_moves() {
            // n=3 -> cols=2, rows=2. Left column: 0 (top), 2 (bottom).
            // Right column: 1 only, spanning the full height.
            assert_eq!(grid_neighbor(2, 3, 1, 0), Some(1));
            assert_eq!(grid_neighbor(1, 3, -1, 0), Some(0));
            assert_eq!(grid_neighbor(1, 3, 0, 1), None);
            assert_eq!(grid_neighbor(0, 3, 0, 1), Some(2));
            assert_eq!(grid_neighbor(0, 3, -1, 0), None);
        }

        #[test]
        fn n4_full_2x2_grid() {
            assert_eq!(grid_neighbor(0, 4, 1, 0), Some(1));
            assert_eq!(grid_neighbor(0, 4, 0, 1), Some(2));
            assert_eq!(grid_neighbor(3, 4, 1, 0), None);
        }
    }

    /// The subscription's capture filter. Escape must survive capture (a
    /// focused text_input eats it to self-blur); nothing else may (Bug 3).
    mod should_forward {
        use super::super::should_forward;
        use iced::keyboard::{key::Named, Key, Modifiers};
        use iced::{event, keyboard, Event};

        fn press(key: Key) -> Event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::empty(),
                text: None,
                repeat: false,
            })
        }

        #[test]
        fn uncaptured_events_always_forward() {
            assert!(should_forward(
                &press(Key::Character("a".into())),
                event::Status::Ignored
            ));
        }

        #[test]
        fn captured_escape_still_forwards() {
            assert!(should_forward(
                &press(Key::Named(Named::Escape)),
                event::Status::Captured
            ));
        }

        #[test]
        fn captured_non_escape_is_dropped() {
            // The load-bearing half: typed characters and Enter belong to the
            // focused field, not to handle_key.
            assert!(!should_forward(
                &press(Key::Character("a".into())),
                event::Status::Captured
            ));
            assert!(!should_forward(
                &press(Key::Named(Named::Enter)),
                event::Status::Captured
            ));
        }
    }

    /// Escape with no modal open must dismiss an armed kill-confirmation or
    /// open agent menu before it ever reaches the PTY (Bug 9).
    mod escape_should_dismiss {
        use super::super::escape_should_dismiss;

        #[test]
        fn false_when_neither_is_set() {
            assert!(!escape_should_dismiss(None, None));
        }

        #[test]
        fn true_when_either_is_set() {
            assert!(escape_should_dismiss(Some(2), None));
            assert!(escape_should_dismiss(None, Some((1, 0))));
            assert!(escape_should_dismiss(Some(2), Some((1, 0))));
        }
    }
}
