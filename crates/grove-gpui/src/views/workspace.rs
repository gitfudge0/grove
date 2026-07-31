//! The root view: the sidebar rail, the divider, and the body showing whatever
//! `WorkspaceState` says is active.
//!
//! Plans 06-07 replace the remaining placeholders (appbar, statusbar, grid,
//! zen). Every dimension comes from a named constant carrying its
//! `src/gui/metrics.rs` line.

use std::collections::HashMap;

use gpui::{div, prelude::*, px, rems, App, Context, Entity, FocusHandle, Focusable, Window};

use crate::activity::{ActivityState, ActivityStore};
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::{SessionId, SessionRegistry};
use crate::entities::terminal_session::TerminalSession;
use crate::entities::workspace_state::WorkspaceState;
use crate::keymap;
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::sidebar::{self, Sidebar};
use crate::views::terminal_view::TerminalView;
use crate::zoom::{self, ZoomState};

/// App bar height (`src/gui/metrics.rs:15`).
const APPBAR_H: f32 = 44.0;
/// Status bar height (`src/gui/metrics.rs:16`).
const STATUS_H: f32 = 26.0;

/// Chrome is authored in `rems` so a single `set_rem_size` scales all of it.
fn r(px_at_1x: f32) -> gpui::Rems {
    rems(px_at_1x / zoom::REM_BASE)
}

pub struct Workspace {
    focus: FocusHandle,
    /// Kept alive here: dropping the clock entity would stop every animation
    /// in the window, including the terminal cursor blink.
    clock: Entity<AnimationClock>,
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
    tree: Entity<ProjectTree>,
    activity: Entity<ActivityStore>,
    sidebar: Entity<Sidebar>,
    /// One view per session, cached by id so switching does not respawn
    /// anything (Task 6 Step 2).
    views: HashMap<SessionId, Entity<TerminalView>>,
    home_views: HashMap<SessionId, Entity<TerminalView>>,
    /// The terminal takes focus on the first frame so keystrokes land without
    /// a click; `window.focus` needs a `&mut Window`, which `new` has not got.
    focused_once: bool,
    _observers: Vec<gpui::Subscription>,
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
        let activity = cx.new(|_| ActivityStore::new());

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
        ];

        Self {
            focus: cx.focus_handle(),
            clock,
            state,
            registry,
            tree,
            activity,
            sidebar,
            views: HashMap::new(),
            home_views: HashMap::new(),
            focused_once: false,
            _observers: observers,
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

    /// The first waiting session in visible order (`update/mod.rs:728-739`).
    /// **Stub-gated:** Plan 06's classifier reports `Idle` for everything
    /// today, so this is a no-op — that is correct, not broken; the wiring is
    /// what this phase owes.
    fn jump_to_waiting(&mut self, cx: &mut Context<Self>) {
        let order = self.visible_order(cx);
        let waiting = {
            let activity = self.activity.read(cx);
            order
                .into_iter()
                .find(|&id| activity.state_of(id) == ActivityState::WaitingForInput)
        };
        let Some(id) = waiting else { return };
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
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

        root.flex()
            .flex_row()
            .size_full()
            .bg(c::BG())
            .text_color(c::FG())
            .child(self.sidebar.clone())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    // App bar placeholder (Plan 06).
                    .child(div().h(r(APPBAR_H)).w_full().bg(c::BG_STRIP()))
                    // Body: the ACTIVE session, or the active home terminal.
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .w_full()
                            .bg(c::BG())
                            .when_some(body, gpui::ParentElement::child),
                    )
                    // Status bar placeholder (Plan 06).
                    .child(div().h(r(STATUS_H)).w_full().bg(c::BG_STRIP())),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rems_are_derived_from_the_pixel_constants() {
        assert_eq!(r(APPBAR_H).0, 44.0 / 16.0);
        assert_eq!(r(STATUS_H).0, 26.0 / 16.0);
        assert_eq!(r(sidebar::SIDEBAR_DIVIDER_W).0, 6.0 / 16.0);
    }
}
