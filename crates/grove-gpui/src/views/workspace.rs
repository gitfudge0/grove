//! The root view: the sidebar rail, the divider, and the body showing whatever
//! `WorkspaceState` says is active.
//!
//! Plans 06-07 replace the remaining placeholders (appbar, statusbar, grid,
//! zen). Every dimension comes from a named constant carrying its
//! `src/gui/metrics.rs` line.

use std::collections::HashMap;

use gpui::{div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Window};

use crate::activity::ActivityState;
use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::{SessionId, SessionRegistry};
use crate::entities::terminal_session::TerminalSession;
use crate::entities::toast::ToastState;
use crate::entities::workspace_state::{
    term_portion_for_cursor, LiveTile, PtyPane, WorkspaceState,
};
use crate::keymap;
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::appbar::{self, AppbarCtx, ChromeAction, WaitingRow};
use crate::views::grid::{self, GridAction, GridCtx, TileData};
use crate::views::modals::{ModalEvent, ModalLayer};
use crate::views::rows;
use crate::views::session_header::{self, SessionHeaderData, ToolAction, ToolCluster};
use crate::views::sidebar::{self, Sidebar};
use crate::views::statusbar::{self, StatusbarCtx};
use crate::views::term_panel::{self, PanelAction, PanelCtx, ShellTab};
use crate::views::terminal_tab::{self, TerminalTabAction, TerminalTabCtx};
use crate::views::terminal_view::TerminalView;
use crate::zoom::{self, ZoomState};

pub struct Workspace {
    focus: FocusHandle,
    /// Kept alive here: dropping the clock entity would stop every animation
    /// in the window, including the terminal cursor blink.
    clock: Entity<AnimationClock>,
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
    tree: Entity<ProjectTree>,
    activity: Entity<ActivityStore>,
    toast: Entity<ToastState>,
    sidebar: Entity<Sidebar>,
    /// The single modal slot, rendered above everything (Plan 08 Task 2).
    modals: Entity<ModalLayer>,
    /// One view per session, cached by id so switching does not respawn
    /// anything (Task 6 Step 2).
    views: HashMap<SessionId, Entity<TerminalView>>,
    home_views: HashMap<SessionId, Entity<TerminalView>>,
    /// One view per **panel shell**, same memoization as the agent views.
    panel_views: HashMap<SessionId, Entity<TerminalView>>,
    /// The split divider's drag state, and the previous press for the 350ms
    /// double-click reset (`layout.rs:162-197`).
    term_panel_dragging: bool,
    last_term_divider_press: Option<std::time::Instant>,
    /// The window's logical width, refreshed each frame — the divider drag maps
    /// a cursor x against it.
    logical_win_w: f32,
    /// The terminal takes focus on the first frame so keystrokes land without
    /// a click; `window.focus` needs a `&mut Window`, which `new` has not got.
    focused_once: bool,
    /// `observe_window_activation` needs a `&mut Window`, which `new` has not
    /// got — registered on the first frame instead.
    activation_observed: bool,
    observers: Vec<gpui::Subscription>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let clock = cx.new(AnimationClock::new);
        let state = cx.new(|cx| WorkspaceState::new(&cx.global::<SettingsState>().store, 1280.0));
        let registry = cx.new(|_| SessionRegistry::new());
        let tree = cx.new(|_| ProjectTree::new());
        let activity = cx.new({
            let (state, registry) = (state.clone(), registry.clone());
            |cx| ActivityStore::start(state, registry, cx)
        });

        // Seed the active project's worktrees so the rail has something to draw
        // on the first frame (`App::refresh_worktrees`).
        let active_path = cx
            .global::<SettingsState>()
            .store
            .active_projects()
            .next()
            .map(|(_, p)| p.path.clone());
        if let Some(path) = active_path {
            tree.update(cx, |t, _| {
                t.set_active_worktrees(grove_core::git::list_worktrees(&path));
            });
        }

        let toast = cx.new(|_| ToastState::new());

        let sidebar = cx.new({
            let (state, tree, registry, activity, clock) = (
                state.clone(),
                tree.clone(),
                registry.clone(),
                activity.clone(),
                clock.clone(),
            );
            |cx| Sidebar::new(state, tree, registry, activity, clock, cx)
        });

        let modals = cx.new({
            let (state, registry, tree, toast, activity, clock) = (
                state.clone(),
                registry.clone(),
                tree.clone(),
                toast.clone(),
                activity.clone(),
                clock.clone(),
            );
            |cx| ModalLayer::new(state, registry, tree, toast, activity, clock, cx)
        });
        sidebar.update(cx, |s, _| s.set_modals(modals.clone(), toast.clone()));

        let observers = vec![
            // The clock drives the cursor blink inside the terminal; the
            // workspace repaints with it so chrome animations stay in phase.
            cx.observe(&clock, |_, _, cx| cx.notify()),
            // Selection changes repaint the body (Task 6 Step 2).
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            // The 480ms pass repaints the chrome that reads it.
            cx.observe(&activity, |_, _, cx| cx.notify()),
            // The toast's own TTL task clears it; the statusbar repaints with it.
            cx.observe(&toast, |_, _, cx| cx.notify()),
            // The modal layer repaints the window and hands back the effects
            // it cannot perform itself.
            cx.observe(&modals, |_, _, cx| cx.notify()),
            cx.subscribe(&modals, Self::on_modal_event),
        ];

        Self {
            focus: cx.focus_handle(),
            clock,
            state,
            registry,
            tree,
            activity,
            toast,
            sidebar,
            modals,
            views: HashMap::new(),
            home_views: HashMap::new(),
            panel_views: HashMap::new(),
            term_panel_dragging: false,
            last_term_divider_press: None,
            logical_win_w: 1280.0,
            focused_once: false,
            activation_observed: false,
            observers,
        }
    }

    /// Applies a new zoom level: state, the debounced persist, repaint.
    pub(crate) fn set_zoom(zoom_value: f32, cx: &mut App) {
        let snapped = zoom::snap(zoom_value);
        if cx.global::<ZoomState>().zoom == snapped {
            return;
        }
        cx.global_mut::<ZoomState>().zoom = snapped;
        SettingsState::update(cx, |s| s.ui_zoom = Some(snapped));
        cx.refresh_windows();
    }

    fn zoom_in(_: &keymap::ZoomIn, _: &mut Window, cx: &mut App) {
        Self::set_zoom(cx.global::<ZoomState>().zoom + zoom::ZOOM_STEP, cx);
    }

    fn zoom_out(_: &keymap::ZoomOut, _: &mut Window, cx: &mut App) {
        Self::set_zoom(cx.global::<ZoomState>().zoom - zoom::ZOOM_STEP, cx);
    }

    fn zoom_reset(_: &keymap::ZoomReset, _: &mut Window, cx: &mut App) {
        Self::set_zoom(zoom::ZOOM_DEFAULT, cx);
    }

    // ── data-carrying actions (Task 6 Step 1) ───────────────────────────

    fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
        let active_proj = self.state.read(cx).proj_idx();
        let registry = self.registry.clone();
        self.tree.clone().update(cx, |tree, cx| {
            let store = &cx.global::<SettingsState>().store;
            tree.snapshot(store, registry.read(cx), active_proj)
        })
    }

    /// The sidebar's own row list is the index space, so keyboard selection and
    /// what is on screen cannot disagree.
    fn visible_order(&self, cx: &App) -> Vec<SessionId> {
        self.sidebar.read(cx).visible_session_order()
    }

    // ── the grid's world (Plan 07 Task 3 Step 4) ────────────────────────

    /// Every live agent session with its stable cross-restart key — the input
    /// `WorkspaceState`'s grid transitions reconcile against.
    fn live_tiles(&self, cx: &App) -> Vec<LiveTile> {
        self.registry
            .read(cx)
            .all()
            .iter()
            .map(|m| LiveTile {
                id: m.id,
                key: crate::grid::session_grid_key(&m.project, &m.wt_path),
            })
            .collect()
    }

    /// The persisted arrangement (`Store::grid_order`).
    fn saved_grid_order(cx: &App) -> Vec<String> {
        cx.global::<SettingsState>().store.grid_order.clone()
    }

    /// Drains whatever the last transition staged and writes it to
    /// `Store::grid_order`, mapped back through each tile's stable key
    /// (`persist_grid_order`, `layout.rs:481-489`).
    fn persist_grid_order(&mut self, cx: &mut Context<Self>) {
        let Some(order) = self.state.update(cx, |s, _| s.take_grid_order_to_persist()) else {
            return;
        };
        let registry = self.registry.read(cx);
        let keys: Vec<String> = order
            .iter()
            .filter_map(|&id| registry.meta(id))
            .map(|m| crate::grid::session_grid_key(&m.project, &m.wt_path))
            .collect();
        SettingsState::update(cx, |s| s.grid_order = keys);
    }

    /// `mod+g` (`layout.rs:199-216`).
    fn toggle_grid(&mut self, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.toggle_grid(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    /// `mod+enter` (`layout.rs:63-103`).
    fn toggle_zen(&mut self, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.toggle_zen(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    /// `mod+t` (`update/mod.rs:472-500`). The transition reports the spawn; the
    /// spawn itself is the view's, exactly as `on_new_home_terminal` is there.
    fn toggle_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        let has_home = self.registry.read(cx).home_terminal_count() > 0;
        let outcome = self.state.update(cx, |s, cx| {
            let outcome = s.toggle_terminal_tab(has_home, &live, &saved);
            cx.notify();
            outcome
        });
        self.persist_grid_order(cx);
        if outcome.spawn_home_terminal {
            self.sidebar
                .clone()
                .update(cx, Sidebar::spawn_home_terminal);
        }
    }

    /// The directional grid chords (`update/mod.rs:1071-1116`).
    fn grid_move(&mut self, dx: i32, dy: i32, swap: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            if swap {
                s.grid_swap(dx, dy);
            } else {
                s.grid_move(dx, dy);
            }
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    /// `mod+N` selects the Nth **visible** session; out of range is a no-op,
    /// not a clamp (`src/gui/update/sessions.rs:394-407`). Inside the grid the
    /// index space is `tile_order`, so the number the user sees on the tile is
    /// the tile they get (`sessions.rs:396-405`).
    fn select_session(
        &mut self,
        action: &keymap::SelectSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = action.index.saturating_sub(1);
        if self.state.read(cx).grid_view() {
            self.state.update(cx, |s, cx| {
                s.select_tile_by_index(n);
                cx.notify();
            });
            return;
        }
        let Some(&id) = self.visible_order(cx).get(n) else {
            return;
        };
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
        self.reanchor_panel(window, cx);
    }

    /// `sessions.rs:365-405`.
    fn cycle(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.visible_order(cx);
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.cycle_session(next, &order, &snap);
            cx.notify();
        });
        self.reanchor_panel(window, cx);
    }

    /// The first waiting session in visible order (`update/mod.rs:728-739`),
    /// snapped to the live screen **before** it is selected — deliberately
    /// unlike a manual `mod+j/k` switch (`sessions.rs:210-223`). Selecting
    /// acknowledges, and Plan 07's dropdown closes off the same transition
    /// (`:229`).
    fn jump_to_waiting(&mut self, cx: &mut Context<Self>) {
        let waiting = self.activity.read(cx).waiting_sessions().first().copied();
        let Some(id) = waiting.or_else(|| {
            let activity = self.activity.read(cx);
            self.visible_order(cx)
                .into_iter()
                .find(|&id| activity.state_of(id) == ActivityState::WaitingForInput)
        }) else {
            return;
        };
        if let Some(session) = self.registry.read(cx).session(id).cloned() {
            session.update(cx, |s, cx| {
                s.snap_to_bottom();
                cx.notify();
            });
        }
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    // ── window chrome (Tasks 5 & 6) ─────────────────────────────────────

    /// The single place an appbar/statusbar click becomes a state change.
    /// Everything Plan 07/08 owns logs a stub naming its plan.
    fn chrome(&mut self, action: ChromeAction, cx: &mut Context<Self>) {
        match action {
            ChromeAction::ToggleAttentionQueue => self.state.update(cx, |s, cx| {
                s.toggle_attention_queue();
                cx.notify();
            }),
            ChromeAction::CloseAttentionQueue => self.state.update(cx, |s, cx| {
                s.close_attention_queue();
                cx.notify();
            }),
            ChromeAction::SelectWaiting(id) => self.select_waiting(id, cx),
            // The zen pill is not a dropdown: it jumps straight to the first
            // waiting session (`appbar.rs:277`).
            ChromeAction::JumpToWaiting => self.jump_to_waiting(cx),
            ChromeAction::ToggleGridView => self.toggle_grid(cx),
            ChromeAction::OpenSessionLauncher => {
                self.open_modal(crate::modal::Modal::SessionLauncher(Box::default()), cx);
            }
            ChromeAction::OpenSettings => self.open_modal(crate::modal::Modal::Settings, cx),
            ChromeAction::OpenShortcutOverlay => {
                self.open_modal(crate::modal::Modal::ShortcutOverlay, cx);
            }
        }
    }

    // ── the grid's clicks (Task 4 Steps 2-6) ────────────────────────────

    fn grid_action(&mut self, action: GridAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            GridAction::Press(idx) => {
                let id = self.state.read(cx).tile_order().get(idx).copied();
                self.state.update(cx, |s, cx| {
                    s.grid_drag_start(idx);
                    cx.notify();
                });
                // `grid_focused` and the focused handle must never disagree
                // (carried amendment 7): focusing a tile focuses its view.
                if let Some(view) = id.and_then(|id| self.views.get(&id)).cloned() {
                    let handle = view.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            GridAction::Hover(idx) => self.state.update(cx, |s, cx| {
                s.grid_drag_hover(idx);
                cx.notify();
            }),
            GridAction::TileZen(id) => {
                self.state.update(cx, |s, cx| {
                    s.tile_zen(id);
                    cx.notify();
                });
                self.persist_grid_order(cx);
            }
            GridAction::RequestKill(id) => self.state.update(cx, |s, cx| {
                s.arm_kill(id);
                cx.notify();
            }),
            GridAction::Kill(id) => self.kill_session(id, cx),
        }
    }

    /// The kill half of the two-step confirm, shared by the tile header and
    /// the session bar. `on_session_removed` + a grid reconcile keep
    /// `tile_order` honest (`layout.rs:276-306`).
    fn kill_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.registry.update(cx, |r, cx| {
            r.remove(id);
            cx.notify();
        });
        self.views.remove(&id);
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.on_session_removed(id);
            s.disarm_kill();
            s.reconcile_after_teardown(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    // ── the session bar's tool cluster (Task 5 Step 4) ──────────────────

    fn tool_action(&mut self, action: ToolAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            // `on_run_script` (`src/gui/update/sessions.rs:147-177`): the run
            // script opens the terminal panel for the active worktree.
            ToolAction::RunScript => self.toggle_term_panel(window, cx),
            ToolAction::ToggleTermPanel => self.toggle_term_panel(window, cx),
            ToolAction::ToggleZen => self.toggle_zen(cx),
            ToolAction::RequestKill => {
                let Some(id) = self.state.read(cx).active_session() else {
                    return;
                };
                self.state.update(cx, |s, cx| {
                    s.arm_kill(id);
                    cx.notify();
                });
            }
            ToolAction::Kill => {
                let Some(id) = self.state.read(cx).active_session() else {
                    return;
                };
                self.kill_session(id, cx);
            }
        }
    }

    // ── the home-terminal tab (Task 5 Step 3) ───────────────────────────

    fn tab_action(&mut self, action: TerminalTabAction, cx: &mut Context<Self>) {
        match action {
            TerminalTabAction::ToggleZen => self.toggle_zen(cx),
            TerminalTabAction::Restart => self.restart_home_terminal(cx),
        }
    }

    /// Replace the active home terminal's shell in place, keeping its slot and
    /// label, and **only once the replacement is live** — a failed spawn toasts
    /// and leaves the (usually exited) shell where it was
    /// (`src/app/terminals.rs:38-53,95-108`).
    fn restart_home_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.state.read(cx).active_terminal() else {
            return;
        };
        let Some(meta) = self.registry.read(cx).home_terminals().get(i).cloned() else {
            return;
        };
        let target = crate::entities::session_registry::SpawnTarget::home(meta.label.clone());
        let session = cx.new(|cx| TerminalSession::spawn(&target, &[], None, cx));
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
            return;
        }
        let old = self.registry.update(cx, |r, cx| {
            let old = r.replace_home(i, session);
            cx.notify();
            old
        });
        if old.is_some() {
            // Dropping the old entity ends its PTY; its cached view must go
            // with it or the tab would keep rendering the dead shell.
            self.home_views.remove(&meta.id);
        }
        drop(old);
    }

    // ── the worktree panel (Task 6) ─────────────────────────────────────

    /// The active session's worktree — the panel's scope
    /// (`pty_input.rs:220-226`).
    fn active_wt_path(&self, cx: &App) -> Option<String> {
        let id = self.state.read(cx).active_session()?;
        self.registry.read(cx).meta(id).map(|m| m.wt_path.clone())
    }

    fn toggle_term_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wt = self.active_wt_path(cx);
        let opened = self.state.update(cx, |s, cx| {
            let opened = s.toggle_term_panel(wt.is_some());
            cx.notify();
            opened
        });
        if !opened {
            return;
        }
        // `ensure_wt_terminal` (`src/app/terminals.rs:133-149`): the panel
        // spawns its first shell on demand.
        if let Some(wt) = wt {
            if self.registry.read(cx).wt_shells_need_spawn(&wt) {
                self.spawn_wt_shell(&wt, cx);
            }
        }
        // Focusing the just-opened panel is the natural default — that is why
        // the user opened it (`sessions.rs:80-84`). With no shell to focus,
        // focus stays on the agent, which is the `pty_input.rs:170-178`
        // fallback made literal.
        self.focus_panel(window, cx);
    }

    /// Move the gpui focus onto the panel's active shell, if there is one.
    fn focus_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel_view(cx) else {
            return;
        };
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The active session changed, so the panel re-anchors to a new worktree:
    /// `reset_focused_pane` picks the intent (`pty_input.rs:128-137`) and the
    /// matching handle takes the gpui focus. A worktree with no shell falls
    /// back to the agent (`:170-178`).
    fn reanchor_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.read(cx).term_panel_open() {
            return;
        }
        // The newly anchored worktree spawns its first shell on demand, exactly
        // as opening the panel does (`ensure_wt_terminal`).
        if let Some(wt) = self.active_wt_path(cx) {
            if self.registry.read(cx).wt_shells_need_spawn(&wt) {
                self.spawn_wt_shell(&wt, cx);
            }
        }
        self.state.update(cx, |s, cx| {
            s.reset_focused_pane();
            cx.notify();
        });
        if self.panel_view(cx).is_some() {
            self.focus_panel(window, cx);
        } else {
            self.focus_agent(window, cx);
        }
    }

    /// Move the gpui focus back onto the agent side.
    fn focus_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.body_view(cx) else {
            return;
        };
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn panel_action(&mut self, action: PanelAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wt) = self.active_wt_path(cx) else {
            return;
        };
        match action {
            PanelAction::NewShell => {
                self.spawn_wt_shell(&wt, cx);
                self.focus_panel(window, cx);
            }
            PanelAction::SelectShell(i) => {
                self.registry.update(cx, |r, cx| {
                    r.select_wt_shell(&wt, i);
                    cx.notify();
                });
                self.state.update(cx, |s, cx| {
                    s.focus_pane(PtyPane::Panel);
                    cx.notify();
                });
                self.focus_panel(window, cx);
            }
            PanelAction::CloseShell(i) => {
                let removed = self.registry.update(cx, |r, cx| {
                    let removed = r.close_wt_shell(&wt, i);
                    cx.notify();
                    removed
                });
                // Dropping the entity ends its PTY: the reader task and the
                // `PtyHandle` both die with it.
                drop(removed);
                // Whatever filled the closed slot — or the agent, if nothing
                // did (`pty_input.rs:170-178`) — takes the input.
                if self.panel_view(cx).is_some() {
                    self.focus_panel(window, cx);
                } else {
                    self.focus_agent(window, cx);
                }
            }
            PanelAction::Collapse => self.toggle_term_panel(window, cx),
            PanelAction::DividerPress => self.term_divider_press(cx),
        }
    }

    /// A press on the split divider arms a drag — or, within 350ms of the
    /// previous press, resets the portion to its 40% default instead, and does
    /// **not** also start a drag (`layout.rs:162-176`, the same double-click
    /// idiom the sidebar divider uses).
    fn term_divider_press(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let double = self
            .last_term_divider_press
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(350));
        if double {
            self.term_panel_dragging = false;
            self.last_term_divider_press = None;
            self.state.update(cx, |s, cx| {
                s.set_term_panel_portion(crate::entities::workspace_state::TERM_PANEL_PORTION);
                cx.notify();
            });
        } else {
            self.last_term_divider_press = Some(now);
            self.term_panel_dragging = true;
        }
    }

    /// The divider drag's move/release, listened for at the root so the pointer
    /// can leave the 6px zone (`layout.rs:178-197`).
    fn on_root_mouse_move(&mut self, x: f32, cx: &mut Context<Self>) {
        if !self.term_panel_dragging {
            return;
        }
        let (win_w, sidebar_w) = (self.logical_win_w, self.state.read(cx).sidebar_width());
        self.state.update(cx, |s, cx| {
            s.set_term_panel_portion(term_portion_for_cursor(x, win_w, sidebar_w));
            cx.notify();
        });
    }

    fn on_root_mouse_up(&mut self, cx: &mut Context<Self>) {
        self.term_panel_dragging = false;
        self.state.update(cx, |s, cx| {
            s.grid_drag_end();
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    /// Spawn a panel shell rooted at the worktree and focus it. Native, not
    /// tmux-pinned: these are convenience shells (`Agent::Terminal`), so
    /// `attention::prepare` returns `None` and there is nothing to thread down.
    fn spawn_wt_shell(&mut self, wt_path: &str, cx: &mut Context<Self>) {
        let (id, label) = self
            .registry
            .update(cx, |r, _| (r.next_home_id(), r.next_wt_label()));
        let target = crate::entities::session_registry::SpawnTarget {
            cwd: wt_path.to_string(),
            agent: grove_core::agent::Agent::Terminal,
            project: String::new(),
            label: label.clone(),
        };
        let session = cx.new(|cx| TerminalSession::spawn(&target, &[], None, cx));
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
            return;
        }
        let meta = crate::entities::session_registry::SessionMeta {
            id,
            project: String::new(),
            wt_path: wt_path.to_string(),
            agent: grove_core::agent::Agent::Terminal,
            label,
            spawned_at: std::time::Instant::now(),
            attention: None,
        };
        self.registry.update(cx, |r, cx| {
            r.push_wt_shell(wt_path, meta, Some(session));
            cx.notify();
        });
        self.state.update(cx, |s, cx| {
            s.focus_pane(PtyPane::Panel);
            cx.notify();
        });
    }

    /// The panel's active shell view, memoized per shell id exactly as the
    /// agent views are.
    fn panel_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let wt = self.active_wt_path(cx)?;
        let registry = self.registry.read(cx);
        let idx = registry.active_wt_shell_idx(&wt)?;
        let id = registry.wt_shells(&wt).get(idx)?.id;
        let session = registry.wt_shell(&wt, idx)?.clone();
        if let Some(view) = self.panel_views.get(&id) {
            return Some(view.clone());
        }
        let clock = self.clock.clone();
        let view = cx.new(|cx| TerminalView::new(session, None, clock, cx));
        self.panel_views.insert(id, view.clone());
        Some(view)
    }

    /// A dropdown row: snap to the live screen first, then select — which
    /// acknowledges and closes the dropdown (`sessions.rs:210-223,229`).
    fn select_waiting(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.registry.read(cx).session(id).cloned() {
            session.update(cx, |s, cx| {
                s.snap_to_bottom();
                cx.notify();
            });
        }
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    /// The attention queue, resolved **once** per frame and shared by the pill
    /// and the dropdown (Task 4 Step 5).
    fn waiting_rows(&self, cx: &App) -> Vec<WaitingRow> {
        let activity = self.activity.read(cx);
        let registry = self.registry.read(cx);
        activity
            .waiting_sessions()
            .iter()
            .filter_map(|&id| {
                let meta = registry.meta(id)?;
                Some(WaitingRow {
                    id,
                    agent_label: meta.agent.label(),
                    project: meta.project.clone(),
                    wt_path: meta.wt_path.clone(),
                    state: activity.state_of(id),
                })
            })
            .collect()
    }

    /// The header for whatever the body is showing. Parameterized by session so
    /// Plan 07 reuses it per grid tile.
    fn header_data(
        &self,
        snap: &crate::entities::workspace_state::TreeSnapshot,
        cx: &App,
    ) -> Option<SessionHeaderData> {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        let registry = self.registry.read(cx);
        let (meta, entity) = if terminal_focused {
            let i = active_terminal?;
            (
                registry.home_terminals().get(i)?.clone(),
                registry.home_terminal(i)?.clone(),
            )
        } else {
            let id = active_session?;
            (registry.meta(id)?.clone(), registry.session(id)?.clone())
        };
        let title = entity.read(cx).title();
        let context = title.as_deref().and_then(|raw| {
            if terminal_focused {
                rows::terminal_context(raw, &meta.label)
            } else {
                rows::session_context(
                    raw,
                    &rows::path_basename(&meta.wt_path),
                    &meta.label,
                    meta.agent.label(),
                )
            }
        });
        // Branchless sessions (home terminals) find no worktree and skip the
        // segment entirely (`terminal.rs:530-535`).
        let branch = snap
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .find(|w| w.path == meta.wt_path)
            .map_or_else(String::new, |w| w.branch.clone());
        let state = self.activity.read(cx).state_of(meta.id);
        Some(SessionHeaderData {
            label: meta.label.clone(),
            branch,
            context,
            icon_name: meta.agent.icon_name(),
            running: state != ActivityState::Exited,
        })
    }

    /// Arms the two-step confirm on whatever is focused
    /// (`sessions.rs:105-122`). The confirm itself is a second press on the
    /// row's tick; Plan 08 owns the keyboard Escape carve-out.
    fn close_focused(&mut self, cx: &mut Context<Self>) {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        self.state.update(cx, |s, cx| {
            match (terminal_focused, active_terminal, active_session) {
                (true, Some(i), _) => s.arm_kill_terminal(i),
                (false, _, Some(id)) => s.arm_kill(id),
                _ => return,
            }
            cx.notify();
        });
    }

    fn scroll_half_page(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_entity(cx) else {
            return;
        };
        session.update(cx, |s, cx| {
            let lines = s.scroll_page_lines() / 2;
            s.scroll_lines(up, lines);
            cx.notify();
        });
    }

    fn active_session_entity(&self, cx: &App) -> Option<Entity<TerminalSession>> {
        let ws = self.state.read(cx);
        let registry = self.registry.read(cx);
        if ws.terminal_focused() {
            return ws
                .active_terminal()
                .and_then(|i| registry.home_terminal(i))
                .cloned();
        }
        ws.active_session()
            .and_then(|id| registry.session(id))
            .cloned()
    }

    // ── the four screens' bodies (Tasks 4-6) ────────────────────────────

    // ── the modal layer (Plan 08) ───────────────────────────────────────

    /// The single entry point for opening a modal from the workspace. Opening
    /// replaces whatever was open; there is no stack.
    pub(crate) fn open_modal(&mut self, modal: crate::modal::Modal, cx: &mut Context<Self>) {
        self.modals.clone().update(cx, |l, cx| l.open(modal, cx));
    }

    /// Effects the layer cannot perform for itself.
    fn on_modal_event(
        &mut self,
        _layer: Entity<ModalLayer>,
        event: &ModalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModalEvent::Quit => {
                // Plan 09 owns `flush_ui_zoom_save` on every exit path; it
                // hooks here, immediately before the window is removed.
                cx.quit();
            }
            ModalEvent::Closed => cx.notify(),
            ModalEvent::SpawnAgent {
                project,
                wt_path,
                agent,
            } => {
                let snap = self.snapshot(cx);
                let Some(p) = snap.projects.iter().find(|p| &p.name == project) else {
                    return;
                };
                let Some(wt) = p.worktrees.iter().position(|w| &w.path == wt_path) else {
                    return;
                };
                let (proj, agent) = (p.idx, *agent);
                self.sidebar
                    .clone()
                    .update(cx, |s, cx| s.spawn_session(proj, wt, agent, cx));
            }
            ModalEvent::NewHomeTerminal => {
                self.sidebar
                    .clone()
                    .update(cx, Sidebar::spawn_home_terminal);
            }
            ModalEvent::SelectSession(id) => {
                let snap = self.snapshot(cx);
                let id = *id;
                self.state.update(cx, |s, cx| {
                    s.select_session(id, &snap);
                    cx.notify();
                });
            }
            ModalEvent::SelectTerminal(i) => {
                let count = self.registry.read(cx).home_terminal_count();
                let i = *i;
                self.state.update(cx, |s, cx| {
                    s.select_home_terminal(i, count);
                    cx.notify();
                });
            }
            ModalEvent::WorktreeAdded | ModalEvent::TreeInvalidated => {
                let active = {
                    let store = &cx.global::<SettingsState>().store;
                    let idx = self.state.read(cx).proj_idx();
                    store.projects.get(idx).map(|p| p.path.clone())
                };
                self.tree.clone().update(cx, |t, cx| {
                    t.rebuild_wt_cache();
                    if let Some(path) = active {
                        t.set_active_worktrees(grove_core::git::list_worktrees(&path));
                    }
                    cx.notify();
                });
                cx.notify();
            }
        }
    }

    /// A dispatch closure of any action kind, routed back into `self`.
    #[allow(clippy::type_complexity)]
    fn dispatcher<A: 'static>(
        &self,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, A, &mut Window, &mut Context<Self>) + 'static,
    ) -> std::rc::Rc<dyn Fn(A, &mut Window, &mut App)> {
        let weak = cx.entity().downgrade();
        std::rc::Rc::new(move |action, window, cx: &mut App| {
            let _ = weak.update(cx, |this: &mut Self, cx| f(this, action, window, cx));
        })
    }

    /// The tiles, resolved once per frame. Each hosts the **same**
    /// `TerminalView` entity the single-session body would (amendment 7).
    fn tile_data(
        &mut self,
        snap: &crate::entities::workspace_state::TreeSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<TileData> {
        let ws = self.state.read(cx);
        let (order, focused, pending_kill) = (
            ws.tile_order().to_vec(),
            ws.grid_focused(),
            ws.pending_kill(),
        );
        let clock = self.clock.clone();
        let mut out = Vec::with_capacity(order.len());
        for id in order {
            let Some(meta) = self.registry.read(cx).meta(id).cloned() else {
                continue;
            };
            let view = if let Some(view) = self.views.get(&id) {
                view.clone()
            } else {
                let Some(session) = self.registry.read(cx).session(id).cloned() else {
                    continue;
                };
                let project = Some(meta.project.clone());
                let view = cx.new({
                    let clock = clock.clone();
                    |cx| TerminalView::new(session, project, clock, cx)
                });
                self.views.insert(id, view.clone());
                view
            };
            let branch = snap
                .projects
                .iter()
                .flat_map(|p| p.worktrees.iter())
                .find(|w| w.path == meta.wt_path)
                .map_or_else(String::new, |w| w.branch.clone());
            out.push(TileData {
                id,
                agent_label: meta.agent.label(),
                icon_name: meta.agent.icon_name(),
                project: meta.project.clone(),
                branch,
                waiting: self.activity.read(cx).state_of(id) == ActivityState::WaitingForInput,
                focused: focused == Some(id),
                confirming_kill: pending_kill == Some(id),
                view,
            });
        }
        out
    }

    /// The session column: its bar atop its PTY, split with the worktree panel
    /// when that is open (`terminal.rs:181-229`). **Never** reached in grid
    /// view — `workspace()` returns `grid_workspace()` first (`:182-184`).
    fn session_body(
        &mut self,
        header: Option<SessionHeaderData>,
        tick: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tool_dispatch = self.dispatcher(cx, |this, action: ToolAction, window, cx| {
            this.tool_action(action, window, cx);
        });
        let ws = self.state.read(cx);
        let (term_panel_open, chrome_visible, portion, pending_kill, active) = (
            ws.term_panel_open(),
            ws.chrome_visible(),
            ws.term_panel_portion(),
            ws.pending_kill(),
            ws.active_session(),
        );
        let has_run_script = active
            .and_then(|id| self.registry.read(cx).meta(id).map(|m| m.project.clone()))
            .is_some_and(|project| {
                cx.global::<SettingsState>().store.projects.iter().any(|p| {
                    p.name == project
                        && p.scripts
                            .run
                            .as_deref()
                            .is_some_and(|s| !s.trim().is_empty())
                })
            });
        let cluster = ToolCluster {
            has_run_script,
            term_panel_open,
            chrome_visible,
            confirming_kill: active.is_some_and(|id| pending_kill == Some(id)),
            dispatch: tool_dispatch,
        };
        let body = self.body_view(cx);
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(c::BG())
            .when_some(header, |d, h| {
                d.child(session_header::session_header(&h, tick, Some(&cluster)))
            })
            .child(
                // Whatever the chrome costs in height comes out of the
                // terminal's rows for free — the element derives its dims from
                // its own bounds in `prepaint` (Plan 04 amendment 7).
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // A click on the agent PTY moves the input intent back to
                    // the agent (`focus_pane`, `pty_input.rs:146-158`); the
                    // keystrokes themselves follow gpui focus, which the
                    // child's own press already took.
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(PtyPane::Agent);
                                cx.notify();
                            });
                        }),
                    )
                    .when_some(body, gpui::ParentElement::child),
            );

        let wt = self.active_wt_path(cx);
        if !term_panel_open || wt.is_none() {
            return column.into_any_element();
        }
        let panel_dispatch = self.dispatcher(cx, |this, action: PanelAction, window, cx| {
            this.panel_action(action, window, cx);
        });
        let panel_view = self.panel_view(cx);
        let tabs = wt.map_or_else(Vec::new, |wt| {
            let registry = self.registry.read(cx);
            let active = registry.active_wt_shell_idx(&wt);
            registry
                .wt_shells(&wt)
                .iter()
                .enumerate()
                .map(|(i, meta)| ShellTab {
                    running: self.activity.read(cx).state_of(meta.id) != ActivityState::Exited,
                    active: active == Some(i),
                })
                .collect()
        });
        let panel_ctx = PanelCtx {
            tabs,
            view: panel_view,
            dispatch: std::rc::Rc::clone(&panel_dispatch),
        };
        // Proportional flex weights, so the ratio is the single source of
        // truth exactly as iced's `FillPortion` makes it.
        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_basis(px(0.0))
                    .flex_grow(f32::from(100 - portion))
                    .h_full()
                    .overflow_hidden()
                    .child(column),
            )
            .child(term_panel::divider(&panel_dispatch))
            .child(
                div()
                    .flex()
                    .flex_basis(px(0.0))
                    .flex_grow(f32::from(portion))
                    .h_full()
                    .overflow_hidden()
                    // The mirror image: clicking the panel returns input to it.
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(PtyPane::Panel);
                                cx.notify();
                            });
                        }),
                    )
                    .child(term_panel::term_panel(&panel_ctx)),
            )
            .into_any_element()
    }

    // ── the body follows the selection (Task 6 Step 2) ──────────────────

    /// The view for whatever is active, minted once per session and cached, so
    /// switching never respawns a PTY.
    fn body_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        let clock = self.clock.clone();
        if terminal_focused {
            let i = active_terminal?;
            let registry = self.registry.read(cx);
            let id = registry.home_terminals().get(i)?.id;
            let session = registry.home_terminal(i)?.clone();
            if let Some(view) = self.home_views.get(&id) {
                return Some(view.clone());
            }
            let view = cx.new(|cx| TerminalView::new(session, None, clock, cx));
            self.home_views.insert(id, view.clone());
            return Some(view);
        }
        let id = active_session?;
        let registry = self.registry.read(cx);
        let session = registry.session(id)?.clone();
        let project = registry.meta(id).map(|m| m.project.clone());
        if let Some(view) = self.views.get(&id) {
            return Some(view.clone());
        }
        let view = cx.new(|cx| TerminalView::new(session, project, clock, cx));
        self.views.insert(id, view.clone());
        Some(view)
    }
}

/// Logs and does nothing. Each stub names the plan that implements it.
impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The single zoom application point. `WithRemSize` does not exist at
        // this rev; `Window::with_rem_size` is for scoped overrides.
        let zoom_value = cx.global::<ZoomState>().zoom;
        window.set_rem_size(px(zoom::REM_BASE * zoom_value));
        // The split divider maps a cursor x against the logical window width.
        self.logical_win_w = f32::from(window.viewport_size().width) / zoom_value.max(0.1);

        // Spec's "always >= 1 home terminal": the section lazily spawns its
        // first shell (`src/app/terminals.rs:21-30`).
        if self.registry.read(cx).home_terminals_need_spawn() {
            self.sidebar
                .clone()
                .update(cx, Sidebar::spawn_home_terminal);
        }

        // The 5s git-state poll, kicked from the frame but running off-thread.
        let paths = {
            let ws = self.state.read(cx);
            let store = &cx.global::<SettingsState>().store;
            self.tree.read(cx).visible_worktree_paths(store, ws)
        };
        self.tree
            .clone()
            .update(cx, |t, cx| t.maybe_poll_git_state(paths, cx));

        // Window activation: `window_focused` gates the "focused session is
        // never waiting" rule, and regaining focus acknowledges the visible
        // session (`layout.rs:34-49`).
        if !self.activation_observed {
            self.activation_observed = true;
            let activity = self.activity.clone();
            let sub = cx.observe_window_activation(window, move |_, window, cx| {
                let active = window.is_window_active();
                activity.update(cx, |a, cx| a.set_window_focused(active, cx));
            });
            self.observers.push(sub);
        }

        // Carried amendment 5: a waiting session is what feeds the frame
        // clock's `animating` term, or the amber pulse would never animate.
        let waiting = self.activity.read(cx).waiting_count();
        let has_ptys =
            !self.registry.read(cx).is_empty() || self.registry.read(cx).home_terminal_count() > 0;
        let window_active = window.is_window_active();
        self.clock.clone().update(cx, |clock, cx| {
            clock.set_busy_inputs(false, has_ptys, window_active, waiting > 0, false, cx);
        });

        let body = self.body_view(cx);
        if !self.focused_once {
            if let Some(view) = body.as_ref() {
                self.focused_once = true;
                let handle = view.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        }

        // Carried amendment 4: gpui scopes by key context, not by a screen
        // flag consulted at match time. `screen_from_flags` survives purely to
        // *choose* the context string, and `Screen::key_context` already emits
        // exactly what `keymap::contexts_for` binds into. There is no fourth
        // screen: iced's own `Screen` enum has three variants
        // (`shortcuts.rs:87-91`) and the terminal tab is orthogonal to it.
        // While a modal is open the workspace stops declaring its screen
        // context, so no screen-scoped chord can fire from behind the scrim.
        // The modal declares its own context instead (spec §4); that, plus the
        // layer claiming every key its verdict table names, is what replaces
        // iced's `MODAL_OPEN` static (carried decision 3).
        let modal_open = self.modals.read(cx).is_open();
        let screen_context = self.state.read(cx).screen().key_context();

        let root = div()
            .track_focus(&self.focus)
            .when(!modal_open, |d| d.key_context(screen_context))
            .on_action(Self::zoom_in)
            .on_action(Self::zoom_out)
            .on_action(Self::zoom_reset)
            .on_action(
                cx.listener(|this, action: &keymap::SelectSession, window, cx| {
                    this.select_session(action, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::NextSession, window, cx| {
                this.cycle(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::PrevSession, window, cx| {
                this.cycle(false, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &keymap::JumpToWaitingSession, _, cx| {
                    this.jump_to_waiting(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::CloseFocusedSession, _, cx| {
                this.close_focused(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ScrollHalfPageUp, _, cx| {
                this.scroll_half_page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ScrollHalfPageDown, _, cx| {
                this.scroll_half_page(false, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleGrid, _, cx| this.toggle_grid(cx)))
            .on_action(cx.listener(|this, _: &keymap::ToggleZen, _, cx| this.toggle_zen(cx)))
            .on_action(cx.listener(|this, _: &keymap::ToggleTerminal, _, cx| {
                this.toggle_terminal_tab(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::NewHomeTerminal, _, cx| {
                this.sidebar
                    .clone()
                    .update(cx, Sidebar::spawn_home_terminal);
            }))
            .on_action(cx.listener(|this, a: &keymap::GridMove, _, cx| {
                this.grid_move(a.dx, a.dy, false, cx);
            }))
            .on_action(cx.listener(|this, a: &keymap::GridSwap, _, cx| {
                this.grid_move(a.dx, a.dy, true, cx);
            }))
            .on_action(cx.listener(|this, a: &keymap::AdjustTermPanel, _, cx| {
                this.state.update(cx, |s, cx| {
                    s.adjust_term_panel_portion(a.delta);
                    cx.notify();
                });
            }));
        // Plan 08 Task 3/5: the five stub actions open real modals. The three
        // palette entry points differ only in which list state the palette
        // opens into, which Task 5 fills.
        let root = root
            .on_action(cx.listener(|this, _: &keymap::NewSession, _, cx| {
                this.open_modal(crate::modal::Modal::SessionLauncher(Box::default()), cx);
            }))
            .on_action(
                cx.listener(|this, _: &keymap::NewSessionInWorktree, _, cx| {
                    this.open_modal(crate::modal::Modal::SessionLauncher(Box::default()), cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::SwitchSession, _, cx| {
                this.open_modal(crate::modal::Modal::SessionLauncher(Box::default()), cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::Settings, _, cx| {
                this.open_modal(crate::modal::Modal::Settings, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShortcutOverlay, _, cx| {
                this.open_modal(crate::modal::Modal::ShortcutOverlay, cx);
            }));

        // The divider drags (sidebar and split alike) and the tile drag all
        // need pointer events that outlive their hit zones, and the root is the
        // only element wide enough to deliver them.
        let root = sidebar::root_drag_listeners(&self.sidebar, root);
        let root = root
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                this.on_root_mouse_move(f32::from(e.position.x), cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| this.on_root_mouse_up(cx)),
            );

        // ── the chrome (Task 6 Step 3) ──────────────────────────────────
        let dispatch: appbar::Dispatch = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |action, _window, cx: &mut App| {
                let _ = weak.update(cx, |this: &mut Self, cx| this.chrome(action, cx));
            })
        };
        let snap = self.snapshot(cx);
        // The terminal tab draws its own bar (`home_terminal_bar`), so the
        // session header is resolved for the agent side only.
        let header = if self.state.read(cx).terminal_focused() {
            None
        } else {
            self.header_data(&snap, cx)
        };
        let appbar_ctx = AppbarCtx {
            sidebar_width: self.state.read(cx).sidebar_width(),
            tick: self.clock.read(cx).tick(),
            pulse: self.activity.read(cx).pulse(),
            // Resolved once, handed to the pill and the dropdown alike.
            waiting: self.waiting_rows(cx),
            grid_view: self.state.read(cx).grid_view(),
            // Plan 09 owns the real upgrade state; the dot renders off.
            upgrade_available: false,
            dispatch: std::rc::Rc::clone(&dispatch),
        };
        let statusbar_ctx = {
            let registry = self.registry.read(cx);
            let activity = self.activity.read(cx);
            let running = registry
                .all()
                .iter()
                .filter(|m| activity.state_of(m.id) != ActivityState::Exited)
                .count();
            let store = &cx.global::<SettingsState>().store;
            StatusbarCtx {
                running,
                backend: if grove_core::tmux::available() {
                    "tmux"
                } else {
                    "native"
                },
                theme_name: store
                    .theme
                    .clone()
                    .unwrap_or_else(|| crate::theme::DEFAULT_DARK_THEME.to_string()),
                skip_permissions: store.dangerously_skip_permissions_enabled.unwrap_or(false),
                toast: self.toast.read(cx).current().cloned(),
                dispatch,
            }
        };
        let queue_open =
            self.state.read(cx).attention_queue_open() && !appbar_ctx.waiting.is_empty();

        // ── the four bodies ─────────────────────────────────────────────
        let tick = appbar_ctx.tick;
        let grid_dispatch = self.dispatcher(cx, |this, action: GridAction, window, cx| {
            this.grid_action(action, window, cx);
        });
        let grid_ctx = GridCtx {
            tiles: self.tile_data(&snap, cx),
            pulse: appbar_ctx.pulse,
            // The scrim's 40-tick triangle wave —
            // `animation_clock::toast_pulse`'s first and only consumer
            // (Plan 06 recorded ambiguity 3).
            scrim_pulse: {
                let phase = crate::entities::animation_clock::toast_pulse(tick) as f32;
                (phase - 20.0).abs() / 20.0
            },
            drag: self.state.read(cx).grid_drag(),
            slide: self.state.read(cx).grid_slide(),
            tile_size: {
                let n = self.state.read(cx).tile_order().len();
                let size = window.viewport_size();
                crate::grid::grid_tile_size(
                    f32::from(size.width),
                    f32::from(size.height),
                    zoom_value,
                    appbar::APPBAR_H + statusbar::STATUS_H,
                    n,
                )
            },
            dispatch: std::rc::Rc::clone(&grid_dispatch),
        };
        let body_el = if self.state.read(cx).terminal_focused() {
            let tab_dispatch = self.dispatcher(cx, |this, action: TerminalTabAction, _, cx| {
                this.tab_action(action, cx);
            });
            let (running, context) = {
                let ws = self.state.read(cx);
                let registry = self.registry.read(cx);
                ws.active_terminal()
                    .and_then(|i| {
                        let meta = registry.home_terminals().get(i)?;
                        let entity = registry.home_terminal(i)?;
                        let title = entity.read(cx).title();
                        Some((
                            self.activity.read(cx).state_of(meta.id) != ActivityState::Exited,
                            title
                                .as_deref()
                                .and_then(|raw| rows::terminal_context(raw, &meta.label)),
                        ))
                    })
                    .unwrap_or((false, None))
            };
            terminal_tab::terminal_tab(&TerminalTabCtx {
                view: self.body_view(cx),
                running,
                context,
                chrome_visible: self.state.read(cx).chrome_visible(),
                dispatch: tab_dispatch,
            })
        } else {
            self.session_body(header, tick, cx)
        };

        // Task 5 Step 1: `chrome_visible` is real. Zen hides the appbar, the
        // sidebar and the statusbar; every height they gave up returns to the
        // terminal for free (findings amendment 7).
        let chrome_visible = self.state.read(cx).chrome_visible();
        let grid_view = self.state.read(cx).grid_view();
        let has_waiting = !appbar_ctx.waiting.is_empty();

        // The grid replaces the whole row **including the sidebar**
        // (`view/mod.rs:66-79`); zen shows the body alone, full-bleed.
        let content = if grid_view {
            grid::grid(&grid_ctx)
        } else if chrome_visible {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .child(self.sidebar.clone())
                .child(body_el)
                .into_any_element()
        } else {
            body_el
        };

        root.flex()
            .flex_col()
            .relative()
            .size_full()
            .bg(c::BG())
            .text_color(c::FG())
            .when(chrome_visible, |d| d.child(appbar::appbar(&appbar_ctx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(content),
            )
            .when(chrome_visible, |d| {
                d.child(statusbar::statusbar(&statusbar_ctx))
            })
            // The zen pill floats over the terminal, but only while something
            // waits (`view/mod.rs:81-99`).
            .when(!chrome_visible && has_waiting, |d| {
                d.child(appbar::zen_attention_pill(&appbar_ctx))
            })
            // The dropdown layer is gated on the chrome too (`view/mod.rs:101`).
            .when(chrome_visible, |d| d)
            .when(queue_open && chrome_visible, |d| {
                d.child(appbar::attention_dropdown(&appbar_ctx))
            })
            // The modal layer is rendered LAST, so it paints above every other
            // layer including the attention dropdown and the zen pill.
            .when(modal_open, |d| d.child(self.modals.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chrome heights are the `src/gui/metrics.rs:15-17` values, and the
    /// three bars agree with the workspace on what they cost vertically.
    #[test]
    fn the_chrome_heights_match_the_iced_metrics() {
        assert!((appbar::APPBAR_H - 44.0).abs() < f32::EPSILON);
        assert!((statusbar::STATUS_H - 26.0).abs() < f32::EPSILON);
        assert!((session_header::SESSBAR_H - 36.0).abs() < f32::EPSILON);
        assert!((sidebar::SIDEBAR_DIVIDER_W - 6.0).abs() < f32::EPSILON);
    }
}
