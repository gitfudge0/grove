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
use crate::entities::workspace_state::WorkspaceState;
use crate::keymap;
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::appbar::{self, AppbarCtx, ChromeAction, WaitingRow};
use crate::views::rows;
use crate::views::session_header::{self, SessionHeaderData};
use crate::views::sidebar::{self, Sidebar};
use crate::views::statusbar::{self, StatusbarCtx};
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
    /// One view per session, cached by id so switching does not respawn
    /// anything (Task 6 Step 2).
    views: HashMap<SessionId, Entity<TerminalView>>,
    home_views: HashMap<SessionId, Entity<TerminalView>>,
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
            views: HashMap::new(),
            home_views: HashMap::new(),
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

    /// `mod+N` selects the Nth **visible** session; out of range is a no-op,
    /// not a clamp (`src/gui/update/sessions.rs:394-407`).
    fn select_session(&mut self, action: &keymap::SelectSession, cx: &mut Context<Self>) {
        let Some(&id) = self.visible_order(cx).get(action.index.saturating_sub(1)) else {
            return;
        };
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    /// `sessions.rs:365-405`.
    fn cycle(&mut self, next: bool, cx: &mut Context<Self>) {
        let order = self.visible_order(cx);
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.cycle_session(next, &order, &snap);
            cx.notify();
        });
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
            ChromeAction::ToggleGridView => {
                tracing::debug!("ToggleGridView: not implemented yet — Plan 07");
            }
            ChromeAction::OpenSessionLauncher => {
                tracing::debug!("OpenSessionLauncher: modal — Plan 08");
            }
            ChromeAction::OpenSettings => tracing::debug!("OpenSettings: modal — Plan 08"),
            ChromeAction::OpenShortcutOverlay => {
                tracing::debug!("OpenShortcutOverlay: modal — Plan 08");
            }
        }
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
macro_rules! stub_action {
    ($div:expr, $action:ty, $plan:literal) => {
        $div.on_action(|_: &$action, _: &mut Window, _: &mut App| {
            tracing::debug!(concat!(
                stringify!($action),
                ": not implemented yet — ",
                $plan
            ));
        })
    };
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The single zoom application point. `WithRemSize` does not exist at
        // this rev; `Window::with_rem_size` is for scoped overrides.
        let zoom_value = cx.global::<ZoomState>().zoom;
        window.set_rem_size(px(zoom::REM_BASE * zoom_value));

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

        let root = div()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(Self::zoom_in)
            .on_action(Self::zoom_out)
            .on_action(Self::zoom_reset)
            .on_action(cx.listener(|this, action: &keymap::SelectSession, _, cx| {
                this.select_session(action, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::NextSession, _, cx| this.cycle(true, cx)))
            .on_action(cx.listener(|this, _: &keymap::PrevSession, _, cx| this.cycle(false, cx)))
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
            }));
        let root = stub_action!(root, keymap::NewSession, "Plan 07");
        let root = stub_action!(root, keymap::NewSessionInWorktree, "Plan 07");
        let root = stub_action!(root, keymap::SwitchSession, "Plan 08");
        let root = stub_action!(root, keymap::ToggleGrid, "Plan 07");
        let root = stub_action!(root, keymap::ToggleZen, "Plan 06");
        let root = stub_action!(root, keymap::Settings, "Plan 08");
        let root = stub_action!(root, keymap::ShortcutOverlay, "Plan 08");
        let root = stub_action!(root, keymap::ToggleTerminal, "Plan 07");
        let root = stub_action!(root, keymap::NewHomeTerminal, "Plan 07");

        // The divider drag needs pointer events that outlive the 6px hit zone,
        // and the root is the only element wide enough to deliver them.
        let root = sidebar::root_drag_listeners(&self.sidebar, root);

        // ── the chrome (Task 6 Step 3) ──────────────────────────────────
        let dispatch: appbar::Dispatch = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |action, _window, cx: &mut App| {
                let _ = weak.update(cx, |this: &mut Self, cx| this.chrome(action, cx));
            })
        };
        let snap = self.snapshot(cx);
        let header = self.header_data(&snap, cx);
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

        // Plan 07 owns zen's chrome-hidden branch and its floating pill; this
        // stays true until then.
        let chrome_visible = true;

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
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.sidebar.clone())
                    .child(
                        // The body: session header atop the terminal. Whatever
                        // the chrome costs in height comes out of the
                        // terminal's rows for free — the element derives its
                        // dims from its own bounds in `prepaint` (Plan 04
                        // amendment 7), so there is no PTY-dim wiring here.
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .bg(c::BG())
                            .when_some(header, |d, h| {
                                d.child(session_header::session_header(&h, appbar_ctx.tick))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .w_full()
                                    .overflow_hidden()
                                    .when_some(body, gpui::ParentElement::child),
                            ),
                    ),
            )
            .when(chrome_visible, |d| {
                d.child(statusbar::statusbar(&statusbar_ctx))
            })
            .when(queue_open, |d| {
                d.child(appbar::attention_dropdown(&appbar_ctx))
            })
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
