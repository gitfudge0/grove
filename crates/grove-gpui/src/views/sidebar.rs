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

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    div, px, App, Context, CursorStyle, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollHandle, SharedString, Window,
};
use grove_core::agent::Agent;

use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::SessionRegistry;
use crate::entities::workspace_state::{TreeExpand, WorkspaceState, RAIL_W};
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::rows::{self, RowAction, RowCtx, TreeRow};

/// Session bar / header height (`src/gui/metrics.rs:17`).
const SESSBAR_H: f32 = 36.0;
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
    pub state: Entity<WorkspaceState>,
    pub tree: Entity<ProjectTree>,
    pub registry: Entity<SessionRegistry>,
    pub activity: Entity<ActivityStore>,
    clock: Entity<AnimationClock>,
    scroll: ScrollHandle,
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
            state,
            tree,
            registry,
            activity,
            clock,
            scroll: ScrollHandle::new(),
            drag: None,
            last_divider_press: None,
            rows: Vec::new(),
            _observers: observers,
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
            RowAction::SelectSession(id) => self.state.update(cx, |s, cx| {
                s.select_session(id, &snap);
                cx.notify();
            }),
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
            RowAction::NewHomeTerminal => self.spawn_home_terminal(cx),
            RowAction::ToggleTerminalsSection => self.state.update(cx, |s, cx| {
                s.toggle_terminals_collapsed();
                cx.notify();
            }),
            RowAction::ToggleCollapseAll => self.state.update(cx, |s, cx| {
                s.toggle_collapse_all(&snap);
                cx.notify();
            }),
            RowAction::SpawnAgent(proj, wt, agent) => self.spawn_session(proj, wt, agent, cx),
            RowAction::AddWorktree(_) => tracing::debug!("AddWorktree: modal — Plan 08"),
            RowAction::DeleteWorktree(..) => tracing::debug!("DeleteWorktree: modal — Plan 08"),
            RowAction::RemoveProject(_) => tracing::debug!("RemoveProject: modal — Plan 08"),
            RowAction::ProjectScripts(_) => tracing::debug!("ProjectScripts: modal — Plan 08"),
            RowAction::RunScript(..) => tracing::debug!("RunScript: Plan 08"),
        }
    }

    // ── registry mutation ───────────────────────────────────────────────

    fn spawn_session(&mut self, proj: usize, wt: usize, agent: Agent, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
        let snap = self.snapshot(cx);
        let Some(project) = snap.projects.iter().find(|p| p.idx == proj) else {
            return;
        };
        let Some(worktree) = project.worktrees.get(wt) else {
            return;
        };
        let (name, cwd) = (project.name.clone(), worktree.path.clone());
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
        self.registry.update(cx, |r, cx| {
            r.attach(id, session);
            cx.notify();
        });
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
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
    fn close_home_terminal(&mut self, i: usize, cx: &mut Context<Self>) {
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
        let home_count = self.registry.read(cx).home_terminal_count();
        let (rows, tick, pulse, width, terminals_collapsed, open_menu, hovered_wt, next_glyph) = {
            let ws = self.state.read(cx);
            let activity = self.activity.read(cx);
            let rows = rows::flatten(&snap, ws, activity, &git_suffix, home_count);
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

        let mut list = div()
            .id("sidebar-tree")
            .flex()
            .flex_col()
            .size_full()
            .pt(px(8.0))
            .pb(px(12.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for row in &rows {
            list = list.child(rows::render_row(row, &ctx));
        }

        let mut tree_area = div().relative().flex_1().w_full().child(list);
        if let Some((proj, wt, top, is_main)) = menu_top {
            tree_area = tree_area.child(self.agent_menu(proj, wt, top, is_main, &ctx));
        }

        let mut rail = div()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(c::BG_RAIL())
            .child(self.header(next_glyph, &ctx))
            .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
            .child(tree_area);
        if terminals_collapsed {
            // Docked outside the scroll area so it is always reachable; the dot
            // is on iff a shell is running (`sidebar.rs:114-129`, `:61-70`).
            rail = rail
                .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
                .child(rows::terminals_header(
                    false,
                    home_count,
                    home_count > 0,
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
            div()
                .id("tree-cycle")
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .text_color(c::FG_MUTE())
                .hover(|s| s.bg(c::BG_HOVER()).text_color(c::FG()))
                .child(crate::icons::icon(next_glyph, 13.0, c::FG_MUTE()))
        };
        div()
            .h(px(SESSBAR_H))
            .w_full()
            .flex()
            .items_center()
            .pl(px(14.0))
            .pr(px(8.0))
            .child(
                div()
                    .flex_1()
                    .font(gpui::font(crate::fonts::UI_FAMILY))
                    .text_size(px(11.0))
                    .text_color(c::FG_MUTE())
                    .child(SharedString::from(rows::tracked("PROJECTS"))),
            )
            .child(
                div()
                    .id("proj-add")
                    .size(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .text_color(c::FG_MUTE())
                    .hover(|s| s.bg(c::BG_HOVER()).text_color(c::FG()))
                    .child(crate::icons::icon("plus", 12.0, c::FG_MUTE()))
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        tracing::debug!("AddProject: modal — Plan 08");
                    }),
            )
            .child(toggle.on_mouse_down(MouseButton::Left, {
                move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    dispatch(RowAction::ToggleCollapseAll, window, cx);
                }
            }))
    }

    /// The absolutely-positioned agent menu over the list
    /// (`src/gui/view/sidebar.rs:99-109`), anchored by
    /// [`rows::agent_menu_top`] so it tracks the row at any scroll position
    /// and collapse state.
    fn agent_menu(
        &self,
        proj: usize,
        wt: usize,
        top: f32,
        _is_main: bool,
        ctx: &RowCtx,
    ) -> impl IntoElement {
        let mut menu = div()
            .absolute()
            .top(px(top + f32::from(self.scroll.offset().y)))
            .left(px(40.0))
            .flex()
            .flex_col()
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(c::BG_STRIP())
            .border_1()
            .border_color(c::BORDER());
        for agent in &ctx.available {
            let dispatch = Rc::clone(&ctx.dispatch);
            let agent = *agent;
            menu = menu.child(
                div()
                    .id(SharedString::from(format!(
                        "menu-{proj}-{wt}-{}",
                        agent.label()
                    )))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_color(c::FG_DIM())
                    .hover(|s| s.bg(c::BG_HOVER()).text_color(c::FG()))
                    .child(crate::icons::icon(agent.icon_name(), 12.0, c::FG_DIM()))
                    .child(
                        div()
                            .font(gpui::font(crate::fonts::UI_FAMILY))
                            .text_size(px(12.0))
                            .child(agent.label()),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        dispatch(RowAction::SpawnAgent(proj, wt, agent), window, cx);
                    }),
            );
        }
        menu
    }

    /// A 1px `BORDER()` line centred in a 6px hit zone with a horizontal-resize
    /// cursor (`src/gui/view/sidebar.rs:72-86`).
    fn divider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        div()
            .id("sidebar-divider")
            .w(px(SIDEBAR_DIVIDER_W))
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
            let x = f32::from(e.position.x);
            let _ = move_target.update(cx, |this: &mut Sidebar, cx| {
                this.on_root_mouse_move(x, window, cx);
            });
        })
        .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
            let _ = up_target.update(cx, |this: &mut Sidebar, cx| this.on_root_mouse_up(cx));
        })
}
