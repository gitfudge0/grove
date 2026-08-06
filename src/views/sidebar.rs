//! The left rail: header, the scrolling project → worktree → session tree, the
//! agent-menu overlay, the docked TERMINALS header, and the draggable divider.
//!
//! # `uniform_list` vs a plain column (carried amendment 2 — DECIDED)
//!
//! **A plain scrollable `div` column.** `uniform_list` at ZED_REV `1a246ef` is
//! not merely optimized for uniform heights, it *requires* them: it "simply
//! measures the first element and then lays out all remaining elements in a
//! line based on that measurement"
//! (`crates/gpui/src/elements/uniform_list.rs:1-5`), with a single
//! `item_to_measure_index`. There is no per-row height hook at this rev. Grove's
//! rows are genuinely non-uniform — a worktree showing a branch chip is
//! `ROW_H + 14` (`src/gui/rows.rs:268`) — so feeding it `ROW_H` would clip every
//! branch chip and desynchronize the agent-menu overlay from the list.
//!
//! The cost is that all rows are built per frame rather than just the visible
//! window. That is the same cost the iced build already pays, and
//! [`crate::views::rows::flatten`] made each row O(1) to build, so the
//! per-frame work is now linear in rows with no nested rescans. If a tree ever
//! grows large enough to matter, the fix is `gpui::list` (which supports
//! variable heights), not `uniform_list`.
//!
//! [`crate::views::rows::row_height`] stays the single height source: the
//! column, [`TreeRow::height`](crate::views::rows::TreeRow::height) and
//! [`crate::views::rows::agent_menu_top`] all go through it.

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    div, px, App, Context, CursorStyle, Entity, MouseButton, MouseMoveEvent, MouseUpEvent,
    ScrollHandle, SharedString, Window,
};
use grove_core::agent::Agent;

use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::SessionRegistry;
use crate::entities::workspace_state::{TreeExpand, WorkspaceState, RAIL_W};
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::components::{divider_h, divider_h_strong, icon_btn, mono, tracked};
use crate::views::rows::{self, RowAction, RowCtx, TreeRow};
use crate::views::session_header::SESSBAR_H;

/// Divider hit zone between sidebar and workspace (`src/gui/metrics.rs:20`).
pub const SIDEBAR_DIVIDER_W: f32 = 6.0;
/// Two presses inside this window are a double-click and reset the width
/// (`src/gui/update/layout.rs:107-110`).
const DOUBLE_CLICK: Duration = Duration::from_millis(350);
/// Below this the release is a plain click, not a drag: no persist, no resize
/// (`src/gui/update/layout.rs:159`).
const DRAG_EPSILON: f32 = 0.5;

/// An in-progress divider drag (`src/gui/state.rs`'s `SidebarDrag`).
#[derive(Clone, Copy, Debug)]
struct DividerDrag {
    /// Captured on the **first move**, not on the press: an off-edge grab must
    /// not make the width jump (`layout.rs:137-147`).
    grab_offset: Option<f32>,
    start_width: f32,
}

pub struct Sidebar {
    /// The single modal slot; row actions that open a modal go through it.
    /// Set by `Workspace::new` right after both entities exist.
    modals: Option<Entity<crate::views::modals::ModalLayer>>,
    /// The statusbar toast slot, for the spawn-failure producer.
    toast: Option<Entity<crate::entities::toast::ToastState>>,

    pub state: Entity<WorkspaceState>,
    pub tree: Entity<ProjectTree>,
    pub registry: Entity<SessionRegistry>,
    pub activity: Entity<ActivityStore>,
    clock: Entity<AnimationClock>,
    scroll: ScrollHandle,
    term_scroll: ScrollHandle,
    drag: Option<DividerDrag>,
    last_divider_press: Option<Instant>,
    /// Rebuilt every frame by `render`; read by the divider handlers and by
    /// [`Self::visible_session_order`] so keyboard selection and the sidebar
    /// agree by construction.
    rows: Vec<TreeRow>,
    _observers: Vec<gpui::Subscription>,
}

impl Sidebar {
    pub fn new(
        state: Entity<WorkspaceState>,
        tree: Entity<ProjectTree>,
        registry: Entity<SessionRegistry>,
        activity: Entity<ActivityStore>,
        clock: Entity<AnimationClock>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observers = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe(&tree, |_, _, cx| cx.notify()),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            // The clock drives the Working spinner and the attention pulse.
            cx.observe(&clock, |_, _, cx| cx.notify()),
        ];
        Self {
            modals: None,
            toast: None,
            state,
            tree,
            registry,
            activity,
            clock,
            scroll: ScrollHandle::new(),
            term_scroll: ScrollHandle::new(),
            drag: None,
            last_divider_press: None,
            rows: Vec::new(),
            _observers: observers,
        }
    }

    /// Hand the sidebar the modal slot. Called by `Workspace::new`; both
    /// entities have to exist first, so it cannot be a constructor argument.
    pub fn set_modals(
        &mut self,
        modals: Entity<crate::views::modals::ModalLayer>,
        toast: Entity<crate::entities::toast::ToastState>,
    ) {
        self.modals = Some(modals);
        self.toast = Some(toast);
    }

    /// Open a modal from a row action, if the slot has been handed over.
    fn open_modal(&mut self, modal: crate::modal::Modal, cx: &mut Context<Self>) {
        if let Some(layer) = self.modals.clone() {
            layer.update(cx, |l, cx| l.open(modal, cx));
        }
    }

    /// `mod+1..9`'s index space and the attention queue's order, straight off
    /// the rows the rail last laid out.
    pub fn visible_session_order(&self) -> Vec<crate::entities::session_registry::SessionId> {
        rows::visible_session_order(&self.rows)
    }

    /// Agents found on PATH, always including `Terminal` (`src/app/mod.rs:168`).
    fn available_agents() -> Vec<Agent> {
        let found: Vec<Agent> = Agent::ALL.into_iter().filter(|a| a.available()).collect();
        if found.is_empty() {
            vec![Agent::Terminal]
        } else {
            found
        }
    }

    // ── row dispatch ────────────────────────────────────────────────────

    /// The single place a row click becomes a state change. Actions that would
    /// open a modal log a stub naming their plan; everything that mutates
    /// selection or the registry is wired for real.
    fn dispatch(&mut self, action: RowAction, cx: &mut Context<Self>) {
        let snap = self.snapshot(cx);
        match action {
            RowAction::SelectProject(proj) => {
                let old = self.state.read(cx).proj_idx();
                self.state.update(cx, |s, cx| {
                    s.select_project(proj);
                    cx.notify();
                });
                let path = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(proj)
                    .map(|p| p.path.clone());
                if let Some(path) = path {
                    self.tree.update(cx, |t, cx| {
                        t.switch_active_project(old, proj, &path);
                        cx.notify();
                    });
                }
            }
            RowAction::SelectWorktree(proj, wt) => {
                let old = self.state.read(cx).proj_idx();
                if old != proj {
                    let path = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .get(proj)
                        .map(|p| p.path.clone());
                    if let Some(path) = path {
                        self.tree.update(cx, |t, cx| {
                            t.switch_active_project(old, proj, &path);
                            cx.notify();
                        });
                    }
                }
                self.state.update(cx, |s, cx| {
                    s.select_worktree(proj, wt, &snap);
                    cx.notify();
                });
            }
            RowAction::HoverWorktree(hovered) => {
                if self.state.read(cx).hovered_wt() != hovered {
                    self.state.update(cx, |s, cx| {
                        s.set_hovered_wt(hovered);
                        cx.notify();
                    });
                }
            }
            RowAction::OpenAgentMenu(open) => self.state.update(cx, |s, cx| {
                s.set_open_agent_menu(open);
                cx.notify();
            }),
            // `select_session` moves `proj_idx` to the clicked session's
            // project, so the project hand-off `SelectWorktree` does has to
            // happen here too. Without it `ProjectTree::worktrees` still holds
            // the OLD project's worktrees while `snapshot` hands them to the
            // new `active_proj` — the tree then renders one project's worktrees
            // under another's header and selection lands on the wrong session.
            RowAction::SelectSession(id) => {
                let old = self.state.read(cx).proj_idx();
                ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
                self.state.update(cx, |s, cx| {
                    s.select_session(id, &snap);
                    cx.notify();
                });
            }
            RowAction::ArmKillSession(id) => self.state.update(cx, |s, cx| {
                s.arm_kill(id);
                cx.notify();
            }),
            RowAction::KillSession(id) => {
                self.registry.update(cx, |r, cx| {
                    r.remove(id);
                    cx.notify();
                });
                self.state.update(cx, |s, cx| {
                    s.on_session_removed(id);
                    cx.notify();
                });
            }
            RowAction::SelectTerminal(i) => {
                let count = self.registry.read(cx).home_terminal_count();
                self.state.update(cx, |s, cx| {
                    s.select_home_terminal(i, count);
                    cx.notify();
                });
            }
            RowAction::ArmKillTerminal(i) => self.state.update(cx, |s, cx| {
                s.arm_kill_terminal(i);
                cx.notify();
            }),
            RowAction::KillTerminal(i) => self.close_home_terminal(i, cx),
            RowAction::NewHomeTerminal => self.new_home_terminal(cx),
            RowAction::ToggleTerminalsSection => self.state.update(cx, |s, cx| {
                s.toggle_terminals_collapsed();
                cx.notify();
            }),
            RowAction::ToggleCollapseAll => self.state.update(cx, |s, cx| {
                s.toggle_collapse_all(&snap);
                cx.notify();
            }),
            RowAction::SpawnAgent(proj, wt, agent) => self.spawn_session(proj, wt, agent, cx),
            // The worktree-name prompt (`src/app/mod.rs:442`).
            RowAction::AddWorktree(proj) => {
                self.state.update(cx, |s, cx| {
                    s.select_project(proj);
                    cx.notify();
                });
                self.open_modal(
                    crate::modal::Modal::Input {
                        title: "New worktree".into(),
                        buffer: String::new(),
                        note: None,
                    },
                    cx,
                );
            }
            // Confirm first; accepting starts the teardown (`app/mod.rs:487`).
            RowAction::DeleteWorktree(proj, wt) => {
                let Some(path) = snap
                    .projects
                    .iter()
                    .find(|p| p.idx == proj)
                    .and_then(|p| p.worktrees.get(wt))
                    .map(|w| w.path.clone())
                else {
                    return;
                };
                self.open_modal(
                    crate::modal::Modal::Confirm {
                        title: "Delete worktree?".into(),
                        prompt: format!("'{path}' will be removed from disk."),
                        destructive: true,
                        kind: crate::modal::ConfirmKind::RemoveWorktree(path),
                    },
                    cx,
                );
            }
            RowAction::RemoveProject(idx) => {
                if let Some(layer) = self.modals.clone() {
                    layer.update(cx, |l, cx| l.open_remove_project(idx, cx));
                }
            }
            // Task 6 fills the editor view; the slot opens now, seeded from
            // the project's persisted scripts.
            RowAction::ProjectScripts(idx) => {
                let state = {
                    let store = &cx.global::<crate::settings::SettingsState>().store;
                    store
                        .projects
                        .get(idx)
                        .map(|p| crate::modal::ScriptsEditorState {
                            project_path: p.path.clone(),
                            name: p.name.clone(),
                            setup: p.scripts.setup.clone().unwrap_or_default(),
                            run: p.scripts.run.clone().unwrap_or_default(),
                            teardown: p.scripts.teardown.clone().unwrap_or_default(),
                            renaming: false,
                        })
                };
                if let Some(state) = state {
                    self.open_modal(crate::modal::Modal::ScriptsEditor(Box::new(state)), cx);
                }
            }
            // `on_run_script` (`src/gui/update/sessions.rs:147-177`) opens the
            // worktree's terminal panel; the workspace owns that split.
            RowAction::RunScript(proj, wt) => {
                self.state.update(cx, |s, cx| {
                    s.select_worktree(proj, wt, &snap);
                    if !s.term_panel_open() {
                        s.toggle_term_panel(true);
                    }
                    cx.notify();
                });
            }
            // Task 4 fills the wizard; the slot opens now.
            RowAction::AddProject => {
                self.open_modal(crate::modal::Modal::AddProject(Box::default()), cx);
            }
        }
    }

    // ── registry mutation ───────────────────────────────────────────────

    pub fn spawn_session(&mut self, proj: usize, wt: usize, agent: Agent, cx: &mut Context<Self>) {
        let snap = self.snapshot(cx);
        let Some(project) = snap.projects.iter().find(|p| p.idx == proj) else {
            return;
        };
        let Some(worktree) = project.worktrees.get(wt) else {
            return;
        };
        let (name, cwd) = (project.name.clone(), worktree.path.clone());
        self.spawn_session_in(name, cwd, agent, cx);
    }

    /// Spawn by concrete project name + worktree path. The palette launches
    /// through here: its target may live in a project whose worktree cache is
    /// cold, so there is no snapshot position to resolve it against.
    pub fn spawn_session_in(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
        // Where iced builds them (`src/app/spawn.rs:26-32`): the agent's own
        // flags, from the persisted Permissions / Claude-in-Chrome settings.
        let args = {
            let store = &cx.global::<SettingsState>().store;
            agent.launch_args(
                store.dangerously_skip_permissions_enabled.unwrap_or(false),
                store.chrome_enabled.unwrap_or(false),
            )
        };
        let (id, extra_args, state_file, target) = self.registry.update(cx, |r, cx| {
            let id = r.insert_meta(name.clone(), cwd.clone(), agent);
            let label = r.meta(id).map_or_else(String::new, |m| m.label.clone());
            let extra_args = r.take_attention_args(id);
            let state_file = r.attention_files(id).map(|f| f.state_file.clone());
            cx.notify();
            (
                id,
                extra_args,
                state_file,
                crate::entities::session_registry::SpawnTarget {
                    cwd,
                    agent,
                    project: name,
                    label,
                    args,
                    use_tmux: true,
                },
            )
        });
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn(
                &target,
                &extra_args,
                state_file.as_deref(),
                cx,
            )
        });
        let tmux_name = match session.read(cx).backend() {
            crate::entities::terminal_session::Backend::Tmux { name } => Some(name.clone()),
            crate::entities::terminal_session::Backend::Native => None,
        };
        let tmux_backed = tmux_name.is_some();
        let spawn_error = session.read(cx).spawn_error().map(str::to_string);
        // Recorded ambiguity 7 (`src/gui/update/sessions.rs:482`): a PTY that
        // never came up is reported here, the one place every spawn path —
        // the rail strip, the agent picker and the launcher — funnels through.
        if let Some(e) = spawn_error.as_deref() {
            crate::telemetry::track("error", vec![("kind", "spawn_failed".into())]);
            let msg = format!("failed to start session: {e}");
            if let Some(toast) = self.toast.clone() {
                toast.update(cx, |t, cx| t.set_error(msg, cx));
            }
        }
        self.registry.update(cx, |r, cx| {
            r.attach(id, session, tmux_name);
            cx.notify();
        });
        if spawn_error.is_none() {
            // `src/gui/update/sessions.rs:463-472` — the counts are read after
            // the attach, so the new session is included exactly as iced's are.
            let (open, open_tmux) = {
                let r = self.registry.read(cx);
                (
                    r.len() as u64,
                    r.all().iter().filter(|m| m.tmux).count() as u64,
                )
            };
            crate::telemetry::track(
                "session_created",
                vec![
                    ("agent", agent.label().into()),
                    ("tmux", tmux_backed.into()),
                    ("open_sessions", open.into()),
                    ("open_native", (open - open_tmux).into()),
                    ("open_tmux", open_tmux.into()),
                ],
            );
        }
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    /// `mod+alt+t` from anywhere, including the grid: leaves the grid first
    /// (a terminal spawned behind the tiles would be invisible) then spawns
    /// exactly as [`Self::spawn_home_terminal`] does
    /// (`update/mod.rs:1008-1022`).
    pub fn new_home_terminal(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.exit_grid_for_terminal();
            cx.notify();
        });
        self.spawn_home_terminal(cx);
    }

    /// Lazily spawn the pinned section's shell, and focus it.
    pub fn spawn_home_terminal(&mut self, cx: &mut Context<Self>) {
        let (label, target) = self.registry.update(cx, |r, _| {
            let label = r.next_home_label();
            (
                label.clone(),
                crate::entities::session_registry::SpawnTarget::home(label),
            )
        });
        // Home terminals are `Agent::Terminal`: `attention::prepare` returns
        // `None` for them, so there is nothing to thread down.
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn(&target, &[], None, cx)
        });
        let count = self.registry.update(cx, |r, cx| {
            let id = r.next_home_id();
            r.push_home(
                crate::entities::session_registry::SessionMeta {
                    id,
                    project: String::new(),
                    wt_path: target.cwd.clone(),
                    agent: Agent::Terminal,
                    label,
                    spawned_at: Instant::now(),
                    attention: None,
                    // Home terminals and panel shells are always native.
                    tmux: false,
                    tmux_name: None,
                },
                session,
            );
            cx.notify();
            r.home_terminal_count()
        });
        self.state.update(cx, |s, cx| {
            s.select_home_terminal(count.saturating_sub(1), count);
            cx.notify();
        });
    }

    /// Spec's "always ≥1 home terminal": closing the last one immediately
    /// respawns a fresh shell (`src/app/terminals.rs:21-30`).
    pub(crate) fn close_home_terminal(&mut self, i: usize, cx: &mut Context<Self>) {
        let remaining = self.registry.update(cx, |r, cx| {
            r.close_home(i);
            cx.notify();
            r.home_terminal_count()
        });
        self.state.update(cx, |s, cx| {
            s.close_home_terminal(i, remaining);
            cx.notify();
        });
        if remaining == 0 {
            self.spawn_home_terminal(cx);
        }
    }

    // ── snapshot ────────────────────────────────────────────────────────

    /// The snapshot every pure helper reads. `Context<ProjectTree>` derefs to
    /// `App`, so the store Global and the registry entity are both readable
    /// inside the `update` without cloning either.
    fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
        let active_proj = self.state.read(cx).proj_idx();
        let registry = self.registry.clone();
        self.tree.clone().update(cx, |tree, cx| {
            let store = &cx.global::<SettingsState>().store;
            tree.snapshot(store, registry.read(cx), active_proj)
        })
    }

    // ── divider ─────────────────────────────────────────────────────────

    fn on_divider_press(&mut self, window: &Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let double = self
            .last_divider_press
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK);
        if double {
            self.drag = None;
            self.last_divider_press = None;
            let win_w = Self::logical_window_width(window, cx);
            self.state.update(cx, |s, cx| {
                s.set_sidebar_width(RAIL_W, win_w);
                cx.notify();
            });
            self.persist_width(cx);
        } else {
            self.last_divider_press = Some(now);
            self.drag = Some(DividerDrag {
                grab_offset: None,
                start_width: self.state.read(cx).sidebar_width(),
            });
        }
    }

    /// Called from the workspace root, which is the only element wide enough to
    /// keep receiving moves once the cursor leaves the 6px hit zone.
    pub fn on_root_mouse_move(&mut self, cursor_x: f32, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        let offset = match drag.grab_offset {
            Some(o) => o,
            None => {
                // Captured on the first move so an off-edge press does not make
                // the width jump (`layout.rs:137-147`).
                let o = self.state.read(cx).sidebar_width() - cursor_x;
                self.drag = Some(DividerDrag {
                    grab_offset: Some(o),
                    ..drag
                });
                o
            }
        };
        let win_w = Self::logical_window_width(window, cx);
        self.state.update(cx, |s, cx| {
            s.set_sidebar_width(cursor_x + offset, win_w);
            cx.notify();
        });
    }

    pub fn on_root_mouse_up(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        // A plain click must not write to disk (`layout.rs:157-162`). The PTY
        // needs no explicit re-dimensioning: the element derives its dims from
        // its own bounds in `prepaint` (Plan 04 amendment 7).
        if (self.state.read(cx).sidebar_width() - drag.start_width).abs() >= DRAG_EPSILON {
            self.persist_width(cx);
        }
    }

    fn persist_width(&self, cx: &mut Context<Self>) {
        let width = self.state.read(cx).sidebar_width();
        // The 250ms `SettingsState` debounce replaces the iced tick-debounce.
        SettingsState::update(cx, |s| s.sidebar_width = Some(width));
    }

    fn logical_window_width(window: &Window, cx: &App) -> f32 {
        let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
        f32::from(window.viewport_size().width) / zoom
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snapshot(cx);
        let git_suffix = self.tree.read(cx).git_suffixes();
        // Whether each home shell is actually running, not merely present —
        // the docked TERMINALS header's activity dot lights only when one is
        // (`src/gui/view/sidebar.rs:61-70,114-119`).
        let home_running: Vec<bool> = {
            let home_count = self.registry.read(cx).home_terminal_count();
            let entities: Vec<_> = (0..home_count)
                .map(|i| self.registry.read(cx).home_terminal(i).cloned())
                .collect();
            entities
                .into_iter()
                .map(|e| e.is_some_and(|s| s.update(cx, |t, _| t.alive())))
                .collect()
        };
        let home_count = home_running.len();
        // Every frame until the user touches the tree manually, so
        // late-arriving sessions (restored asynchronously) still land the
        // default expansion/highlight rather than freezing on the first,
        // possibly-empty snapshot. Deliberately no `cx.notify()` — this runs
        // inside a render pass that is already under way.
        self.state.update(cx, |s, _| s.sync_default_tree(&snap));
        let (rows, tick, pulse, width, terminals_collapsed, open_menu, hovered_wt, next_glyph) = {
            let ws = self.state.read(cx);
            let activity = self.activity.read(cx);
            let rows = rows::flatten(&snap, ws, activity, &git_suffix, &home_running);
            let next_glyph = match ws.tree_expand().next() {
                TreeExpand::SessionsOnly => "expand-sessions",
                TreeExpand::All => "expand-all",
                TreeExpand::Collapsed => "collapse-all",
            };
            (
                rows,
                self.clock.read(cx).tick(),
                activity.pulse(),
                ws.sidebar_width(),
                ws.terminals_collapsed(),
                ws.open_agent_menu(),
                ws.hovered_wt(),
                next_glyph,
            )
        };
        self.rows.clone_from(&rows);
        // Publish the flattened order so the attention queue resolves in tree
        // order without the `ActivityStore` reaching into a view. Deliberately
        // without `cx.notify()`: this is derived data that changed *because* a
        // repaint was already under way.
        let order = self.visible_session_order();
        self.state.update(cx, |s, _| s.set_visible_order(order));

        let ctx = self.row_ctx(&rows, tick, pulse, hovered_wt, cx);
        let menu_top = open_menu.and_then(|open| rows::agent_menu_top(&rows, open));

        // The expanded TERMINALS section (header + terminal rows) is emitted
        // by `rows::flatten` at the tail of the row list, but it belongs
        // docked at the bottom of the rail rather than inside the scrolling
        // tree — split it off here instead of touching `flatten`.
        let split = rows
            .iter()
            .position(|r| matches!(r, TreeRow::TerminalsHeader { .. }))
            .unwrap_or(rows.len());
        let (tree_rows, term_rows) = rows.split_at(split);

        let mut list = div()
            .id("sidebar-tree")
            .flex()
            .flex_col()
            .size_full()
            .pt(rpx(SPACE_LG))
            .pb(rpx(SPACE_2XL))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for row in tree_rows {
            list = list.child(rows::render_row(row, &ctx));
        }

        let mut tree_area = div().relative().flex_1().w_full().child(list);
        if let Some((proj, wt, top, is_main)) = menu_top {
            // The row offset is authored in design px (so it zooms), but the
            // scroll offset is real window pixels — resolve the rem to pixels
            // and add them in that one space.
            let top = rpx(top).to_pixels(window.rem_size()) + self.scroll.offset().y;
            tree_area = tree_area.child(self.agent_menu(proj, wt, top, is_main, &ctx));
        }

        let mut rail = div()
            .w(rpx(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(c::BG_RAIL())
            .child(self.header(next_glyph, &ctx))
            .child(divider_h())
            .child(tree_area);
        if !term_rows.is_empty() {
            // Docked at the bottom of the rail, separated by a divider and
            // capped at 20% of the rail height so a long terminal list
            // scrolls internally rather than pushing the tree off-screen.
            let mut term_list = div()
                .id("sidebar-terminals")
                .flex()
                .flex_col()
                .w_full()
                .flex_none()
                .max_h(gpui::relative(0.2))
                .overflow_y_scroll()
                .track_scroll(&self.term_scroll);
            for row in term_rows {
                term_list = term_list.child(rows::render_row(row, &ctx));
            }
            rail = rail.child(divider_h()).child(term_list);
        }
        if terminals_collapsed {
            // Docked outside the scroll area so it is always reachable; the dot
            // is on iff a shell is running (`sidebar.rs:114-129`, `:61-70`).
            rail = rail.child(divider_h()).child(rows::terminals_header(
                false,
                home_count,
                home_running.iter().any(|&r| r),
                &ctx,
            ));
        }

        let _ = window;
        div()
            .flex()
            .flex_row()
            .h_full()
            .child(rail)
            .child(self.divider(cx))
    }
}

impl Sidebar {
    fn row_ctx(
        &self,
        rows: &[TreeRow],
        tick: u64,
        pulse: f32,
        hovered_wt: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> RowCtx {
        // Live OSC titles (Plan 05 deviation 5 closes here). `session_context`
        // strips the worktree name, the internal label and the agent label,
        // then `sanitize_ui_text` drops the emoji/box-drawing the UI font
        // cannot render — the header applies the same filter
        // (`common.rs:179-190`).
        //
        // The iced `cached_context` memo (`rows.rs:748`) exists because the
        // sanitize ran per frame per row inside `view()`. It is **not** ported:
        // no profile showed it mattering here, and the same omission was
        // recorded for Plan 05's PTY-theme memo. Revisit only with a profile.
        let registry = self.registry.read(cx);
        let mut session_text = std::collections::HashMap::new();
        for row in rows {
            if let TreeRow::Session { id, .. } = row {
                if let Some(meta) = registry.meta(*id) {
                    let context = registry
                        .session(*id)
                        .and_then(|e| e.read(cx).title())
                        .and_then(|raw| {
                            rows::session_context(
                                &raw,
                                &rows::path_basename(&meta.wt_path),
                                &meta.label,
                                meta.agent.label(),
                            )
                        });
                    session_text.insert(*id, (meta.agent, context));
                }
            }
        }
        let terminal_text = (0..registry.home_terminal_count())
            .map(|i| {
                let label = registry.home_terminals().get(i)?.label.clone();
                let raw = registry.home_terminal(i)?.read(cx).title()?;
                rows::terminal_context(&raw, &label)
            })
            .collect();
        let weak = cx.entity().downgrade();
        RowCtx {
            tick,
            pulse,
            hovered_wt,
            available: Self::available_agents(),
            session_text,
            terminal_text,
            dispatch: Rc::new(move |action, _window, cx: &mut App| {
                let _ = weak.update(cx, |this: &mut Self, cx| this.dispatch(action, cx));
            }),
        }
    }

    /// The `SESSBAR_H` header: the letter-spaced `PROJECTS` label, the add
    /// button, and the cycle button whose glyph previews the **next** action
    /// (`src/gui/view/sidebar.rs:141-223`).
    fn header(&self, next_glyph: &'static str, ctx: &RowCtx) -> impl IntoElement {
        let dispatch = Rc::clone(&ctx.dispatch);
        let toggle = {
            let dispatch = Rc::clone(&dispatch);
            icon_btn(
                "tree-cycle",
                next_glyph,
                CONTROL_H,
                CONTROL_H,
                ICON_SM,
                c::FG_MUTE(),
                c::BG_HOVER(),
                Some(c::FG()),
                false,
                move |window, cx| dispatch(RowAction::ToggleCollapseAll, window, cx),
            )
        };
        div()
            .h(rpx(SESSBAR_H))
            .w_full()
            .flex()
            .items_center()
            .pl(rpx(SPACE_3XL))
            .pr(rpx(SPACE_LG))
            .child(
                mono(
                    SharedString::from(tracked("PROJECTS")),
                    TEXT_MICRO,
                    c::FG_MUTE(),
                )
                .flex_1(),
            )
            .child({
                let dispatch = std::rc::Rc::clone(&dispatch);
                icon_btn(
                    "proj-add",
                    "plus",
                    CONTROL_H,
                    CONTROL_H,
                    ICON_SM,
                    c::FG_MUTE(),
                    c::BG_HOVER(),
                    Some(c::FG()),
                    false,
                    move |window, cx| dispatch(RowAction::AddProject, window, cx),
                )
            })
            .child(toggle)
    }

    /// The absolutely-positioned agent menu over the list
    /// (`src/gui/view/sidebar.rs:99-109`), anchored by
    /// [`rows::agent_menu_top`] so it tracks the row at any scroll position
    /// and collapse state.
    fn agent_menu(
        &self,
        proj: usize,
        wt: usize,
        top: gpui::Pixels,
        is_main: bool,
        ctx: &RowCtx,
    ) -> impl IntoElement {
        // Click-away backdrop: a full-bleed transparent layer beneath the menu
        // that dismisses it (`src/gui/widgets/primitives.rs:74-110`).
        let backdrop = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .on_mouse_down(MouseButton::Left, {
                let dispatch = Rc::clone(&ctx.dispatch);
                move |_, window, cx| dispatch(RowAction::OpenAgentMenu(None), window, cx)
            });

        let item = |label: &'static str,
                    icon_name: Option<&'static str>,
                    danger: bool,
                    action: RowAction,
                    ctx: &RowCtx| {
            let dispatch = Rc::clone(&ctx.dispatch);
            let (fg, hover_fg) = if danger {
                (c::RED(), c::RED())
            } else {
                (c::FG_DIM(), c::FG())
            };
            let mut row = div()
                .id(SharedString::from(format!("menu-{proj}-{wt}-{label}")))
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .px(rpx(SPACE_XL))
                .py(rpx(SPACE_SM))
                .text_color(fg)
                .hover(move |s| s.bg(c::BG_HOVER()).text_color(hover_fg))
                .cursor_pointer();
            if let Some(glyph) = icon_name {
                row = row.child(crate::icons::icon(glyph, ICON_SM, fg));
            }
            row.child(
                // Deliberately *not* `components::ui`: the item's hover recolor
                // lives on the row, so this label must inherit its color.
                div()
                    .font(gpui::font(crate::fonts::UI_FAMILY))
                    .text_size(rpx(TEXT_BODY))
                    .child(label),
            )
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                dispatch(action, window, cx);
            })
        };

        let mut menu = div()
            .absolute()
            .top(top)
            .right(rpx(SPACE_LG))
            .flex()
            .flex_col()
            .py(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_GROUP))
            .bg(c::BG_STRIP())
            .border_1()
            .border_color(c::BORDER());
        // Same availability gate as the inline spawn chips, and the same
        // agent subset as the iced menu — only Codex/OpenCode
        // (`src/gui/widgets/primitives.rs:74-165`).
        for agent in [Agent::Codex, Agent::OpenCode] {
            if !ctx.available.contains(&agent) {
                continue;
            }
            menu = menu.child(item(
                agent.label(),
                Some(agent.icon_name()),
                false,
                RowAction::SpawnAgent(proj, wt, agent),
                ctx,
            ));
        }
        if !is_main {
            menu = menu.child(divider_h_strong().my(rpx(SPACE_SM))).child(item(
                "delete",
                None,
                true,
                RowAction::DeleteWorktree(proj, wt),
                ctx,
            ));
        }

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .child(backdrop)
            .child(menu)
    }

    /// A 1px `BORDER()` line centred in a 6px hit zone with a horizontal-resize
    /// cursor (`src/gui/view/sidebar.rs:72-86`).
    fn divider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        div()
            .id("sidebar-divider")
            .w(rpx(SIDEBAR_DIVIDER_W))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(div().w(px(1.0)).h_full().bg(c::BORDER()))
            .on_mouse_down(
                MouseButton::Left,
                move |_, window: &mut Window, cx: &mut App| {
                    let _ =
                        weak.update(cx, |this: &mut Self, cx| this.on_divider_press(window, cx));
                },
            )
    }
}

/// Wire the workspace root's pointer stream into an in-progress divider drag.
/// The root is the only element that keeps receiving moves once the cursor
/// leaves the 6px hit zone.
pub fn root_drag_listeners(sidebar: &Entity<Sidebar>, element: gpui::Div) -> gpui::Div {
    let move_target = sidebar.downgrade();
    let up_target = sidebar.downgrade();
    element
        .on_mouse_move(move |e: &MouseMoveEvent, window: &mut Window, cx| {
            // The stored sidebar width is a *logical* (design-px) value that
            // renders through `rpx`, so the cursor has to be divided back out
            // of zoom before it can be compared against it.
            let zoom = cx.global::<crate::zoom::ZoomState>().zoom.max(0.1);
            let x = f32::from(e.position.x) / zoom;
            let _ = move_target.update(cx, |this: &mut Sidebar, cx| {
                this.on_root_mouse_move(x, window, cx);
            });
        })
        .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
            let _ = up_target.update(cx, |this: &mut Sidebar, cx| this.on_root_mouse_up(cx));
        })
}
