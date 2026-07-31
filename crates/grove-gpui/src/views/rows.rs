//! The sidebar's row model: the pure helpers each row renders from, and the
//! **flattening** that turns the project → worktree → session tree into the
//! single `Vec<TreeRow>` the view scrolls.
//!
//! # Flattening decision (carried amendment 2)
//!
//! Rows are emitted **pre-resolved**: every field a renderer needs is on the
//! row. Nothing looks back into `WorkspaceState` / the registry per row, which
//! is what made the iced version O(projects × worktrees × sessions) per frame
//! (`src/gui/view/sidebar.rs:227-237`).
//!
//! Rows are *not* uniformly tall in the iced build — a worktree showing a
//! branch chip is `ROW_H + 14` (`src/gui/rows.rs:268`). [`row_height`] is the
//! single height function; **`uniform_list` is therefore not usable as-is** and
//! the decision belongs to Task 5, which renders. Whatever it picks, it and the
//! agent-menu overlay walk must both call [`row_height`], or the overlay lands
//! on the wrong row.

// The renderers that consume these land in Task 5.
#![allow(dead_code)]

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, SharedString,
    Window,
};
use grove_core::agent::Agent;

use crate::activity::{most_urgent, ActivityState};
use crate::entities::activity_store::ActivityStore;
use crate::entities::session_registry::SessionId;
use crate::entities::workspace_state::{TreeSnapshot, WorkspaceState};
use crate::theme as c;
use crate::{fonts, icons};

/// Sidebar row height (`src/gui/metrics.rs:7`).
pub const ROW_H: f32 = 28.0;

// ── pure row helpers ───────────────────────────────────────────────────────

/// Only show a branch chip for non-default worktrees: the main worktree's
/// branch is redundant with the project name (`src/gui/rows.rs:249`).
#[must_use]
pub fn worktree_shows_branch(is_main: bool, branch: &str, name: &str) -> bool {
    !is_main && branch != name && !branch.is_empty()
}

/// Rendered height of a worktree row (`src/gui/rows.rs:268`). The agent-menu
/// overlay position is computed from this, so the renderer must agree with it.
#[must_use]
pub fn row_height(show_branch: bool) -> f32 {
    if show_branch {
        ROW_H + 14.0
    } else {
        ROW_H
    }
}

/// Basename of a path, falling back to the whole string.
///
/// Reimplemented rather than moved out of `crate::app::path_basename`
/// (`src/app/util.rs:23-29`) per Global Constraint 3 candidate 1 — it is three
/// lines of path arithmetic, and grove-core/the iced app stay read-only.
#[must_use]
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

/// Strip characters the UI font cannot render — emoji, box drawing,
/// private-use — and collapse the resulting whitespace
/// (`src/gui/rows.rs:809-834`). `None` when nothing useful is left.
#[must_use]
pub fn sanitize_ui_text(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
            *c == ' '
                || *c == '·'
                || (*c >= '\u{0020}' && *c <= '\u{007E}')
                || (*c >= '\u{00A0}' && *c <= '\u{024F}')
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '·' | '-' | ':' | '|' | '/'))
        .to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Remove every case-insensitive occurrence of `needle` from `hay`, UTF-8-safe
/// (`src/gui/rows.rs:895-920`).
#[must_use]
pub fn remove_all_ci(hay: &str, needle: &str) -> String {
    if needle.is_empty() {
        return hay.to_string();
    }
    let hay_lower = hay.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(hay.len());
    let mut cursor = 0;
    while let Some(rel) = hay_lower[cursor..].find(&needle_lower) {
        let start = cursor + rel;
        out.push_str(&hay[cursor..start]);
        cursor = start + needle_lower.len();
    }
    out.push_str(&hay[cursor..]);
    out
}

/// Contextual title for a home terminal: its OSC title with the internal
/// `terminal N` label stripped (`src/gui/rows.rs:776-782`).
///
/// The iced build memoizes this per session (`cached_context`, `:748`) because
/// `view()` runs every 33ms tick; the gpui rows are built once per repaint from
/// an already-materialized title, so **the cache is deliberately not ported**.
#[must_use]
pub fn terminal_context(raw_title: &str, label: &str) -> Option<String> {
    sanitize_ui_text(&remove_all_ci(raw_title, label))
}

/// The short context string next to the agent name: the OSC title with the
/// worktree name, the session label and the agent label stripped out, so what
/// is left is the actual task (`src/gui/rows.rs:784-806`).
#[must_use]
pub fn session_context(
    raw_title: &str,
    wt_name: &str,
    label: &str,
    agent_label: &str,
) -> Option<String> {
    let mut out = raw_title.to_string();
    for needle in [wt_name, label, agent_label] {
        if needle.is_empty() {
            continue;
        }
        out = remove_all_ci(&out, needle);
    }
    sanitize_ui_text(&out)
}

/// Title/subtitle for the sidebar project-tree empty state, or `None` when the
/// tree has rows (`src/gui/widgets/primitives.rs:195-220`).
///
/// The two states must never share copy: each has a different fix, and one
/// message would send the user to the wrong place. This pair is also how this
/// phase satisfies Appendix A's "archived-projects row" — the archived *list*
/// is a Settings modal (Plan 08); the tree only ever shows `active_projects`.
#[must_use]
pub fn sidebar_empty_copy(
    total_projects: usize,
    active_projects: usize,
) -> Option<(&'static str, &'static str)> {
    match (total_projects, active_projects) {
        (_, a) if a > 0 => None,
        (0, _) => Some(("No projects yet", "Add one with + above.")),
        _ => Some((
            "All projects archived",
            "Restore one from Settings → Archived projects.",
        )),
    }
}

// ── the flattened tree ─────────────────────────────────────────────────────

/// One row of the sidebar, carrying everything its renderer needs.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeRow {
    Project {
        /// TRUE `store.projects` index (`storage.rs:174`).
        idx: usize,
        name: String,
        count: usize,
        expanded: bool,
        is_git: bool,
        rollup: Option<ActivityState>,
    },
    Worktree {
        proj: usize,
        wt: usize,
        name: String,
        branch: String,
        is_main: bool,
        active: bool,
        expanded: bool,
        has_run: bool,
        rollup: Option<ActivityState>,
        git_suffix: Option<String>,
    },
    Session {
        id: SessionId,
        active: bool,
        pending_kill: bool,
        state: ActivityState,
    },
    Empty {
        title: &'static str,
        subtitle: &'static str,
    },
    TerminalsHeader {
        expanded: bool,
        count: usize,
        activity_dot: bool,
    },
    Terminal {
        idx: usize,
        active: bool,
        pending_kill: bool,
    },
}

impl TreeRow {
    /// This row's rendered height. Only worktrees showing a branch chip differ
    /// (`src/gui/rows.rs:268`).
    #[must_use]
    pub fn height(&self) -> f32 {
        match self {
            Self::Worktree {
                name,
                branch,
                is_main,
                ..
            } => row_height(worktree_shows_branch(*is_main, branch, name)),
            _ => ROW_H,
        }
    }
}

/// Build the sidebar's rows, in exactly the order `tree_view` pushes them
/// (`src/gui/view/sidebar.rs:225-381`).
///
/// `git_suffix` is the off-thread git-state map, already rendered to text by
/// `grove_core::git::git_state_suffix`.
#[must_use]
pub fn flatten(
    snap: &TreeSnapshot,
    ws: &WorkspaceState,
    activity: &ActivityStore,
    git_suffix: &HashMap<String, String>,
    home_terminal_count: usize,
) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut any_active = false;

    for p in &snap.projects {
        any_active = true;
        let expanded = !ws.project_collapsed(p.idx);
        let sessions: Vec<SessionId> = p
            .worktrees
            .iter()
            .flat_map(|w| w.sessions.iter().copied())
            .collect();
        rows.push(TreeRow::Project {
            idx: p.idx,
            name: p.name.clone(),
            count: sessions.len(),
            expanded,
            is_git: p.is_git,
            // Collapsed parents surface the most urgent descendant state;
            // expanded parents show nothing extra (`sidebar.rs:251-257`).
            rollup: (!expanded)
                .then(|| most_urgent(sessions.iter().map(|&id| activity.state_of(id))))
                .flatten(),
        });
        if !expanded {
            continue;
        }

        for (wi, w) in p.worktrees.iter().enumerate() {
            let wt_expanded = !ws.worktree_collapsed(p.idx, wi);
            rows.push(TreeRow::Worktree {
                proj: p.idx,
                wt: wi,
                name: w.name.clone(),
                branch: w.branch.clone(),
                is_main: w.is_main,
                active: p.idx == ws.proj_idx() && wi == ws.wt_idx(),
                expanded: wt_expanded,
                has_run: p.has_run,
                // Same roll-up rule as projects (`sidebar.rs:296-303`).
                rollup: (!wt_expanded)
                    .then(|| most_urgent(w.sessions.iter().map(|&id| activity.state_of(id))))
                    .flatten(),
                git_suffix: git_suffix.get(&w.path).cloned(),
            });
            if !wt_expanded {
                continue;
            }
            for &id in &w.sessions {
                rows.push(TreeRow::Session {
                    id,
                    // A session must not look active while a home terminal is
                    // on screen (`sidebar.rs:338`).
                    active: !ws.terminal_focused() && ws.active_session() == Some(id),
                    pending_kill: ws.pending_kill() == Some(id),
                    state: activity.state_of(id),
                });
            }
        }
    }

    if let Some((title, subtitle)) =
        sidebar_empty_copy(snap.total_projects, usize::from(any_active))
    {
        rows.push(TreeRow::Empty { title, subtitle });
    }

    if !ws.terminals_collapsed() {
        // Expanded: every terminal renders its own row below, so the header's
        // "something is running in here" dot would be redundant — always off
        // (`sidebar.rs:363-372`). The divider above it is the view's job.
        rows.push(TreeRow::TerminalsHeader {
            expanded: true,
            count: home_terminal_count,
            activity_dot: false,
        });
        for i in 0..home_terminal_count {
            rows.push(TreeRow::Terminal {
                idx: i,
                active: ws.terminal_focused() && ws.active_terminal() == Some(i),
                pending_kill: ws.pending_kill_terminal() == Some(i),
            });
        }
    }

    rows
}

/// Session ids in the order the tree renders them, honoring both collapse sets
/// (`src/gui/view/sidebar.rs:386-417`).
///
/// Derived from [`flatten`]'s output rather than walking the tree a second
/// time: this is `mod+1..9`'s index space **and** the attention queue's order
/// (`update/mod.rs:728-739`), and two walks would be two chances to drift.
#[must_use]
pub fn visible_session_order(rows: &[TreeRow]) -> Vec<SessionId> {
    rows.iter()
        .filter_map(|r| match r {
            TreeRow::Session { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

// ── the agent-menu overlay walk ────────────────────────────────────────────

/// Y-offset of the agent menu for the worktree it is open on, walking the
/// **same** rows the list lays out (`src/gui/view/sidebar.rs:421-470`).
///
/// Carried amendment 2: this and the list share [`row_height`] through
/// [`TreeRow::height`], so the overlay cannot land on the wrong row. Returns
/// `(proj, wt, top, is_main)`; the `6.0` is the tree area's top padding minus
/// the menu's own 2px lift (`sidebar.rs:449`).
#[must_use]
pub fn agent_menu_top(rows: &[TreeRow], open: (usize, usize)) -> Option<(usize, usize, f32, bool)> {
    let mut acc_y = 0.0_f32;
    for row in rows {
        if let TreeRow::Worktree {
            proj, wt, is_main, ..
        } = row
        {
            if (*proj, *wt) == open {
                return Some((*proj, *wt, 6.0 + acc_y + row.height(), *is_main));
            }
        }
        acc_y += row.height();
    }
    None
}

// ── renderers ──────────────────────────────────────────────────────────────

/// What a row click asks the [`crate::views::sidebar::Sidebar`] to do. Rows
/// never reach into state themselves: they emit one of these and the view is
/// the single place that decides what it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAction {
    SelectProject(usize),
    RemoveProject(usize),
    ProjectScripts(usize),
    AddWorktree(usize),
    SelectWorktree(usize, usize),
    HoverWorktree(Option<(usize, usize)>),
    OpenAgentMenu(Option<(usize, usize)>),
    SpawnAgent(usize, usize, Agent),
    RunScript(usize, usize),
    DeleteWorktree(usize, usize),
    SelectSession(SessionId),
    ArmKillSession(SessionId),
    KillSession(SessionId),
    SelectTerminal(usize),
    ArmKillTerminal(usize),
    KillTerminal(usize),
    NewHomeTerminal,
    ToggleTerminalsSection,
    ToggleCollapseAll,
}

/// The one place a row's click becomes a state change.
pub type Dispatch = Rc<dyn Fn(RowAction, &mut Window, &mut App)>;

/// Everything a renderer needs beyond the row itself.
pub struct RowCtx {
    /// Animation clock, driving the Working spinner.
    pub tick: u64,
    /// Attention pulse in `[0, 1]` (0 = opaque, 1 = max dim).
    pub pulse: f32,
    /// Worktree under the mouse — drives the action strip.
    pub hovered_wt: Option<(usize, usize)>,
    /// Agents found on PATH, for the spawn strip (`src/app/mod.rs:168`).
    pub available: Vec<Agent>,
    /// Per-session display text: `(agent, context)`, resolved by the view.
    pub session_text: HashMap<SessionId, (Agent, Option<String>)>,
    /// Per-home-terminal context text, positional like the rows themselves.
    pub terminal_text: Vec<Option<String>>,
    pub dispatch: Dispatch,
}

impl RowCtx {
    fn on(&self, action: RowAction) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
        let dispatch = Rc::clone(&self.dispatch);
        move |_, window, cx| dispatch(action, window, cx)
    }
}

/// Status glyph in a fixed 14px slot (`src/gui/rows.rs:870-892`). Working spins
/// on the clock tick; WaitingForInput **dims** rather than hides, so the row
/// layout never moves.
pub fn state_glyph(state: ActivityState, tick: u64, pulse: f32) -> AnyElement {
    let inner = match state {
        ActivityState::Working => icons::spinner(11.0, c::GREEN(), tick),
        ActivityState::WaitingForInput => icons::icon(
            "question",
            11.0,
            Hsla {
                a: 1.0 - 0.45 * pulse,
                ..c::AMBER()
            },
        ),
        ActivityState::Done => icons::icon("check", 11.0, c::FG_MUTE()),
        ActivityState::Idle => icons::icon("dot", 11.0, c::FG_MUTE()),
        ActivityState::Exited => icons::icon("ring", 11.0, c::FG_MUTE()),
    };
    div()
        .w(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
        .into_any_element()
}

/// A 22px square icon button: transparent + `FG_MUTE` at rest, `BG_HOVER` +
/// `FG` on hover (`src/gui/view/sidebar.rs:165-172`).
fn tool_button(
    id: &'static str,
    key: impl std::fmt::Display,
    glyph: &'static str,
    size: f32,
    danger: bool,
    ctx: &RowCtx,
    action: RowAction,
) -> AnyElement {
    let (hover_bg, hover_fg) = if danger {
        (c::RED_WASH(), c::RED())
    } else {
        (c::BG_HOVER(), c::FG())
    };
    div()
        .id(SharedString::from(format!("{id}-{key}")))
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_color(c::FG_MUTE())
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .child(icons::icon(glyph, size, c::FG_MUTE()))
        .on_mouse_down(MouseButton::Left, ctx.on(action))
        .into_any_element()
}

pub fn ui_text(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(fonts::UI_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into())
}

/// The `main` tag (`src/gui/rows.rs:253-265`). Shares its slot with the hover
/// icons so the two never compete for width.
fn main_tag() -> AnyElement {
    ui_text("main", 10.0, c::GREEN()).into_any_element()
}

/// Letter-spaced section label. Neither iced nor gpui at this rev has
/// letter-spacing, so the characters are joined with U+2009 THIN SPACE exactly
/// as `src/gui/rows.rs:650-655` does.
#[must_use]
pub fn tracked(label: &str) -> String {
    label
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join("\u{2009}")
}

/// One row, fully resolved. Reads only from `row` and `ctx` — never back into
/// the tree (`src/gui/view/sidebar.rs:227-237`).
pub fn render_row(row: &TreeRow, ctx: &RowCtx) -> AnyElement {
    match row {
        TreeRow::Project {
            idx,
            name,
            count,
            expanded,
            is_git,
            rollup,
        } => project_row(*idx, name, *count, *expanded, *is_git, *rollup, ctx),
        TreeRow::Worktree { .. } => worktree_row(row, ctx),
        TreeRow::Session {
            id,
            active,
            pending_kill,
            state,
        } => session_row(*id, *active, *pending_kill, *state, ctx),
        TreeRow::Empty { title, subtitle } => empty_row(title, subtitle),
        TreeRow::TerminalsHeader {
            expanded,
            count,
            activity_dot,
        } => terminals_header(*expanded, *count, *activity_dot, ctx),
        TreeRow::Terminal {
            idx,
            active,
            pending_kill,
        } => terminal_row(*idx, *active, *pending_kill, ctx),
    }
}

fn project_row(
    idx: usize,
    name: &str,
    count: usize,
    expanded: bool,
    is_git: bool,
    rollup: Option<ActivityState>,
    ctx: &RowCtx,
) -> AnyElement {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let count_color = if count > 0 { c::GREEN() } else { c::FG_MUTE() };
    let mut right = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .pr(px(8.0))
        .when_some(rollup, |d, st| {
            d.child(state_glyph(st, ctx.tick, ctx.pulse))
        });
    if is_git {
        // Worktrees and worktree-lifecycle scripts are git-only.
        right = right
            .child(tool_button(
                "wt-add",
                idx,
                "plus",
                12.0,
                false,
                ctx,
                RowAction::AddWorktree(idx),
            ))
            .child(tool_button(
                "proj-scripts",
                idx,
                "cog",
                12.0,
                false,
                ctx,
                RowAction::ProjectScripts(idx),
            ));
    } else {
        right = right.child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(icons::icon("no-git", 11.0, c::FG_MUTE()))
                .child(ui_text("no git", 10.0, c::FG_MUTE())),
        );
    }
    right = right.child(tool_button(
        "proj-remove",
        idx,
        "trash",
        12.0,
        true,
        ctx,
        RowAction::RemoveProject(idx),
    ));

    div()
        .id(SharedString::from(format!("proj-{idx}")))
        .h(px(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(8.0))
                .pl(px(12.0))
                .pr(px(4.0))
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectProject(idx)))
                .child(
                    div()
                        .w(px(14.0))
                        .child(icons::icon(twist, 10.0, c::FG_MUTE())),
                )
                .child(
                    ui_text(name.to_uppercase(), 12.0, c::FG())
                        .font_weight(FontWeight::BOLD)
                        .overflow_hidden(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(dot(count_color))
                        .child(ui_text(format!("{count}"), 11.0, count_color)),
                ),
        )
        .child(right)
        .into_any_element()
}

fn dot(color: Hsla) -> Div {
    div().size(px(6.0)).rounded_full().bg(color)
}

fn worktree_row(row: &TreeRow, ctx: &RowCtx) -> AnyElement {
    let TreeRow::Worktree {
        proj,
        wt,
        name,
        branch,
        is_main,
        active,
        expanded,
        has_run,
        rollup,
        git_suffix,
    } = row
    else {
        return div().into_any_element();
    };
    let (proj, wt) = (*proj, *wt);
    let show_branch = worktree_shows_branch(*is_main, branch, name);
    let h = row.height();
    let twist = if *expanded { "chev-down" } else { "chev-right" };
    let hovered = ctx.hovered_wt == Some((proj, wt));

    let mut label = div().flex().flex_col().overflow_hidden();
    label = label.child(ui_text(name.clone(), 13.0, c::FG_DIM()).overflow_hidden());
    if show_branch {
        // Branch chip: a soft-bordered pill under the name.
        label = label.child(
            div().pt(px(2.0)).child(
                ui_text(branch.clone(), 10.0, c::FG_DIM())
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(c::BORDER_SOFT()),
            ),
        );
    }

    // The `main` tag and the hover action strip share one fixed right-hand
    // slot, so they never render at once and never shift the layout
    // (`src/gui/rows.rs:253-265,396-420`).
    let actions: AnyElement = if hovered {
        let mut strip = div().flex().items_center().gap(px(6.0)).pr(px(8.0));
        for agent in &ctx.available {
            strip = strip.child(tool_button(
                "wt-spawn",
                format!("{proj}-{wt}-{}", agent.label()),
                agent.icon_name(),
                12.0,
                false,
                ctx,
                RowAction::SpawnAgent(proj, wt, *agent),
            ));
        }
        if *has_run {
            strip = strip.child(tool_button(
                "wt-run",
                format!("{proj}-{wt}"),
                "play",
                12.0,
                false,
                ctx,
                RowAction::RunScript(proj, wt),
            ));
        }
        strip = strip.child(tool_button(
            "wt-more",
            format!("{proj}-{wt}"),
            "more",
            12.0,
            false,
            ctx,
            RowAction::OpenAgentMenu(Some((proj, wt))),
        ));
        if !*is_main {
            strip = strip.child(tool_button(
                "wt-del",
                format!("{proj}-{wt}"),
                "trash",
                12.0,
                true,
                ctx,
                RowAction::DeleteWorktree(proj, wt),
            ));
        }
        strip.into_any_element()
    } else if *is_main {
        div().px(px(8.0)).child(main_tag()).into_any_element()
    } else {
        div().into_any_element()
    };

    div()
        .id(SharedString::from(format!("wt-{proj}-{wt}")))
        .h(px(h))
        .w_full()
        .flex()
        .items_center()
        .when(*active, |d| d.bg(c::BG_HL()))
        .on_mouse_move({
            let dispatch = Rc::clone(&ctx.dispatch);
            move |_, window, cx| dispatch(RowAction::HoverWorktree(Some((proj, wt))), window, cx)
        })
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(6.0))
                .pl(px(26.0))
                .pr(px(6.0))
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    ctx.on(RowAction::SelectWorktree(proj, wt)),
                )
                .child(
                    div()
                        .w(px(14.0))
                        .child(icons::icon(twist, 10.0, c::FG_MUTE())),
                )
                .child(label),
        )
        .when_some(git_suffix.clone(), |d, s| {
            d.child(ui_text(s, 11.0, c::FG_MUTE()))
        })
        .when_some(*rollup, |d, st| {
            d.child(state_glyph(st, ctx.tick, ctx.pulse))
        })
        .child(actions)
        .into_any_element()
}

fn session_row(
    id: SessionId,
    active: bool,
    pending_kill: bool,
    state: ActivityState,
    ctx: &RowCtx,
) -> AnyElement {
    let (agent, context) = ctx
        .session_text
        .get(&id)
        .cloned()
        .unwrap_or((Agent::Terminal, None));
    let agent_color = if active {
        c::CYAN()
    } else {
        match state {
            ActivityState::Working | ActivityState::WaitingForInput => c::FG(),
            ActivityState::Done | ActivityState::Idle => c::FG_DIM(),
            ActivityState::Exited => c::FG_MUTE(),
        }
    };
    let mut meta = div()
        .flex()
        .flex_1()
        .items_center()
        .gap(px(6.0))
        .overflow_hidden()
        .child(icons::icon(agent.icon_name(), 12.0, agent_color))
        .child(ui_text(agent.label(), 12.0, agent_color));
    if let Some(ctx_text) = context {
        meta = meta
            .child(ui_text("·", 11.0, c::FG_MUTE()))
            .child(ui_text(truncate_ellipsis(&ctx_text, 28), 11.0, c::FG_MUTE()).overflow_hidden());
    }

    // Two-step confirm: the first press arms (red tick), the second kills
    // (`src/gui/rows.rs:519-524`).
    let close = if pending_kill {
        tool_button(
            "sess-kill",
            id.raw(),
            "check",
            11.0,
            true,
            ctx,
            RowAction::KillSession(id),
        )
    } else {
        tool_button(
            "sess-arm",
            id.raw(),
            "close",
            11.0,
            false,
            ctx,
            RowAction::ArmKillSession(id),
        )
    };

    div()
        .id(SharedString::from(format!("sess-{}", id.raw())))
        .h(px(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.0))
        .pl(px(46.0))
        .pr(px(8.0))
        .when(active, |d| d.bg(c::BG_HL()))
        .when(state == ActivityState::WaitingForInput, |d| {
            d.bg(Hsla {
                a: 0.12,
                ..c::AMBER()
            })
            .border_l(px(3.0))
            .border_color(c::AMBER())
        })
        .hover(|s| s.bg(c::BG_HOVER()))
        .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectSession(id)))
        .child(state_glyph(state, ctx.tick, ctx.pulse))
        .child(meta)
        .child(close)
        .into_any_element()
}

fn empty_row(title: &'static str, subtitle: &'static str) -> AnyElement {
    div()
        .w_full()
        .py(px(24.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .child(ui_text(title, 14.0, c::FG_DIM()))
        .child(ui_text(subtitle, 12.0, c::FG_MUTE()))
        .into_any_element()
}

/// The collapsible TERMINALS header (`src/gui/rows.rs:643-...`). Rendered both
/// inline (expanded) and docked at the rail's bottom (collapsed) — the dot is
/// the only difference, and the caller decides it.
pub fn terminals_header(
    expanded: bool,
    count: usize,
    activity_dot: bool,
    ctx: &RowCtx,
) -> AnyElement {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    div()
        .id("terminals-header")
        .h(px(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(6.0))
                .pl(px(12.0))
                .on_mouse_down(MouseButton::Left, ctx.on(RowAction::ToggleTerminalsSection))
                .child(icons::icon(twist, 10.0, c::FG_MUTE()))
                .child(icons::icon("term", 11.0, c::FG_MUTE()))
                .child(ui_text(tracked("TERMINALS"), 10.0, c::FG_MUTE()))
                .child(ui_text(format!("{count}"), 10.0, c::FG_MUTE()))
                .when(activity_dot, |d| d.child(dot(c::GREEN()))),
        )
        .child(tool_button(
            "term-new",
            "home",
            "plus",
            12.0,
            false,
            ctx,
            RowAction::NewHomeTerminal,
        ))
        .into_any_element()
}

fn terminal_row(idx: usize, active: bool, pending_kill: bool, ctx: &RowCtx) -> AnyElement {
    // No synthetic "terminal N" name — the icon plus the shell's own title,
    // falling back to `~` (`src/gui/rows.rs:596-600`).
    let context = ctx
        .terminal_text
        .get(idx)
        .cloned()
        .flatten()
        .unwrap_or_else(|| "~".to_string());
    let name_color = if active { c::CYAN() } else { c::FG() };
    let close = if pending_kill {
        tool_button(
            "term-kill",
            idx,
            "check",
            11.0,
            true,
            ctx,
            RowAction::KillTerminal(idx),
        )
    } else {
        tool_button(
            "term-arm",
            idx,
            "close",
            11.0,
            false,
            ctx,
            RowAction::ArmKillTerminal(idx),
        )
    };
    div()
        .id(SharedString::from(format!("term-{idx}")))
        .h(px(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.0))
        .pl(px(16.0))
        .pr(px(8.0))
        .when(active, |d| d.bg(c::BG_HL()))
        .hover(|s| s.bg(c::BG_HOVER()))
        .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectTerminal(idx)))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(6.0))
                .overflow_hidden()
                .child(icons::icon("term", 12.0, name_color))
                .child(ui_text(truncate_ellipsis(&context, 28), 12.0, name_color)),
        )
        .child(close)
        .into_any_element()
}

/// One line, `…` when too long (`src/gui/rows.rs:857-867`).
#[must_use]
pub fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};

    // ── pure helpers ────────────────────────────────────────────────────

    /// `src/gui/rows.rs:249`.
    #[test]
    fn branch_chip_only_for_non_main_worktrees_with_a_distinct_branch() {
        assert!(worktree_shows_branch(false, "feature", "wt"));
        assert!(!worktree_shows_branch(true, "feature", "wt"));
        assert!(!worktree_shows_branch(false, "wt", "wt"));
        assert!(!worktree_shows_branch(false, "", "wt"));
    }

    /// `src/gui/rows.rs:268`.
    #[test]
    fn a_branch_chip_makes_the_row_fourteen_pixels_taller() {
        assert!((row_height(false) - 28.0).abs() < f32::EPSILON);
        assert!((row_height(true) - 42.0).abs() < f32::EPSILON);
    }

    /// Constraint 3 candidate 1.
    #[test]
    fn path_basename_handles_trailing_slashes_roots_and_odd_input() {
        assert_eq!(path_basename("/a/b/c"), "c");
        assert_eq!(path_basename("/a/b/c/"), "c");
        assert_eq!(path_basename("/"), "/");
        assert_eq!(path_basename(""), "");
        assert_eq!(path_basename("plain"), "plain");
        assert_eq!(path_basename("/a/b/../c"), "c");
    }

    /// `src/gui/rows.rs:809-834`.
    #[test]
    fn sanitize_drops_unrenderable_characters_and_collapses_whitespace() {
        assert_eq!(
            sanitize_ui_text("  hello   world "),
            Some("hello world".into())
        );
        assert_eq!(sanitize_ui_text("✨ build ✨"), Some("build".into()));
        assert_eq!(sanitize_ui_text("· - review :"), Some("review".into()));
        assert_eq!(sanitize_ui_text("✨✨"), None);
        assert_eq!(sanitize_ui_text(""), None);
        // Latin-1 supplement / extended-A survive.
        assert_eq!(sanitize_ui_text("café"), Some("café".into()));
    }

    /// `src/gui/rows.rs:895-920` — UTF-8-safe, case-insensitive.
    #[test]
    fn remove_all_ci_is_case_insensitive_and_utf8_safe() {
        assert_eq!(remove_all_ci("Claude claude CLAUDE x", "claude"), "   x");
        assert_eq!(remove_all_ci("caféXcafé", "X"), "cafécafé");
        assert_eq!(remove_all_ci("abc", ""), "abc");
        assert_eq!(remove_all_ci("abc", "zzz"), "abc");
        assert_eq!(remove_all_ci("", "a"), "");
    }

    /// `src/gui/rows.rs:776-806`.
    #[test]
    fn contexts_strip_the_internal_labels() {
        assert_eq!(
            terminal_context("terminal 2 — ~/dev", "terminal 2"),
            Some("~/dev".into())
        );
        assert_eq!(terminal_context("terminal 1", "terminal 1"), None);
        assert_eq!(
            session_context("grove claude 1 Review pull", "grove", "claude 1", "Claude"),
            Some("Review pull".into())
        );
        assert_eq!(
            session_context("grove", "grove", "claude 1", "Claude"),
            None
        );
    }

    /// `src/gui/widgets/primitives.rs:407-419` — the two states stay textually
    /// distinct.
    #[test]
    fn empty_and_all_archived_pick_distinct_copy() {
        let Some(none) = sidebar_empty_copy(0, 0) else {
            unreachable!("no projects at all is an empty state")
        };
        assert_eq!(none, ("No projects yet", "Add one with + above."));
        let Some(archived) = sidebar_empty_copy(3, 0) else {
            unreachable!("all-archived is an empty state")
        };
        assert_eq!(
            archived,
            (
                "All projects archived",
                "Restore one from Settings → Archived projects."
            )
        );
        assert_ne!(none, archived);
        assert!(sidebar_empty_copy(1, 1).is_none());
        assert!(sidebar_empty_copy(5, 2).is_none());
    }

    // ── flattening ──────────────────────────────────────────────────────

    fn sid(n: u64) -> SessionId {
        SessionId::from_raw(n)
    }

    /// TRUE indices 0 and 2 are active; index 1 is archived.
    /// - p0 `alpha`: `/a` (main, sessions 1,2), `/a-x` (branch `feature`, no sessions)
    /// - p2 `gamma`: `/g` (main, session 3)
    fn fixture() -> TreeSnapshot {
        TreeSnapshot {
            total_projects: 3,
            projects: vec![
                SnapshotProject {
                    idx: 0,
                    name: "alpha".into(),
                    is_git: true,
                    has_run: true,
                    worktrees: vec![
                        SnapshotWorktree {
                            path: "/a".into(),
                            name: "alpha".into(),
                            branch: "main".into(),
                            is_main: true,
                            sessions: vec![sid(1), sid(2)],
                        },
                        SnapshotWorktree {
                            path: "/a-x".into(),
                            name: "a-x".into(),
                            branch: "feature".into(),
                            is_main: false,
                            sessions: vec![],
                        },
                    ],
                },
                SnapshotProject {
                    idx: 2,
                    name: "gamma".into(),
                    is_git: false,
                    has_run: false,
                    worktrees: vec![SnapshotWorktree {
                        path: "/g".into(),
                        name: "gamma".into(),
                        branch: "main".into(),
                        is_main: true,
                        sessions: vec![sid(3)],
                    }],
                },
            ],
        }
    }

    fn rows_of(snap: &TreeSnapshot, ws: &WorkspaceState, homes: usize) -> Vec<TreeRow> {
        flatten(snap, ws, &ActivityStore::new(), &HashMap::new(), homes)
    }

    /// A compact shape description, so order assertions read as the tree.
    fn shape(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Project { idx, name, .. } => format!("P{idx}:{name}"),
                TreeRow::Worktree { proj, wt, name, .. } => format!("W{proj}.{wt}:{name}"),
                TreeRow::Session { id, .. } => format!("S{}", id.raw()),
                TreeRow::Empty { title, .. } => format!("E:{title}"),
                TreeRow::TerminalsHeader { count, .. } => format!("TH{count}"),
                TreeRow::Terminal { idx, .. } => format!("T{idx}"),
            })
            .collect()
    }

    /// `sidebar.rs:238-355`: projects (active only, TRUE indices) → worktrees
    /// → sessions.
    #[test]
    fn order_is_projects_then_worktrees_then_sessions() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        assert_eq!(
            shape(&rows_of(&snap, &ws, 0)),
            vec![
                "P0:alpha",
                "W0.0:alpha",
                "S1",
                "S2",
                "W0.1:a-x",
                "P2:gamma",
                "W2.0:gamma",
                "S3",
            ]
        );
    }

    /// `sidebar.rs:269-271`.
    #[test]
    fn a_collapsed_project_emits_no_descendants() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        ws.select_project(0);
        assert_eq!(
            shape(&rows_of(&snap, &ws, 0)),
            vec!["P0:alpha", "P2:gamma", "W2.0:gamma", "S3"]
        );
    }

    /// `sidebar.rs:330-332`.
    #[test]
    fn a_collapsed_worktree_emits_no_session_rows() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        ws.select_worktree(0, 0, &snap);
        assert_eq!(
            shape(&rows_of(&snap, &ws, 0)),
            vec![
                "P0:alpha",
                "W0.0:alpha",
                "W0.1:a-x",
                "P2:gamma",
                "W2.0:gamma",
                "S3"
            ]
        );
    }

    /// `sidebar.rs:251-257`, `:296-303` — roll-ups only on collapsed parents.
    #[test]
    fn rollups_appear_only_on_collapsed_parents() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        for row in rows_of(&snap, &ws, 0) {
            match row {
                TreeRow::Project { rollup, .. } | TreeRow::Worktree { rollup, .. } => {
                    assert_eq!(rollup, None, "an expanded parent must not roll up");
                }
                _ => {}
            }
        }
        // Collapsed: the roll-up slot is populated from the store (which stubs
        // every session to Idle, so `most_urgent` is None — Plan 06 fills it).
        ws.select_project(0);
        let rows = rows_of(&snap, &ws, 0);
        let Some(TreeRow::Project { expanded, .. }) = rows.first() else {
            unreachable!("the first row is the project")
        };
        assert!(!expanded);
    }

    /// `sidebar.rs:272-278` — a cache miss yields no worktree rows, not a panic.
    #[test]
    fn a_worktree_cache_miss_yields_no_worktree_rows() {
        let mut snap = fixture();
        snap.projects[0].worktrees.clear();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        assert_eq!(
            shape(&rows_of(&snap, &ws, 0)),
            vec!["P0:alpha", "P2:gamma", "W2.0:gamma", "S3"]
        );
    }

    /// `sidebar.rs:338` — the comment there is the contract.
    #[test]
    fn a_session_is_never_active_while_a_home_terminal_is_on_screen() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.select_session(sid(2), &snap);
        let active: Vec<bool> = rows_of(&snap, &ws, 1)
            .iter()
            .filter_map(|r| match r {
                TreeRow::Session { active, .. } => Some(*active),
                _ => None,
            })
            .collect();
        assert_eq!(active, vec![false, true, false]);

        ws.select_home_terminal(0, 1);
        let rows = rows_of(&snap, &ws, 1);
        assert!(rows
            .iter()
            .all(|r| !matches!(r, TreeRow::Session { active: true, .. })));
        // …and the terminal row is the active one instead (`sidebar.rs:374`).
        assert!(rows.iter().any(|r| matches!(
            r,
            TreeRow::Terminal {
                idx: 0,
                active: true,
                ..
            }
        )));
    }

    #[test]
    fn a_session_row_carries_its_own_pending_kill_arm() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.arm_kill(sid(3));
        let armed: Vec<SessionId> = rows_of(&snap, &ws, 0)
            .iter()
            .filter_map(|r| match r {
                TreeRow::Session {
                    id,
                    pending_kill: true,
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(armed, vec![sid(3)]);
    }

    #[test]
    fn the_active_worktree_is_the_one_the_highlight_points_at() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        ws.select_session(sid(3), &snap);
        let active: Vec<(usize, usize)> = rows_of(&snap, &ws, 0)
            .iter()
            .filter_map(|r| match r {
                TreeRow::Worktree {
                    proj,
                    wt,
                    active: true,
                    ..
                } => Some((*proj, *wt)),
                _ => None,
            })
            .collect();
        assert_eq!(active, vec![(2, 0)]);
    }

    #[test]
    fn the_git_suffix_comes_from_the_poll_map_keyed_by_path() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let mut suffixes = HashMap::new();
        suffixes.insert("/g".to_string(), "+2".to_string());
        let rows = flatten(&snap, &ws, &ActivityStore::new(), &suffixes, 0);
        let found: Vec<Option<String>> = rows
            .iter()
            .filter_map(|r| match r {
                TreeRow::Worktree { git_suffix, .. } => Some(git_suffix.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(found, vec![None, None, Some("+2".to_string())]);
    }

    /// `sidebar.rs:357-361`.
    #[test]
    fn the_empty_state_is_emitted_only_when_no_active_project_produced_a_row() {
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        assert!(!rows_of(&fixture(), &ws, 0)
            .iter()
            .any(|r| matches!(r, TreeRow::Empty { .. })));

        let all_archived = TreeSnapshot {
            total_projects: 3,
            projects: vec![],
        };
        assert_eq!(
            shape(&rows_of(&all_archived, &ws, 0)),
            vec!["E:All projects archived"]
        );

        let no_projects = TreeSnapshot::default();
        assert_eq!(
            shape(&rows_of(&no_projects, &ws, 0)),
            vec!["E:No projects yet"]
        );
    }

    /// `sidebar.rs:363-374`.
    #[test]
    fn the_expanded_terminals_section_forces_its_activity_dot_off() {
        let snap = fixture();
        let ws = WorkspaceState::default();
        let rows = rows_of(&snap, &ws, 2);
        let Some(TreeRow::TerminalsHeader {
            expanded,
            count,
            activity_dot,
        }) = rows
            .iter()
            .find(|r| matches!(r, TreeRow::TerminalsHeader { .. }))
            .cloned()
        else {
            unreachable!("the expanded section emits its header")
        };
        assert!(expanded);
        assert_eq!(count, 2);
        assert!(!activity_dot);
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, TreeRow::Terminal { .. }))
                .count(),
            2
        );
    }

    /// `sidebar.rs:114-129` — collapsed, the section emits nothing here; the
    /// view docks the header outside the scroll area.
    #[test]
    fn the_collapsed_terminals_section_emits_nothing() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = rows_of(&snap, &ws, 3);
        assert!(!rows.iter().any(|r| matches!(
            r,
            TreeRow::TerminalsHeader { .. } | TreeRow::Terminal { .. }
        )));
    }

    // ── visible_session_order ───────────────────────────────────────────

    /// `sidebar.rs:386-417` — the same walk, sessions only.
    #[test]
    fn visible_order_is_the_flattened_session_rows() {
        let snap = fixture();
        let ws = WorkspaceState::default();
        let rows = rows_of(&snap, &ws, 1);
        assert_eq!(visible_session_order(&rows), vec![sid(1), sid(2), sid(3)]);
    }

    #[test]
    fn a_collapsed_project_hides_its_sessions_from_the_numbering() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.select_project(0);
        assert_eq!(visible_session_order(&rows_of(&snap, &ws, 0)), vec![sid(3)]);
    }

    #[test]
    fn a_collapsed_worktree_hides_its_sessions_from_the_numbering() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.select_worktree(0, 0, &snap);
        assert_eq!(visible_session_order(&rows_of(&snap, &ws, 0)), vec![sid(3)]);
    }

    #[test]
    fn the_order_is_stable_across_an_unrelated_projects_collapse_toggle() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        let before = visible_session_order(&rows_of(&snap, &ws, 0));
        ws.select_project(2);
        ws.select_project(2);
        assert_eq!(visible_session_order(&rows_of(&snap, &ws, 0)), before);
    }

    /// `sidebar.rs:421-470` — the overlay walk uses the same height function
    /// the list lays out with, so it cannot land on the wrong row.
    #[test]
    fn the_agent_menu_anchors_below_its_worktree_row() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = rows_of(&snap, &ws, 0);
        // P0(28) W0.0(28) S1(28) S2(28) -> /a-x starts at 112 and is 42 tall.
        let Some((proj, wt, top, is_main)) = agent_menu_top(&rows, (0, 1)) else {
            unreachable!("the open worktree is in the tree")
        };
        assert_eq!((proj, wt), (0, 1));
        assert!(!is_main);
        assert!((top - (6.0 + 112.0 + 42.0)).abs() < f32::EPSILON);

        // The main worktree is the second row, 28 tall.
        let Some((.., top, is_main)) = agent_menu_top(&rows, (0, 0)) else {
            unreachable!("the main worktree is in the tree")
        };
        assert!(is_main);
        assert!((top - (6.0 + 28.0 + 28.0)).abs() < f32::EPSILON);

        // A worktree hidden under a collapsed project has no anchor.
        ws.select_project(0);
        let collapsed = rows_of(&snap, &ws, 0);
        assert!(agent_menu_top(&collapsed, (0, 1)).is_none());
    }

    /// Row heights come from one function, so the overlay walk and the list
    /// cannot disagree (carried amendment 2).
    #[test]
    fn only_a_branch_showing_worktree_row_is_taller_than_row_h() {
        let snap = fixture();
        let ws = WorkspaceState::default();
        for row in rows_of(&snap, &ws, 1) {
            let expected = match &row {
                TreeRow::Worktree {
                    name: n,
                    branch,
                    is_main,
                    ..
                } => row_height(worktree_shows_branch(*is_main, branch, n)),
                _ => ROW_H,
            };
            assert!((row.height() - expected).abs() < f32::EPSILON);
        }
        // `/a-x` is the non-main worktree on branch `feature`.
        let tall = rows_of(&snap, &ws, 0)
            .iter()
            .filter(|r| (r.height() - ROW_H).abs() > f32::EPSILON)
            .count();
        assert_eq!(tall, 1);
    }
}
