//! The left rail: header, the scrolling project → worktree → session tree, the
//! agent-menu overlay, the docked TERMINALS header, and the draggable divider.
//!
//! Uses a plain scrollable `div` column rather than `uniform_list`: Grove's
//! rows are genuinely non-uniform (a branch chip makes a worktree row taller),
//! and `uniform_list` at this gpui rev requires uniform heights.

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;
use std::time::Instant;

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
use crate::entities::workspace_state::{RailMode, TreeExpand, WorkspaceState, RAIL_W};
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::components::{
    divider_h, divider_h_strong, icon_btn, mono, tracked, DividerDrag, DOUBLE_CLICK, DRAG_EPSILON,
};
use crate::views::rows::{self, RowAction, RowCtx, TreeRow};
use crate::views::session_header::SESSBAR_H;

pub struct Sidebar {
    modals: Option<Entity<crate::views::modals::ModalLayer>>,
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
    /// Rebuilt every frame by `render`; read by the divider handlers and [`Self::visible_session_order`].
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

    pub fn set_modals(
        &mut self,
        modals: Entity<crate::views::modals::ModalLayer>,
        toast: Entity<crate::entities::toast::ToastState>,
    ) {
        self.modals = Some(modals);
        self.toast = Some(toast);
    }

    fn open_modal(&mut self, modal: crate::modal::Modal, cx: &mut Context<Self>) {
        if let Some(layer) = self.modals.clone() {
            layer.update(cx, |l, cx| l.open(modal, cx));
        }
    }

    pub fn visible_session_order(&self) -> Vec<crate::entities::session_registry::SessionId> {
        rows::visible_session_order(&self.rows)
    }

    fn available_agents() -> Vec<Agent> {
        let found: Vec<Agent> = Agent::ALL.into_iter().filter(|a| a.available()).collect();
        if found.is_empty() {
            vec![Agent::Terminal]
        } else {
            found
        }
    }

    fn dispatch(&mut self, action: RowAction, window: &mut Window, cx: &mut Context<Self>) {
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
            // select_session moves proj_idx, so the ProjectTree hand-off SelectWorktree does must happen here too.
            RowAction::SelectSession(id) => {
                let old = self.state.read(cx).proj_idx();
                ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
                self.state.update(cx, |s, cx| {
                    s.select_session(id, &snap);
                    cx.notify();
                });
            }
            RowAction::OpenDiff(id) => {
                let Some(wt_path) = self.registry.read(cx).meta(id).map(|m| m.wt_path.clone())
                else {
                    return;
                };
                if let Some(layer) = self.modals.clone() {
                    layer.update(cx, |l, cx| {
                        l.open(crate::modal::Modal::DiffViewer { wt_path }, cx);
                    });
                }
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
            RowAction::ToggleRailMode => {
                let mode = self.state.update(cx, |s, cx| {
                    let mode = s.toggle_rail_mode();
                    cx.notify();
                    mode
                });
                SettingsState::update(cx, |s| s.rail_sessions = mode == RailMode::Sessions);
            }
            // Only Workspace can make this whole-workspace transition, so ask for it the way mod+g does.
            RowAction::ToggleGridView => {
                window.dispatch_action(Box::new(crate::keymap::ToggleGrid), cx);
            }
            RowAction::SpawnAgent(proj, wt, agent) => self.spawn_session(proj, wt, agent, cx),
            RowAction::AddWorktree(proj) => {
                self.state.update(cx, |s, cx| {
                    s.begin_add_worktree(proj);
                    cx.notify();
                });
                self.open_modal(
                    crate::modal::Modal::Input {
                        title: "New worktree".into(),
                        buffer: String::new(),
                        note: None,
                        // Seeded empty; `ModalLayer::open` kicks the background branch listing.
                        base: crate::modal::BaseBranchState::default(),
                    },
                    cx,
                );
            }
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
            RowAction::RunScript(proj, wt) => {
                self.state.update(cx, |s, cx| {
                    s.select_worktree(proj, wt, &snap);
                    if !s.term_panel_open() {
                        s.toggle_term_panel(true);
                    }
                    cx.notify();
                });
                let wt_path = snap
                    .projects
                    .iter()
                    .find(|p| p.idx == proj)
                    .and_then(|p| p.worktrees.get(wt))
                    .map(|w| w.path.clone());
                let Some(wt_path) = wt_path else {
                    return;
                };
                let script = {
                    let store = &cx.global::<SettingsState>().store;
                    grove_core::storage::project_for_worktree_path(&store.projects, &wt_path)
                        .and_then(|(_, p)| p.scripts.run.as_deref().map(str::trim))
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                let Some(script) = script else {
                    return;
                };
                crate::views::scripts::spawn_wt_script(
                    &self.registry,
                    &self.state,
                    self.toast.as_ref(),
                    &wt_path,
                    &script,
                    cx,
                );
            }
            RowAction::AddProject => {
                self.open_modal(crate::modal::Modal::AddProject(Box::default()), cx);
            }
            RowAction::LaunchInWorktree => {
                self.open_modal(
                    crate::modal::Modal::SessionLauncher(Box::new(
                        crate::modal::LauncherSlotState {
                            scope: crate::launcher::PaletteScope::WorktreesOnly,
                            ..Default::default()
                        },
                    )),
                    cx,
                );
            }
        }
    }

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

    /// Spawn by concrete project name + worktree path; the palette's target may live in a project with no cached snapshot position.
    pub fn spawn_session_in(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        let args = {
            let store = &cx.global::<SettingsState>().store;
            agent.launch_args(
                store.dangerously_skip_permissions_enabled.unwrap_or(false),
                store.chrome_enabled.unwrap_or(false),
            )
        };
        self.spawn_session_in_with_args(name, cwd, agent, args, cx)
    }

    /// Spawn one agent session with a primary worktree and agent-specific extra arguments.
    pub fn spawn_session_in_with_args(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.spawn_session_in_with_context(name, cwd, agent, args, Vec::new(), cx)
    }

    /// Spawn one agent session with its ordered writable worktree context.
    pub fn spawn_session_in_with_context(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        context_roots: Vec<grove_core::session_meta::ContextRoot>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
        let (id, extra_args, state_file, target) = self.registry.update(cx, |r, cx| {
            let id =
                r.insert_meta_with_context(name.clone(), cwd.clone(), agent, context_roots.clone());
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
                    context_roots,
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
        spawn_error.is_none()
    }

    /// Leaves the grid first, or a terminal spawned behind the tiles would be invisible.
    pub fn new_home_terminal(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.exit_grid_for_terminal();
            cx.notify();
        });
        self.spawn_home_terminal(cx);
    }

    pub fn spawn_home_terminal(&mut self, cx: &mut Context<Self>) {
        let (label, target) = self.registry.update(cx, |r, _| {
            let label = r.next_home_label();
            (
                label.clone(),
                crate::entities::session_registry::SpawnTarget::home(label),
            )
        });
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
                    context_roots: Vec::new(),
                    label,
                    spawned_at: Instant::now(),
                    attention: None,
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

    /// Always ≥1 home terminal: closing the last one immediately respawns a fresh shell.
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

    fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
        let active_proj = self.state.read(cx).proj_idx();
        let registry = self.registry.clone();
        self.tree.clone().update(cx, |tree, cx| {
            let store = &cx.global::<SettingsState>().store;
            tree.snapshot(store, registry.read(cx), active_proj)
        })
    }

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

    pub fn on_root_mouse_move(&mut self, cursor_x: f32, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        let offset = match drag.grab_offset {
            Some(o) => o,
            None => {
                // Captured on first move so an off-edge press does not make the width jump.
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
        // A plain click must not write to disk.
        if (self.state.read(cx).sidebar_width() - drag.start_width).abs() >= DRAG_EPSILON {
            self.persist_width(cx);
        }
    }

    fn persist_width(&self, cx: &mut Context<Self>) {
        let width = self.state.read(cx).sidebar_width();
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
        // Runs every frame until the user touches the tree manually, so late-arriving sessions still get the default expansion.
        self.state.update(cx, |s, _| s.sync_default_tree(&snap));
        let session_info: std::collections::HashMap<_, _> = {
            let registry = self.registry.read(cx);
            registry
                .all()
                .iter()
                .map(|m| {
                    let title = registry
                        .session(m.id)
                        .and_then(|e| e.read(cx).title())
                        .and_then(|raw| {
                            rows::session_context(
                                &raw,
                                &rows::path_basename(&m.wt_path),
                                &m.label,
                                m.agent.label(),
                            )
                        });
                    (
                        m.id,
                        rows::SessionInfo {
                            project: m.project.clone(),
                            wt_path: m.wt_path.clone(),
                            label: m.label.clone(),
                            agent: m.agent,
                            context_roots: m.context_roots.clone(),
                            title,
                            spawned_at: m.spawned_at,
                        },
                    )
                })
                .collect()
        };
        let git_states = self.tree.read(cx).git_states();
        let (
            rows,
            tick,
            pulse,
            width,
            terminals_collapsed,
            open_menu,
            hovered_wt,
            next_glyph,
            rail_mode,
            grid_view,
        ) = {
            let ws = self.state.read(cx);
            let activity = self.activity.read(cx);
            let rail_mode = ws.rail_mode();
            let rows = match rail_mode {
                RailMode::Tree => rows::flatten(&snap, ws, activity, &git_suffix, &home_running),
                RailMode::Sessions => rows::flatten_sessions(
                    &snap,
                    ws,
                    activity,
                    &session_info,
                    &git_states,
                    &home_running,
                ),
            };
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
                rail_mode,
                ws.grid_view(),
            )
        };
        self.rows.clone_from(&rows);
        let order = self.visible_session_order();
        self.state.update(cx, |s, _| s.set_visible_order(order));

        let ctx = self.row_ctx(&rows, tick, pulse, hovered_wt, cx);
        let menu_top = open_menu.and_then(|open| rows::agent_menu_top(&rows, open));

        // TERMINALS section belongs docked at the bottom, not in the scrolling tree — split it off here.
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
            .track_scroll(&self.scroll)
            // Sessions mode is an inset card list; tree rows are full-bleed.
            .when(rail_mode == RailMode::Sessions, |d| {
                d.px(rpx(SPACE_LG)).gap(rpx(SPACE_MD))
            });
        for row in tree_rows {
            list = list.child(rows::render_row(row, &ctx));
        }

        let mut tree_area = div().relative().flex_1().w_full().child(list);
        if let Some((proj, wt, top, is_main)) = menu_top {
            // Row offset is design px (zooms); scroll offset is real window pixels — resolve both to pixels before adding.
            let top = rpx(top).to_pixels(window.rem_size()) + self.scroll.offset().y;
            tree_area = tree_area.child(self.agent_menu(proj, wt, top, is_main, &ctx));
        }

        let mut rail = div()
            .w(rpx(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(c::BG_RAIL())
            .child(self.header(next_glyph, rail_mode, grid_view, &ctx))
            .child(divider_h())
            .child(tree_area);
        if !term_rows.is_empty() {
            // Capped at 20% of the rail height so a long list scrolls internally instead of pushing the tree off-screen.
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
        // No per-row sanitize cache ported — no profile has shown it mattering here.
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
            dispatch: Rc::new(move |action, window, cx: &mut App| {
                let _ = weak.update(cx, |this: &mut Self, cx| this.dispatch(action, window, cx));
            }),
        }
    }

    fn header(
        &self,
        next_glyph: &'static str,
        rail_mode: RailMode,
        grid_view: bool,
        ctx: &RowCtx,
    ) -> impl IntoElement {
        let dispatch = Rc::clone(&ctx.dispatch);
        let mode_glyph = match rail_mode {
            RailMode::Tree => "rail-sessions",
            RailMode::Sessions => "rail-tree",
        };
        let mode_btn = {
            let dispatch = Rc::clone(&dispatch);
            icon_btn(
                "rail-mode",
                mode_glyph,
                CONTROL_H,
                CONTROL_H,
                ICON_SM,
                c::FG_MUTE(),
                c::BG_HOVER(),
                Some(c::FG()),
                false,
                move |window, cx| dispatch(RowAction::ToggleRailMode, window, cx),
            )
        };
        // State button, not a preview one: CYAN while the grid is up.
        let grid_btn = {
            let dispatch = Rc::clone(&dispatch);
            icon_btn(
                "rail-grid",
                "grid",
                CONTROL_H,
                CONTROL_H,
                ICON_SM,
                if grid_view { c::CYAN() } else { c::FG_MUTE() },
                c::BG_HOVER(),
                Some(if grid_view { c::CYAN() } else { c::FG() }),
                false,
                move |window, cx| dispatch(RowAction::ToggleGridView, window, cx),
            )
        };
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
                    SharedString::from(tracked(match rail_mode {
                        RailMode::Tree => "PROJECTS",
                        RailMode::Sessions => "SESSIONS",
                    })),
                    TEXT_MICRO,
                    c::FG_MUTE(),
                )
                .flex_1(),
            )
            .child({
                let dispatch = std::rc::Rc::clone(&dispatch);
                let add = match rail_mode {
                    RailMode::Tree => RowAction::AddProject,
                    RailMode::Sessions => RowAction::LaunchInWorktree,
                };
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
                    move |window, cx| dispatch(add, window, cx),
                )
            })
            .child(grid_btn)
            .child(mode_btn)
            // Nothing to cycle in the flat sessions list.
            .when(rail_mode == RailMode::Tree, |d| d.child(toggle))
    }

    /// Anchored by [`rows::agent_menu_top`] so it tracks the row at any scroll position/collapse state.
    fn agent_menu(
        &self,
        proj: usize,
        wt: usize,
        top: gpui::Pixels,
        is_main: bool,
        ctx: &RowCtx,
    ) -> impl IntoElement {
        // Click-away backdrop: full-bleed transparent layer beneath the menu that dismisses it.
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
                // Not components::ui — the hover recolor lives on the row, so this label must inherit it.
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

    fn divider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        div()
            .id("sidebar-divider")
            .w(rpx(DIVIDER_DRAG_HIT_W))
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

/// Wires the workspace root's pointer stream into an in-progress divider drag — the root keeps receiving moves once the cursor leaves the hit zone.
pub fn root_drag_listeners(sidebar: &Entity<Sidebar>, element: gpui::Div) -> gpui::Div {
    let move_target = sidebar.downgrade();
    let up_target = sidebar.downgrade();
    element
        .on_mouse_move(move |e: &MouseMoveEvent, window: &mut Window, cx| {
            // Sidebar width is logical (design-px); divide the cursor back out of zoom first.
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
