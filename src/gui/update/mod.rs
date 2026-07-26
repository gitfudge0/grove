//! `Grove` lifecycle: construction, subscriptions, and all `Msg` handling.

mod layout;
mod modals;
mod onboarding;
mod palette;
mod pty_input;
mod sessions;
mod settings_rows;
mod shortcuts;
mod theme_manager;
mod theme_picker;
mod tick;
mod upgrade;

pub(crate) use settings_rows::SettingRow;
pub(in crate::gui) use shortcuts::global_mods;
pub(crate) use shortcuts::{
    grid_neighbor, platform_mod_label, scope_allows, slide_progress, GlobalShortcut, Scope,
    ShortcutDef, SHORTCUTS,
};

use pty_input::{
    escape_should_dismiss, is_copy_shortcut, is_paste_shortcut, keyboard_scroll_intent,
    should_forward, term_panel_resize_delta, ScrollAmount, MODAL_OPEN,
};
use shortcuts::{
    close_focused_session_decision, match_global_shortcut, screen_from_flags,
    should_sync_grid_focus, CloseFocusedDecision, Screen,
};

use super::keys::key_to_bytes;
use super::metrics::{
    clamp_sidebar_width, compute_pty_dims, PTY_ZOOM_DEFAULT, PTY_ZOOM_MAX, PTY_ZOOM_MIN,
    PTY_ZOOM_STEP, RAIL_W, TERM_PANEL_PORTION,
};
use super::session_launcher;
use super::session_launcher::{update_available_actions, UpdateAction};
use super::state::{
    Animations, ChangelogState, DragState, FocusedPane, GridDragMsg, Grove, Msg, PtyLayout,
    UpgradeState,
};
use crate::app::{App, Modal};
use grove_core::agent::Agent;
use grove_core::session::Session;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::Id;
use iced::{event, keyboard, Event, Subscription, Task};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// How many `Msg::Tick`s (fired every 60ms — see the tick subscription
/// below) of no further zoom change must pass before the debounced
/// `ui_zoom` write in `set_ui_zoom` is flushed to disk. 4 ticks * 60ms ≈
/// 250ms, chosen to comfortably outlast a single wheel/keyboard event burst.
const ZOOM_SAVE_QUIET_TICKS: u8 = 4;

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
pub(super) fn scroll_to(id: Id, offset: iced::widget::scrollable::AbsoluteOffset) -> Task<Msg> {
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
        grove_core::attention::cleanup_stale_files();
        // Compute initial PTY dimensions from the default window size (1280×800).
        // Corrected on the first `WindowResized` event after startup.
        let window_size = iced::Size::new(1280.0, 800.0);
        // Startup config load. There is no UI yet to show an error in, so a
        // failure here is genuinely unrecoverable — panicking is the intended
        // behavior, not an unhandled case.
        #[allow(clippy::expect_used)]
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
            pty_layout: PtyLayout {
                rows: pty_rows,
                cols: pty_cols,
                sess_cols: pty_cols,
                panel_cols: pty_cols,
                zoom: ui_zoom,
                zoom_save_countdown: None,
                window_size,
            },
            open_agent_menu: None,
            attention_open: false,
            pty_selection: None,
            pty_drag: None,
            pty_press_focused: false,
            anim: Animations {
                blink_tick: 0,
                attention_anim: Self::attention_animation(),
                onb_step_anim: Self::onb_step_animation(),
                grid_slide: None,
            },
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
            claude_poller: grove_core::claude_agents::Poller::new(),
            // Assumed focused at launch (iced can't be queried); corrected by
            // the first Focused/Unfocused event. Worst case: one missed dock
            // bounce in the first moments of an unfocused launch.
            window_focused: true,
            last_badge: 0,
            sidebar_width,
            drag: DragState::default(),
            grid_view: false,
            tile_order: Vec::new(),
            grid_focused: None,
            grid_view_before_zen: false,
            scripts_editor: None,
            add_project: None,
            launcher: None,
            theme_manager_editor: None,
            settings_tools: Vec::new(),
            upgrade: UpgradeState::Idle,
            upgrade_method: grove_core::upgrade::detect(),
            upgrade_progress: std::sync::Arc::new(std::sync::Mutex::new(
                crate::gui::state::UpgradeProgress::default(),
            )),
            changelog: ChangelogState::Idle,
            show_changelog: false,
            git_state: Default::default(),
            last_git_poll: None,
            git_poll_inflight: Default::default(),
            wt_rebuild_pending: false,
            wt_rebuild_inflight: false,
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
        g.app
            .ensure_home_terminal(g.pty_layout.rows, g.pty_layout.cols);
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
            g.anim.onb_step_anim.go_mut(true, std::time::Instant::now());
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
        self.anim.onb_step_anim = Self::onb_step_animation();
        self.anim
            .onb_step_anim
            .go_mut(true, std::time::Instant::now());
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        // Only forward un-captured keys; widgets (search input) handle their own first.
        // Exception: a focused text_input captures Escape to blur itself
        // (iced_widget text_input.rs) without telling the app, so Escape would
        // otherwise need a second press to reach the modal's cancel handler.
        // Forward it regardless of status; every other captured key stays dropped.
        // `MODAL_OPEN` tells `should_forward` whether a global-mods chord
        // captured by a focused text widget belongs to a modal's own
        // shortcut handling (`handle_modal_key`, reached only while a modal
        // is open) — see that function's doc comment for why it must NOT
        // also forward outside a modal, where `handle_key` would otherwise
        // double-handle the same chord via the PTY copy/paste shortcuts.
        // `event::listen_with` requires a plain `fn` (no captures), so the
        // current value is stashed in a static ahead of time rather than
        // captured by the closure below — refreshed every `subscription()`
        // call, which iced makes on every update, so it never lags by more
        // than the current frame.
        MODAL_OPEN.store(!matches!(self.app.modal, Modal::None), Ordering::Relaxed);
        let keys = event::listen_with(|ev, status, _| {
            if !should_forward(&ev, status, MODAL_OPEN.load(Ordering::Relaxed)) {
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
                // so `session_launcher::Msg::InputChanged` can tell a real typed
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
        // - 1s fallback otherwise — toast TTLs, background-job results,
        //   git-status polling, attention-state polling, the 24h update check.
        //
        // The 60ms arm is *not* gated on `has_ptys` alone: an unfocused window
        // with quiet PTYs has nothing to repaint, and paying ~16fps forever for
        // it was the single largest idle cost. It still runs unfocused while a
        // PTY is actually producing output (`dirty`), so background agents keep
        // streaming and classifying at full rate.
        let has_ptys = !self.app.sessions.is_empty()
            || !self.app.home_terminals.is_empty()
            || self.app.wt_terminals.values().any(|v| !v.is_empty());
        let busy = self.pty_drag.is_some()
            || self.app.teardown.is_some()
            || matches!(self.upgrade, UpgradeState::Updating(_));
        let animating = self.anim.attention_anim.value()
            || self.anim.grid_slide.is_some()
            || self
                .anim
                .onb_step_anim
                .is_animating(std::time::Instant::now());
        let fast = busy
            || (has_ptys && (self.window_focused || animating || self.any_pty_dirty()))
            || self.wt_rebuild_pending;
        if fast {
            subs.push(iced::time::every(Duration::from_millis(60)).map(|_| Msg::Tick));
        } else {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Msg::Tick));
        }
        // Frame-rate redraw trick shared by every short-lived animation: the
        // needs-attention pulse (while active), the tile-slide reorder
        // animation (~150ms window after a grid swap), and the onboarding
        // wizard's step-transition entrance. A single subscription covers all
        // three so an idle app carries zero frame-rate cost.
        if animating {
            subs.push(iced::window::frames().map(|_| Msg::AnimationFrame));
        }
        subs.push(iced::window::close_requests().map(Msg::CloseRequested));
        // Always-on: drives "system" theme mode, whether or not it's active,
        // so toggling it on later doesn't need a fresh OS notification first.
        subs.push(iced::system::theme_changes().map(Msg::SystemThemeChanged));
        // While the divider is held, listen globally for cursor motion and the
        // button-release — the 1px handle can't drive `mouse_area::on_move` once
        // the cursor leaves its bounds, so the drag is tracked at the app level.
        if self.drag.sidebar_drag.is_some() {
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
        if self.drag.term_panel_dragging {
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
        if self.drag.grid_drag.is_some() {
            let drag = event::listen_with(|ev, _status, _| match ev {
                Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Msg::GridDrag(GridDragMsg::End))
                }
                _ => None,
            });
            subs.push(drag);
        }
        Subscription::batch(subs)
    }

    /// Whether any PTY has produced output that the render cache has not
    /// picked up yet. Gates the 60ms tick for an unfocused window: quiet PTYs
    /// have nothing to repaint, a noisy one still needs full-rate streaming.
    /// Read-only — the flag is consumed (and cleared) by `pty()`.
    fn any_pty_dirty(&self) -> bool {
        fn dirty(s: &Session) -> bool {
            s.dirty.load(Ordering::Relaxed)
        }
        self.app.sessions.iter().any(dirty)
            || self.app.home_terminals.iter().any(dirty)
            || self.app.wt_terminals.values().any(|v| v.iter().any(dirty))
    }

    /// Surface a failed settings write. Without this the toggle flips in the
    /// UI, never reaches disk, and silently reverts on the next start.
    fn report_setting_save(&mut self, res: anyhow::Result<()>) {
        if let Err(e) = res {
            self.app
                .set_error_toast(format!("failed to save setting: {e}"));
        }
    }

    pub(in crate::gui) fn on_new_home_terminal(&mut self) {
        self.app
            .new_home_terminal(self.pty_layout.rows, self.pty_layout.cols);
        self.pty_selection = None;
        self.invalidate_pty_render_cache();
    }

    pub(in crate::gui) fn on_request_close_home_terminal(&mut self, i: usize) {
        self.pending_kill_terminal = Some(i);
    }

    pub(in crate::gui) fn on_request_kill_session(&mut self, i: usize) {
        self.open_agent_menu = None;
        self.pending_kill = Some(i);
    }

    pub(in crate::gui) fn on_set_backend_tmux(&mut self, enabled: bool) {
        let res = self.app.set_tmux_enabled(enabled);
        self.report_setting_save(res);
    }

    pub(in crate::gui) fn on_set_skip_permissions(&mut self, enabled: bool) {
        let res = self.app.set_skip_permissions_enabled(enabled);
        self.report_setting_save(res);
    }

    pub(in crate::gui) fn on_telemetry_toggle(&mut self, v: bool) {
        let res = self.app.set_telemetry_enabled(v);
        self.report_setting_save(res);
    }

    pub(in crate::gui) fn on_project_themes_toggle(&mut self, v: bool) {
        let res = self.app.set_project_themes_enabled(v);
        self.report_setting_save(res);
        // Every open PTY's baked-in colors may now need to switch between the
        // global theme and a project override.
        self.invalidate_pty_render_cache();
    }

    pub(in crate::gui) fn on_zoom_in(&mut self) {
        crate::telemetry::track("zoom_changed", vec![]);
        self.adjust_ui_zoom(PTY_ZOOM_STEP);
    }

    pub(in crate::gui) fn on_zoom_out(&mut self) {
        crate::telemetry::track("zoom_changed", vec![]);
        self.adjust_ui_zoom(-PTY_ZOOM_STEP);
    }

    pub(in crate::gui) fn on_zoom_reset(&mut self) {
        crate::telemetry::track("zoom_changed", vec![]);
        self.set_ui_zoom(PTY_ZOOM_DEFAULT);
    }

    pub(in crate::gui) fn on_open_settings(&mut self) -> Task<Msg> {
        crate::telemetry::track("settings_opened", vec![]);
        self.app.open_settings();
        self.detect_tools_task()
    }

    pub(in crate::gui) fn on_open_shortcut_overlay(&mut self) {
        self.set_modal(Modal::ShortcutOverlay);
    }

    pub(in crate::gui) fn on_set_default_agent(&mut self, agent: Agent) {
        if let Err(e) = self.app.set_default_agent(agent) {
            self.app.set_error_toast(e.to_string());
        }
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Tick => return self.on_tick(),
            // No state to mutate for the attention pulse — the message exists
            // to trigger a redraw so it can interpolate against a fresh
            // Instant. The tile-slide animation, if active, self-clears once
            // its 150ms window elapses so its frame subscription stops.
            Msg::AnimationFrame => self.on_animation_frame(),
            Msg::WindowFocusChanged(f) => return self.on_window_focus_changed(f),
            Msg::RestartHomeTerminal => {
                self.app
                    .restart_active_terminal(self.pty_layout.rows, self.pty_layout.cols);
                self.invalidate_pty_render_cache();
            }
            Msg::NewHomeTerminal => self.on_new_home_terminal(),
            Msg::SelectHomeTerminal(i) => self.on_select_home_terminal(i),
            Msg::RequestCloseHomeTerminal(i) => self.on_request_close_home_terminal(i),
            Msg::CloseHomeTerminal(i) => self.on_close_home_terminal(i),
            Msg::ToggleTerminalsSection => {
                self.terminals_collapsed = !self.terminals_collapsed;
            }
            Msg::WindowResized(size) => self.on_window_resized(size),
            Msg::WtCacheRebuilt(lists) => self.on_wt_cache_rebuilt(lists),
            Msg::CloseRequested(id) => return self.on_close_requested(id),
            Msg::BackendNative => self.on_set_backend_tmux(false),
            Msg::BackendTmux => self.on_set_backend_tmux(true),
            Msg::SkipPermissionsEnable => self.on_set_skip_permissions(true),
            Msg::SkipPermissionsDisable => self.on_set_skip_permissions(false),
            Msg::TelemetryToggle(v) => self.on_telemetry_toggle(v),
            Msg::ProjectThemesToggle(v) => self.on_project_themes_toggle(v),
            Msg::ChooseTmux(enabled) => self.choose_tmux(enabled),
            Msg::AgentPickerSelect(i) => self.agent_picker_select(i),
            Msg::AgentPickerToggleDefault => self.agent_picker_toggle_default(),
            Msg::AgentPickerSubmit => self.submit_agent_picker(),
            Msg::ToggleCollapseAll => self.on_toggle_collapse_all(),
            Msg::ProjectClicked(i) => self.on_project_clicked(i),
            Msg::WorktreeClicked { proj, wt } => self.on_worktree_clicked(proj, wt),
            Msg::HoverWorktree(target) => {
                self.hovered_wt = target;
            }
            Msg::StartSession { proj, wt, agent } => self.on_start_session(proj, wt, agent),
            Msg::StartTerminal { proj, wt } => self.on_start_terminal(proj, wt),
            Msg::ToggleTermPanel => return self.on_toggle_term_panel(),
            Msg::NewWtTerminal => self.on_new_wt_terminal(),
            Msg::SelectWtTerminal(i) => self.on_select_wt_terminal(i),
            Msg::CloseWtTerminal(i) => self.on_close_wt_terminal(i),
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
            Msg::JumpToWaitingSession => return self.on_jump_to_waiting_session(),
            Msg::SelectSession(i) => self.on_select_session(i),
            Msg::RequestKillSession(i) => self.on_request_kill_session(i),
            Msg::KillSession(i) => self.on_kill_session(i),
            Msg::ModifiersChanged(mods) => {
                self.live_mods = mods;
            }
            Msg::KeyPress(key, modified_key, mods) => {
                return self.on_key_press(key, modified_key, mods)
            }
            Msg::FileDropped(path) => return self.on_file_dropped(path),
            Msg::PtyMouseDown(pane, x, y) => return self.on_pty_mouse_down(pane, x, y),
            Msg::PtyMouseDrag(pane, x, y) => return self.on_pty_mouse_drag(pane, x, y),
            Msg::PtyScroll { pane, up, x, y } => return self.on_pty_scroll(pane, up, x, y),
            Msg::ToggleZen => return self.on_toggle_zen(),
            Msg::ZoomIn => self.on_zoom_in(),
            Msg::ZoomOut => self.on_zoom_out(),
            Msg::ZoomReset => self.on_zoom_reset(),
            Msg::PtyMouseUp => return self.on_pty_mouse_up(),
            Msg::SidebarDragStart => self.on_sidebar_drag_start(),
            Msg::SidebarDragMove(cursor_x) => self.on_sidebar_drag_move(cursor_x),
            Msg::SidebarDragEnd => self.on_sidebar_drag_end(),
            Msg::TermPanelDragStart => self.on_term_panel_drag_start(),
            Msg::TermPanelDragMove(cursor_x) => self.on_term_panel_drag_move(cursor_x),
            Msg::TermPanelDragEnd => self.on_term_panel_drag_end(),
            Msg::AddProject(msg) => return self.on_add_project(msg),
            Msg::AddWorktree { proj } => return self.on_add_worktree(proj),
            Msg::DeleteWorktree { proj, wt } => self.on_delete_worktree(proj, wt),
            Msg::RemoveProject { proj } => self.on_remove_project(proj),
            Msg::RunScript { proj, wt } => self.on_run_script(proj, wt),
            Msg::Scripts(msg) => return self.on_scripts(msg),
            Msg::ToggleRemoveWorktrees(v) => self.on_toggle_remove_worktrees(v),
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
            Msg::ModalConfirm(yes) => return self.on_modal_confirm(yes),
            Msg::AddProjectBrowse => return self.on_add_project_browse(),
            Msg::AddProjectPicked(picked) => return self.on_add_project_picked(picked),
            Msg::OpenSettings => return self.on_open_settings(),
            Msg::OpenShortcutOverlay => self.on_open_shortcut_overlay(),
            Msg::RefreshTools => return self.detect_tools_task(),
            Msg::SetDefaultAgent(agent) => self.on_set_default_agent(agent),
            Msg::ToolVersionsDetected(results) => {
                self.settings_tools = results.into_iter().map(|(_, status)| status).collect();
            }
            Msg::Upgrade(msg) => return self.on_upgrade(msg),
            Msg::ThemePicker(msg) => return self.on_theme_picker(msg),
            Msg::SystemThemeChanged(mode) => self.on_system_theme_changed(mode),
            Msg::Onboarding(msg) => return self.on_onboarding(msg),
            Msg::ToggleGridView => self.on_toggle_grid_view(),
            Msg::GridDrag(msg) => return self.on_grid_drag(msg),
            Msg::GridTileZen(si) => self.on_grid_tile_zen(si),
            Msg::SessionLauncher(msg) => return self.on_session_launcher(msg),
            Msg::ThemeManager(msg) => return self.on_theme_manager(msg),
        }
        Task::none()
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
        self.anim
            .attention_anim
            .interpolate(0.0, 1.0, std::time::Instant::now())
    }

    /// Session indices currently waiting for input, in tree/on-screen order —
    /// the "attention queue". Drives the appbar pill/dropdown, the zen pill,
    /// and `mod+'`.
    pub(crate) fn waiting_sessions(&self) -> Vec<usize> {
        self.visible_session_order()
            .into_iter()
            .filter(|&si| {
                self.app.sessions.get(si).is_some_and(|s| {
                    matches!(
                        self.activity_state(s),
                        super::activity::ActivityState::WaitingForInput
                    )
                })
            })
            .collect()
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
            self.set_modal(Modal::Message(format!("Default agent failed: {e}")));
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
                self.set_modal(Modal::Settings);
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
                    ScrollAmount::All => grove_core::session::SCROLLBACK_LINES,
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
            GlobalShortcut::NewSession => self.on_session_launcher(session_launcher::Msg::Open),
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
            GlobalShortcut::Settings => self.on_open_settings(),
            GlobalShortcut::ToggleZen => self.on_toggle_zen(),
            GlobalShortcut::ToggleGrid => {
                self.on_toggle_grid_view();
                Task::none()
            }
            GlobalShortcut::ZoomIn => {
                self.on_zoom_in();
                Task::none()
            }
            GlobalShortcut::ZoomOut => {
                self.on_zoom_out();
                Task::none()
            }
            GlobalShortcut::ZoomReset => {
                self.on_zoom_reset();
                Task::none()
            }
            GlobalShortcut::NextSession => self.cycle_session(1),
            GlobalShortcut::PrevSession => self.cycle_session(-1),
            GlobalShortcut::SelectSession(n) => self.select_visible_session(n),
            GlobalShortcut::ShortcutOverlay => {
                self.on_open_shortcut_overlay();
                Task::none()
            }
            GlobalShortcut::CloseFocusedSession => {
                // A focused home terminal takes priority, and goes through
                // the same two-step confirm-to-kill flow as an agent session
                // (`pending_kill_terminal` mirrors `pending_kill`).
                if self.terminal_focused {
                    return match close_focused_session_decision(
                        self.app.active_terminal,
                        self.pending_kill_terminal,
                    ) {
                        CloseFocusedDecision::Kill(idx) => {
                            self.on_close_home_terminal(idx);
                            Task::none()
                        }
                        CloseFocusedDecision::Request(idx) => {
                            self.on_request_close_home_terminal(idx);
                            Task::none()
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
                    CloseFocusedDecision::Kill(si) => {
                        self.on_kill_session(si);
                        Task::none()
                    }
                    CloseFocusedDecision::Request(si) => {
                        self.on_request_kill_session(si);
                        Task::none()
                    }
                    CloseFocusedDecision::NoOp => Task::none(),
                }
            }
            GlobalShortcut::NewHomeTerminal => {
                self.on_new_home_terminal();
                self.terminal_focused = true;
                Task::none()
            }
            GlobalShortcut::JumpToWaitingSession => self.on_jump_to_waiting_session(),
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
    pub(super) fn set_grid_focus(&mut self, focus: Option<usize>) {
        if self.grid_focused != focus {
            self.pty_selection = None;
        }
        self.grid_focused = focus;
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
    pub(super) fn leave_terminal_tab(&mut self) {
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

    fn worktrees_for_project(&self, proj: usize) -> &[grove_core::git::Worktree] {
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
                match grove_core::git::worktree_git_state(&path) {
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
            let wts = grove_core::git::list_worktrees(&p.path);
            self.wt_cache.insert(proj, wts);
        }
    }

    /// Invalidate the per-project worktree cache and request a fresh sweep.
    ///
    /// Only the *active* project's list is refreshed inline (a single `git
    /// worktree list`, and it is what the tree renders immediately); the
    /// remaining projects are swept off the UI thread — see
    /// `maybe_rebuild_wt_cache`, kicked from the next `Msg::Tick`. Until the
    /// result lands, inactive projects render with no worktrees, exactly as
    /// they already do on a cold cache.
    pub(super) fn rebuild_wt_cache(&mut self) {
        self.wt_cache.clear();
        let n = self.app.store.projects.len();
        if self.app.proj_idx >= n {
            self.app.proj_idx = n.saturating_sub(1);
        }
        self.app.refresh_worktrees();
        self.wt_rebuild_pending = true;
    }

    /// Kick off the off-thread `git worktree list` sweep requested by
    /// `rebuild_wt_cache`, unless one is already in flight. Mirrors the
    /// git-status poll's in-flight guard so requests never overlap.
    pub(super) fn maybe_rebuild_wt_cache(&mut self) -> Task<Msg> {
        if !self.wt_rebuild_pending || self.wt_rebuild_inflight {
            return Task::none();
        }
        self.wt_rebuild_pending = false;
        self.wt_rebuild_inflight = true;
        let paths: Vec<String> = self
            .app
            .store
            .projects
            .iter()
            .map(|p| p.path.clone())
            .collect();
        Task::perform(
            // `list_worktrees_many` fans the subprocesses out itself; running
            // it inside the async block keeps it off the UI thread, the same
            // shape `remove_worktree_task` uses.
            async move { grove_core::git::list_worktrees_many(&paths) },
            Msg::WtCacheRebuilt,
        )
    }

    /// Fold a finished worktree sweep into `wt_cache`. A sweep whose length no
    /// longer matches the project list raced a project add/remove — its
    /// indices are meaningless, so it is dropped and the rebuild that caused
    /// the change re-requests a fresh one.
    fn on_wt_cache_rebuilt(&mut self, lists: Vec<Vec<grove_core::git::Worktree>>) {
        self.wt_rebuild_inflight = false;
        if lists.len() != self.app.store.projects.len() {
            return;
        }
        for (i, wts) in lists.into_iter().enumerate() {
            if i != self.app.proj_idx {
                self.wt_cache.insert(i, wts);
            }
        }
    }

    /// Run strip action `idx` (⏎ or click). `StartUpdate`'s handler replaces
    /// the palette with the Updating progress modal on its own; `SkipVersion`
    /// flips `upgrade` out of `Available`, so the strip closes with it (the
    /// row's value slot re-derives to "Up to date"); `CopyUrl` is a pure side
    /// effect — the strip stays open for a follow-up action.
    pub(super) fn update_actions_commit(&mut self, idx: usize) -> Task<Msg> {
        let method_unknown = matches!(
            self.upgrade_method,
            grove_core::upgrade::InstallMethod::Unknown
        );
        let Some(&action) = update_available_actions(method_unknown).get(idx) else {
            return Task::none();
        };
        match action {
            UpdateAction::UpdateNow => self.on_start_update(),
            UpdateAction::SkipVersion => {
                self.on_skip_version();
                self.close_update_actions_strip();
                Task::none()
            }
            UpdateAction::CopyUrl => {
                self.on_copy_release_url();
                Task::none()
            }
        }
    }

    // ── Modal::ThemeManager LIST sub-view ───────────────────────────────────

    // ── Palette Theme pane bridge into the editor ───────────────────────────

    /// Extract text inside the current PTY selection. The selection is stored
    /// in scrollback-stable absolute rows, so this may span content that is not
    /// currently visible — extraction walks the session's scrollback to read it.
    /// Whether the workspace is currently focused on a home terminal rather
    /// than the active agent session.
    pub(super) fn terminal_tab(&self) -> bool {
        self.terminal_focused
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
            let res = grove_core::git::remove_worktree(&project_path, &path);
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
