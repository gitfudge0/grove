//! `Grove` lifecycle: construction, subscriptions, and all `Msg` handling.

use super::keys::key_to_bytes;
use super::metrics::{
    clamp_sidebar_width, compute_pty_dims, pty_cols_for_fraction, pty_metrics,
    term_portion_for_cursor, PTY_ZOOM_DEFAULT, PTY_ZOOM_MAX, PTY_ZOOM_MIN, PTY_ZOOM_STEP, RAIL_W,
    TERM_PANEL_PORTION, TERM_PANEL_PORTION_MAX, TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_STEP,
};
use super::state::{
    AbsCell, ChangelogState, FocusedPane, GridDrag, GridSlide, Grove, Msg, PtyCell, PtyDrag,
    PtyPane, ScriptField, ScriptsEditorState, SidebarDrag, ThemeManagerEditorState, ToolStatus,
    UpgradeState,
};
use crate::agent::Agent;
use crate::app::{
    AddProjectStep, App, ConfirmKind, LauncherOptions, LauncherSettings, Modal, OnboardStep,
    PaletteRowIdentity, Pane, RowActionsState, SettingsPane,
};
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
            attention_open: false,
            pty_selection: None,
            pty_drag: None,
            pty_press_focused: false,
            blink_tick: 0,
            attention_anim: Self::attention_animation(),
            onb_step_anim: Self::onb_step_animation(),
            pending_kill: None,
            pending_kill_terminal: None,
            hovered_wt: None,
            terminal_focused: false,
            term_panel_open: false,
            terminals_collapsed: false,
            term_panel_portion: TERM_PANEL_PORTION,
            focused_pane: FocusedPane::Agent,
            dir_cache: Default::default(),
            picker_open: false,
            activity: Default::default(),
            claude_poller: crate::claude_agents::Poller::new(),
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
            grid_view_before_zen: false,
            last_divider_press: None,
            term_panel_dragging: false,
            last_term_divider_press: None,
            scripts_editor: None,
            theme_manager_editor: None,
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
            live_mods: Modifiers::empty(),
        };
        // Prime the per-project worktree cache so `view()` never has to shell
        // out to `git worktree list` (it runs on every 33ms tick).
        let n = g.app.store.projects.len();
        for i in 0..n {
            g.ensure_wt_cached(i);
        }
        // The pinned home-terminal section at the bottom of the tree is
        // always visible now (no separate "switch to terminal view" step to
        // trigger this lazily), so make sure at least one terminal exists.
        g.app.ensure_home_terminal(g.pty_rows, g.pty_cols);
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
        // Play the wizard's entrance animation on first show.
        if matches!(g.app.modal, Modal::Onboarding { .. }) {
            g.onb_step_anim.go_mut(true, std::time::Instant::now());
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

    /// Idle-state constructor for the onboarding step-transition animation:
    /// parked at `false` (pre-entrance). `go_mut(true, ..)` plays a single
    /// quick (200ms, `EaseOut`) fade-in/slide-up; unlike `attention_animation`
    /// it doesn't repeat, so it naturally stops animating (and drops out of
    /// the frame subscription) once settled.
    fn onb_step_animation() -> iced::animation::Animation<bool> {
        iced::animation::Animation::new(false)
            .quick()
            .easing(iced::animation::Easing::EaseOut)
    }

    /// Restart the wizard's entrance animation — called whenever the
    /// onboarding step changes (or the wizard first opens).
    fn restart_onb_anim(&mut self) {
        self.onb_step_anim = Self::onb_step_animation();
        self.onb_step_anim.go_mut(true, std::time::Instant::now());
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
                // Tracked live (never `Captured` by `text_input` — it only
                // records it internally, see `should_forward`'s doc comment)
                // so `Msg::LauncherInputChanged` can tell a real typed
                // character apart from a `global_mods` chord the focused
                // search field doesn't special-case (⌘D, ⌘⌫, ...): iced's
                // `text_input` inserts/deletes unconditionally on those,
                // publishing its own `on_input` message *before* the
                // corresponding `KeyPressed` is even broadcast to this
                // subscription (widget dispatch happens first each tick —
                // see `iced_winit`'s event loop), so the fix can't live in
                // the `KeyPress` handler alone.
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                    Some(Msg::ModifiersChanged(modifiers))
                }
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
        // Frame-rate redraw trick shared by every short-lived animation: the
        // needs-attention pulse (while active), the tile-slide reorder
        // animation (~150ms window after a grid swap), and the onboarding
        // wizard's step-transition entrance. A single subscription covers all
        // three so an idle app carries zero frame-rate cost.
        if self.attention_anim.value()
            || self.grid_slide.is_some()
            || self.onb_step_anim.is_animating(std::time::Instant::now())
        {
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
                    self.terminal_focused = true;
                    self.pty_selection = None;
                    self.pending_kill_terminal = None;
                    // Symmetry with new/close/restart: don't rely on `resize`
                    // happening to dirty the target to surface the right frame.
                    self.invalidate_pty_render_cache();
                }
            }
            Msg::RequestCloseHomeTerminal(i) => {
                self.pending_kill_terminal = Some(i);
            }
            Msg::CloseHomeTerminal(i) => {
                // Shift any pending confirmation index across the removal so
                // it can't end up pointing at a different terminal (mirrors
                // `KillSession`'s handling of `pending_kill`).
                self.pending_kill_terminal = match self.pending_kill_terminal {
                    Some(p) if p == i => None,
                    Some(p) if p > i => Some(p - 1),
                    other => other,
                };
                self.app.close_home_terminal(i);
                self.pty_selection = None;
                self.invalidate_pty_render_cache();
            }
            Msg::ToggleTerminalsSection => {
                self.terminals_collapsed = !self.terminals_collapsed;
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
                    title: "Quit Grove?".into(),
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
            Msg::ProjectThemesToggle(v) => {
                let _ = self.app.set_project_themes_enabled(v);
                // Every open PTY's baked-in colors may now need to switch
                // between the global theme and a project override.
                self.invalidate_pty_render_cache();
            }
            Msg::ChooseTmux(enabled) => {
                if let Err(e) = self.app.choose_tmux_enabled(enabled) {
                    self.app.modal = Modal::Message(format!("Tmux setup failed: {e}"));
                }
            }
            Msg::AgentPickerSelect(i) => self.agent_picker_select(i),
            Msg::AgentPickerToggleDefault => self.agent_picker_toggle_default(),
            Msg::AgentPickerSubmit => self.submit_agent_picker(),
            Msg::ToggleCollapseAll => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                self.pending_kill_terminal = None;
                self.tree_expand = self.tree_expand.next();
                self.apply_tree_expand();
            }
            Msg::ProjectClicked(i) => {
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
            Msg::WorktreeClicked { proj, wt } => {
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
            Msg::ToggleAttentionQueue => {
                self.attention_open = !self.attention_open;
                return Task::none();
            }
            Msg::CloseAttentionQueue => {
                self.attention_open = false;
                return Task::none();
            }
            Msg::JumpToWaitingSession => {
                if let Some(&first) = self.waiting_sessions().first() {
                    // Jumping here is meant to show the live prompt that's
                    // waiting on the user, not wherever the view happened to
                    // be scrolled — unlike a manual mod+j/k switch, which
                    // should preserve a scroll position left on purpose.
                    if let Some(s) = self.app.sessions.get_mut(first) {
                        s.snap_to_bottom();
                    }
                    return self.update(Msg::SelectSession(first));
                }
                return Task::none();
            }
            Msg::SelectSession(i) => {
                self.open_agent_menu = None;
                self.pending_kill = None;
                self.pending_kill_terminal = None;
                self.attention_open = false;
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
            Msg::ModifiersChanged(mods) => {
                self.live_mods = mods;
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
                    self.pty_press_focused = self.grid_focused != Some(si);
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
                self.pending_kill_terminal = None;
                // Clicking a PTY focuses its pane (so subsequent keystrokes,
                // scroll, and this very selection route there). Honored only
                // while the panel is open; otherwise the agent always owns input.
                let pane_before = self.focused_pane;
                self.focus_pane(pane);
                self.pty_press_focused = self.focused_pane != pane_before;
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
                // The press that switched focus is focus-only: swallow its
                // release so refocusing a pane never moves the caret (a second
                // click does).
                let press_focused = std::mem::take(&mut self.pty_press_focused);
                if let Some((a, h)) = self.pty_selection {
                    if a == h {
                        self.pty_selection = None;
                        if press_focused {
                            return Task::none();
                        }
                        // No drag happened — treat it as a click-to-move-caret.
                        // `pixel_to_abs` only clamps into the visible window
                        // when scrollback is 0, so bail if the view has been
                        // scrolled: clicking history must be inert.
                        if let Some((h_rows, sb)) = self.pty_view_geom() {
                            if sb == 0 && h_rows > 0 {
                                let row = (h_rows - 1).saturating_sub(a.a_row) as u16;
                                if let Some(s) = self.focused_session_mut() {
                                    s.click(a.col as u16, row);
                                }
                            }
                        }
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
                    self.app.modal = Modal::Message(format!("Add project failed: {e}"));
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
            Msg::OpenProjectThemePicker { proj } => {
                self.app.open_project_theme_picker(proj);
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
            Msg::ThemePickerSelectDefault => {
                self.app.theme_picker_select_default();
                // Preview: this project's visible PTYs must immediately
                // switch to showing the global theme.
                self.invalidate_pty_render_cache();
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
            Msg::OnbBack => {
                self.app.onboard_back();
                self.restart_onb_anim();
            }
            Msg::OnbSkip => self.onboard_skip(),
            Msg::OnbPathChanged(s) => self.app.onboard_set_path(s),
            Msg::OnbNameChanged(s) => self.app.onboard_set_name(s),
            Msg::OnbPickDir(p) => {
                self.app.onboard_pick_dir(p);
                return move_cursor_to_end(crate::gui::view::modal_input_id());
            }
            Msg::OnbAgentSelect(i) => self.app.onboard_agent_select(i),
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
                return focus(crate::gui::view::modal_input_id());
            }
            Msg::LauncherInputChanged(s) => {
                // A `global_mods` chord (⌘D, ⌘⌫, ...) is a command, never
                // text — but `iced_widget::text_input` only special-cases
                // ⌘C/⌘X/⌘V/⌘A explicitly; every other chord still gets
                // inserted/deleted unconditionally by its fallback character/
                // Backspace handling, publishing this very message. Dropping
                // it here (using `live_mods`, tracked independently — see
                // `Msg::ModifiersChanged`'s doc comment for why `KeyPress`'s
                // own `mods` arrives too late to gate this) leaves `input`
                // untouched; the next render re-diffs the field back to that
                // unchanged value, undoing the widget's own transient edit.
                if global_mods(self.live_mods) {
                    return Task::none();
                }
                // The switch-to-session drill-in filters live by `input`
                // (same idiom as OPEN WITH's agent list, which also keeps
                // its own state open while the query underneath changes) —
                // resolved by identity (which session, not which position)
                // before the mutable borrow below, same principle as
                // `resolve_selected` for the main list: a query edit can
                // reorder/drop rows in the filtered list, so re-anchoring by
                // position alone (the old `clamp`-based behavior) could land
                // the cursor on a different session than the one highlighted.
                let switch_open = matches!(
                    &self.app.modal,
                    Modal::SessionLauncher {
                        switch: Some(_),
                        ..
                    }
                );
                let new_switch_pos = switch_open.then(|| self.resolve_switch_position(&s));
                if let Modal::SessionLauncher {
                    input,
                    row_actions,
                    settings,
                    ..
                } = &mut self.app.modal
                {
                    *input = s;
                    // `row_actions` is pinned to a specific (proj, wt_path)
                    // resolved from the root/typing list; once that list is
                    // re-derived for the new query, the row it was anchored
                    // to may no longer be rendered (or may have moved) —
                    // collapse the strip rather than risk it going stale.
                    *row_actions = None;
                    // Settings drill-in stays open across a query edit (only
                    // Esc backs out). Only the panes that actually filter on
                    // the query (Root, Theme) reset their cursor — the input
                    // stays focused in every pane, so a stray keystroke in
                    // Backend/Permissions/DefaultAgent must not snap their
                    // (unfiltered) cursor to 0. The update-actions strip
                    // collapses for the same reason `row_actions` does: the
                    // row it hangs under may no longer be rendered. Typing
                    // also leaves App-size resize mode — the user is
                    // searching now, and the filtered list may not even
                    // contain the App-size row anymore.
                    if let Some(st) = settings {
                        if matches!(
                            st.pane,
                            SettingsPane::Root
                                | SettingsPane::Theme { .. }
                                | SettingsPane::ProjectTheme { .. }
                        ) {
                            st.selected = 0;
                            st.update_actions = None;
                            st.resizing = false;
                        }
                    }
                }
                // Root/typed list cursor snapped to 0 for the new query —
                // capture its identity too (see `set_palette_selected`).
                self.set_palette_selected(0);
                if let Some(pos) = new_switch_pos {
                    self.set_switch_selected(pos);
                }
                // Theme sub-pane / drill-in Root: the new query reshapes
                // the list and the cursor just snapped to 0 — scroll the
                // list back with it.
                if let Modal::SessionLauncher {
                    settings: Some(ls), ..
                } = &self.app.modal
                {
                    return match ls.pane {
                        SettingsPane::Theme { .. } => self.scroll_launcher_theme_to_selection(),
                        SettingsPane::ProjectTheme { .. } => {
                            self.scroll_launcher_project_theme_to_selection()
                        }
                        SettingsPane::Root => self.scroll_launcher_settings_to_selection(),
                        _ => Task::none(),
                    };
                }
                // Root/typed list: the query edit just reshaped the list and
                // snapped the cursor to 0 above — scroll it back with it,
                // same as the Settings drill-in's Root pane.
                return self.scroll_launcher_palette_to_selection();
            }
            Msg::LauncherActivate(i) => return self.launcher_activate(i),
            Msg::LauncherOptionsPick(i) => {
                let len = self.app.available_agents.len();
                if let Modal::SessionLauncher {
                    options: Some(r), ..
                } = &mut self.app.modal
                {
                    if i < len {
                        r.agent = i;
                    }
                }
                self.launcher_start();
            }
            Msg::LauncherSwitchSessionPick(si) => return self.launcher_switch_to(si),
            Msg::LauncherRowActionPick(action) => {
                let row_actions = match &self.app.modal {
                    Modal::SessionLauncher {
                        row_actions: Some(r),
                        ..
                    } => Some(r.clone()),
                    _ => None,
                };
                if let Some(r) = row_actions {
                    return self.launcher_run_row_action(r.proj, r.wt_path, r.agent, action);
                }
            }
            Msg::LauncherSettingActivate(i) => {
                let input = match &self.app.modal {
                    Modal::SessionLauncher {
                        input,
                        settings: Some(_),
                        ..
                    } => input.clone(),
                    _ => return Task::none(),
                };
                let rows = self.settings_rows_filtered(&input);
                if let Some(&s) = rows.get(i) {
                    if let Modal::SessionLauncher {
                        settings: Some(ls), ..
                    } = &mut self.app.modal
                    {
                        ls.selected = i;
                        // Clicking any row while the update-actions strip is
                        // expanded collapses it first — activating the
                        // CheckUpdates row itself just re-opens it below.
                        ls.update_actions = None;
                    }
                    return self.activate_setting(s);
                }
            }
            Msg::LauncherThemePaneSelect(i) => {
                if self.launcher_pane_is_project_theme() {
                    return self.project_theme_pane_select(i);
                }
                return self.theme_pane_select(i);
            }
            Msg::LauncherThemePaneDark => {
                if self.launcher_pane_is_project_theme() {
                    return self.project_theme_pane_set_kind(crate::theme::ThemeKind::Dark);
                }
                return self.theme_pane_set_kind(crate::theme::ThemeKind::Dark);
            }
            Msg::LauncherThemePaneLight => {
                if self.launcher_pane_is_project_theme() {
                    return self.project_theme_pane_set_kind(crate::theme::ThemeKind::Light);
                }
                return self.theme_pane_set_kind(crate::theme::ThemeKind::Light);
            }
            Msg::LauncherThemePaneSystem => return self.theme_pane_set_system(),
            Msg::OpenThemeManager => return self.open_theme_manager(),
            Msg::ThemeManagerSelect(i) => self.theme_manager_select(i),
            Msg::ThemeManagerRenameStart(i) => self.theme_manager_rename_start(i),
            Msg::ThemeManagerRenameChanged(s) => self.theme_manager_rename_changed(s),
            Msg::ThemeManagerRenameSubmit => self.theme_manager_rename_submit(),
            Msg::ThemeManagerRenameCancel => self.theme_manager_rename_cancel(),
            Msg::ThemeManagerDuplicate(i) => self.theme_manager_duplicate(i),
            Msg::ThemeManagerDeleteStart(i) => self.theme_manager_delete_start(i),
            Msg::ThemeManagerDeleteConfirm => self.theme_manager_delete_confirm(),
            Msg::ThemeManagerDeleteCancel => self.theme_manager_delete_cancel(),
            Msg::ThemeManagerNew => return self.theme_manager_new(),
            Msg::ThemeManagerClose => self.theme_manager_close(),
            Msg::ThemeManagerEditStart(i) => return self.theme_manager_edit_start(i),
            Msg::ThemeManagerEditorRowSelect(i) => return self.theme_manager_editor_row_select(i),
            Msg::ThemeManagerEditorHexChanged(s) => return self.theme_manager_editor_hex_changed(s),
            Msg::ThemeManagerEditorNameChanged(s) => self.theme_manager_editor_name_changed(s),
            Msg::ThemeManagerEditorKindDark => {
                self.theme_manager_editor_set_kind(crate::theme::ThemeKind::Dark)
            }
            Msg::ThemeManagerEditorKindLight => {
                self.theme_manager_editor_set_kind(crate::theme::ThemeKind::Light)
            }
            Msg::ThemeManagerEditorTogglePreview => self.theme_manager_editor_toggle_preview(),
            Msg::ThemeManagerEditorSave => return self.theme_manager_editor_save(),
            Msg::ThemeManagerEditorEsc => return self.theme_manager_editor_esc(),
            Msg::ThemeManagerEditorDiscardConfirm => return self.theme_manager_editor_discard(),
            Msg::ThemeManagerEditorDiscardCancel => self.theme_manager_editor_discard_cancel(),
            Msg::ThemeManagerPasteAction(action) => self.theme_manager_paste_action(action),
            Msg::ThemeManagerPasteApply => self.theme_manager_paste_apply(),
            Msg::LauncherSettingsPaneActivate(i) => {
                let pane = match &self.app.modal {
                    Modal::SessionLauncher {
                        settings: Some(ls), ..
                    } => ls.pane.clone(),
                    _ => return Task::none(),
                };
                match pane {
                    SettingsPane::Backend => return self.backend_pane_commit(i),
                    SettingsPane::Permissions => return self.permissions_pane_commit(i),
                    SettingsPane::DefaultAgent => return self.default_agent_pane_commit(i),
                    SettingsPane::Root
                    | SettingsPane::Theme { .. }
                    | SettingsPane::ProjectTheme { .. } => {}
                }
            }
            Msg::LauncherUpdateActionPick(i) => {
                if let Modal::SessionLauncher {
                    settings: Some(ls), ..
                } = &mut self.app.modal
                {
                    if ls.update_actions.is_some() {
                        ls.update_actions = Some(i);
                        return self.update_actions_commit(i);
                    }
                }
            }
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
        self.restart_onb_anim();
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
            self.app.modal = Modal::Message(format!("Setup failed: {e}"));
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
                self.app.modal = Modal::Message(format!("Setup failed: {e}"));
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
        use crate::app::{Modal, ThemePickerScope};
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            project_use_default,
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
        match scope {
            ThemePickerScope::App => {
                // Picking a concrete theme from the list opts back out of "system".
                *follow_system = false;
                crate::theme::set(themes[index].clone());
            }
            ThemePickerScope::Project(_) => {
                // Project scope never previews into the global active theme.
                *project_use_default = false;
            }
        }
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
                crate::theme::set(t.clone());
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
            self.app.modal = crate::app::Modal::Message(format!("Theme failed: {e}"));
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
    ///
    /// Signal precedence, highest first: native poll (`claude_agents`) >
    /// hook state file (`attention`) > screen-scraping heuristics
    /// (`activity::classify`). See the per-session loop below.
    fn refresh_activity(&mut self) {
        use super::activity::{classify, ActivityState, Signals};
        let now = std::time::Instant::now();
        let mut live_keys: Vec<u64> = Vec::with_capacity(self.app.sessions.len());
        let mut newly_waiting = false;

        // Only worth polling `claude agents --json` while at least one live
        // Claude session exists to inform — see `claude_agents::Poller`.
        let any_live_claude = self.app.sessions.iter().any(|s| {
            matches!(s.status(), crate::session::SessionStatus::Running) && s.agent == Agent::Claude
        });
        self.claude_poller.set_wanted(any_live_claude);

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
            // Precedence, highest first: native poll (`claude_agents`) >
            // hook state file (`attention`) > screen-scraping heuristics
            // (`activity::classify`). The native poll is the most
            // authoritative signal when available (it comes straight from
            // the Claude CLI, not a hook we injected or the terminal
            // contents), and it's also the only one of the three that works
            // for tmux sessions reattached across a grove restart. It's
            // consulted only for alive Claude sessions; everything else
            // falls straight through to the existing hook/heuristic chain,
            // unchanged.
            let native = if alive && s.agent == Agent::Claude {
                self.claude_poller.status_for(s.root_pid(), &s.wt_path)
            } else {
                None
            };
            let new_state = if let Some(native_status) = native {
                match native_status {
                    crate::claude_agents::NativeStatus::Busy => ActivityState::Working,
                    // A `Waiting` signal while focused is treated like the
                    // user has already seen it, mirroring the same downgrade
                    // rule the hook-state-file branch below applies to
                    // `NeedsYou` (never resurrect the highest-urgency state
                    // on the session they're looking at).
                    crate::claude_agents::NativeStatus::Waiting => {
                        if !focused {
                            ActivityState::WaitingForInput
                        } else {
                            ActivityState::Working
                        }
                    }
                    crate::claude_agents::NativeStatus::Idle => {
                        if tracker.was_working {
                            ActivityState::Done
                        } else {
                            ActivityState::Idle
                        }
                    }
                }
            } else {
                // Claude/Codex sessions with a hook/notify state file get a
                // deterministic signal that outranks the screen-scraping
                // heuristic below (but never a dead process — a stale `working`
                // left behind by a killed agent must still show Exited). A
                // `NeedsYou` signal while focused is treated like the user has
                // already seen it (never resurrect the highest-urgency state on
                // the session they're looking at, mirroring
                // `Tracker::acknowledge`'s existing downgrade rule).
                match (alive, s.attention_state()) {
                    (false, _) => classify(s.agent, &tail, &sig),
                    (true, Some(crate::attention::AttentionState::NeedsYou)) if !focused => {
                        ActivityState::WaitingForInput
                    }
                    (true, Some(crate::attention::AttentionState::NeedsYou)) => {
                        ActivityState::Working
                    }
                    (true, Some(crate::attention::AttentionState::Done)) => ActivityState::Done,
                    (true, Some(crate::attention::AttentionState::Working)) => {
                        ActivityState::Working
                    }
                    (true, None) => classify(s.agent, &tail, &sig),
                }
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

    /// Session indices currently waiting for input, in tree/on-screen order —
    /// the "attention queue". Drives the appbar pill/dropdown, the zen pill,
    /// and `mod+'`.
    pub(crate) fn waiting_sessions(&self) -> Vec<usize> {
        self.visible_session_order()
            .into_iter()
            .filter(|&si| {
                self.app.sessions.get(si).map_or(false, |s| {
                    matches!(
                        self.activity_state(s),
                        super::activity::ActivityState::WaitingForInput
                    )
                })
            })
            .collect()
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
            self.app.modal = Modal::Message(format!("Default agent failed: {e}"));
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
            && escape_should_dismiss(
                self.pending_kill,
                self.pending_kill_terminal,
                self.open_agent_menu,
                self.attention_open,
            )
        {
            self.pending_kill = None;
            self.pending_kill_terminal = None;
            self.open_agent_menu = None;
            self.attention_open = false;
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
        // Keyboard scrollback for the focused session: Shift+PageUp/PageDown
        // scrolls by a page, Shift+Home/End jumps to the top/bottom. Checked
        // before key_to_bytes so these chords never reach the PTY; plain
        // PageUp/PageDown/Home/End (no Shift) fall through unchanged.
        if let Some((up, amount)) = keyboard_scroll_intent(&key, mods) {
            if let Some(sess) = self.focused_session_mut() {
                let lines = match amount {
                    ScrollAmount::Page => sess.scroll_page_lines(),
                    ScrollAmount::All => crate::session::SCROLLBACK_LINES,
                };
                sess.scroll_lines(up, lines);
            }
            return Task::none();
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
            GlobalShortcut::SwitchSession => {
                // Zen-only: outside zen the workspace/grid already shows
                // every session, so opening straight into the drill-in would
                // be redundant (`switch_to_session_active` is the same gate
                // `PaletteRow::SwitchToSession`'s active/inert split uses).
                // The modal-open guard above already keeps this from firing
                // while the palette itself is open.
                if self.switch_to_session_active() {
                    self.open_session_launcher();
                    self.launcher_enter_switch();
                    return focus(crate::gui::view::modal_input_id());
                }
                Task::none()
            }
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
                // A focused home terminal takes priority, and goes through
                // the same two-step confirm-to-kill flow as an agent session
                // (`pending_kill_terminal` mirrors `pending_kill`).
                if self.terminal_focused {
                    return match close_focused_session_decision(
                        self.app.active_terminal,
                        self.pending_kill_terminal,
                    ) {
                        CloseFocusedDecision::Kill(idx) => self.update(Msg::CloseHomeTerminal(idx)),
                        CloseFocusedDecision::Request(idx) => {
                            self.update(Msg::RequestCloseHomeTerminal(idx))
                        }
                        CloseFocusedDecision::NoOp => Task::none(),
                    };
                }
                // Grid: the focused tile. Everywhere else (tree sidebar,
                // zen): the active session, whose row renders the same
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
            GlobalShortcut::NewHomeTerminal => {
                let _ = self.update(Msg::NewHomeTerminal);
                self.terminal_focused = true;
                Task::none()
            }
            GlobalShortcut::JumpToWaitingSession => self.update(Msg::JumpToWaitingSession),
            GlobalShortcut::GridMove(dx, dy) => {
                crate::telemetry::track("tile_moved", vec![]);
                self.grid_move(dx, dy);
                Task::none()
            }
            GlobalShortcut::GridSwap(dx, dy) => {
                self.grid_swap(dx, dy);
                Task::none()
            }
            GlobalShortcut::ScrollHalfPage(up) => {
                if let Some(sess) = self.focused_session_mut() {
                    let lines = sess.scroll_page_lines().div_ceil(2).max(1);
                    sess.scroll_lines(up, lines);
                }
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
        // tree layout rather than raw session index, so the number the user
        // sees is the session they get.
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
                            self.app.modal = Modal::Message(format!("Add project failed: {e}"));
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
            Modal::ThemeManager {
                rename,
                pending_delete,
                ..
            } => {
                if self.theme_manager_editor.is_some() {
                    let confirm_discard = self
                        .theme_manager_editor
                        .as_ref()
                        .is_some_and(|ed| ed.confirm_discard);
                    if confirm_discard {
                        // "Discard changes…?" up: only its own Enter
                        // (discard)/Esc (keep editing) apply — rows are
                        // frozen underneath it.
                        match key {
                            Key::Named(Named::Enter) => return self.theme_manager_editor_discard(),
                            Key::Named(Named::Escape) => self.theme_manager_editor_discard_cancel(),
                            _ => {}
                        }
                    } else {
                        let dir_delta: Option<i32> = match &key {
                            Key::Named(Named::ArrowDown) => Some(1),
                            Key::Named(Named::ArrowUp) => Some(-1),
                            _ => None,
                        };
                        if let Some(delta) = dir_delta {
                            return self.theme_manager_editor_move(delta);
                        }
                        match key {
                            // ⌘⏎ (Apply the paste box) must be checked ahead
                            // of the plain-Enter arm below, which would
                            // otherwise shadow it.
                            Key::Named(Named::Enter) if global_mods(mods) => {
                                self.theme_manager_paste_apply()
                            }
                            Key::Named(Named::Escape) => return self.theme_manager_editor_esc(),
                            Key::Named(Named::Enter) => return self.theme_manager_editor_save(),
                            Key::Character(s) if global_mods(mods) => match s.as_str() {
                                "s" | "S" => return self.theme_manager_editor_save(),
                                "p" | "P" => self.theme_manager_editor_toggle_preview(),
                                _ => {}
                            },
                            _ => {}
                        }
                    }
                } else if pending_delete.is_some() {
                    // y/n mirrors `confirm_modal`'s destructive-confirm
                    // convention (Enter deliberately does not there); Enter
                    // still works here too since this dialog has no other
                    // use for it.
                    match key {
                        Key::Named(Named::Enter) => self.theme_manager_delete_confirm(),
                        Key::Named(Named::Escape) => self.theme_manager_delete_cancel(),
                        Key::Character(s) => match s.as_str() {
                            "y" | "Y" => self.theme_manager_delete_confirm(),
                            "n" | "N" => self.theme_manager_delete_cancel(),
                            _ => {}
                        },
                        _ => {}
                    }
                } else if rename.is_some() {
                    match key {
                        Key::Named(Named::Enter) => self.theme_manager_rename_submit(),
                        Key::Named(Named::Escape) => self.theme_manager_rename_cancel(),
                        _ => {}
                    }
                } else {
                    match key {
                        Key::Named(Named::Escape) => self.theme_manager_close(),
                        Key::Named(Named::ArrowDown) => self.theme_manager_move(1),
                        Key::Named(Named::ArrowUp) => self.theme_manager_move(-1),
                        _ => {}
                    }
                }
            }
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
                input,
                selected,
                browse_all,
                options,
                switch,
                row_actions,
                settings,
                ..
            } => {
                let (input, selected, browse_all) = (input.clone(), *selected, *browse_all);
                if let Some(r) = options.clone() {
                    // Options state: ↑↓ move the agent selection (clamped, no
                    // wrap). ←→ are no-ops. Plain letters are never bound —
                    // the search input keeps keyboard focus and owns them
                    // (same convention as Modal::Input / AddProject).
                    let list_delta: Option<i32> = match &key {
                        Key::Named(Named::ArrowDown) => Some(1),
                        Key::Named(Named::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if let Some(delta) = list_delta {
                        let len = self.app.available_agents.len();
                        let new_agent = crate::gui::launcher::clamp(r.agent, delta, len);
                        if let Modal::SessionLauncher {
                            options: Some(rr), ..
                        } = &mut self.app.modal
                        {
                            rr.agent = new_agent;
                        }
                    } else {
                        match key {
                            Key::Named(Named::Escape) => {
                                // Options is only ever entered via the
                                // row-actions strip's "Launch session…"
                                // action — Esc returns to that strip rather
                                // than dropping all the way to bare root.
                                if let Modal::SessionLauncher {
                                    options,
                                    row_actions,
                                    ..
                                } = &mut self.app.modal
                                {
                                    let origin = options.take().map(|o| o.origin);
                                    *row_actions = origin;
                                }
                            }
                            Key::Named(Named::Enter) => self.launcher_start(),
                            _ => {}
                        }
                    }
                } else if let Some(sel) = *switch {
                    // "Switch to session" drill-in: ↑↓ move the session
                    // selection (clamped, no wrap); Enter switches focus and
                    // closes the palette; Esc backs out to the root list.
                    let len = self.switch_session_rows(&input).len();
                    let list_delta: Option<i32> = match &key {
                        Key::Named(Named::ArrowDown) => Some(1),
                        Key::Named(Named::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if let Some(delta) = list_delta {
                        let new_sel = crate::gui::launcher::clamp(sel, delta, len);
                        self.set_switch_selected(new_sel);
                    } else {
                        match key {
                            Key::Named(Named::Escape) => {
                                if let Modal::SessionLauncher { switch, .. } = &mut self.app.modal {
                                    *switch = None;
                                }
                                // The root list was recomputed against the cleared
                                // input when the drill-in opened; the old cursor no
                                // longer points at the row it was on.
                                self.set_palette_selected(0);
                            }
                            Key::Named(Named::Enter) => {
                                if let Some(si) = self.resolve_switch_selected(&input) {
                                    return self.launcher_switch_to(si);
                                }
                            }
                            _ => {}
                        }
                    }
                } else if let Some(s) = settings.clone() {
                    // Settings drill-in. `resizing` (Root pane only, D4) takes
                    // priority: it's a modal-within-the-modal for the App-size
                    // row where arrows/±/0 adjust zoom instead of moving the
                    // list cursor, and Enter/Esc merely *leave* the mode
                    // rather than popping the drill-in. Otherwise, behavior
                    // branches on `s.pane`: Root keeps the phase-1 filtered-
                    // list nav (Enter now opens a sub-pane for the five enum
                    // rows via `activate_setting`, not a no-op); each sub-pane
                    // has its own short, unfiltered row count and its own
                    // commit/cancel (see the `Grove::*_pane_*` methods).
                    let dir_delta: Option<i32> = match &key {
                        Key::Named(Named::ArrowDown) => Some(1),
                        Key::Named(Named::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if s.resizing {
                        if let Some(delta) = dir_delta {
                            // ↑↓ exit resizing, then move the Root cursor
                            // exactly as it would outside resize mode.
                            let rows_len = self.settings_rows_filtered(&input).len();
                            let new_sel = crate::gui::launcher::clamp(s.selected, delta, rows_len);
                            if let Modal::SessionLauncher {
                                settings: Some(ss), ..
                            } = &mut self.app.modal
                            {
                                ss.resizing = false;
                                ss.selected = new_sel;
                            }
                            return self.scroll_launcher_settings_to_selection();
                        } else {
                            match key {
                                Key::Named(Named::ArrowLeft) => return self.update(Msg::ZoomOut),
                                Key::Named(Named::ArrowRight) => return self.update(Msg::ZoomIn),
                                Key::Named(Named::Enter) | Key::Named(Named::Escape) => {
                                    if let Modal::SessionLauncher {
                                        settings: Some(ss), ..
                                    } = &mut self.app.modal
                                    {
                                        ss.resizing = false;
                                    }
                                }
                                Key::Character(ch) => match ch.as_str() {
                                    "-" => return self.update(Msg::ZoomOut),
                                    "+" => return self.update(Msg::ZoomIn),
                                    "0" => return self.update(Msg::ZoomReset),
                                    _ => {}
                                },
                                _ => {}
                            }
                        }
                    } else if let Some(strip_sel) = s.update_actions {
                        // Update-actions strip (E3): ←→/Tab move across the
                        // strip's actions, ⏎ runs one, Esc collapses just
                        // the strip. ↑↓ collapse it and move the Root cursor
                        // as normal, same shape as resizing above.
                        let len = update_available_actions(matches!(
                            self.upgrade_method,
                            crate::upgrade::InstallMethod::Unknown
                        ))
                        .len();
                        if let Some(delta) = dir_delta {
                            let rows_len = self.settings_rows_filtered(&input).len();
                            let new_sel = crate::gui::launcher::clamp(s.selected, delta, rows_len);
                            if let Modal::SessionLauncher {
                                settings: Some(ss), ..
                            } = &mut self.app.modal
                            {
                                ss.update_actions = None;
                                ss.selected = new_sel;
                            }
                            return self.scroll_launcher_settings_to_selection();
                        } else {
                            let strip_delta: Option<i32> = match &key {
                                Key::Named(Named::ArrowLeft) => Some(-1),
                                Key::Named(Named::ArrowRight) | Key::Named(Named::Tab) => Some(1),
                                _ => None,
                            };
                            if let Some(delta) = strip_delta {
                                let new_sel = crate::gui::launcher::clamp(strip_sel, delta, len);
                                if let Modal::SessionLauncher {
                                    settings: Some(ss), ..
                                } = &mut self.app.modal
                                {
                                    ss.update_actions = Some(new_sel);
                                }
                            } else {
                                match key {
                                    Key::Named(Named::Escape) => self.close_update_actions_strip(),
                                    Key::Named(Named::Enter) => {
                                        return self.update_actions_commit(strip_sel)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    } else {
                        match s.pane {
                            SettingsPane::Root => {
                                let sel = s.selected;
                                let rows = self.settings_rows_filtered(&input);
                                if let Some(delta) = dir_delta {
                                    let new_sel =
                                        crate::gui::launcher::clamp(sel, delta, rows.len());
                                    if let Modal::SessionLauncher {
                                        settings: Some(ss), ..
                                    } = &mut self.app.modal
                                    {
                                        ss.selected = new_sel;
                                    }
                                    return self.scroll_launcher_settings_to_selection();
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            if let Modal::SessionLauncher {
                                                settings, input, ..
                                            } = &mut self.app.modal
                                            {
                                                *settings = None;
                                                input.clear();
                                            }
                                            self.set_palette_selected(0);
                                        }
                                        Key::Named(Named::Enter) => {
                                            if let Some(&sr) = rows.get(sel) {
                                                return self.activate_setting(sr);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SettingsPane::Theme { .. } => {
                                if let Some(delta) = dir_delta {
                                    return self.theme_pane_move(delta);
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            return self.theme_pane_cancel()
                                        }
                                        Key::Named(Named::Enter) => {
                                            return self.theme_pane_commit()
                                        }
                                        // Tab cycles the mode row; bare
                                        // letters always stay with the
                                        // search input (fuzzy-filtering the
                                        // list) — edit/manage are ⌘-chorded
                                        // specifically so they never collide
                                        // with typing a theme name.
                                        Key::Named(Named::Tab) => {
                                            return self.theme_pane_cycle_mode()
                                        }
                                        Key::Character(s) if global_mods(mods) => {
                                            match s.as_str() {
                                                "e" | "E" => {
                                                    return self.theme_pane_open_editor()
                                                }
                                                "m" | "M" => return self.open_theme_manager(),
                                                _ => {}
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SettingsPane::Backend => {
                                if let Some(delta) = dir_delta {
                                    let new_sel = crate::gui::launcher::clamp(s.selected, delta, 2);
                                    if let Modal::SessionLauncher {
                                        settings: Some(ss), ..
                                    } = &mut self.app.modal
                                    {
                                        ss.selected = new_sel;
                                    }
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            return self
                                                .return_to_settings_root(SettingRow::Backend)
                                        }
                                        Key::Named(Named::Enter) => {
                                            return self.backend_pane_commit(s.selected)
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SettingsPane::Permissions => {
                                if let Some(delta) = dir_delta {
                                    let new_sel = crate::gui::launcher::clamp(s.selected, delta, 2);
                                    if let Modal::SessionLauncher {
                                        settings: Some(ss), ..
                                    } = &mut self.app.modal
                                    {
                                        ss.selected = new_sel;
                                    }
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            return self
                                                .return_to_settings_root(SettingRow::Permissions)
                                        }
                                        Key::Named(Named::Enter) => {
                                            return self.permissions_pane_commit(s.selected)
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SettingsPane::ProjectTheme { .. } => {
                                if let Some(delta) = dir_delta {
                                    return self.project_theme_pane_move(delta);
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            return self.project_theme_pane_cancel()
                                        }
                                        Key::Named(Named::Enter) => {
                                            return self.project_theme_pane_commit()
                                        }
                                        // Tab cycles Dark/Light only — no
                                        // System mode for a project override.
                                        Key::Named(Named::Tab) => {
                                            return self.project_theme_pane_cycle_kind()
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SettingsPane::DefaultAgent => {
                                let len = Agent::ALL.len();
                                if let Some(delta) = dir_delta {
                                    let new_sel =
                                        crate::gui::launcher::clamp(s.selected, delta, len);
                                    if let Modal::SessionLauncher {
                                        settings: Some(ss), ..
                                    } = &mut self.app.modal
                                    {
                                        ss.selected = new_sel;
                                    }
                                } else {
                                    match key {
                                        Key::Named(Named::Escape) => {
                                            return self
                                                .return_to_settings_root(SettingRow::DefaultAgent)
                                        }
                                        Key::Named(Named::Enter) => {
                                            return self.default_agent_pane_commit(s.selected)
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(ra) = row_actions.clone() {
                    // Inline row-actions strip: ↑↓ move between the two
                    // actions (clamped, no wrap); Enter runs the selected
                    // action; Esc collapses the strip back to the plain list.
                    let list_delta: Option<i32> = match &key {
                        Key::Named(Named::ArrowDown) => Some(1),
                        Key::Named(Named::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if let Some(delta) = list_delta {
                        let base = if self.app.project_themes_enabled() {
                            3
                        } else {
                            2
                        };
                        let action_count = base + self.row_action_scripts(ra.proj).len();
                        let new_action =
                            crate::gui::launcher::clamp(ra.action, delta, action_count);
                        if let Modal::SessionLauncher {
                            row_actions: Some(rr),
                            ..
                        } = &mut self.app.modal
                        {
                            rr.action = new_action;
                        }
                    } else {
                        match key {
                            Key::Named(Named::Escape) => {
                                if let Modal::SessionLauncher { row_actions, .. } =
                                    &mut self.app.modal
                                {
                                    *row_actions = None;
                                }
                            }
                            Key::Named(Named::Enter) => {
                                return self.launcher_run_row_action(
                                    ra.proj,
                                    ra.wt_path,
                                    ra.agent,
                                    ra.action,
                                )
                            }
                            _ => {}
                        }
                    }
                } else {
                    // Root or typing/browse-all: ↑↓ move the list selection;
                    // Tab reveals contextual actions (Recent/Combo rows) or
                    // opens the switch-to-session drill-in (arrows can't:
                    // ←→ move the caret in the focused search input). Plain
                    // letters belong to the input too, never to nav.
                    let rows = self.palette_rows(&input, browse_all);
                    let list_delta: Option<i32> = match &key {
                        Key::Named(Named::ArrowDown) => Some(1),
                        Key::Named(Named::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if let Some(delta) = list_delta {
                        let new_selected = crate::gui::launcher::clamp(selected, delta, rows.len());
                        self.set_palette_selected(new_selected);
                        return self.scroll_launcher_palette_to_selection();
                    } else {
                        let enter_actions = matches!(&key, Key::Named(Named::Tab));
                        if enter_actions {
                            return match self.resolve_selected(&rows) {
                                Some(idx) => {
                                    self.launcher_enter_row_actions(idx, &input, browse_all)
                                }
                                None => Task::none(),
                            };
                        } else {
                            match key {
                                Key::Named(Named::Escape) => self.cancel_modal(),
                                Key::Named(Named::Enter) => {
                                    return match self.resolve_selected(&rows) {
                                        Some(idx) => self.launcher_activate(idx),
                                        None => Task::none(),
                                    }
                                }
                                Key::Character(s) if global_mods(mods) => {
                                    if let Some(n) =
                                        s.parse::<usize>().ok().filter(|n| (1..=9).contains(n))
                                    {
                                        // mod+digit addresses sessions only:
                                        // in typed mode Setting rows sort
                                        // above Combo rows (B2), so a raw
                                        // list index would hand ⌘1 to a
                                        // setting instead of the first
                                        // session. No-op when fewer than
                                        // `n` session rows match.
                                        if let Some(idx) = nth_session_row(&rows, n) {
                                            return self.launcher_activate(idx);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
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
                    Key::Named(Named::ArrowDown) => {
                        if step == crate::app::OnboardStep::Project {
                            self.app.onboard_dir_move(1)
                        }
                    }
                    Key::Named(Named::ArrowUp) => {
                        if step == crate::app::OnboardStep::Project {
                            self.app.onboard_dir_move(-1)
                        }
                    }
                    Key::Named(Named::Tab) => {
                        if step == crate::app::OnboardStep::Project {
                            if self.app.onboard_toggle_project_focus() {
                                return focus(crate::gui::view::modal_name_id());
                            }
                            self.app.onboard_dir_pick();
                            return Task::batch([
                                focus(crate::gui::view::modal_input_id()),
                                move_cursor_to_end(crate::gui::view::modal_input_id()),
                            ]);
                        }
                    }
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
            self.app.modal = Modal::Message(format!("Tmux setup failed: {e}"));
        }
    }

    fn submit_modal_input(&mut self) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_input() {
            self.app.modal = Modal::Message(format!("Input failed: {e}"));
        }
        self.resize_new_sessions(&before);
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        self.rebuild_wt_cache();
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
            self.app.modal = Modal::Message(format!("Action failed: {e}"));
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
            self.app.modal = Modal::Message(format!("Failed to save scripts: {e}"));
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
    /// When focusing an agent session, drop out of the home-terminal focus so
    /// `workspace()` (which checks `terminal_tab()` before `active_session`)
    /// renders the session instead of the terminal.
    fn leave_terminal_tab(&mut self) {
        self.terminal_focused = false;
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
    /// spawned thread. The tree is always visible now, so this always runs.
    fn maybe_poll_git_state(&mut self) {
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

    /// Open the command palette at root state (recents + actions). Warms the
    /// worktree cache for every project since the typing/browse-all list
    /// needs it, and refreshes `available_agents` for the same reason.
    fn open_session_launcher(&mut self) {
        self.app.refresh_available_agents();
        let n = self.app.store.projects.len();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
        self.app.modal = Modal::SessionLauncher {
            input: String::new(),
            selected: 0,
            selected_identity: None,
            browse_all: false,
            options: None,
            switch: None,
            switch_identity: None,
            row_actions: None,
            settings: None,
        };
        self.set_palette_selected(0);
    }

    /// Start the session for the current options-state selection: Enter, or
    /// an agent row click (`Msg::LauncherOptionsPick`, which sets the
    /// selection then calls this). No-op outside options state, or if the
    /// selection no longer resolves.
    fn launcher_start(&mut self) {
        let Modal::SessionLauncher {
            options: Some(r), ..
        } = self.app.modal.clone()
        else {
            return;
        };
        self.launcher_launch(r.proj, r.wt, r.agent);
    }

    /// Spawn the session for `(proj, wt, agent_idx)`, close the palette, and
    /// (grid always open here) append it to `tile_order` and focus it. Shared
    /// by `launcher_activate`'s Recent/Combo case and `launcher_start`'s
    /// options path.
    fn launcher_launch(&mut self, proj: usize, wt: usize, agent_idx: usize) {
        let Some(project) = self.app.store.projects.get(proj) else {
            return;
        };
        let pname = project.name.clone();
        let worktrees = self.launcher_worktrees(proj);
        let Some(w) = worktrees.get(wt).cloned() else {
            return;
        };
        let Some(ag) = self.app.available_agents.get(agent_idx).copied() else {
            return;
        };
        let label = crate::gui::launcher::default_label(w.is_main, &pname, &w.path);
        let args = ag.launch_args(self.app.skip_permissions_enabled());
        let before = self.session_keys();
        self.app.modal = Modal::None;
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

    /// Move the root/typed list's selection cursor to `idx` and capture that
    /// row's identity in the same step (`PaletteRowIdentity`). Every write
    /// site for `SessionLauncher::selected` routes through this — reads
    /// `input`/`browse_all` off the *current* modal state, so callers must
    /// apply any other field changes (new `input`, flipped `browse_all`,
    /// …) first. No-op outside `SessionLauncher`.
    fn set_palette_selected(&mut self, idx: usize) {
        let (input, browse_all) = match &self.app.modal {
            Modal::SessionLauncher {
                input, browse_all, ..
            } => (input.clone(), *browse_all),
            _ => return,
        };
        let rows = self.palette_rows(&input, browse_all);
        let identity = rows.get(idx).map(row_identity);
        if let Modal::SessionLauncher {
            selected,
            selected_identity,
            ..
        } = &mut self.app.modal
        {
            *selected = idx;
            *selected_identity = identity;
        }
    }

    /// The keyboard Enter/Tab activation path's index resolution: match the
    /// modal's `selected_identity` against `rows` (see
    /// `resolve_row_by_identity`) instead of trusting `selected` directly —
    /// this is what actually closes the staleness window `set_palette_
    /// selected` opens up: a background list change between the last
    /// keypress that moved the cursor and this one can't make Enter/Tab fire
    /// a different row than the one highlighted on screen. Mouse clicks
    /// (`Msg::LauncherActivate(i)`) name their row directly and skip this —
    /// the click event itself is already exact.
    fn resolve_selected(&self, rows: &[PaletteRow]) -> Option<usize> {
        let (selected, identity) = match &self.app.modal {
            Modal::SessionLauncher {
                selected,
                selected_identity,
                ..
            } => (*selected, selected_identity.clone()),
            _ => return None,
        };
        resolve_row_by_identity(rows, &identity, selected)
    }

    /// Activate the row at `i` in the currently-rendered root/typing/
    /// browse-all list: launch a `Recent`/`Combo` row directly, or run the
    /// effect of an action row.
    fn launcher_activate(&mut self, i: usize) -> Task<Msg> {
        let (input, browse_all) = match &self.app.modal {
            Modal::SessionLauncher {
                input, browse_all, ..
            } => (input.clone(), *browse_all),
            _ => return Task::none(),
        };
        let rows = self.palette_rows(&input, browse_all);
        let Some(row) = rows.get(i) else {
            return Task::none();
        };
        match row {
            PaletteRow::Recent { proj, wt_path, .. } | PaletteRow::Combo { proj, wt_path, .. } => {
                let proj = *proj;
                let agent = match row {
                    PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. } => *agent,
                    _ => unreachable!(),
                };
                let Some(wt) = self
                    .launcher_worktrees(proj)
                    .iter()
                    .position(|w| &w.path == wt_path)
                else {
                    return Task::none();
                };
                let Some(agent_idx) = self.app.available_agents.iter().position(|a| *a == agent)
                else {
                    return Task::none();
                };
                self.launcher_launch(proj, wt, agent_idx);
                Task::none()
            }
            PaletteRow::NewSession => {
                if let Modal::SessionLauncher { browse_all, .. } = &mut self.app.modal {
                    *browse_all = true;
                }
                self.set_palette_selected(0);
                Task::none()
            }
            PaletteRow::TerminalHome => {
                let _ = self.update(Msg::NewHomeTerminal);
                self.terminal_focused = true;
                self.app.modal = Modal::None;
                Task::none()
            }
            PaletteRow::TerminalWt => {
                if !self.term_panel_open {
                    let _ = self.update(Msg::ToggleTermPanel);
                } else {
                    let _ = self.update(Msg::NewWtTerminal);
                }
                self.app.modal = Modal::None;
                Task::none()
            }
            PaletteRow::AddProject => {
                self.app.modal = Modal::None;
                self.update(Msg::AddProject)
            }
            PaletteRow::SwitchToSession => {
                // Inert outside zen (row still shows, muted, with a "zen
                // only" hint — see `palette_row_view`): swallow the Enter,
                // keep the palette open, no state change.
                if self.switch_to_session_active() {
                    self.launcher_enter_switch();
                }
                Task::none()
            }
            PaletteRow::Settings => self.open_settings_drill_in(),
            PaletteRow::Setting(s) => {
                let s = *s;
                let task = self.activate_setting(s);
                self.reselect_typed_setting(s, &input, browse_all, i);
                task
            }
            PaletteRow::ReloadThemes => self.reload_themes(),
        }
    }

    /// The typed-root-list arm of the toggle re-anchor (the drill-in arm is
    /// `reselect_after_toggle`): a toggle can drop its own row out of a
    /// value-matched query, leaving the cursor on a shifted row. Keep the
    /// cursor on the same setting row if it's still rendered, else clamp.
    /// Toggles only — every other Setting row just entered the drill-in,
    /// whose own cursor logic owns the selection now.
    fn reselect_typed_setting(&mut self, s: SettingRow, input: &str, browse_all: bool, old: usize) {
        if !matches!(s, SettingRow::ProjectThemes | SettingRow::Telemetry) {
            return;
        }
        let rows = self.palette_rows(input, browse_all);
        let new_sel = rows
            .iter()
            .position(|r| matches!(r, PaletteRow::Setting(x) if *x == s))
            .unwrap_or_else(|| crate::gui::launcher::clamp(old, 0, rows.len()));
        self.set_palette_selected(new_sel);
    }

    /// Enter the "switch to session" drill-in: selects the first row and
    /// clears the search input so the full session list is visible
    /// immediately, rather than still filtered by whatever root/typing query
    /// was active (e.g. "swi", which is how the row itself was found).
    /// Typing afterward re-filters as normal (`Msg::LauncherInputChanged`).
    /// Esc backs out without restoring that query — same as OPEN WITH, which
    /// doesn't touch `input` at all going in or coming out.
    fn launcher_enter_switch(&mut self) {
        if let Modal::SessionLauncher { input, .. } = &mut self.app.modal {
            input.clear();
        }
        self.set_switch_selected(0);
    }

    /// Enter the Settings drill-in (root "Settings…" row, Enter or Tab):
    /// selects the first row and clears the search input, same rationale as
    /// `launcher_enter_switch` — a query like "settings" that found the row
    /// shouldn't still be filtering the drill-in's own, unrelated list.
    /// Returns the scroll-to-top task: the scrollable's offset persists by
    /// widget id, so a reopened drill-in would otherwise resume wherever a
    /// previous visit left it — with the cursor invisibly back at row 0.
    fn open_settings_drill_in(&mut self) -> Task<Msg> {
        if let Modal::SessionLauncher {
            settings, input, ..
        } = &mut self.app.modal
        {
            *settings = Some(LauncherSettings {
                pane: SettingsPane::Root,
                selected: 0,
                resizing: false,
                update_actions: None,
            });
            input.clear();
        }
        self.set_palette_selected(0);
        self.scroll_launcher_settings_to_selection()
    }

    /// Apply the effect of activating setting `s` — from a root-mode direct
    /// `PaletteRow::Setting` match, or an Enter/click inside the Settings
    /// drill-in. The two toggles flip in place through the exact `Msg`
    /// handlers `settings_modal`'s checkboxes already use, so persistence
    /// stays on that single existing path; the value shown in the palette
    /// re-reads live state (`setting_value`) on the next frame, so no local
    /// mirror is needed here. `CheckUpdates` kicks off the same off-thread
    /// check `Msg::CheckForUpdates` does — its `Task` must be returned to
    /// the iced runtime (not discarded) or the check never fires — unless a
    /// release is already known to be available, in which case it expands
    /// the update-actions strip instead of pointlessly re-checking (E3; see
    /// `check_updates_opens_strip`). Enum rows (Theme/Backend/Permissions/
    /// DefaultAgent/AppSize) each open a dedicated sub-pane (`SettingsPane`)
    /// — see the `enter_*_pane` methods.
    fn activate_setting(&mut self, s: SettingRow) -> Task<Msg> {
        match s {
            SettingRow::ProjectThemes => {
                let task =
                    self.update(Msg::ProjectThemesToggle(!self.app.project_themes_enabled()));
                Task::batch([task, self.reselect_after_toggle(s)])
            }
            SettingRow::Telemetry => {
                let task = self.update(Msg::TelemetryToggle(!self.app.telemetry_enabled()));
                Task::batch([task, self.reselect_after_toggle(s)])
            }
            SettingRow::CheckUpdates => {
                if check_updates_opens_strip(&self.upgrade) {
                    self.open_update_actions_strip()
                } else {
                    self.update(Msg::CheckForUpdates { manual: true })
                }
            }
            SettingRow::Theme => self.enter_theme_pane(),
            SettingRow::Backend => self.enter_backend_pane(),
            SettingRow::Permissions => self.enter_permissions_pane(),
            SettingRow::DefaultAgent => self.enter_default_agent_pane(),
            SettingRow::AppSize => self.enter_appsize_resize(),
        }
    }

    /// Re-anchor the drill-in Root cursor after a toggle: flipping On/Off
    /// rewrites the value string the active query may have been matching
    /// (e.g. "on"), so the row can drop out of — or shift within — the
    /// filtered list under the unmoved cursor. Keep the cursor on the
    /// toggled row when it survived the refilter (`reselect_setting`), else
    /// clamp, then scroll with it. No-op outside the drill-in — the
    /// root/typed list re-anchors in `launcher_activate`'s `Setting` arm.
    fn reselect_after_toggle(&mut self, activated: SettingRow) -> Task<Msg> {
        let input = match &self.app.modal {
            Modal::SessionLauncher {
                input,
                settings: Some(_),
                ..
            } => input.clone(),
            _ => return Task::none(),
        };
        let rows = self.settings_rows_filtered(&input);
        if let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        {
            ls.selected = reselect_setting(&rows, activated, ls.selected);
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Land `pane`/`selected` on the Settings drill-in and clear the query,
    /// same rationale as `open_settings_drill_in` — a query that found the
    /// enum row at root shouldn't keep filtering a sub-pane whose own list
    /// means something else. Reachable straight from a root/typing
    /// `PaletteRow::Setting` match (B2 in the mock), so the drill-in is
    /// opened first when absent — Esc from the pane then pops to the
    /// drill-in Root list, one level at a time, like any other pane exit.
    fn enter_settings_pane(&mut self, pane: SettingsPane, selected: usize) {
        if !matches!(
            &self.app.modal,
            Modal::SessionLauncher {
                settings: Some(_),
                ..
            }
        ) {
            // The Root-list scroll task is deliberately not chained: this
            // immediately switches to a sub-pane, whose view doesn't render
            // that scrollable at all, and every path back to Root
            // (`return_to_settings_root`) re-scrolls on its own.
            let _ = self.open_settings_drill_in();
        }
        if let Modal::SessionLauncher {
            settings: Some(ls),
            input,
            ..
        } = &mut self.app.modal
        {
            ls.pane = pane;
            ls.selected = selected;
            ls.update_actions = None;
            input.clear();
        }
    }

    /// Pop a sub-pane back to the Root settings list, landing the cursor on
    /// `from`'s row. Root's own list is recomputed unfiltered
    /// (`settings_rows_filtered("")`) since the query was cleared entering
    /// the sub-pane, mirroring `enter_settings_pane`. Returns the scroll
    /// task landing the viewport with the cursor — `from` can sit near the
    /// bottom of the list (Default agent, Check for updates).
    fn return_to_settings_root(&mut self, from: SettingRow) -> Task<Msg> {
        let selected = self
            .settings_rows_filtered("")
            .iter()
            .position(|s| *s == from)
            .unwrap_or(0);
        if let Modal::SessionLauncher {
            settings: Some(ls),
            input,
            ..
        } = &mut self.app.modal
        {
            ls.pane = SettingsPane::Root;
            ls.selected = selected;
            ls.resizing = false;
            ls.update_actions = None;
            input.clear();
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Expand the update-available actions strip under the Check-for-updates
    /// row (E3). From the Settings drill-in the strip simply opens in place;
    /// from a root-mode `PaletteRow::Setting` match the drill-in is opened
    /// first, landed on that row, so the strip has a row to hang under.
    /// Returns the scroll task for that landing — CheckUpdates is the last
    /// row, guaranteed below the 380px fold of a fresh drill-in.
    fn open_update_actions_strip(&mut self) -> Task<Msg> {
        let in_drill_in = matches!(
            &self.app.modal,
            Modal::SessionLauncher {
                settings: Some(_),
                ..
            }
        );
        if !in_drill_in {
            // Entry scroll superseded by the one returned below, once the
            // cursor has landed on the CheckUpdates row.
            let _ = self.open_settings_drill_in();
            let idx = self
                .settings_rows_filtered("")
                .iter()
                .position(|s| *s == SettingRow::CheckUpdates)
                .unwrap_or(0);
            if let Modal::SessionLauncher {
                settings: Some(ls), ..
            } = &mut self.app.modal
            {
                ls.selected = idx;
            }
        }
        if let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        {
            ls.update_actions = Some(0);
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Collapse the update-actions strip, staying in the drill-in Root list.
    fn close_update_actions_strip(&mut self) {
        if let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        {
            ls.update_actions = None;
        }
    }

    /// Run strip action `idx` (⏎ or click). `StartUpdate`'s handler replaces
    /// the palette with the Updating progress modal on its own; `SkipVersion`
    /// flips `upgrade` out of `Available`, so the strip closes with it (the
    /// row's value slot re-derives to "Up to date"); `CopyUrl` is a pure side
    /// effect — the strip stays open for a follow-up action.
    fn update_actions_commit(&mut self, idx: usize) -> Task<Msg> {
        let method_unknown = matches!(self.upgrade_method, crate::upgrade::InstallMethod::Unknown);
        let Some(&action) = update_available_actions(method_unknown).get(idx) else {
            return Task::none();
        };
        match action {
            UpdateAction::UpdateNow => self.update(Msg::StartUpdate),
            UpdateAction::SkipVersion => {
                let task = self.update(Msg::SkipVersion);
                self.close_update_actions_strip();
                task
            }
            UpdateAction::CopyUrl => self.update(Msg::CopyReleaseUrl),
        }
    }

    /// Enter the Theme sub-pane (D1 in the palette redesign mock): previews
    /// live like `Modal::ThemePicker`, so `original` is captured up front for
    /// Esc to restore. Starts on the active theme's own kind list, cursor on
    /// the active theme (mirrors `App::open_theme_picker`).
    fn enter_theme_pane(&mut self) -> Task<Msg> {
        let original = crate::theme::current();
        let kind = original.kind;
        let selected = theme_pane_selected_index(kind, &original.name);
        let follow_system = self.app.theme_follow_system;
        self.enter_settings_pane(
            SettingsPane::Theme {
                original,
                kind,
                follow_system,
            },
            selected,
        );
        // The entry scroll is load-bearing: `themes_of` is alphabetical, so
        // without it the pre-selected current theme usually sits below the
        // pane's 280px fold and the pane appears to have selected nothing.
        self.scroll_launcher_theme_to_selection()
    }

    /// Enter the Backend sub-pane (D2): cursor starts on the active backend.
    fn enter_backend_pane(&mut self) -> Task<Msg> {
        let selected = backend_pane_selected_index(self.app.use_tmux());
        self.enter_settings_pane(SettingsPane::Backend, selected);
        Task::none()
    }

    /// Enter the Permissions sub-pane (E1): cursor starts on the active
    /// choice (Ask/Skip).
    fn enter_permissions_pane(&mut self) -> Task<Msg> {
        let selected = permissions_pane_selected_index(self.app.skip_permissions_enabled());
        self.enter_settings_pane(SettingsPane::Permissions, selected);
        Task::none()
    }

    /// Enter the DefaultAgent sub-pane (D3): cursor starts on the current
    /// default (or `Agent::ALL[0]` if none set). Kicks off the same tool
    /// detection `settings_modal` triggers on open when it hasn't run yet,
    /// so install status/version populate instead of showing "detecting…"
    /// forever.
    fn enter_default_agent_pane(&mut self) -> Task<Msg> {
        let selected = default_agent_pane_selected_index(self.app.store.default_agent);
        self.enter_settings_pane(SettingsPane::DefaultAgent, selected);
        if self.settings_tools.is_empty() {
            return self.update(Msg::RefreshTools);
        }
        Task::none()
    }

    /// Enter App-size inline-edit mode (D4): stays on the Root pane —
    /// `resizing` swaps the selected row's value slot for the live stepper.
    /// From a root-mode `PaletteRow::Setting` match the drill-in is opened
    /// first, landed on the App-size row, so the stepper has a row to live on
    /// (same shape as `open_update_actions_strip`).
    fn enter_appsize_resize(&mut self) -> Task<Msg> {
        if !matches!(
            &self.app.modal,
            Modal::SessionLauncher {
                settings: Some(_),
                ..
            }
        ) {
            // Entry scroll superseded by the one returned below, once the
            // cursor has landed on the App-size row.
            let _ = self.open_settings_drill_in();
            let idx = self
                .settings_rows_filtered("")
                .iter()
                .position(|s| *s == SettingRow::AppSize)
                .unwrap_or(0);
            if let Modal::SessionLauncher {
                settings: Some(ls), ..
            } = &mut self.app.modal
            {
                ls.selected = idx;
            }
        }
        if let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        {
            ls.resizing = true;
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Scroll the Theme sub-pane's list so the selected row is centered —
    /// same shape as `scroll_theme_picker_to_selection`, against the pane's
    /// own 36px rows and 280px cap. Chained from every path that moves the
    /// selection or reshapes the list (entry, ↑↓, clicks, mode switches,
    /// query edits): `themes_of` is alphabetical, so without this the
    /// current theme usually sits below the fold on entry and the highlight
    /// moves invisibly.
    fn scroll_launcher_theme_to_selection(&self) -> Task<Msg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Modal::SessionLauncher {
            input,
            settings:
                Some(LauncherSettings {
                    pane: SettingsPane::Theme { kind, .. },
                    selected,
                    ..
                }),
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        // Approximation: this pitches every row (including the "Custom"
        // section header/empty-hint) at the uniform 38px row pitch rather
        // than each element's true rendered height, so centering drifts
        // slightly once the Custom section is showing. Good enough to keep
        // the selection on-screen; a pixel-exact version would need the
        // section-aware walk `settings_root_scroll_offset` uses.
        let total = theme_pane_combined_rows(*kind, input).len();
        let y = launcher_theme_scroll_offset(total, *selected);
        scroll_to(
            super::view::launcher_theme_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Scroll the Settings drill-in's Root list so the selected row is
    /// centered — the Root-pane counterpart of
    /// `scroll_launcher_theme_to_selection`, chained from every path that
    /// moves the Root cursor or rebuilds the list (↑↓, drill-in entry,
    /// sub-pane exits landing near the bottom, query edits). No-op outside
    /// the Root pane.
    fn scroll_launcher_settings_to_selection(&self) -> Task<Msg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Modal::SessionLauncher {
            input,
            settings:
                Some(LauncherSettings {
                    pane: SettingsPane::Root,
                    selected,
                    ..
                }),
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        let rows = self.settings_rows_filtered(input);
        let y = settings_root_scroll_offset(&rows, *selected);
        scroll_to(
            super::view::launcher_settings_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Scroll the root/typed palette list so the selected row is centered —
    /// the un-drilled-in list's counterpart to `scroll_launcher_settings_
    /// to_selection`. No-op whenever a sub-state (options, switch drill-in,
    /// settings drill-in, or the row-actions strip) is showing instead of
    /// this list — see the `else` branch in `view.rs` that renders it.
    fn scroll_launcher_palette_to_selection(&self) -> Task<Msg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Modal::SessionLauncher {
            input,
            selected,
            browse_all,
            options: None,
            switch: None,
            row_actions: None,
            settings: None,
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        let rows = self.palette_rows(input, *browse_all);
        let zero_projects = self.app.store.projects.is_empty();
        let root_mode = input.is_empty() && !*browse_all && !zero_projects;
        let y = palette_scroll_offset(&rows, *selected, root_mode);
        scroll_to(
            super::view::launcher_palette_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Theme sub-pane ↑↓ / row click: move (or jump straight to, for a
    /// click) the selection within the *currently shown* kind's
    /// fuzzy-filtered list and preview it immediately (`theme::set`) — same
    /// live-preview idiom as `Grove::theme_picker_select`/`theme_picker_move`.
    /// Selecting a concrete theme opts back out of "follow system", mirroring
    /// `ThemePickerScope::App`'s arm of `App::theme_picker_move`.
    fn theme_pane_select(&mut self, idx: usize) -> Task<Msg> {
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return Task::none(),
        };
        let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        else {
            return Task::none();
        };
        let SettingsPane::Theme {
            kind,
            follow_system,
            ..
        } = &mut ls.pane
        else {
            return Task::none();
        };
        let rows = theme_pane_combined_rows(*kind, &input);
        let Some(theme) = rows.get(idx).cloned() else {
            return Task::none();
        };
        ls.selected = idx;
        *follow_system = false;
        crate::theme::set(theme);
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme sub-pane ↑↓ (keyboard): delta-move `theme_pane_select` over the
    /// currently shown kind's fuzzy-filtered combined (Built-in + Custom)
    /// list length.
    fn theme_pane_move(&mut self, delta: i32) -> Task<Msg> {
        let (input, selected, kind) = match &self.app.modal {
            Modal::SessionLauncher {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            } => (input.clone(), *selected, *kind),
            _ => return Task::none(),
        };
        let len = theme_pane_combined_rows(kind, &input).len();
        let new_sel = crate::gui::launcher::clamp(selected, delta, len);
        self.theme_pane_select(new_sel)
    }

    /// Theme sub-pane Dark/Light segment (click or Tab-cycle): switches
    /// which kind's list is shown, opts out of "follow system", and previews
    /// the active theme's position in the new list (or the first entry if it
    /// has none).
    fn theme_pane_set_kind(&mut self, kind: crate::theme::ThemeKind) -> Task<Msg> {
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return Task::none(),
        };
        let rows = theme_pane_combined_rows(kind, &input);
        let active = crate::theme::current();
        let idx = rows.iter().position(|t| t.name == active.name).unwrap_or(0);
        let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        else {
            return Task::none();
        };
        let SettingsPane::Theme {
            kind: k,
            follow_system,
            ..
        } = &mut ls.pane
        else {
            return Task::none();
        };
        *k = kind;
        *follow_system = false;
        ls.selected = idx;
        if let Some(t) = rows.get(idx) {
            crate::theme::set(t.clone());
        }
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme sub-pane System segment (click or Tab-cycle): previews the
    /// resolved system theme and marks "follow system" as a local draft
    /// (persisted on ⏎) — mirrors `Grove::theme_picker_toggle_system(true)`.
    /// The list always falls back to the dark set under "system", since
    /// system mode still needs a concrete dark choice.
    fn theme_pane_set_system(&mut self) -> Task<Msg> {
        let name = self
            .app
            .resolve_system_theme_name(self.app.system_theme_mode)
            .to_string();
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return Task::none(),
        };
        // The list snaps back to the (filtered) dark set: re-clamp the
        // cursor so a selection made deep in a longer list can't dangle
        // past the end.
        let dark_len = theme_pane_combined_rows(crate::theme::ThemeKind::Dark, &input).len();
        let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        else {
            return Task::none();
        };
        let SettingsPane::Theme {
            kind,
            follow_system,
            ..
        } = &mut ls.pane
        else {
            return Task::none();
        };
        ls.selected = crate::gui::launcher::clamp(ls.selected, 0, dark_len);
        *kind = crate::theme::ThemeKind::Dark;
        *follow_system = true;
        crate::theme::set_by_name(&name);
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme sub-pane Tab: cycle the mode row Dark → Light → System → Dark
    /// (`next_theme_mode`), routed through the same `theme_pane_set_kind` /
    /// `theme_pane_set_system` paths the segment clicks take, so preview and
    /// selection behave identically to clicking.
    fn theme_pane_cycle_mode(&mut self) -> Task<Msg> {
        let (kind, follow_system) = match &self.app.modal {
            Modal::SessionLauncher {
                settings:
                    Some(LauncherSettings {
                        pane:
                            SettingsPane::Theme {
                                kind,
                                follow_system,
                                ..
                            },
                        ..
                    }),
                ..
            } => (*kind, *follow_system),
            _ => return Task::none(),
        };
        match next_theme_mode(kind, follow_system) {
            ThemeMode::Dark => self.theme_pane_set_kind(crate::theme::ThemeKind::Dark),
            ThemeMode::Light => self.theme_pane_set_kind(crate::theme::ThemeKind::Light),
            ThemeMode::System => self.theme_pane_set_system(),
        }
    }

    /// Theme sub-pane ⏎: persist the previewed theme (or "follow system")
    /// through the same `Store` fields `App::theme_picker_submit` writes,
    /// then return to Root landed on the App theme row.
    fn theme_pane_commit(&mut self) -> Task<Msg> {
        let follow_system = match &self.app.modal {
            Modal::SessionLauncher {
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { follow_system, .. },
                        ..
                    }),
                ..
            } => *follow_system,
            _ => return Task::none(),
        };
        self.app.theme_follow_system = follow_system;
        self.app.store.theme_follow_system = follow_system;
        if follow_system {
            self.app.apply_system_theme();
        } else {
            let chosen = crate::theme::current();
            self.app.store.theme = Some(chosen.name.to_string());
            match chosen.kind {
                crate::theme::ThemeKind::Dark => {
                    self.app.store.theme_dark = Some(chosen.name.to_string())
                }
                crate::theme::ThemeKind::Light => {
                    self.app.store.theme_light = Some(chosen.name.to_string())
                }
            }
            crate::theme::set(chosen);
        }
        let _ = crate::storage::save(&self.app.store);
        self.invalidate_pty_render_cache();
        self.return_to_settings_root(SettingRow::Theme)
    }

    /// Theme sub-pane Esc: restore the pre-entry theme and return to Root.
    fn theme_pane_cancel(&mut self) -> Task<Msg> {
        let original = match &self.app.modal {
            Modal::SessionLauncher {
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { original, .. },
                        ..
                    }),
                ..
            } => original.clone(),
            _ => return Task::none(),
        };
        crate::theme::set(original);
        self.invalidate_pty_render_cache();
        self.return_to_settings_root(SettingRow::Theme)
    }

    /// "Reload themes" command (palette root, keyword-only — see
    /// `palette_rows`): re-reads `themes.json` via `theme::load_custom`,
    /// replacing `App::theme_load_errors` with the fresh result and
    /// surfacing it as a toast (mock E1/E2). If the active theme's name no
    /// longer resolves anywhere afterward (edited out from under the app,
    /// deleted, synced from another machine, ...), falls back to the mode
    /// default *silently* — this is routine config drift, not an error, per
    /// the mock's E3 sticky note. If the Theme sub-pane happens to be open,
    /// its combined Built-in+Custom list just changed under it, so the
    /// cursor is reclamped and rescrolled instead of left dangling; there's
    /// nothing pane-specific to refresh otherwise, so the palette just closes
    /// (same "run and dismiss" idiom as `PaletteRow::AddProject`).
    fn reload_themes(&mut self) -> Task<Msg> {
        self.app.theme_load_errors = crate::theme::load_custom();

        let active = crate::theme::current();
        if let Some(fallback) = theme_reload_fallback(&active.name, active.kind) {
            crate::theme::set_by_name(fallback);
            self.app.store.theme = Some(fallback.to_string());
            match active.kind {
                crate::theme::ThemeKind::Dark => {
                    self.app.store.theme_dark = Some(fallback.to_string())
                }
                crate::theme::ThemeKind::Light => {
                    self.app.store.theme_light = Some(fallback.to_string())
                }
            }
            let _ = crate::storage::save(&self.app.store);
        }

        match crate::theme_file::summarize_errors(&self.app.theme_load_errors) {
            Some(summary) => self.app.set_error_toast(summary),
            None => self.app.set_toast("themes.json reloaded"),
        }

        let theme_pane_kind_input = match &self.app.modal {
            Modal::SessionLauncher {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        ..
                    }),
                ..
            } => Some((*kind, input.clone())),
            _ => None,
        };
        self.invalidate_pty_render_cache();
        match theme_pane_kind_input {
            Some((kind, input)) => {
                let total = theme_pane_combined_rows(kind, &input).len();
                if let Modal::SessionLauncher {
                    settings: Some(ls), ..
                } = &mut self.app.modal
                {
                    ls.selected = ls.selected.min(total.saturating_sub(1));
                }
                self.scroll_launcher_theme_to_selection()
            }
            None => {
                self.app.modal = Modal::None;
                Task::none()
            }
        }
    }


    // ── Modal::ThemeManager LIST sub-view ───────────────────────────────────

    /// Opens the dedicated theme-management modal, replacing whatever's
    /// currently showing (including the palette, if it was open) — this is a
    /// top-level `Modal`, not a palette drill-in pane.
    fn open_theme_manager(&mut self) -> Task<Msg> {
        self.app.modal = Modal::ThemeManager {
            selected: 0,
            rename: None,
            rename_error: None,
            pending_delete: None,
        };
        self.theme_manager_editor = None;
        Task::none()
    }

    fn theme_manager_close(&mut self) {
        self.app.modal = Modal::None;
        self.theme_manager_editor = None;
    }

    fn theme_manager_select(&mut self, idx: usize) {
        let len = crate::theme::all_custom_themes().len();
        if idx >= len {
            return;
        }
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = idx;
        }
    }

    fn theme_manager_move(&mut self, delta: i32) {
        let len = crate::theme::all_custom_themes().len();
        if len == 0 {
            return;
        }
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = crate::gui::launcher::clamp(*selected, delta, len);
        }
    }

    /// "New theme": creates a fresh custom theme seeded from the *current
    /// active theme's* mode default (`DEFAULT_DARK_THEME`/`DEFAULT_LIGHT_
    /// THEME`), auto-named via `theme::duplicate_name("untitled")`, and opens
    /// the EDITOR sub-view on it directly.
    fn theme_manager_new(&mut self) -> Task<Msg> {
        let kind = crate::theme::current().kind;
        let seed_name = match kind {
            crate::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
            crate::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
        };
        let Some(seed) = crate::theme::by_name(seed_name) else {
            return Task::none(); // defensive: the mode defaults are builtins
        };
        let new_name = crate::theme::duplicate_name("untitled");
        let mut new_theme = seed;
        new_theme.name = std::borrow::Cow::Owned(new_name);
        new_theme.kind = kind;
        if let Err(e) = crate::theme::add_custom(new_theme.clone()) {
            self.app.set_error_toast(e);
            return Task::none();
        }
        self.open_theme_manager_editor(new_theme)
    }

    /// Duplicates the theme at row `idx` into a new custom theme (auto-named
    /// via `theme::duplicate_name`) and selects the copy.
    fn theme_manager_duplicate(&mut self, idx: usize) {
        let Some(base) = crate::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        let new_name = crate::theme::duplicate_name(&base.name);
        let mut copy = base;
        copy.name = std::borrow::Cow::Owned(new_name.clone());
        if let Err(e) = crate::theme::add_custom(copy) {
            self.app.set_error_toast(e);
            return;
        }
        let idx = crate::theme::all_custom_themes()
            .iter()
            .position(|t| t.name == new_name)
            .unwrap_or(idx);
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = idx;
        }
    }

    fn theme_manager_rename_start(&mut self, idx: usize) {
        let Some(theme) = crate::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        if let Modal::ThemeManager {
            selected,
            rename,
            rename_error,
            ..
        } = &mut self.app.modal
        {
            *selected = idx;
            let name = theme.name.to_string();
            *rename = Some((name.clone(), name));
            *rename_error = None;
        }
    }

    fn theme_manager_rename_changed(&mut self, s: String) {
        if let Modal::ThemeManager { rename: Some((_, buf)), rename_error, .. } = &mut self.app.modal
        {
            *buf = s;
            *rename_error = None;
        }
    }

    fn theme_manager_rename_submit(&mut self) {
        let Modal::ThemeManager {
            rename: Some((original, buffer)),
            ..
        } = &self.app.modal
        else {
            return;
        };
        let (original, buffer) = (original.clone(), buffer.clone());
        let new_name = buffer.trim().to_string();
        if new_name.is_empty() {
            if let Modal::ThemeManager { rename_error, .. } = &mut self.app.modal {
                *rename_error = Some("name can't be empty".to_string());
            }
            return;
        }
        if new_name == original {
            self.theme_manager_rename_cancel();
            return;
        }
        match crate::theme::rename_custom(&original, &new_name) {
            Ok(()) => {
                self.invalidate_pty_render_cache();
                let idx = crate::theme::all_custom_themes()
                    .iter()
                    .position(|t| t.name == new_name)
                    .unwrap_or(0);
                if let Modal::ThemeManager {
                    selected,
                    rename,
                    rename_error,
                    ..
                } = &mut self.app.modal
                {
                    *selected = idx;
                    *rename = None;
                    *rename_error = None;
                }
            }
            Err(e) => {
                if let Modal::ThemeManager { rename_error, .. } = &mut self.app.modal {
                    *rename_error = Some(e);
                }
            }
        }
    }

    fn theme_manager_rename_cancel(&mut self) {
        if let Modal::ThemeManager {
            rename,
            rename_error,
            ..
        } = &mut self.app.modal
        {
            *rename = None;
            *rename_error = None;
        }
    }

    fn theme_manager_delete_start(&mut self, idx: usize) {
        let Some(theme) = crate::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        if let Modal::ThemeManager {
            selected,
            pending_delete,
            ..
        } = &mut self.app.modal
        {
            *selected = idx;
            *pending_delete = Some(theme.name.to_string());
        }
    }

    /// Deletes the pending custom theme. Falls back to the mode default
    /// (`DEFAULT_DARK_THEME`/`DEFAULT_LIGHT_THEME`) if it was the active
    /// theme — same fallback the palette's Theme pane used.
    fn theme_manager_delete_confirm(&mut self) {
        let Modal::ThemeManager {
            pending_delete: Some(name),
            ..
        } = &self.app.modal
        else {
            return;
        };
        let name = name.clone();
        let Some(kind) = crate::theme::all_custom_themes()
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.kind)
        else {
            return;
        };
        let was_active = crate::theme::current().name == name;
        crate::theme::delete_custom(&name);
        if was_active {
            let fallback = match kind {
                crate::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
                crate::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
            };
            crate::theme::set_by_name(fallback);
            self.app.store.theme = Some(fallback.to_string());
            match kind {
                crate::theme::ThemeKind::Dark => {
                    self.app.store.theme_dark = Some(fallback.to_string())
                }
                crate::theme::ThemeKind::Light => {
                    self.app.store.theme_light = Some(fallback.to_string())
                }
            }
            let _ = crate::storage::save(&self.app.store);
        }
        self.invalidate_pty_render_cache();
        let total = crate::theme::all_custom_themes().len();
        if let Modal::ThemeManager {
            selected,
            pending_delete,
            ..
        } = &mut self.app.modal
        {
            *pending_delete = None;
            *selected = (*selected).min(total.saturating_sub(1));
        }
    }

    fn theme_manager_delete_cancel(&mut self) {
        if let Modal::ThemeManager { pending_delete, .. } = &mut self.app.modal {
            *pending_delete = None;
        }
    }

    /// List row's "Edit" button: opens the EDITOR sub-view on the custom
    /// theme at row `idx`.
    fn theme_manager_edit_start(&mut self, idx: usize) -> Task<Msg> {
        let Some(theme) = crate::theme::all_custom_themes().get(idx).cloned() else {
            return Task::none();
        };
        self.open_theme_manager_editor(theme)
    }

    // ── Palette Theme pane bridge into the editor ───────────────────────────

    /// Theme sub-pane ⌘E on a custom row: opens `Modal::ThemeManager`
    /// directly in its EDITOR sub-view for that theme (closing the palette —
    /// `ThemeManager` is a top-level modal, not a palette drill-in pane).
    fn theme_pane_open_editor(&mut self) -> Task<Msg> {
        let (input, selected, kind) = match &self.app.modal {
            Modal::SessionLauncher {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            } => (input.clone(), *selected, *kind),
            _ => return Task::none(),
        };
        if !theme_pane_row_is_custom(kind, &input, selected) {
            return Task::none(); // builtins aren't editable
        }
        let Some(draft) = theme_pane_combined_rows(kind, &input)
            .get(selected)
            .cloned()
        else {
            return Task::none();
        };
        self.open_theme_manager_editor(draft)
    }

    /// Shared landing for every path into `Modal::ThemeManager`'s EDITOR
    /// sub-view (list row's Edit button, "New theme", the palette's ⌘E):
    /// opens/keeps the modal on the list's state (so Esc-back-to-list always
    /// has somewhere sane to land), captures the pre-edit whole-app theme for
    /// discard/preview-off, seeds the row-0 hex buffer, applies `draft` as a
    /// live preview immediately, and scrolls to row 0.
    fn open_theme_manager_editor(&mut self, draft: crate::theme::Theme) -> Task<Msg> {
        let list_selected = crate::theme::all_custom_themes()
            .iter()
            .position(|t| t.name == draft.name)
            .unwrap_or(0);
        // Always land on a clean list state (no stray rename/delete
        // confirmation left open underneath) with the cursor on this theme's
        // row, whether the modal was already open or not.
        self.app.modal = Modal::ThemeManager {
            selected: list_selected,
            rename: None,
            rename_error: None,
            pending_delete: None,
        };
        let original_active = crate::theme::current();
        let original_name = draft.name.to_string();
        let saved = draft.clone();
        let hex_buf = crate::theme_file::to_hex(draft.field(0));
        // Prefill the paste box with the draft's own current values (named-
        // lines format) — a copyable reference and an editable starting
        // point, rather than an empty box.
        let paste = iced::widget::text_editor::Content::with_text(
            &crate::theme_file::to_named_lines(&draft),
        );
        self.theme_manager_editor = Some(ThemeManagerEditorState {
            original_name,
            draft: draft.clone(),
            saved,
            original_active,
            selected: 0,
            invalid: [false; 11],
            hex_buf,
            preview_on: true,
            confirm_discard: false,
            paste,
            paste_status: None,
            return_selected: list_selected,
        });
        crate::theme::set(draft);
        self.invalidate_pty_render_cache();
        self.scroll_theme_manager_editor_to_selection()
    }

    /// Editor ↑/↓: moves the row cursor and reseeds `hex_buf` with the new
    /// row's live hex text.
    fn theme_manager_editor_move(&mut self, delta: i32) -> Task<Msg> {
        let Some(ed) = &self.theme_manager_editor else {
            return Task::none();
        };
        let new_idx = crate::gui::launcher::clamp(ed.selected, delta, 11);
        self.theme_manager_editor_row_select(new_idx)
    }

    /// Editor row click / `theme_manager_editor_move`'s landing: jumps the
    /// row cursor straight to `idx` and reseeds `hex_buf` with that row's hex
    /// text.
    fn theme_manager_editor_row_select(&mut self, idx: usize) -> Task<Msg> {
        let Some(ed) = &mut self.theme_manager_editor else {
            return Task::none();
        };
        if idx >= 11 {
            return Task::none();
        }
        ed.selected = idx;
        ed.hex_buf = crate::theme_file::to_hex(ed.draft.field(idx));
        self.scroll_theme_manager_editor_to_selection()
    }

    /// Editor hex-field edit: a valid `#rrggbb` updates the focused row's
    /// color in `draft` and, while previewing, re-applies the whole draft
    /// live; invalid hex just flags that row's error state without touching
    /// `draft`.
    fn theme_manager_editor_hex_changed(&mut self, s: String) -> Task<Msg> {
        let parsed = crate::theme_file::parse_hex(&s).ok();
        let Some(ed) = &mut self.theme_manager_editor else {
            return Task::none();
        };
        ed.hex_buf = s;
        match parsed {
            Some(color) => {
                ed.draft.set_field(ed.selected, color);
                ed.invalid[ed.selected] = false;
                if ed.preview_on {
                    crate::theme::set(ed.draft.clone());
                }
                // The paste box reflects the draft, not a scratchpad the
                // user's own edits there must survive — resync it every time
                // a swatch-row hex commits to a valid value.
                ed.paste = iced::widget::text_editor::Content::with_text(
                    &crate::theme_file::to_named_lines(&ed.draft),
                );
            }
            None => ed.invalid[ed.selected] = true,
        }
        self.invalidate_pty_render_cache();
        Task::none()
    }

    /// Editor name field edit: live-updates `draft.name` (collision checking
    /// stays deferred to Save's `theme::update_custom` call, same as the old
    /// design's rename-on-save).
    fn theme_manager_editor_name_changed(&mut self, s: String) {
        if let Some(ed) = &mut self.theme_manager_editor {
            ed.draft.name = std::borrow::Cow::Owned(s);
        }
    }

    /// Editor Dark/Light toggle: updates `draft.kind` and re-previews.
    fn theme_manager_editor_set_kind(&mut self, kind: crate::theme::ThemeKind) {
        if let Some(ed) = &mut self.theme_manager_editor {
            ed.draft.kind = kind;
            if ed.preview_on {
                crate::theme::set(ed.draft.clone());
            }
        }
        self.invalidate_pty_render_cache();
    }

    /// Editor ⌘P / Preview button: toggles applying the draft to the whole
    /// app. ON re-applies `draft`; OFF shows `original_active` again without
    /// losing the draft's edits.
    fn theme_manager_editor_toggle_preview(&mut self) {
        let Some(ed) = &mut self.theme_manager_editor else {
            return;
        };
        ed.preview_on = !ed.preview_on;
        let next = if ed.preview_on {
            ed.draft.clone()
        } else {
            ed.original_active.clone()
        };
        crate::theme::set(next);
        self.invalidate_pty_render_cache();
    }

    /// Editor ⏎ (only reaches here with no text field focused) / ⌘S / Save
    /// button: persists `draft` into the `CUSTOM` registry via
    /// `theme::update_custom`, keeps it live-applied, and returns to the
    /// list landed on the saved theme's row.
    fn theme_manager_editor_save(&mut self) -> Task<Msg> {
        let Some(ed) = self.theme_manager_editor.take() else {
            return Task::none();
        };
        if let Err(e) = crate::theme::update_custom(&ed.original_name, ed.draft.clone()) {
            self.app.set_error_toast(e);
            self.theme_manager_editor = Some(ed);
            return Task::none();
        }
        self.app.set_toast(format!("theme saved: {}", ed.draft.name));
        let name = ed.draft.name.to_string();
        crate::theme::set(ed.draft);
        let idx = crate::theme::all_custom_themes()
            .iter()
            .position(|t| t.name == name)
            .unwrap_or(ed.return_selected);
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = idx;
        }
        self.invalidate_pty_render_cache();
        Task::none()
    }

    /// Editor Esc: dirty edits show the "Discard changes…?" confirmation
    /// (mirroring `Modal::Confirm`'s Enter=yes/Esc=no convention); with
    /// nothing to lose, Esc discards immediately.
    fn theme_manager_editor_esc(&mut self) -> Task<Msg> {
        let Some(ed) = &mut self.theme_manager_editor else {
            return Task::none();
        };
        if !ed.is_dirty() {
            return self.theme_manager_editor_discard();
        }
        ed.confirm_discard = true;
        Task::none()
    }

    /// Editor discard-confirmation Esc ("Keep editing"): drops the
    /// confirmation, keeps the draft and the editor open.
    fn theme_manager_editor_discard_cancel(&mut self) {
        if let Some(ed) = &mut self.theme_manager_editor {
            ed.confirm_discard = false;
        }
    }

    /// Editor discard-confirmation Enter ("Discard"), or Esc outright when
    /// there was nothing dirty to confirm: reverts the whole app to
    /// `original_active` and returns to the list.
    fn theme_manager_editor_discard(&mut self) -> Task<Msg> {
        let Some(ed) = self.theme_manager_editor.take() else {
            return Task::none();
        };
        crate::theme::set(ed.original_active);
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = ed.return_selected;
        }
        self.invalidate_pty_render_cache();
        Task::none()
    }

    /// Paste box edit: routes through the same `global_mods`/`live_mods`
    /// spurious-edit guard `LauncherInputChanged` uses — a chord like ⌘S or
    /// ⌘⏎ is a command, never text, but a focused `text_editor` (like
    /// `text_input`) doesn't special-case an arbitrary Cmd/Ctrl+Shift chord
    /// and would otherwise insert/delete on it unconditionally.
    fn theme_manager_paste_action(&mut self, action: iced::widget::text_editor::Action) {
        if action.is_edit() && global_mods(self.live_mods) {
            return;
        }
        if let Some(ed) = &mut self.theme_manager_editor {
            ed.paste.perform(action);
        }
    }

    /// Paste box "Apply" / ⌘⏎: parses the paste box's current text via
    /// `theme_file::parse_paste` and applies the result to the draft (a
    /// subset of fields is valid — only those fields change; `name`/`kind`
    /// prefill when the paste included them). Success shows "Applied N
    /// colors"; failure shows the parser's message, both inline under the
    /// box.
    fn theme_manager_paste_apply(&mut self) {
        let Some(ed) = &mut self.theme_manager_editor else {
            return;
        };
        let text = ed.paste.text();
        match crate::theme_file::parse_paste(&text) {
            Ok(applied) => {
                let n = applied.colors.len();
                crate::theme_file::apply_pasted_colors(&mut ed.draft, &applied);
                ed.invalid = [false; 11];
                ed.hex_buf = crate::theme_file::to_hex(ed.draft.field(ed.selected));
                if ed.preview_on {
                    crate::theme::set(ed.draft.clone());
                }
                // Resync the box to the (now-updated) draft — it's a
                // reflection of the draft, not a scratchpad to preserve.
                ed.paste = iced::widget::text_editor::Content::with_text(
                    &crate::theme_file::to_named_lines(&ed.draft),
                );
                ed.paste_status = Some(Ok(format!("Applied {n} colors")));
            }
            Err(e) => ed.paste_status = Some(Err(e)),
        }
        self.invalidate_pty_render_cache();
    }

    /// Scroll the editor's row list so the selected color row is centered —
    /// same idiom as `scroll_launcher_theme_to_selection`, via
    /// `theme_editor_scroll_offset`'s section-aware walk (Surfaces/Text/
    /// Accents headers, then the derived strip, aren't uniform-height rows).
    fn scroll_theme_manager_editor_to_selection(&self) -> Task<Msg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Some(ed) = &self.theme_manager_editor else {
            return Task::none();
        };
        let y = theme_editor_scroll_offset(ed.selected);
        scroll_to(
            super::view::theme_manager_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Whether the Settings drill-in is currently showing the ProjectTheme
    /// sub-pane — lets the shared `LauncherThemePaneSelect/Dark/Light` `Msg`s
    /// route to this pane's handlers instead of the app-theme pane's without
    /// adding parallel `Msg` variants.
    fn launcher_pane_is_project_theme(&self) -> bool {
        matches!(
            &self.app.modal,
            Modal::SessionLauncher {
                settings: Some(LauncherSettings {
                    pane: SettingsPane::ProjectTheme { .. },
                    ..
                }),
                ..
            }
        )
    }

    /// Enter the ProjectTheme sub-pane from a session row's actions strip
    /// (action `2`) — unlike `enter_theme_pane`, this is never reached from
    /// the Settings root list, so it sets `Modal::SessionLauncher` fields
    /// directly rather than routing through `enter_settings_pane`/
    /// `open_settings_drill_in` (which assume that entry point and don't
    /// touch `row_actions`). Starts on the project's pinned theme if it has
    /// one, else the current global theme's kind with no preview ("Use app
    /// theme").
    fn enter_project_theme_pane(&mut self, proj: usize) -> Task<Msg> {
        let pinned = self
            .app
            .store
            .projects
            .get(proj)
            .and_then(|p| p.theme.as_deref())
            .and_then(crate::theme::by_name);
        let kind = pinned
            .as_ref()
            .map(|t| t.kind)
            .unwrap_or(crate::theme::current().kind);
        let rows = project_theme_pane_rows(kind, "");
        let selected = rows
            .iter()
            .position(|t| match (t, &pinned) {
                (Some(a), Some(b)) => a.name == b.name,
                (None, None) => true,
                _ => false,
            })
            .unwrap_or(0);
        if let Modal::SessionLauncher {
            input,
            selected: outer_selected,
            row_actions,
            settings,
            ..
        } = &mut self.app.modal
        {
            input.clear();
            *outer_selected = 0;
            *row_actions = None;
            *settings = Some(LauncherSettings {
                pane: SettingsPane::ProjectTheme {
                    proj,
                    kind,
                    preview: pinned,
                },
                selected,
                resizing: false,
                update_actions: None,
            });
        }
        self.invalidate_pty_render_cache();
        self.scroll_launcher_project_theme_to_selection()
    }

    /// Scroll the ProjectTheme sub-pane's list so the selected row is
    /// centered — same geometry/idiom as `scroll_launcher_theme_to_selection`.
    fn scroll_launcher_project_theme_to_selection(&self) -> Task<Msg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Modal::SessionLauncher {
            input,
            settings:
                Some(LauncherSettings {
                    pane: SettingsPane::ProjectTheme { kind, .. },
                    selected,
                    ..
                }),
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        let total = project_theme_pane_rows(*kind, input).len();
        let y = launcher_theme_scroll_offset(total, *selected);
        scroll_to(
            super::view::launcher_theme_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// ProjectTheme sub-pane ↑↓ / row click: move (or jump straight to) the
    /// selection and update `preview` — never touches the global active
    /// theme (no `theme::set`), only the local draft `project_theme_override`
    /// reads.
    fn project_theme_pane_select(&mut self, idx: usize) -> Task<Msg> {
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return Task::none(),
        };
        let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        else {
            return Task::none();
        };
        let SettingsPane::ProjectTheme { kind, preview, .. } = &mut ls.pane else {
            return Task::none();
        };
        let rows = project_theme_pane_rows(*kind, &input);
        let Some(row) = rows.get(idx).cloned() else {
            return Task::none();
        };
        *preview = row;
        ls.selected = idx;
        self.invalidate_pty_render_cache();
        self.scroll_launcher_project_theme_to_selection()
    }

    /// ProjectTheme sub-pane ↑↓ (keyboard): delta-move over the currently
    /// shown kind's fuzzy-filtered list length.
    fn project_theme_pane_move(&mut self, delta: i32) -> Task<Msg> {
        let (input, selected, kind) = match &self.app.modal {
            Modal::SessionLauncher {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::ProjectTheme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            } => (input.clone(), *selected, *kind),
            _ => return Task::none(),
        };
        let len = project_theme_pane_rows(kind, &input).len();
        let new_sel = crate::gui::launcher::clamp(selected, delta, len);
        self.project_theme_pane_select(new_sel)
    }

    /// ProjectTheme sub-pane Dark/Light segment (click or Tab-cycle):
    /// switches which kind's list is shown, keeping the same theme selected
    /// by name if it exists in the new kind's list, else falling back to
    /// index 0 ("Use app theme" if the query is empty, else the first match).
    fn project_theme_pane_set_kind(&mut self, kind: crate::theme::ThemeKind) -> Task<Msg> {
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return Task::none(),
        };
        let rows = project_theme_pane_rows(kind, &input);
        let Modal::SessionLauncher {
            settings: Some(ls), ..
        } = &mut self.app.modal
        else {
            return Task::none();
        };
        let SettingsPane::ProjectTheme {
            kind: k, preview, ..
        } = &mut ls.pane
        else {
            return Task::none();
        };
        let current_name = preview.as_ref().map(|t| t.name.to_string());
        let idx = rows
            .iter()
            .position(|row| row.as_ref().map(|t| t.name.to_string()) == current_name)
            .unwrap_or(0);
        *k = kind;
        ls.selected = idx;
        *preview = rows.get(idx).cloned().flatten();
        self.invalidate_pty_render_cache();
        self.scroll_launcher_project_theme_to_selection()
    }

    /// ProjectTheme sub-pane Tab: cycle Dark ↔ Light only — no System mode
    /// for a project override.
    fn project_theme_pane_cycle_kind(&mut self) -> Task<Msg> {
        let kind = match &self.app.modal {
            Modal::SessionLauncher {
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::ProjectTheme { kind, .. },
                        ..
                    }),
                ..
            } => *kind,
            _ => return Task::none(),
        };
        let next = match kind {
            crate::theme::ThemeKind::Dark => crate::theme::ThemeKind::Light,
            crate::theme::ThemeKind::Light => crate::theme::ThemeKind::Dark,
        };
        self.project_theme_pane_set_kind(next)
    }

    /// ProjectTheme sub-pane ⏎: persist `preview` as the project's pinned
    /// theme (or clear it) through the same `Store` write/toast
    /// `App::theme_picker_submit`'s project-scope arm uses, then close the
    /// drill-in back to the plain session list (not Settings root — this
    /// pane was never entered from there).
    fn project_theme_pane_commit(&mut self) -> Task<Msg> {
        let (proj, preview) = match &self.app.modal {
            Modal::SessionLauncher {
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::ProjectTheme { proj, preview, .. },
                        ..
                    }),
                ..
            } => (*proj, preview.clone()),
            _ => return Task::none(),
        };
        match self.app.store.projects.get_mut(proj) {
            Some(p) => {
                p.theme = preview.as_ref().map(|t| t.name.to_string());
                let _ = crate::storage::save(&self.app.store);
                let label = preview
                    .as_ref()
                    .map(|t| t.name.to_string())
                    .unwrap_or_else(|| "default".to_string());
                self.app.set_toast(format!("project theme: {label}"));
            }
            None => {
                self.app.set_error_toast("project no longer exists");
            }
        }
        if let Modal::SessionLauncher {
            settings, input, ..
        } = &mut self.app.modal
        {
            *settings = None;
            input.clear();
        }
        self.invalidate_pty_render_cache();
        Task::none()
    }

    /// ProjectTheme sub-pane Esc: drop the preview and close the drill-in
    /// back to the plain session list — nothing was written, so there's
    /// nothing to restore.
    fn project_theme_pane_cancel(&mut self) -> Task<Msg> {
        if let Modal::SessionLauncher {
            settings, input, ..
        } = &mut self.app.modal
        {
            *settings = None;
            input.clear();
        }
        self.invalidate_pty_render_cache();
        Task::none()
    }

    /// Backend/Permissions/DefaultAgent sub-pane ⏎/click: commit row
    /// `selected` and return to Root. Shared by the three since they're all
    /// "pick one of a short fixed list, apply immediately" — only which
    /// `Msg` fires (and, for DefaultAgent, the installed-agent guard) differs.
    fn backend_pane_commit(&mut self, selected: usize) -> Task<Msg> {
        let task = if selected == 0 {
            self.update(Msg::BackendNative)
        } else {
            self.update(Msg::BackendTmux)
        };
        Task::batch([task, self.return_to_settings_root(SettingRow::Backend)])
    }

    fn permissions_pane_commit(&mut self, selected: usize) -> Task<Msg> {
        let task = if selected == 0 {
            self.update(Msg::SkipPermissionsDisable)
        } else {
            self.update(Msg::SkipPermissionsEnable)
        };
        Task::batch([task, self.return_to_settings_root(SettingRow::Permissions)])
    }

    /// Whether the DefaultAgent sub-pane row for `agent` is interactable.
    /// `Terminal` is always available; while tool detection is still
    /// empty/in-flight, every agent is treated as installed-unknown (no
    /// version text, but not inert) rather than inert.
    pub(super) fn default_agent_pane_row_installed(&self, agent: Agent) -> bool {
        if agent == Agent::Terminal || self.settings_tools.is_empty() {
            return true;
        }
        self.settings_tools
            .iter()
            .find(|t| t.agent == agent)
            .map(|t| t.installed)
            .unwrap_or(true)
    }

    fn default_agent_pane_commit(&mut self, selected: usize) -> Task<Msg> {
        let Some(&agent) = Agent::ALL.get(selected) else {
            return Task::none();
        };
        if !self.default_agent_pane_row_installed(agent) {
            return Task::none();
        }
        let task = self.update(Msg::SetDefaultAgent(agent));
        Task::batch([task, self.return_to_settings_root(SettingRow::DefaultAgent)])
    }

    /// Every `SettingRow` (in `SettingRow::ALL`'s section/definition order)
    /// fuzzy-filtered by `input`, for the Settings drill-in's live list and
    /// its keyboard nav. Shares the same 3-way (label/value/section) match
    /// `palette_rows` uses for root-mode `Setting` rows, via
    /// `launcher::matching_settings`, so the same query surfaces a setting
    /// whether you're still at root or already inside the drill-in.
    pub(super) fn settings_rows_filtered(&self, input: &str) -> Vec<SettingRow> {
        let values: Vec<String> = SettingRow::ALL
            .iter()
            .map(|s| self.setting_value(*s))
            .collect();
        let candidates: Vec<(SettingRow, &str, &str, &str)> = SettingRow::ALL
            .iter()
            .zip(values.iter())
            .map(|(s, v)| (*s, s.label(), v.as_str(), s.section()))
            .collect();
        crate::gui::launcher::matching_settings(input, &candidates)
    }

    /// Live value string for `s`, as shown right-aligned on its palette row.
    /// Cross-checked against `settings_modal`'s own value sources (view.rs)
    /// so the palette and the browse-view Settings modal never disagree.
    pub(super) fn setting_value(&self, s: SettingRow) -> String {
        match s {
            SettingRow::Theme => crate::theme::current().name.to_string(),
            SettingRow::AppSize => format!("{:.0}%", self.ui_zoom * 100.0),
            SettingRow::ProjectThemes => {
                if self.app.project_themes_enabled() {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingRow::Backend => {
                if self.app.use_tmux() {
                    "Tmux".to_string()
                } else {
                    "Native".to_string()
                }
            }
            SettingRow::Permissions => {
                if self.app.skip_permissions_enabled() {
                    "Skip".to_string()
                } else {
                    "Ask".to_string()
                }
            }
            SettingRow::Telemetry => {
                if self.app.telemetry_enabled() {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingRow::DefaultAgent => self
                .app
                .store
                .default_agent
                .map(|a| a.label().to_string())
                .unwrap_or_else(|| "auto".to_string()),
            SettingRow::CheckUpdates => {
                let ver = env!("CARGO_PKG_VERSION");
                match &self.upgrade {
                    UpgradeState::Idle => format!("v{ver}"),
                    UpgradeState::Checking => "Checking…".to_string(),
                    UpgradeState::UpToDate => format!("v{ver} · Up to date"),
                    UpgradeState::Available(r) => format!("Update available: {}", r.tag),
                    _ => "Updating…".to_string(),
                }
            }
        }
    }

    /// Tab in root/typing state: if the row at `i` is a `Recent`/`Combo`,
    /// reveal its inline contextual-action strip (Launch session…/Delete
    /// worktree); if it's `SwitchToSession`, open the switch-to-session
    /// drill-in directly (Tab behaves the same as Enter there); if it's a
    /// `Setting`/`Settings` row, Tab also mirrors Enter — enum settings
    /// extend into their sub-pane, toggles flip in place. Any other row
    /// is a no-op, same as before.
    fn launcher_enter_row_actions(&mut self, i: usize, input: &str, browse_all: bool) -> Task<Msg> {
        let rows = self.palette_rows(input, browse_all);
        let Some(row) = rows.get(i) else {
            return Task::none();
        };
        match row {
            PaletteRow::Recent {
                proj, wt_path, agent, ..
            }
            | PaletteRow::Combo {
                proj, wt_path, agent, ..
            } => {
                let ra = crate::app::RowActionsState {
                    proj: *proj,
                    wt_path: wt_path.clone(),
                    agent: *agent,
                    action: 0,
                };
                if let Modal::SessionLauncher { row_actions, .. } = &mut self.app.modal {
                    *row_actions = Some(ra);
                }
            }
            PaletteRow::SwitchToSession => {
                // Tab is inert outside zen too — same gate as Enter.
                if self.switch_to_session_active() {
                    self.launcher_enter_switch();
                }
            }
            PaletteRow::Settings => return self.open_settings_drill_in(),
            PaletteRow::Setting(s) => {
                let s = *s;
                let task = self.activate_setting(s);
                self.reselect_typed_setting(s, input, browse_all, i);
                return task;
            }
            _ => {}
        }
        Task::none()
    }

    /// Enter the "Launch session…" flow for `(proj, wt_path)`: the full OPEN
    /// WITH agent-picker state (`Modal::SessionLauncher::options`). Used by
    /// the row-actions strip's "Launch session…" action (action `0`); `origin`
    /// is that strip's state, restored on Esc from options.
    fn launcher_open_options_for(&mut self, proj: usize, wt_path: String, origin: RowActionsState) {
        let Some(wt) = self
            .launcher_worktrees(proj)
            .iter()
            .position(|w| w.path == wt_path)
        else {
            return;
        };
        let pname = self
            .app
            .store
            .projects
            .get(proj)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let agent = self
            .app
            .store
            .recent_launches
            .iter()
            .find(|r| r.project == pname && r.wt_path == wt_path)
            .map(|r| r.agent)
            .unwrap_or_else(|| {
                self.app
                    .available_agents
                    .first()
                    .copied()
                    .unwrap_or(Agent::Terminal)
            });
        let agent_idx = self
            .app
            .available_agents
            .iter()
            .position(|a| *a == agent)
            .unwrap_or(0);
        if let Modal::SessionLauncher {
            options,
            row_actions,
            ..
        } = &mut self.app.modal
        {
            *options = Some(LauncherOptions {
                proj,
                wt,
                agent: agent_idx,
                origin,
            });
            *row_actions = None;
        }
    }

    /// Build the current root or typing/browse-all row list for the command
    /// palette. Root (`input` empty, `!browse_all`): up to 6 valid recents
    /// (skipping any whose project name or worktree path no longer resolves)
    /// plus action rows. Zero projects: only `AddProject` + `TerminalHome`.
    /// Typing/browse-all: every `(proj, wt)` combo across every project,
    /// fuzzy-filtered by `input`.
    pub(super) fn palette_rows(&self, input: &str, browse_all: bool) -> Vec<PaletteRow> {
        if self.app.store.projects.is_empty() {
            return vec![PaletteRow::AddProject, PaletteRow::TerminalHome];
        }
        if input.is_empty() && !browse_all {
            let mut rows = Vec::new();
            for r in self.app.store.recent_launches.iter().take(6) {
                let Some(proj) = self
                    .app
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == r.project)
                else {
                    continue;
                };
                if !self
                    .launcher_worktrees(proj)
                    .iter()
                    .any(|w| w.path == r.wt_path)
                {
                    continue;
                }
                rows.push(PaletteRow::Recent {
                    proj,
                    wt_path: r.wt_path.clone(),
                    agent: r.agent,
                });
            }
            // No valid recents (fresh install, or every recent's project/
            // worktree since disappeared) but at least one project exists:
            // fall back to listing worktrees directly, active project first,
            // so root state is never just the bare action rows. Capped at 6
            // total, same as the recents list it replaces. `view.rs` renders
            // a "{PROJECT} — WORKTREES" header per project run (detected by
            // the same "root_mode but rows contain a Combo" signal used
            // here) and a persistent "starts a session" row hint in place of
            // the recents list's per-row digit accelerator.
            if rows.is_empty() && !self.app.available_agents.is_empty() {
                let proj_order =
                    root_project_order(self.app.store.projects.len(), self.app.proj_idx);
                let mut count = 0;
                'projects: for proj in proj_order {
                    let Some(p) = self.app.store.projects.get(proj) else {
                        continue;
                    };
                    for w in self.launcher_worktrees(proj) {
                        if count >= 6 {
                            break 'projects;
                        }
                        let agent = self
                            .app
                            .store
                            .recent_launches
                            .iter()
                            .find(|r| r.project == p.name && r.wt_path == w.path)
                            .map(|r| r.agent)
                            .unwrap_or(self.app.available_agents[0]);
                        rows.push(PaletteRow::Combo {
                            proj,
                            wt_path: w.path.clone(),
                            agent,
                        });
                        count += 1;
                    }
                }
            }
            rows.push(PaletteRow::NewSession);
            rows.push(PaletteRow::TerminalHome);
            if self.app.active_session.is_some() && !self.terminal_focused {
                rows.push(PaletteRow::TerminalWt);
            }
            rows.push(PaletteRow::AddProject);
            if self.switch_to_session_row_visible() {
                rows.push(PaletteRow::SwitchToSession);
            }
            rows.push(PaletteRow::Settings);
            rows
        } else {
            if self.app.available_agents.is_empty() {
                return Vec::new();
            }
            let mut rows = Vec::new();
            // Direct settings matches (name/value/section) go first, so the
            // typing-state SETTINGS section (see `view.rs`'s header logic)
            // prints above SESSIONS — B2 in the palette redesign mock.
            if !browse_all {
                for s in self.settings_rows_filtered(input) {
                    rows.push(PaletteRow::Setting(s));
                }
            }
            // Score every (project, worktree) combo, keep only the ones that
            // match at all, then rank by score desc with a recency tiebreak:
            // a combo whose (project, wt_path) sits earlier in
            // `recent_launches` wins a tie (`sort_by` is stable, so combos
            // absent from recents — tied at `usize::MAX` — keep their
            // relative store order).
            let mut scored: Vec<(u32, usize, PaletteRow)> = Vec::new();
            for (proj, p) in self.app.store.projects.iter().enumerate() {
                for w in self.launcher_worktrees(proj) {
                    let name = if w.branch.is_empty() {
                        crate::app::path_basename(&w.path)
                    } else {
                        w.branch.clone()
                    };
                    let agent = self
                        .app
                        .store
                        .recent_launches
                        .iter()
                        .find(|r| r.project == p.name && r.wt_path == w.path)
                        .map(|r| r.agent)
                        .unwrap_or(self.app.available_agents[0]);
                    let Some(score) =
                        crate::gui::launcher::fuzzy_score(input, &p.name, &name, agent.label())
                    else {
                        continue;
                    };
                    let recency = self
                        .app
                        .store
                        .recent_launches
                        .iter()
                        .position(|r| r.project == p.name && r.wt_path == w.path)
                        .unwrap_or(usize::MAX);
                    scored.push((
                        score,
                        recency,
                        PaletteRow::Combo {
                            proj,
                            wt_path: w.path.clone(),
                            agent,
                        },
                    ));
                }
            }
            rows.extend(rank_and_group_combos(scored));
            // Typing (not the "+ new session…" browse-all list): the ACTIONS
            // rows stay reachable by keyword, e.g. "swi" -> "Switch to
            // session…" (D5 in the palette redesign mock).
            if !browse_all {
                if crate::gui::launcher::fuzzy_match(input, "add project", "", "") {
                    rows.push(PaletteRow::AddProject);
                }
                if self.switch_to_session_row_visible()
                    && crate::gui::launcher::fuzzy_match(input, "switch to session", "", "")
                {
                    rows.push(PaletteRow::SwitchToSession);
                }
                if crate::gui::launcher::fuzzy_match(input, "settings", "", "") {
                    rows.push(PaletteRow::Settings);
                }
                if crate::gui::launcher::fuzzy_match(input, "reload themes", "", "") {
                    rows.push(PaletteRow::ReloadThemes);
                }
            }
            rows
        }
    }

    /// Whether `PaletteRow::SwitchToSession` should be *rendered* at all
    /// (root and typed lists alike): just needs a session to switch to.
    /// Outside zen the row still shows, but inert — see
    /// `switch_to_session_active`.
    pub(super) fn switch_to_session_row_visible(&self) -> bool {
        // At least one session you could actually switch *to* — the active
        // one is hidden from the drill-in list, so it doesn't count.
        (0..self.app.sessions.len()).any(|i| self.app.active_session != Some(i))
    }

    /// Whether `PaletteRow::SwitchToSession` is *actionable*: also requires
    /// zen (`!chrome_visible` — the flag `Msg::ToggleZen`/mod+enter flips).
    /// Outside zen the workspace/grid already shows every session, so
    /// opening the drill-in would be redundant there; the row still renders
    /// (muted, "in zen · ⌘⏎") but Enter/Tab on it are no-ops.
    pub(super) fn switch_to_session_active(&self) -> bool {
        self.switch_to_session_row_visible() && !self.app.chrome_visible
    }

    /// Active sessions across every project/worktree, for the "switch to
    /// session" drill-in list: every index into `App::sessions` (in their
    /// existing display order) fuzzy-filtered by `input` against the
    /// session's project/worktree/agent — same matching used by the
    /// typing-state Combo list. Empty `input` matches everything.
    pub(super) fn switch_session_rows(&self, input: &str) -> Vec<usize> {
        (0..self.app.sessions.len())
            // Switching to the session already on screen is a no-op; hide it.
            .filter(|&i| self.app.active_session != Some(i))
            .filter(|&i| {
                let s = &self.app.sessions[i];
                let wt_name = crate::app::path_basename(&s.wt_path);
                crate::gui::launcher::fuzzy_match(input, &s.project, &wt_name, s.agent.label())
            })
            .collect()
    }

    /// Switch focus to `App::sessions[si]` and close the palette. Shared by
    /// the drill-in's Enter key and row-click paths.
    fn launcher_switch_to(&mut self, si: usize) -> Task<Msg> {
        self.app.modal = Modal::None;
        self.update(Msg::SelectSession(si))
    }

    /// Move the "switch to session" drill-in's cursor to position `idx`
    /// within `switch_session_rows(&input)` and capture that row's session
    /// `id` in the same step — same principle as `set_palette_selected`,
    /// applied to this drill-in's own list. Reads `input` off the *current*
    /// modal state, so callers must clear/update it first.
    fn set_switch_selected(&mut self, idx: usize) {
        let input = match &self.app.modal {
            Modal::SessionLauncher { input, .. } => input.clone(),
            _ => return,
        };
        let rows = self.switch_session_rows(&input);
        let identity = rows
            .get(idx)
            .and_then(|&si| self.app.sessions.get(si))
            .map(|s| s.id);
        if let Modal::SessionLauncher {
            switch,
            switch_identity,
            ..
        } = &mut self.app.modal
        {
            *switch = Some(idx);
            *switch_identity = identity;
        }
    }

    /// The position (within a freshly filtered `switch_session_rows(input)`)
    /// of the session at `switch_identity` — used to re-anchor the drill-in
    /// cursor after `input` itself changes (a query edit can reorder/drop
    /// rows in the filtered list, so clamping the raw position, the old
    /// behavior, could land on a different session than the one
    /// highlighted). Defaults to the top of the list when there's no
    /// identity yet, or that session no longer matches the new query.
    fn resolve_switch_position(&self, input: &str) -> usize {
        let identity = match &self.app.modal {
            Modal::SessionLauncher {
                switch_identity, ..
            } => *switch_identity,
            _ => None,
        };
        let rows = self.switch_session_rows(input);
        match identity {
            Some(id) => rows
                .iter()
                .position(|&si| self.app.sessions.get(si).map(|s| s.id) == Some(id))
                .unwrap_or(0),
            None => 0,
        }
    }

    /// Resolve the switch drill-in's Enter target: the `App::sessions` index
    /// whose `id` matches `switch_identity`, re-derived against a fresh
    /// `switch_session_rows` rather than trusting the drill-in's raw cursor
    /// position — the same staleness hazard `resolve_selected` closes for
    /// the main list (a session closing/reordering between two keystrokes
    /// can't make Enter switch to a different session than the one
    /// highlighted). `Session::id` is the identity here rather than
    /// `PaletteRowIdentity`, since a switch-drill-in row already *is* a
    /// session, with its own stable, never-reused id (see
    /// `crate::gui::launcher::session_grid_key` for the analogous
    /// project+path key used elsewhere — an id is simpler and available
    /// here).
    fn resolve_switch_selected(&self, input: &str) -> Option<usize> {
        let (sel, identity) = match &self.app.modal {
            Modal::SessionLauncher {
                switch: Some(sel),
                switch_identity,
                ..
            } => (*sel, *switch_identity),
            _ => return None,
        };
        let rows = self.switch_session_rows(input);
        match identity {
            Some(id) => rows
                .into_iter()
                .find(|&si| self.app.sessions.get(si).map(|s| s.id) == Some(id)),
            None => rows.get(sel).copied(),
        }
    }

    /// Run the row-actions strip action `action` for the `(proj, wt_path)`
    /// identity it's pinned to: `0` enters the existing OPEN WITH agent-picker
    /// ("Launch session…"); `1` closes the palette and routes through the
    /// sidebar's own delete-worktree confirmation flow (`Msg::DeleteWorktree`,
    /// the same message the sidebar trash icon sends — reusing it means the
    /// existing teardown-script/confirm modal applies here too, rather than
    /// duplicating that flow in the palette). Resolving straight from the
    /// identity (rather than re-deriving `palette_rows()` and indexing into
    /// it) means a query/list change since the strip was opened can't make
    /// this act on the wrong row.
    /// The project's configured lifecycle scripts, in fixed order
    /// setup/run/teardown, skipping any that are unset or blank. Indices
    /// into this vec are the row-actions strip's script actions, offset by
    /// the strip's base action count — used by the view, the keyboard-nav
    /// clamp, and the run handler so all three stay in sync.
    pub(crate) fn row_action_scripts(&self, proj: usize) -> Vec<(&'static str, String)> {
        let Some(p) = self.app.store.projects.get(proj) else {
            return Vec::new();
        };
        [
            ("setup", &p.scripts.setup),
            ("run", &p.scripts.run),
            ("teardown", &p.scripts.teardown),
        ]
        .into_iter()
        .filter_map(|(kind, script)| {
            let s = script.as_deref()?.trim();
            if s.is_empty() {
                None
            } else {
                Some((kind, s.to_string()))
            }
        })
        .collect()
    }

    fn launcher_run_row_action(
        &mut self,
        proj: usize,
        wt_path: String,
        agent: crate::agent::Agent,
        action: usize,
    ) -> Task<Msg> {
        if action == 0 {
            let origin = RowActionsState {
                proj,
                wt_path: wt_path.clone(),
                agent,
                action: 0,
            };
            self.launcher_open_options_for(proj, wt_path, origin);
            return Task::none();
        }
        if action == 2 && self.app.project_themes_enabled() {
            return self.enter_project_theme_pane(proj);
        }
        let base = if self.app.project_themes_enabled() {
            3
        } else {
            2
        };
        if action >= base {
            let scripts = self.row_action_scripts(proj);
            let Some((kind, script)) = scripts.get(action - base) else {
                return Task::none();
            };
            let Some(pname) = self.app.store.projects.get(proj).map(|p| p.name.clone()) else {
                return Task::none();
            };
            self.app.modal = Modal::None;
            self.app.spawn_script_session(kind, pname, wt_path, script);
            return Task::none();
        }
        let worktrees = self.launcher_worktrees(proj);
        let Some(w) = worktrees.iter().find(|w| w.path == wt_path) else {
            return Task::none();
        };
        // The strip's second action is worktree-dependent: the project's
        // default/base checkout can't be removed (`App::start_delete` bounces
        // it to a "can't remove the project's main checkout" message), so its
        // strip offers "Create worktree…" there instead of "Delete worktree".
        if w.is_main {
            self.app.modal = Modal::None;
            return self.update(Msg::AddWorktree { proj });
        }
        let Some(wt) = worktrees.iter().position(|w| w.path == wt_path) else {
            return Task::none();
        };
        self.app.modal = Modal::None;
        self.update(Msg::DeleteWorktree { proj, wt })
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
            pname.clone(),
            w.path.clone(),
            agent,
            &args,
            &w.path,
            use_tmux,
        ) {
            Ok(mut s) => {
                s.resize(self.pty_rows, self.pty_sess_cols);
                self.app.sessions.push(s);
                crate::gui::launcher::push_recent_launch(
                    &mut self.app.store.recent_launches,
                    crate::storage::RecentLaunch {
                        project: pname,
                        wt_path: w.path.clone(),
                        agent,
                    },
                );
                let _ = crate::storage::save(&self.app.store);
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
    /// Whether the workspace is currently focused on a home terminal rather
    /// than the active agent session.
    pub(super) fn terminal_tab(&self) -> bool {
        self.terminal_focused
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

/// One row of the command palette's list, in display order.
#[derive(Clone)]
pub(super) enum PaletteRow {
    Recent {
        proj: usize,
        wt_path: String,
        agent: Agent,
    },
    Combo {
        proj: usize,
        wt_path: String,
        agent: Agent,
    },
    NewSession,
    TerminalHome,
    TerminalWt,
    AddProject,
    /// ACTIONS row: Enter or Tab opens the "switch to session" drill-in
    /// (`Modal::SessionLauncher::switch`).
    SwitchToSession,
    /// ACTIONS row: Enter or Tab opens the Settings drill-in
    /// (`Modal::SessionLauncher::settings`).
    Settings,
    /// A direct settings match surfaced while typing at root (not
    /// `browse_all`) — name, current value, and section all searchable. See
    /// `Grove::activate_setting`: toggles flip in place here without opening
    /// the drill-in; enum rows are phase-1 no-ops.
    Setting(SettingRow),
    /// ACTIONS row, keyword-only (never shown at bare root — see
    /// `palette_rows`): re-reads `themes.json` via `theme::load_custom` and
    /// surfaces any skipped-entry errors (mock E2). Stateless, so unlike
    /// `Setting` there's no value to display or toggle.
    ReloadThemes,
}

/// One settings entry surfaced by the palette, either as a root-mode direct
/// match (`PaletteRow::Setting`) or as a row in the Settings drill-in.
/// Ordering of the variants is the drill-in's display order within its
/// section (see `SettingRow::ALL`, `section`) — enum panes for the "enum"
/// rows (`Theme`/`Backend`/`Permissions`/`DefaultAgent`/`AppSize`) are a
/// later phase; here they're inert stubs (see `Grove::activate_setting`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum SettingRow {
    Theme,
    AppSize,
    ProjectThemes,
    Backend,
    Permissions,
    Telemetry,
    DefaultAgent,
    CheckUpdates,
}

impl SettingRow {
    /// Every setting, in section/definition (= drill-in display) order.
    pub(super) const ALL: [SettingRow; 8] = [
        SettingRow::Theme,
        SettingRow::AppSize,
        SettingRow::ProjectThemes,
        SettingRow::Backend,
        SettingRow::Permissions,
        SettingRow::Telemetry,
        SettingRow::DefaultAgent,
        SettingRow::CheckUpdates,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            SettingRow::Theme => "App theme",
            SettingRow::AppSize => "App size",
            SettingRow::ProjectThemes => "Project themes",
            SettingRow::Backend => "Backend",
            SettingRow::Permissions => "Permissions",
            SettingRow::Telemetry => "Telemetry",
            SettingRow::DefaultAgent => "Default agent",
            SettingRow::CheckUpdates => "Check for updates",
        }
    }

    /// Name of the inline SVG sprite (see `gui::icons`) shown in this row's
    /// leading 24px icon slot. `ProjectThemes`/`Telemetry` render a checkbox
    /// glyph instead (see `palette_row_view`'s `Setting` arm) and never
    /// consult this. Several picks are the nearest existing sprite standing
    /// in for one the redesign mock uses that isn't in `icons.rs` — see the
    /// per-arm comments below.
    pub(super) fn icon_name(self) -> &'static str {
        match self {
            // Mock uses a dedicated palette glyph; `contrast` (the existing
            // light/dark toggle icon) is the closest stand-in for "theme".
            SettingRow::Theme => "contrast",
            // Mock's own choice — already in `icons.rs`.
            SettingRow::AppSize => "grid",
            SettingRow::ProjectThemes => "check",
            // Mock uses a monitor glyph; `term` (terminal) is the closest
            // existing sprite for "backend".
            SettingRow::Backend => "term",
            // Mock uses a shield glyph; `ring` (a plain protective circle)
            // is the closest existing stand-in.
            SettingRow::Permissions => "ring",
            SettingRow::Telemetry => "check",
            // Mock uses a bot glyph; `sparkle` is the closest existing
            // "agent/AI" stand-in.
            SettingRow::DefaultAgent => "sparkle",
            // Matches the existing refresh icon used for the same action in
            // `settings_modal` (view.rs).
            SettingRow::CheckUpdates => "restart",
        }
    }

    pub(super) fn section(self) -> &'static str {
        match self {
            SettingRow::Theme | SettingRow::AppSize | SettingRow::ProjectThemes => "APPEARANCE",
            SettingRow::Backend | SettingRow::Permissions | SettingRow::Telemetry => {
                "AGENTS / TERMINAL"
            }
            SettingRow::DefaultAgent => "TOOLS",
            SettingRow::CheckUpdates => "UPDATES",
        }
    }
}

/// Pure selection-index math for entering each Settings sub-pane (see the
/// `Grove::enter_*_pane` methods) — kept free of `Grove`/`Modal` so it's
/// directly unit-testable without building a GUI.
pub(super) fn backend_pane_selected_index(tmux_on: bool) -> usize {
    if tmux_on {
        1
    } else {
        0
    }
}

pub(super) fn permissions_pane_selected_index(skip_on: bool) -> usize {
    if skip_on {
        1
    } else {
        0
    }
}

pub(super) fn default_agent_pane_selected_index(default: Option<Agent>) -> usize {
    default
        .and_then(|a| Agent::ALL.iter().position(|&x| x == a))
        .unwrap_or(0)
}

/// Selects the row (builtin or custom) matching `name` within `kind`'s
/// *unfiltered* combined list — builtins first, then customs — falling back
/// to row 0. Builtins and customs never share a name (custom names that
/// collide with a builtin are rejected at creation/load), so this never
/// double-matches.
pub(super) fn theme_pane_selected_index(kind: crate::theme::ThemeKind, name: &str) -> usize {
    let builtins = crate::theme::themes_of(kind);
    if let Some(i) = builtins.iter().position(|t| t.name == name) {
        return i;
    }
    crate::theme::custom_themes_of(kind)
        .iter()
        .position(|t| t.name == name)
        .map(|i| builtins.len() + i)
        .unwrap_or(0)
}

/// Builtin themes of `kind` fuzzy-filtered by `input`, in `theme::themes_of`'s
/// alphabetical order — the Theme sub-pane's Built-in section (`view.rs`) and
/// its keyboard/mouse selection (`Grove::theme_pane_select`/`theme_pane_move`)
/// share this so they never disagree on what row N is.
pub(super) fn theme_pane_rows(
    kind: crate::theme::ThemeKind,
    input: &str,
) -> Vec<crate::theme::Theme> {
    crate::theme::themes_of(kind)
        .into_iter()
        .filter(|t| crate::gui::launcher::fuzzy_match(input, &t.name, "", ""))
        .collect()
}

/// Custom themes of `kind` fuzzy-filtered by `input`, same alphabetical/
/// fuzzy-match contract as `theme_pane_rows` — the Theme sub-pane's Custom
/// section, rendered *after* (never interleaved with) `theme_pane_rows`.
pub(super) fn theme_pane_custom_rows(
    kind: crate::theme::ThemeKind,
    input: &str,
) -> Vec<crate::theme::Theme> {
    crate::theme::custom_themes_of(kind)
        .into_iter()
        .filter(|t| crate::gui::launcher::fuzzy_match(input, &t.name, "", ""))
        .collect()
}

/// The Theme sub-pane's full selectable list: every builtin row followed by
/// every custom row (never interleaved) — selection/keyboard nav flows
/// continuously across the boundary, so callers that only need "the theme
/// at index N" or "how many rows total" use this rather than combining
/// `theme_pane_rows`/`theme_pane_custom_rows` themselves.
pub(super) fn theme_pane_combined_rows(
    kind: crate::theme::ThemeKind,
    input: &str,
) -> Vec<crate::theme::Theme> {
    let mut rows = theme_pane_rows(kind, input);
    rows.extend(theme_pane_custom_rows(kind, input));
    rows
}

/// Whether row `idx` of the Theme sub-pane's combined list (see
/// `theme_pane_combined_rows`) falls in the Custom section — the only rows
/// where rename/delete/edit are offered.
/// `Grove::reload_themes`'s "does the active theme still resolve?" decision,
/// pulled out as a pure function so it's testable without a full `Grove`:
/// `None` when `active_name` still resolves to something (builtin or
/// custom) — nothing to do; `Some(fallback)` — the mode default for `kind`
/// — when it doesn't, so the caller can silently reassign it (mock E3: this
/// is routine config drift, not an error).
pub(super) fn theme_reload_fallback(
    active_name: &str,
    kind: crate::theme::ThemeKind,
) -> Option<&'static str> {
    if crate::theme::by_name(active_name).is_some() {
        return None;
    }
    Some(match kind {
        crate::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
        crate::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
    })
}

pub(super) fn theme_pane_row_is_custom(kind: crate::theme::ThemeKind, input: &str, idx: usize) -> bool {
    // Resolved by name via `theme::is_custom` rather than an
    // `idx >= builtins.len()` position check — same answer today (customs
    // always sort after builtins in `theme_pane_combined_rows`), but doesn't
    // depend on that ordering staying that way.
    theme_pane_combined_rows(kind, input)
        .get(idx)
        .map(|t| crate::theme::is_custom(&t.name))
        .unwrap_or(false)
}

/// The ProjectTheme sub-pane's list: same `theme_pane_rows` filtering,
/// fronted by a "Use app theme" row (`None`) — only while the query is empty,
/// so a fuzzy search doesn't dangle a static row above an unrelated match
/// list. Row N here is exactly what `Grove::project_theme_pane_select`/
/// `_move` index into, mirroring `theme_pane_rows`'s contract.
pub(super) fn project_theme_pane_rows(
    kind: crate::theme::ThemeKind,
    input: &str,
) -> Vec<Option<crate::theme::Theme>> {
    let mut rows: Vec<Option<crate::theme::Theme>> = Vec::new();
    if input.trim().is_empty() {
        rows.push(None);
    }
    rows.extend(theme_pane_rows(kind, input).into_iter().map(Some));
    rows
}

/// The Theme sub-pane's list geometry: 36px rows (the sub-pane row height,
/// vs the standalone picker's `ROW_H`) under a 280px viewport cap — must
/// match the pane's `max_height` in `view.rs` or the centering drifts. The
/// full 280 is viewport: that container carries no padding of its own (the
/// pane's 8px padding sits on the outer wrapper around context/mode/list).
/// Both the Theme and ProjectTheme panes render their list as
/// `Column::new().spacing(2)` (view.rs ~3785/3806 and ~3919/3960) — the true
/// row pitch is 38px (36 + 2px column spacing), not the bare row height, and
/// the last row's gap doesn't exist (content is `total*38 − 2`, not
/// `total*38`).
const THEME_PANE_ROW_H: f32 = 36.0;
const THEME_PANE_SPACING: f32 = 2.0;
const THEME_PANE_VIEWPORT_CAP: f32 = 280.0;

/// Center-and-clamp scroll offset for the Theme sub-pane's list: the y that
/// centers row `selected` of `total` in the capped viewport, clamped to the
/// scrollable's valid range (0 when everything already fits). Same math as
/// `scroll_theme_picker_to_selection`, kept pure for testing — but pitched at
/// `THEME_PANE_ROW_H + THEME_PANE_SPACING` per row (see the geometry note
/// above), not the bare row height.
pub(super) fn launcher_theme_scroll_offset(total: usize, selected: usize) -> f32 {
    let pitch = THEME_PANE_ROW_H + THEME_PANE_SPACING;
    let content_h = if total == 0 {
        0.0
    } else {
        total as f32 * pitch - THEME_PANE_SPACING
    };
    let sel_y = selected as f32 * pitch;
    let viewport_h = content_h.min(THEME_PANE_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - THEME_PANE_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The theme editor's group-header geometry: same `SETTINGS_ROOT_HEADER_
/// LABEL_H` (10px text at iced's default 1.3 line height) the Settings Root
/// list uses, but the editor's `section_header(group, top, 4.0)` calls (see
/// `view.rs`) always pass `top = 10.0` for every header after the first
/// (never `12.0` like Root's), and a `4.0` bottom margin (Root uses `6.0`).
const THEME_EDITOR_GROUP_HEADER_TOP: f32 = 10.0;
const THEME_EDITOR_GROUP_HEADER_BOTTOM: f32 = 4.0;
/// The trailing "derived — not editable" chip row's height: its 12px
/// swatches sit shorter than the row's 10px label text at iced's default
/// 1.3 line height (`SETTINGS_ROOT_HEADER_LABEL_H`, same constant), plus the
/// row container's own `padding(Padding::from([4, 12]))` (4px top + bottom).
const THEME_EDITOR_DERIVED_ROW_H: f32 = SETTINGS_ROOT_HEADER_LABEL_H + 8.0;

/// Center-and-clamp scroll offset for the theme editor's row list: the
/// Theme sub-pane already keeps keyboard selection in view via
/// `launcher_theme_scroll_offset`, but that helper assumes every row is a
/// uniform `THEME_PANE_ROW_H` pitch — the editor's list isn't, since its 11
/// color rows are grouped under Surfaces/Text/Accents section headers
/// (`theme::FIELD_GROUPS`) and followed by a "derived — not editable"
/// header + chip row. Walks the same header/row sequence `view.rs` renders
/// (`settings_root_scroll_offset`'s idiom) so the selected row's y always
/// matches what's actually on screen. `selected` is always one of the 11
/// color rows — the derived strip is never selectable — but it still
/// contributes to `content_h`/`max_y` since it occupies real scroll height.
pub(super) fn theme_editor_scroll_offset(selected: usize) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;
    let mut prev_group: Option<&'static str> = None;
    for (i, &group) in crate::theme::FIELD_GROUPS.iter().enumerate() {
        if prev_group != Some(group) {
            let top = if prev_group.is_none() {
                0.0
            } else {
                THEME_EDITOR_GROUP_HEADER_TOP
            };
            if content_h > 0.0 {
                content_h += THEME_PANE_SPACING;
            }
            content_h +=
                top + SETTINGS_ROOT_HEADER_LABEL_H + THEME_EDITOR_GROUP_HEADER_BOTTOM;
            prev_group = Some(group);
        }
        if content_h > 0.0 {
            content_h += THEME_PANE_SPACING;
        }
        if i == selected {
            sel_y = content_h;
        }
        content_h += THEME_PANE_ROW_H;
    }
    // The trailing "DERIVED — NOT EDITABLE" header + its chip row: not
    // selectable, but still real scrollable content that bounds `max_y`.
    content_h += THEME_PANE_SPACING;
    content_h += THEME_EDITOR_GROUP_HEADER_TOP
        + SETTINGS_ROOT_HEADER_LABEL_H
        + THEME_EDITOR_GROUP_HEADER_BOTTOM;
    content_h += THEME_PANE_SPACING;
    content_h += THEME_EDITOR_DERIVED_ROW_H;

    let viewport_h = content_h.min(THEME_PANE_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - THEME_PANE_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The Settings drill-in Root list's geometry, mirroring its `view.rs`
/// render exactly: 44px palette rows and section headers in a 2px-spaced
/// column. The header total is its label — 10px text at iced's default 1.3
/// relative line height = 13px — plus the render loop's margins (top 0 for
/// the first header, 12 for later ones; bottom 6).
const SETTINGS_ROOT_ROW_H: f32 = 44.0;
const SETTINGS_ROOT_SPACING: f32 = 2.0;
const SETTINGS_ROOT_HEADER_LABEL_H: f32 = 13.0;
/// The scrollable's true viewport: the list container caps at
/// `max_height(380.0)` but carries `padding(8)` on that same container
/// (unlike the Theme pane's), and `max_height` bounds padding included —
/// 380 − 2·8. Clamping against the raw 380 under-scrolls by exactly that
/// 16px, clipping the bottom row.
const SETTINGS_ROOT_VIEWPORT_CAP: f32 = 380.0 - 16.0;

/// Center-and-clamp scroll offset for the Settings drill-in's Root list —
/// `launcher_theme_scroll_offset`'s idiom, but this list isn't uniform
/// height: a section header precedes every row whose section differs from
/// the previous row's, so the selected row's y comes from walking the
/// rendered element sequence rather than multiplying an index.
pub(super) fn settings_root_scroll_offset(rows: &[SettingRow], selected: usize) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;
    let mut prev_section: Option<&'static str> = None;
    for (i, row) in rows.iter().enumerate() {
        let section = row.section();
        if prev_section != Some(section) {
            let top = if prev_section.is_none() { 0.0 } else { 12.0 };
            if content_h > 0.0 {
                content_h += SETTINGS_ROOT_SPACING;
            }
            content_h += top + SETTINGS_ROOT_HEADER_LABEL_H + 6.0;
            prev_section = Some(section);
        }
        if content_h > 0.0 {
            content_h += SETTINGS_ROOT_SPACING;
        }
        if i == selected {
            sel_y = content_h;
        }
        content_h += SETTINGS_ROOT_ROW_H;
    }
    let viewport_h = content_h.min(SETTINGS_ROOT_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - SETTINGS_ROOT_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The root/typed palette list's geometry, mirroring its `view.rs` render
/// exactly — same shape as `SETTINGS_ROOT_*` above, just under the palette's
/// own section-header vocabulary (RECENT/WORKTREES/ACTIONS at root,
/// SETTINGS/SESSIONS/ACTIONS plus per-project sub-headers when typed/
/// browse-all list is long enough to group).
const PALETTE_LIST_ROW_H: f32 = 44.0;
const PALETTE_LIST_SPACING: f32 = 2.0;
const PALETTE_LIST_HEADER_LABEL_H: f32 = 13.0;
/// Same reasoning as `SETTINGS_ROOT_VIEWPORT_CAP`: the list container caps
/// at `max_height(380.0)` but that bound includes its own `padding(8)`.
const PALETTE_LIST_VIEWPORT_CAP: f32 = 380.0 - 16.0;

/// Center-and-clamp scroll offset for the palette's root/typed list —
/// `settings_root_scroll_offset`'s idiom, walking the same header/row
/// sequence `view.rs`'s `else` branch (view.rs:4462-4544) renders, so the
/// selected row's y always matches what's actually on screen.
pub(super) fn palette_scroll_offset(rows: &[PaletteRow], selected: usize, root_mode: bool) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;

    let push_header = |content_h: &mut f32, top: f32, bottom: f32| {
        if *content_h > 0.0 {
            *content_h += PALETTE_LIST_SPACING;
        }
        *content_h += top + PALETTE_LIST_HEADER_LABEL_H + bottom;
    };

    if root_mode {
        let mut printed_recent = false;
        let mut last_wt_project: Option<usize> = None;
        let mut printed_actions = false;
        for (i, row) in rows.iter().enumerate() {
            match row {
                PaletteRow::Recent { .. } => {
                    if !printed_recent {
                        push_header(&mut content_h, 0.0, 6.0);
                        printed_recent = true;
                    }
                }
                PaletteRow::Combo { proj, .. } => {
                    if last_wt_project != Some(*proj) {
                        let top = if i == 0 { 0.0 } else { 12.0 };
                        push_header(&mut content_h, top, 6.0);
                        last_wt_project = Some(*proj);
                    }
                }
                _ => {
                    if !printed_actions {
                        let top = if printed_recent || last_wt_project.is_some() {
                            12.0
                        } else {
                            0.0
                        };
                        push_header(&mut content_h, top, 6.0);
                        printed_actions = true;
                    }
                }
            }
            if content_h > 0.0 {
                content_h += PALETTE_LIST_SPACING;
            }
            if i == selected {
                sel_y = content_h;
            }
            content_h += PALETTE_LIST_ROW_H;
        }
    } else {
        let has_settings = rows.iter().any(|r| matches!(r, PaletteRow::Setting(_)));
        let mut printed_settings = false;
        let mut printed_sessions = false;
        let mut printed_actions = false;
        let session_project_order: Vec<usize> = {
            let mut seen = Vec::new();
            for r in rows.iter() {
                if let PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } = r {
                    if !seen.contains(proj) {
                        seen.push(*proj);
                    }
                }
            }
            seen
        };
        let session_row_count = rows
            .iter()
            .filter(|r| matches!(r, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }))
            .count();
        let grouped_by_project = session_project_order.len() > 2 || session_row_count > 10;
        let mut last_grouped_project: Option<usize> = None;

        for (i, row) in rows.iter().enumerate() {
            let is_setting = matches!(row, PaletteRow::Setting(_));
            let is_session = matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. });
            if has_settings && is_setting && !printed_settings {
                push_header(&mut content_h, 0.0, 6.0);
                printed_settings = true;
            } else if is_session && !printed_sessions {
                let top = if printed_settings { 12.0 } else { 0.0 };
                push_header(&mut content_h, top, 6.0);
                printed_sessions = true;
            } else if !is_setting && !is_session && !printed_actions {
                let top = if printed_sessions || printed_settings {
                    12.0
                } else {
                    0.0
                };
                push_header(&mut content_h, top, 6.0);
                printed_actions = true;
            }
            if grouped_by_project && is_session {
                let proj = match row {
                    PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } => *proj,
                    _ => unreachable!(),
                };
                if last_grouped_project != Some(proj) {
                    let top = if last_grouped_project.is_none() {
                        0.0
                    } else {
                        8.0
                    };
                    push_header(&mut content_h, top, 4.0);
                    last_grouped_project = Some(proj);
                }
            }
            if content_h > 0.0 {
                content_h += PALETTE_LIST_SPACING;
            }
            if i == selected {
                sel_y = content_h;
            }
            content_h += PALETTE_LIST_ROW_H;
        }
    }

    let viewport_h = content_h.min(PALETTE_LIST_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - PALETTE_LIST_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// Keep-identity-else-clamp reselection for the drill-in Root list after a
/// toggle refilters it: the cursor follows `activated` to its new position
/// when the row survived, and otherwise clamps the old index into the new
/// length (`launcher::clamp` handles the empty list).
pub(super) fn reselect_setting(rows: &[SettingRow], activated: SettingRow, old: usize) -> usize {
    rows.iter()
        .position(|s| *s == activated)
        .unwrap_or_else(|| crate::gui::launcher::clamp(old, 0, rows.len()))
}

/// Resolve mod+digit `n` (1-based) to the list index of the nth session
/// (`Recent`/`Combo`) row, skipping settings and action rows — in typed
/// mode those sort above sessions (B2), and the digits must keep meaning
/// "nth session", not "nth row". `None` when fewer than `n` session rows
/// exist. Root mode is unchanged by construction: recents come first there.
pub(super) fn nth_session_row(rows: &[PaletteRow], n: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }))
        .nth(n.checked_sub(1)?)
        .map(|(i, _)| i)
}

/// A row's [`PaletteRowIdentity`] — the content-based key `resolve_row_by_
/// identity` matches against, decoupled from the row's transient index.
pub(super) fn row_identity(row: &PaletteRow) -> PaletteRowIdentity {
    match row {
        PaletteRow::Recent {
            proj,
            wt_path,
            agent,
        }
        | PaletteRow::Combo {
            proj,
            wt_path,
            agent,
        } => PaletteRowIdentity::Session {
            proj: *proj,
            wt_path: wt_path.clone(),
            agent: *agent,
        },
        PaletteRow::NewSession => PaletteRowIdentity::NewSession,
        PaletteRow::TerminalHome => PaletteRowIdentity::TerminalHome,
        PaletteRow::TerminalWt => PaletteRowIdentity::TerminalWt,
        PaletteRow::AddProject => PaletteRowIdentity::AddProject,
        PaletteRow::SwitchToSession => PaletteRowIdentity::SwitchToSession,
        PaletteRow::Settings => PaletteRowIdentity::Settings,
        PaletteRow::Setting(s) => PaletteRowIdentity::Setting(*s),
        PaletteRow::ReloadThemes => PaletteRowIdentity::ReloadThemes,
    }
}

/// Resolve an activation target by identity rather than by trusting a
/// possibly-stale index: `identity` is the row that was highlighted the last
/// time `selected` was written (see `Grove::set_palette_selected`); this
/// finds that same row wherever it now sits in a freshly rebuilt `rows`
/// (self-healing past a re-sort/re-group/re-filter that happened since), or
/// reports `None` if it's simply gone — it never falls back to activating
/// whatever row now happens to sit at the stale index. `fallback` only
/// applies when `identity` itself is `None` (defensive: a state that hasn't
/// captured one yet).
pub(super) fn resolve_row_by_identity(
    rows: &[PaletteRow],
    identity: &Option<PaletteRowIdentity>,
    fallback: usize,
) -> Option<usize> {
    match identity {
        Some(id) => rows.iter().position(|r| row_identity(r) == *id),
        None => (fallback < rows.len()).then_some(fallback),
    }
}

/// Rank the typed/browse-all Combo list: score desc, recency asc as a
/// tiebreak (`sort_by` is stable, so combos absent from recents — tied at
/// `usize::MAX` — keep their relative store-build order), then re-cluster
/// into per-project runs once the list is too broad to read as one flat
/// ranking (see `view.rs`'s header logic, which recomputes the same
/// threshold from the returned rows rather than trusting a flag).
pub(super) fn rank_and_group_combos(mut scored: Vec<(u32, usize, PaletteRow)>) -> Vec<PaletteRow> {
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut combos: Vec<PaletteRow> = scored.into_iter().map(|(_, _, r)| r).collect();
    let mut project_order: Vec<usize> = Vec::new();
    for r in &combos {
        if let PaletteRow::Combo { proj, .. } = r {
            if !project_order.contains(proj) {
                project_order.push(*proj);
            }
        }
    }
    if project_order.len() > 2 || combos.len() > 10 {
        let mut grouped = Vec::with_capacity(combos.len());
        for proj in project_order {
            grouped.extend(
                combos
                    .iter()
                    .filter(|r| matches!(r, PaletteRow::Combo { proj: p, .. } if *p == proj))
                    .cloned(),
            );
        }
        combos = grouped;
    }
    combos
}

/// Project visit order for the root state's no-recents worktree fallback:
/// the active project first, then every other project in store order.
/// Clamps `active` into range so a stale index (project removed mid-session)
/// can't panic.
pub(super) fn root_project_order(n: usize, active: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let first = active.min(n - 1);
    let mut order = vec![first];
    order.extend((0..n).filter(|&i| i != first));
    order
}

/// The three states of the Theme sub-pane's mode row, in Tab-cycle order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Tab in the Theme sub-pane cycles the mode row Dark → Light → System →
/// Dark. The current mode is System whenever `follow_system` is set (that's
/// also how the segments render), else the shown list's kind.
pub(super) fn next_theme_mode(kind: crate::theme::ThemeKind, follow_system: bool) -> ThemeMode {
    if follow_system {
        ThemeMode::Dark
    } else {
        match kind {
            crate::theme::ThemeKind::Dark => ThemeMode::Light,
            crate::theme::ThemeKind::Light => ThemeMode::System,
        }
    }
}

/// One action in the update-available strip under the Check-for-updates row
/// (E3). Mirrors the Settings modal's update-available action row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum UpdateAction {
    UpdateNow,
    SkipVersion,
    CopyUrl,
}

impl UpdateAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            UpdateAction::UpdateNow => "Update now",
            UpdateAction::SkipVersion => "Skip version",
            UpdateAction::CopyUrl => "Copy URL",
        }
    }
}

/// The update-available strip's actions, in display order. "Update now" is
/// hidden for `InstallMethod::Unknown` installs (notify-only) — the same
/// guard `settings_modal`'s action row applies — so the strip and the
/// keyboard nav derive from one list and indices can never disagree.
pub(super) fn update_available_actions(method_unknown: bool) -> Vec<UpdateAction> {
    let mut actions = Vec::with_capacity(3);
    if !method_unknown {
        actions.push(UpdateAction::UpdateNow);
    }
    actions.push(UpdateAction::SkipVersion);
    actions.push(UpdateAction::CopyUrl);
    actions
}

/// Whether activating the Check-for-updates row expands the actions strip
/// (a release is already known to be available — re-checking would only
/// throw that answer away) instead of firing a fresh check.
pub(super) fn check_updates_opens_strip(upgrade: &UpgradeState) -> bool {
    matches!(upgrade, UpgradeState::Available(_))
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

/// Whether `mods` carries the grid tile-swap modifier on top of
/// [`global_mods`]. On mac this is Alt *or* Shift: Cmd+Opt+H collides with the
/// OS-level "Hide Others" shortcut, so Shift is accepted as an equivalent
/// swap modifier there (Cmd+Shift+h/j/k/l/arrows), and is what's displayed.
/// Cmd+Alt keeps working too, since some layouts/users already rely on it.
/// On non-mac, `global_mods` already requires Shift as part of its base
/// chord, so only Alt distinguishes swap from move there.
fn grid_swap_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.alt() || mods.shift();
    #[cfg(not(target_os = "macos"))]
    return mods.alt();
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
    /// Spawn a new home terminal and focus it.
    NewHomeTerminal,
    /// Select the first session currently waiting for input, in tree order.
    JumpToWaitingSession,
    /// Move keyboard focus between grid tiles by `(dx, dy)`. Grid screen only.
    GridMove(i32, i32),
    /// Swap the focused tile with its neighbor by `(dx, dy)`. Grid screen only.
    GridSwap(i32, i32),
    /// Scroll the focused session by half a page (`true` = up).
    ScrollHalfPage(bool),
    /// Open the command palette straight into the "switch to session"
    /// drill-in. Zen-only (a no-op outside zen) — see `PaletteRow::SwitchToSession`.
    SwitchSession,
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
        triggers: &["p", "P"],
        display_keys: "p",
        description: "New session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewSessionInWorktree),
        triggers: &["n", "N"],
        display_keys: "n",
        description: "New session in current worktree",
        scopes: G,
        requires_alt: true,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::SwitchSession),
        triggers: &["s", "S"],
        display_keys: "s",
        description: "Switch to session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NextSession),
        triggers: &["j", "J"],
        display_keys: "j",
        description: "Next session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::PrevSession),
        triggers: &["k", "K"],
        display_keys: "k",
        description: "Previous session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: the matcher handles 1–9 dynamically (see match_global_shortcut).
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "1–9",
        description: "Select nth session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleGrid),
        triggers: &["g", "G"],
        display_keys: "g",
        description: "Toggle grid view",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleZen),
        triggers: &[],
        display_keys: "enter",
        description: "Toggle zen mode",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::Settings),
        triggers: &[","],
        display_keys: ",",
        description: "Settings",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomIn),
        triggers: &["=", "+"],
        display_keys: "=",
        description: "Zoom in",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomOut),
        triggers: &["-", "_"],
        display_keys: "-",
        description: "Zoom out",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomReset),
        triggers: &["0"],
        display_keys: "0",
        description: "Reset zoom",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ShortcutOverlay),
        triggers: &["/", "?"],
        display_keys: "/",
        description: "This overlay",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::CloseFocusedSession),
        triggers: &["w", "W"],
        display_keys: "w",
        description: "close focused session / terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewHomeTerminal),
        triggers: &["t", "T"],
        display_keys: "t",
        description: "New home terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::JumpToWaitingSession),
        triggers: &["'"],
        display_keys: "'",
        description: "Jump to session needing you",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: matched by `keyboard_scroll_intent` in `handle_key`, not
    // `match_global_shortcut` — plain PageUp/PageDown/Home/End (no Shift) must
    // fall through to the PTY, so these live outside the registry lookup.
    // Applies on every screen: `focused_session_mut()` resolves the grid's
    // focused tile too.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "shift+pgup/pgdn",
        description: "Scroll session by page",
        scopes: G,
        requires_alt: false,
        literal: true,
    },
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "shift+home/end",
        description: "Scroll to top / bottom",
        scopes: G,
        requires_alt: false,
        literal: true,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ScrollHalfPage(true)),
        triggers: &["u", "U"],
        display_keys: "u",
        description: "Scroll half page up",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ScrollHalfPage(false)),
        triggers: &["d", "D"],
        display_keys: "d",
        description: "Scroll half page down",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: `match_global_shortcut` handles both of these rows ahead
    // of the registry lookup (dynamic dx/dy per key, `grid_swap_mods` picks
    // move vs. swap), scoped to Screen::Grid by hand there — keep the three
    // in sync. The swap row's `display_keys` differs per platform: mac shows
    // the Shift chord (Cmd+Opt collides with the OS "Hide Others" shortcut on
    // H), non-mac keeps Alt (Cmd+Alt still works on mac too, just isn't the
    // one advertised).
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "h j k l / ←↓↑→",
        description: "Move focus in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: if cfg!(target_os = "macos") {
            "shift+h j k l / ←↓↑→"
        } else {
            "alt+h j k l / ←↓↑→"
        },
        description: "Move tile in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        // On mac this row displays as `{platform_mod_label()}+shift+...` (no
        // Alt infix — see `shortcut_overlay_modal`'s `key_label`); non-mac
        // keeps the Alt infix as before.
        requires_alt: !cfg!(target_os = "macos"),
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
        description: "Resize terminal panel",
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
            return Some(if grid_swap_mods(mods) {
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

/// `tile_order` position to refocus after killing the focused tile, given the
/// killed tile's position before removal (`killed_pos`) and the tile count
/// after removal (`len`). The killed slot is filled by whatever shifted into
/// it, so we refocus that same slot; if the killed tile was last, clamp to
/// the new last slot instead. Kept as a free function so it's testable
/// without constructing a full `Grove`.
fn grid_focus_after_kill(killed_pos: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(killed_pos.unwrap_or(0).min(len - 1))
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
/// Captured events belong to the widget that consumed them — with two
/// carve-outs:
/// - Escape: a focused `text_input` captures it only to blur itself and
///   never tells the app, so without this cancelling a modal would take two
///   presses.
/// - A `global_mods` chord (Cmd on mac, Ctrl+Shift elsewhere): a focused
///   `text_input` captures *every* `KeyPressed`, chord or not, since as far
///   as the widget is concerned a character key just gets typed. But a
///   chord is a command, never text — without this carve-out, e.g. ⌘D in
///   the Theme sub-pane never reached `handle_modal_key`'s Theme-pane arm at
///   all; the user just saw "d" typed into the focused search field.
fn should_forward(ev: &Event, status: event::Status) -> bool {
    if status != event::Status::Captured {
        return true;
    }
    match ev {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        }) => true,
        Event::Keyboard(keyboard::Event::KeyPressed { modifiers, .. }) => global_mods(*modifiers),
        _ => false,
    }
}

/// Whether Escape has something to dismiss when no modal is open. `false`
/// means Escape must reach the PTY — many TUI programs need it, and
/// swallowing it unconditionally would regress that. The caller clears both
/// states, so which one is armed doesn't matter.
fn escape_should_dismiss(
    pending_kill: Option<usize>,
    pending_kill_terminal: Option<usize>,
    open_agent_menu: Option<(usize, usize)>,
    attention_open: bool,
) -> bool {
    pending_kill.is_some()
        || pending_kill_terminal.is_some()
        || open_agent_menu.is_some()
        || attention_open
}

/// Chord + scope check for the terminal-panel resize (see the registry's
/// display-only "resize terminal panel" row): Ctrl+Shift+Left/Right, Workspace
/// only, on every platform (unlike `global_mods`, this isn't Cmd on macOS).
/// Doesn't know about `term_panel_open` — that's runtime state the caller
/// gates separately so a closed panel falls through to the PTY.
/// How far a keyboard scroll chord should move the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollAmount {
    /// One page (the session's viewport height, minus a line of overlap).
    Page,
    /// The full scrollback, i.e. jump to the top or back to the bottom.
    All,
}

/// Maps a key event to a keyboard-scroll intent: `Some((up, amount))` when
/// the classic terminal scroll chords are pressed — Shift+PageUp/PageDown
/// (page) or Shift+Home/End (top/bottom) — and `None` otherwise, including
/// when Ctrl/Logo/Alt are also held (so readline/TUI chords like
/// Ctrl+Shift+PageUp aren't stolen) or when Shift isn't held at all (so plain
/// PageUp/PageDown/Home/End keep reaching the PTY).
fn keyboard_scroll_intent(key: &Key, mods: Modifiers) -> Option<(bool, ScrollAmount)> {
    if !mods.shift() || mods.control() || mods.logo() || mods.alt() {
        return None;
    }
    match key {
        Key::Named(Named::PageUp) => Some((true, ScrollAmount::Page)),
        Key::Named(Named::PageDown) => Some((false, ScrollAmount::Page)),
        Key::Named(Named::Home) => Some((true, ScrollAmount::All)),
        Key::Named(Named::End) => Some((false, ScrollAmount::All)),
        _ => None,
    }
}

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
    use super::{
        backend_pane_selected_index, default_agent_pane_selected_index, match_global_shortcut,
        permissions_pane_selected_index, project_theme_pane_rows, slide_progress,
        theme_pane_combined_rows, theme_pane_row_is_custom, theme_pane_rows,
        theme_pane_selected_index, theme_reload_fallback, GlobalShortcut, Screen, SettingRow,
        GRID_SLIDE,
    };
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
            match_global_shortcut(&ch("p"), gmods(), screen),
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

    /// `mod+u`/`mod+d` scroll the focused session by half a page; plain
    /// "u"/"d" (no platform modifier) must not be treated as shortcuts, so
    /// they keep reaching the PTY (e.g. Ctrl+U/D line-kill / EOF).
    #[test]
    fn scroll_half_page_requires_platform_modifier() {
        use GlobalShortcut::ScrollHalfPage;
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("u"), gmods(), screen),
            Some(ScrollHalfPage(true))
        );
        assert_eq!(
            match_global_shortcut(&ch("d"), gmods(), screen),
            Some(ScrollHalfPage(false))
        );
        assert_eq!(
            match_global_shortcut(&ch("u"), Modifiers::empty(), screen),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("d"), Modifiers::empty(), screen),
            None
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
        // Plain platform modifier (no Alt) on `n` is no longer a shortcut —
        // NewSession moved to `p`; only the alt-chord claims `n` now.
        assert_eq!(match_global_shortcut(&ch("n"), gmods(), screen), None);
        assert_eq!(
            match_global_shortcut(&ch("p"), gmods(), screen),
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

    /// Mac-only: Cmd+Opt+H collides with the OS "Hide Others" shortcut, so
    /// Cmd+Shift is also accepted (and is what's displayed) for the swap
    /// chord there. Cmd+Alt must keep working too (checked above by the
    /// shared `alt` chord in `grid_swap_shortcuts_scoped_to_grid_screen`).
    #[test]
    #[cfg(target_os = "macos")]
    fn grid_swap_shortcuts_accept_shift_on_mac() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        let shift = gmods() | Modifiers::SHIFT;
        assert_eq!(
            match_global_shortcut(&ch("h"), shift, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), shift, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), shift, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), shift, screen),
            Some(GridSwap(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), shift, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), shift, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), shift, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), shift, screen),
            Some(GridSwap(0, 1))
        );
        // Cmd alone (no Alt, no Shift) is still GridMove, not GridSwap.
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
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

    /// Keyboard scrollback chords (Shift+PageUp/PageDown/Home/End) are
    /// matched by `keyboard_scroll_intent` ahead of `key_to_bytes` in
    /// `handle_key`, screen-independent (applies on Workspace/Zen/Grid alike
    /// via `focused_session_mut`), and must require Shift alone — no
    /// Ctrl/Logo/Alt — so readline/TUI chords like Ctrl+Shift+PageUp aren't
    /// stolen and plain PageUp/PageDown/Home/End keep reaching the PTY.
    mod keyboard_scroll {
        use super::super::{keyboard_scroll_intent, ScrollAmount};
        use iced::keyboard::{key::Named, Key, Modifiers};

        #[test]
        fn shift_page_up_down_scroll_by_page() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageUp), Modifiers::SHIFT),
                Some((true, ScrollAmount::Page))
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageDown), Modifiers::SHIFT),
                Some((false, ScrollAmount::Page))
            );
        }

        #[test]
        fn shift_home_end_jump_top_and_bottom() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::Home), Modifiers::SHIFT),
                Some((true, ScrollAmount::All))
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::End), Modifiers::SHIFT),
                Some((false, ScrollAmount::All))
            );
        }

        #[test]
        fn plain_page_up_down_fall_through_to_the_pty() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageUp), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageDown), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::Home), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::End), Modifiers::empty()),
                None
            );
        }

        #[test]
        fn extra_modifiers_are_not_stolen_from_readline_or_tui_chords() {
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::CTRL | Modifiers::SHIFT
                ),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::LOGO | Modifiers::SHIFT
                ),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::ALT | Modifiers::SHIFT
                ),
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

    /// Killing the focused tile shouldn't leave the grid with nothing
    /// focused — whatever slides into the killed slot should take focus.
    mod grid_focus_after_kill {
        use super::super::grid_focus_after_kill;

        #[test]
        fn killed_in_middle_focuses_same_slot() {
            // Tile that shifted into the killed slot takes focus.
            assert_eq!(grid_focus_after_kill(Some(1), 3), Some(1));
        }

        #[test]
        fn killed_at_end_clamps_to_new_last_slot() {
            // Nothing slides into the last slot, so clamp instead.
            assert_eq!(grid_focus_after_kill(Some(2), 2), Some(1));
        }

        #[test]
        fn no_tiles_remain_focuses_nothing() {
            assert_eq!(grid_focus_after_kill(Some(0), 0), None);
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
        use super::gmods;
        use iced::keyboard::{key::Named, Key, Modifiers};
        use iced::{event, keyboard, Event};

        fn press(key: Key) -> Event {
            press_mods(key, Modifiers::empty())
        }

        fn press_mods(key: Key, modifiers: Modifiers) -> Event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers,
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

        /// The bug this regresses: the palette's focused search input
        /// captures every `KeyPressed` (it's a text field), so without this
        /// carve-out a chord like ⌘D never reached `handle_modal_key`'s
        /// Theme-pane arm at all — the user just saw "d" typed into the
        /// query. Chords are commands, never text, regardless of what
        /// widget currently has focus.
        #[test]
        fn captured_global_mod_chord_still_forwards() {
            assert!(should_forward(
                &press_mods(Key::Character("d".into()), gmods()),
                event::Status::Captured
            ));
            // Named keys chord too (⌘⌫ delete).
            assert!(should_forward(
                &press_mods(Key::Named(Named::Backspace), gmods()),
                event::Status::Captured
            ));
        }

        /// A bare modifier that doesn't add up to the platform's *global*
        /// chord (`global_mods` requires Ctrl+Shift together on non-mac,
        /// Cmd-without-Ctrl on mac) must not forward — otherwise every
        /// Ctrl-chord a text field captures for its own editing (e.g.
        /// word-delete) would leak into `handle_modal_key` too.
        #[test]
        fn captured_partial_or_unrelated_modifier_still_dropped() {
            assert!(!should_forward(
                &press_mods(Key::Character("d".into()), Modifiers::CTRL),
                event::Status::Captured
            ));
            assert!(!should_forward(
                &press_mods(Key::Character("d".into()), Modifiers::SHIFT),
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
            assert!(!escape_should_dismiss(None, None, None, false));
        }

        #[test]
        fn true_when_either_is_set() {
            assert!(escape_should_dismiss(Some(2), None, None, false));
            assert!(escape_should_dismiss(None, Some(3), None, false));
            assert!(escape_should_dismiss(None, None, Some((1, 0)), false));
            assert!(escape_should_dismiss(Some(2), None, Some((1, 0)), false));
        }

        #[test]
        fn true_when_attention_queue_is_open() {
            assert!(escape_should_dismiss(None, None, None, true));
        }
    }

    #[test]
    fn setting_row_label_section_and_icon_are_total_and_nonempty() {
        for s in SettingRow::ALL {
            assert!(!s.label().is_empty());
            assert!(!s.section().is_empty());
            assert!(!s.icon_name().is_empty());
        }
        // Spot-check a few, so a typo'd match arm can't silently return the
        // wrong (but still non-empty) string for the wrong variant.
        assert_eq!(SettingRow::Telemetry.label(), "Telemetry");
        assert_eq!(SettingRow::Telemetry.section(), "AGENTS / TERMINAL");
        assert_eq!(SettingRow::CheckUpdates.label(), "Check for updates");
        assert_eq!(SettingRow::CheckUpdates.section(), "UPDATES");
    }

    #[test]
    fn settings_row_keyword_matches_root_query() {
        // `palette_rows` needs a full `Grove` to construct (sessions, PTYs,
        // store, …) — impractical in a unit test — so this exercises the
        // exact keyword condition it uses to surface `PaletteRow::Settings`
        // while typing at root (not `browse_all`): test (b), "typed input
        // 'settings' yields a Settings row".
        assert!(crate::gui::launcher::fuzzy_match(
            "settings", "settings", "", ""
        ));
        assert!(crate::gui::launcher::fuzzy_match("set", "settings", "", ""));
        assert!(!crate::gui::launcher::fuzzy_match(
            "zzz", "settings", "", ""
        ));
    }

    // ── Settings sub-panes (phase 2) ─────────────────────────────────────

    #[test]
    fn backend_pane_selects_the_active_backend() {
        assert_eq!(backend_pane_selected_index(false), 0); // Native
        assert_eq!(backend_pane_selected_index(true), 1); // Tmux
    }

    #[test]
    fn permissions_pane_selects_the_active_choice() {
        assert_eq!(permissions_pane_selected_index(false), 0); // Ask
        assert_eq!(permissions_pane_selected_index(true), 1); // Skip
    }

    #[test]
    fn default_agent_pane_selects_the_current_default() {
        use crate::agent::Agent;
        assert_eq!(default_agent_pane_selected_index(None), 0);
        assert_eq!(default_agent_pane_selected_index(Some(Agent::Claude)), 0);
        assert_eq!(default_agent_pane_selected_index(Some(Agent::Codex)), 1);
        assert_eq!(default_agent_pane_selected_index(Some(Agent::OpenCode)), 2);
        assert_eq!(default_agent_pane_selected_index(Some(Agent::Terminal)), 3);
    }

    #[test]
    fn theme_pane_selects_the_active_theme_within_its_kind() {
        use crate::theme::ThemeKind;
        // `tokyonight` is alphabetically first among the builtin dark
        // themes shipped today; a name with no match falls back to 0
        // rather than panicking.
        let dark = crate::theme::themes_of(ThemeKind::Dark);
        let idx = dark.iter().position(|t| t.name == "tokyonight").unwrap();
        assert_eq!(
            theme_pane_selected_index(ThemeKind::Dark, "tokyonight"),
            idx
        );
        assert_eq!(
            theme_pane_selected_index(ThemeKind::Dark, "no-such-theme"),
            0
        );
    }

    #[test]
    fn theme_pane_rows_lists_only_the_requested_kind_fuzzy_filtered() {
        use crate::theme::ThemeKind;
        let all_dark = crate::theme::themes_of(ThemeKind::Dark);
        let all_light = crate::theme::themes_of(ThemeKind::Light);
        // Unfiltered: exactly the kind's own theme set, same order.
        assert_eq!(
            theme_pane_rows(ThemeKind::Dark, "")
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            all_dark.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        );
        // Every row is actually of the requested kind — Light never leaks
        // into a Dark query or vice versa.
        assert!(theme_pane_rows(ThemeKind::Light, "")
            .iter()
            .all(|t| t.kind == ThemeKind::Light));
        assert_ne!(all_dark.len(), 0);
        assert_ne!(all_light.len(), 0);
        // Fuzzy-filtered: only names containing the query survive.
        let filtered = theme_pane_rows(ThemeKind::Dark, "tokyonight");
        assert!(!filtered.is_empty());
        assert!(filtered.iter().all(|t| t.name.contains("tokyonight")));
        // No match anywhere in the kind's list.
        assert!(theme_pane_rows(ThemeKind::Dark, "zzz-no-such-theme").is_empty());
    }

    #[test]
    fn theme_pane_combined_rows_lists_customs_after_builtins() {
        use crate::theme::{Color, ThemeKind};
        let _lock = crate::theme::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let name = "zz-test-custom-batch2";
        let custom = crate::theme::Theme {
            name: std::borrow::Cow::Owned(name.to_string()),
            kind: ThemeKind::Dark,
            bg: Color::Rgb(0, 0, 0),
            bg_highlight: Color::Rgb(0, 0, 0),
            fg: Color::Rgb(0, 0, 0),
            fg_dark: Color::Rgb(0, 0, 0),
            comment: Color::Rgb(0, 0, 0),
            blue: Color::Rgb(0, 0, 0),
            cyan: Color::Rgb(0, 0, 0),
            magenta: Color::Rgb(0, 0, 0),
            green: Color::Rgb(0, 0, 0),
            yellow: Color::Rgb(0, 0, 0),
            red: Color::Rgb(0, 0, 0),
        };
        crate::theme::add_custom(custom).expect("add_custom");

        let builtins_len = theme_pane_rows(ThemeKind::Dark, "").len();
        let combined = theme_pane_combined_rows(ThemeKind::Dark, "");
        // Every builtin row comes first, in the same order, then the custom
        // row(s) — never interleaved.
        assert_eq!(combined.len(), builtins_len + 1);
        assert_eq!(combined.last().unwrap().name, name);
        assert!(!theme_pane_row_is_custom(ThemeKind::Dark, "", 0));
        assert!(theme_pane_row_is_custom(ThemeKind::Dark, "", builtins_len));
        assert_eq!(
            theme_pane_selected_index(ThemeKind::Dark, name),
            builtins_len
        );

        crate::theme::delete_custom(name);
    }

    #[test]
    fn theme_reload_fallback_none_when_active_theme_still_resolves() {
        use crate::theme::ThemeKind;
        let name = crate::theme::BUILTINS[0].name.to_string();
        assert_eq!(theme_reload_fallback(&name, ThemeKind::Dark), None);
    }

    #[test]
    fn theme_reload_fallback_falls_back_to_mode_default_when_active_theme_vanished() {
        use crate::theme::ThemeKind;
        assert_eq!(
            theme_reload_fallback("a-name-that-was-deleted", ThemeKind::Dark),
            Some(crate::app::DEFAULT_DARK_THEME)
        );
        assert_eq!(
            theme_reload_fallback("a-name-that-was-deleted", ThemeKind::Light),
            Some(crate::app::DEFAULT_LIGHT_THEME)
        );
    }

    #[test]
    fn project_theme_pane_rows_has_use_default_row_only_when_query_is_empty() {
        use crate::theme::ThemeKind;
        // Empty query: "Use app theme" (None) leads, followed by every dark
        // theme in `theme_pane_rows` order.
        let rows = project_theme_pane_rows(ThemeKind::Dark, "");
        assert!(rows[0].is_none());
        assert_eq!(
            rows[1..]
                .iter()
                .map(|t| t.clone().unwrap().name)
                .collect::<Vec<_>>(),
            theme_pane_rows(ThemeKind::Dark, "")
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        );
        // Whitespace-only query counts as empty too.
        assert!(project_theme_pane_rows(ThemeKind::Dark, "   ")[0].is_none());
        // Any real query drops the "Use app theme" row — only fuzzy matches
        // remain.
        let filtered = project_theme_pane_rows(ThemeKind::Dark, "tokyonight");
        assert!(!filtered.is_empty());
        assert!(filtered.iter().all(|t| t.is_some()));
        assert!(filtered
            .iter()
            .all(|t| t.clone().unwrap().name.contains("tokyonight")));
        // No match anywhere still yields an empty list, not a dangling
        // default row.
        assert!(project_theme_pane_rows(ThemeKind::Dark, "zzz-no-such-theme").is_empty());
    }

    #[test]
    fn project_theme_pane_rows_kind_switch_yields_different_lists() {
        use crate::theme::ThemeKind;
        let dark = project_theme_pane_rows(ThemeKind::Dark, "");
        let light = project_theme_pane_rows(ThemeKind::Light, "");
        assert!(dark
            .iter()
            .skip(1)
            .all(|t| t.clone().unwrap().kind == ThemeKind::Dark));
        assert!(light
            .iter()
            .skip(1)
            .all(|t| t.clone().unwrap().kind == ThemeKind::Light));
        assert_ne!(dark.len(), light.len());
    }

    #[test]
    fn update_actions_hide_update_now_for_unknown_installs() {
        use super::{update_available_actions, UpdateAction};
        // Known install method: all three actions, "Update now" first.
        assert_eq!(
            update_available_actions(false),
            vec![
                UpdateAction::UpdateNow,
                UpdateAction::SkipVersion,
                UpdateAction::CopyUrl
            ]
        );
        // Unknown install (notify-only): "Update now" is hidden, same guard
        // `settings_modal` applies — indices shift down with it.
        assert_eq!(
            update_available_actions(true),
            vec![UpdateAction::SkipVersion, UpdateAction::CopyUrl]
        );
    }

    #[test]
    fn check_updates_activation_opens_strip_only_when_update_available() {
        use super::{check_updates_opens_strip, UpgradeState};
        let release = crate::upgrade::Release {
            version: semver::Version::new(0, 9, 5),
            tag: "v0.9.5".into(),
            html_url: String::new(),
            body: String::new(),
            dmg_url: None,
        };
        // Only a known-available release expands the strip…
        assert!(check_updates_opens_strip(&UpgradeState::Available(release)));
        // …every other state falls through to firing a fresh check.
        assert!(!check_updates_opens_strip(&UpgradeState::Idle));
        assert!(!check_updates_opens_strip(&UpgradeState::Checking));
        assert!(!check_updates_opens_strip(&UpgradeState::UpToDate));
        assert!(!check_updates_opens_strip(&UpgradeState::Error(
            "offline".into()
        )));
    }

    #[test]
    fn launcher_theme_scroll_offset_centers_and_clamps() {
        use super::launcher_theme_scroll_offset;
        // Everything fits (7 rows ≤ 280px cap): no scrolling, ever.
        assert_eq!(launcher_theme_scroll_offset(7, 0), 0.0);
        assert_eq!(launcher_theme_scroll_offset(7, 6), 0.0);
        // 30 rows at the true 38px pitch (36px row + 2px column spacing) =
        // 30·38 − 2 = 1138px content (no trailing gap after the last row)
        // against a 280px viewport.
        // Top rows clamp to 0 rather than centering above the list…
        assert_eq!(launcher_theme_scroll_offset(30, 0), 0.0);
        // …the last row clamps to the bottom (1138 − 280 = 858)…
        assert_eq!(launcher_theme_scroll_offset(30, 29), 858.0);
        // …and a middle row centers: y = 15·38 − (280 − 36)/2 = 448.
        assert_eq!(launcher_theme_scroll_offset(30, 15), 448.0);
        // Empty list degenerates to 0, not NaN/negative.
        assert_eq!(launcher_theme_scroll_offset(0, 0), 0.0);
    }

    /// Regression for the reported bug: ↑/↓ in the theme editor didn't
    /// scroll at all (`theme_manager_editor_row_select` never called any
    /// scroll function). Values hand-derived from the exact `view.rs` render
    /// geometry (2 Surfaces + 3 Text + 6 Accents rows, each group preceded
    /// by a 10/13/4 header, followed by the derived strip's own header +
    /// chip row) — see `theme_editor_scroll_offset`'s doc comment for the
    /// constants.
    #[test]
    fn theme_editor_scroll_offset_centers_and_clamps() {
        use super::theme_editor_scroll_offset;
        // Row 0 (first "Surfaces" row): would center above the list, so it
        // clamps to the top instead.
        assert_eq!(theme_editor_scroll_offset(0), 0.0);
        // Row 2 (first "Text" row, right after a group-header transition):
        // sel_y = 124 (17 + 2 + 36 + 2 + 36 + 27 + 2), centered at
        // 124 − (280 − 36)/2 = 2.
        assert_eq!(theme_editor_scroll_offset(2), 2.0);
        // Row 5 (first "Accents" row): centers normally, no clamp.
        assert_eq!(theme_editor_scroll_offset(5), 145.0);
        // Row 10 ("red", the last color row): the trailing derived-strip
        // header + chip row push total content to 545px, so the bottom
        // clamp (545 − 280 = 265) kicks in rather than over-scrolling past
        // the real content.
        assert_eq!(theme_editor_scroll_offset(10), 265.0);
    }

    #[test]
    fn theme_pane_tab_cycles_dark_light_system() {
        use super::{next_theme_mode, ThemeMode};
        use crate::theme::ThemeKind;
        // Dark → Light → System → Dark, matching the segment order (System
        // is active whenever follow_system is set, whatever the list kind).
        assert_eq!(next_theme_mode(ThemeKind::Dark, false), ThemeMode::Light);
        assert_eq!(next_theme_mode(ThemeKind::Light, false), ThemeMode::System);
        assert_eq!(next_theme_mode(ThemeKind::Dark, true), ThemeMode::Dark);
        assert_eq!(next_theme_mode(ThemeKind::Light, true), ThemeMode::Dark);
    }

    #[test]
    fn settings_root_scroll_offset_accounts_for_headers_and_clamps() {
        use super::settings_root_scroll_offset;
        // The full unfiltered list: 8 rows across 4 sections. Element walk
        // (2px column spacing throughout): first header 19px (0+13+6), later
        // headers 31px (12+13+6), rows 44px — content = 4 headers (112) +
        // 8 rows (352) + 11 gaps (22) = 486px against the 364px viewport
        // (the 380px max_height minus the same container's 2·8px padding),
        // so max scroll = 122.
        let rows = SettingRow::ALL;
        // Row 0 sits right under the first header: centering clamps to 0.
        assert_eq!(settings_root_scroll_offset(&rows, 0), 0.0);
        // The last row (CheckUpdates, y = 442) clamps to the bottom:
        // content_h − viewport_h = 486 − 364…
        assert_eq!(settings_root_scroll_offset(&rows, 7), 122.0);
        // …which leaves all 44px of it inside the viewport: its bottom edge
        // (y + row) sits exactly at the viewport's bottom (offset + 364).
        let max_offset = settings_root_scroll_offset(&rows, 7);
        assert!(442.0 + 44.0 <= max_offset + 364.0);
        // A row past a mid-list header (Backend, first of AGENTS/TERMINAL):
        // y = 192 → centered 192 − (364 − 44)/2 = 32. Uniform-height math
        // (i·46) would put y at 138 and clamp the centering to 0 — the
        // headers are what make the difference.
        assert_eq!(settings_root_scroll_offset(&rows, 3), 32.0);
        assert!(
            settings_root_scroll_offset(&rows, 3)
                > (3.0 * 46.0 - (364.0 - 44.0) / 2.0_f32).max(0.0)
        );
        // Empty (fully filtered-out) list degenerates to 0.
        assert_eq!(settings_root_scroll_offset(&[], 0), 0.0);
    }

    #[test]
    fn palette_scroll_offset_root_mode_accounts_for_headers_and_clamps() {
        use super::{palette_scroll_offset, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        // Root-mode shape: one RECENT row, two per-project WORKTREES groups
        // of three rows each, then one ACTIONS row — same header sizes as
        // `settings_root_scroll_offset`'s test (19px first header, 31px
        // later ones, 44px rows, 2px gaps throughout), so content sums to
        // the same 486px against the same 364px viewport (max scroll 122).
        let rows = vec![
            PaletteRow::Recent {
                proj: 0,
                wt_path: "/wt/0".into(),
                agent: Agent::Claude,
            },
            combo(0),
            combo(0),
            combo(0),
            combo(1),
            combo(1),
            combo(1),
            PaletteRow::NewSession,
        ];
        // Row 0 sits right under the RECENT header: centering clamps to 0.
        assert_eq!(palette_scroll_offset(&rows, 0, true), 0.0);
        // The last row (NewSession, y = 442) clamps to the bottom.
        assert_eq!(palette_scroll_offset(&rows, 7, true), 122.0);
        // A row past the second WORKTREES header (proj 0's 3rd combo row,
        // y = 192): centered 192 − (364 − 44)/2 = 32 — the header is what
        // shifts it off the naive index·46 estimate.
        assert_eq!(palette_scroll_offset(&rows, 3, true), 32.0);
        // Empty list degenerates to 0.
        assert_eq!(palette_scroll_offset(&[], 0, true), 0.0);
    }

    #[test]
    fn palette_scroll_offset_typed_list_fits_and_clamps() {
        use super::{palette_scroll_offset, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        // A short typed list (well under the 364px viewport): no scrolling,
        // ever, whichever row is selected.
        let short = vec![
            PaletteRow::Setting(SettingRow::Theme),
            PaletteRow::Setting(SettingRow::Telemetry),
            combo(0),
        ];
        assert_eq!(palette_scroll_offset(&short, 0, false), 0.0);
        assert_eq!(palette_scroll_offset(&short, 2, false), 0.0);
        // A long typed list of 15 same-project session rows: >10 rows alone
        // triggers `grouped_by_project`, adding a per-project sub-header
        // under the SESSIONS header. Selection 0 still clamps to 0 (row 0
        // sits right under both headers)…
        let long: Vec<PaletteRow> = (0..15).map(|_| combo(0)).collect();
        assert_eq!(palette_scroll_offset(&long, 0, false), 0.0);
        // …and the last row clamps to the bottom of the (728px) content
        // against the 364px viewport: max scroll = 364.
        assert_eq!(palette_scroll_offset(&long, 14, false), 364.0);
    }

    #[test]
    fn nth_session_row_skips_settings_and_action_rows() {
        use super::{nth_session_row, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        // Typed-mode shape: settings sort above the session rows (B2).
        let rows = vec![
            PaletteRow::Setting(SettingRow::Theme),
            PaletteRow::Setting(SettingRow::Telemetry),
            combo(0),
            combo(1),
            PaletteRow::SwitchToSession,
        ];
        // ⌘1/⌘2 land on the sessions, not the settings above them…
        assert_eq!(nth_session_row(&rows, 1), Some(2));
        assert_eq!(nth_session_row(&rows, 2), Some(3));
        // …and digits past the session count are a no-op, even though other
        // row kinds are still below.
        assert_eq!(nth_session_row(&rows, 3), None);
        assert_eq!(nth_session_row(&rows, 0), None);
        // Recent rows count the same as Combo (root-mode list shape).
        let root = vec![
            PaletteRow::Recent {
                proj: 0,
                wt_path: "/wt/0".into(),
                agent: Agent::Codex,
            },
            PaletteRow::NewSession,
        ];
        assert_eq!(nth_session_row(&root, 1), Some(0));
        assert_eq!(nth_session_row(&root, 2), None);
    }

    #[test]
    fn resolve_row_by_identity_follows_the_row_after_a_synthetic_reorder() {
        use super::{resolve_row_by_identity, row_identity, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        // The user highlighted combo(1) (index 1) and its identity was
        // captured then — simulating `set_palette_selected`.
        let rendered = vec![combo(0), combo(1), combo(2)];
        let identity = Some(row_identity(&rendered[1]));
        // Before Enter lands, something reorders the list out from under
        // the stale index (e.g. a re-rank/re-group pass, or an async
        // recents update) — combo(1) is now at index 0, not 1.
        let mutated = vec![combo(1), combo(0), combo(2)];
        // A naive `mutated.get(1)` would now resolve to combo(0) — the
        // wrong row. Identity resolution finds combo(1) wherever it moved.
        assert_eq!(resolve_row_by_identity(&mutated, &identity, 1), Some(0));
    }

    #[test]
    fn resolve_row_by_identity_refuses_to_substitute_a_different_row() {
        use super::{resolve_row_by_identity, row_identity, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        let identity = Some(row_identity(&combo(1)));
        // combo(1) is simply gone from the rebuilt list (worktree removed,
        // filtered out, …): must not silently activate whatever now sits at
        // the stale index instead.
        let mutated = vec![combo(0), combo(2)];
        assert_eq!(resolve_row_by_identity(&mutated, &identity, 1), None);
    }

    #[test]
    fn resolve_row_by_identity_falls_back_to_index_with_no_identity() {
        use super::{resolve_row_by_identity, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize| PaletteRow::Combo {
            proj,
            wt_path: format!("/wt/{proj}"),
            agent: Agent::Claude,
        };
        let rows = vec![combo(0), combo(1)];
        // No identity captured yet: falls back to the raw index (clamped to
        // the list bounds).
        assert_eq!(resolve_row_by_identity(&rows, &None, 1), Some(1));
        assert_eq!(resolve_row_by_identity(&rows, &None, 5), None);
    }

    #[test]
    fn row_identity_distinguishes_action_and_setting_rows() {
        use super::{row_identity, PaletteRow};
        // Action/singleton rows carry no per-row data, but must still
        // compare unequal to each other so a stale identity can't match
        // the wrong action.
        assert!(row_identity(&PaletteRow::NewSession) != row_identity(&PaletteRow::AddProject));
        assert_eq!(
            row_identity(&PaletteRow::Setting(SettingRow::Theme)),
            row_identity(&PaletteRow::Setting(SettingRow::Theme))
        );
        assert!(
            row_identity(&PaletteRow::Setting(SettingRow::Theme))
                != row_identity(&PaletteRow::Setting(SettingRow::Telemetry))
        );
    }

    #[test]
    fn rank_and_group_combos_orders_by_score_then_recency() {
        use super::{rank_and_group_combos, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize, path: &str| PaletteRow::Combo {
            proj,
            wt_path: path.into(),
            agent: Agent::Claude,
        };
        // Two combos tie on score (10): the one at recency index 0 (earlier
        // in `recent_launches`) must sort first, even though it was pushed
        // second.
        let scored = vec![
            (10, usize::MAX, combo(0, "/b")),
            (10, 0, combo(0, "/a")),
            (5, usize::MAX, combo(0, "/c")),
        ];
        let ranked = rank_and_group_combos(scored);
        let paths: Vec<&str> = ranked
            .iter()
            .map(|r| match r {
                PaletteRow::Combo { wt_path, .. } => wt_path.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(paths, vec!["/a", "/b", "/c"]);
    }

    #[test]
    fn rank_and_group_combos_ties_with_no_recency_keep_store_order() {
        use super::{rank_and_group_combos, PaletteRow};
        use crate::agent::Agent;
        let combo = |path: &str| PaletteRow::Combo {
            proj: 0,
            wt_path: path.into(),
            agent: Agent::Claude,
        };
        // Both tied at usize::MAX (absent from recents): stable sort keeps
        // their original (store) relative order.
        let scored = vec![
            (10, usize::MAX, combo("/first")),
            (10, usize::MAX, combo("/second")),
        ];
        let ranked = rank_and_group_combos(scored);
        let paths: Vec<&str> = ranked
            .iter()
            .map(|r| match r {
                PaletteRow::Combo { wt_path, .. } => wt_path.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(paths, vec!["/first", "/second"]);
    }

    #[test]
    fn rank_and_group_combos_groups_by_project_above_threshold() {
        use super::{rank_and_group_combos, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize, path: &str, score: u32| {
            (
                score,
                usize::MAX,
                PaletteRow::Combo {
                    proj,
                    wt_path: path.into(),
                    agent: Agent::Claude,
                },
            )
        };
        // 3 distinct projects (over the 2-project threshold): re-clustered
        // by project, each project's run led by its own best-scored row —
        // project 1's best (20) beats project 0's best (15), so project 1's
        // whole run comes first.
        let scored = vec![
            combo(0, "/p0/a", 15),
            combo(1, "/p1/a", 20),
            combo(2, "/p2/a", 5),
            combo(0, "/p0/b", 10),
            combo(1, "/p1/b", 8),
        ];
        let ranked = rank_and_group_combos(scored);
        let projects: Vec<usize> = ranked
            .iter()
            .map(|r| match r {
                PaletteRow::Combo { proj, .. } => *proj,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(projects, vec![1, 1, 0, 0, 2]);
    }

    #[test]
    fn rank_and_group_combos_stays_flat_at_or_below_threshold() {
        use super::{rank_and_group_combos, PaletteRow};
        use crate::agent::Agent;
        let combo = |proj: usize, path: &str, score: u32| {
            (
                score,
                usize::MAX,
                PaletteRow::Combo {
                    proj,
                    wt_path: path.into(),
                    agent: Agent::Claude,
                },
            )
        };
        // Exactly 2 distinct projects, ≤10 rows: flat rank order, untouched.
        let scored = vec![combo(0, "/p0/a", 5), combo(1, "/p1/a", 20)];
        let ranked = rank_and_group_combos(scored);
        let projects: Vec<usize> = ranked
            .iter()
            .map(|r| match r {
                PaletteRow::Combo { proj, .. } => *proj,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(projects, vec![1, 0]);
    }

    #[test]
    fn root_project_order_puts_active_first_then_clamps_stale_index() {
        use super::root_project_order;
        assert_eq!(root_project_order(3, 1), vec![1, 0, 2]);
        assert_eq!(root_project_order(3, 0), vec![0, 1, 2]);
        // Stale/out-of-range active index clamps rather than panicking.
        assert_eq!(root_project_order(3, 99), vec![2, 0, 1]);
        assert_eq!(root_project_order(0, 0), Vec::<usize>::new());
    }

    #[test]
    fn reselect_setting_keeps_identity_else_clamps() {
        use super::reselect_setting;
        // The toggled row survived the refilter (moved up): follow it.
        let rows = [SettingRow::Telemetry, SettingRow::CheckUpdates];
        assert_eq!(reselect_setting(&rows, SettingRow::Telemetry, 5), 0);
        // The toggled row dropped out (value no longer matches the query):
        // the stale index clamps into the shrunk list.
        assert_eq!(reselect_setting(&rows, SettingRow::ProjectThemes, 5), 1);
        assert_eq!(reselect_setting(&rows, SettingRow::ProjectThemes, 0), 0);
        // Everything filtered out: clamp degenerates to 0.
        assert_eq!(reselect_setting(&[], SettingRow::Telemetry, 3), 0);
    }
}
