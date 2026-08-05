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

use crate::views::rpx;
use crate::views::tokens::*;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, FontWeight, MouseButton, MouseDownEvent, SharedString, Window,
};
use grove_core::agent::Agent;

use crate::activity::{most_urgent, ActivityState};
use crate::entities::activity_store::ActivityStore;
use crate::entities::session_registry::SessionId;
use crate::entities::workspace_state::{TreeSnapshot, WorkspaceState};
use crate::icons;
use crate::theme as c;
use crate::views::components::{icon_btn, keycap_filled, mono, status_dot, tracked, ui};

/// Sidebar row height (`src/gui/metrics.rs:7`).
pub const ROW_H: f32 = 28.0;

/// The extra height a worktree row gains when it shows a branch chip: the
/// chip's own line. [`row_height`] and the renderer must both use this — the
/// agent-menu overlay is positioned from [`row_height`], so a renderer that
/// disagreed would misplace the menu (§8.1).
const BRANCH_LINE_H: f32 = 14.0;

/// The fixed-width slot every row's leading glyph (twisty, state glyph) sits
/// in. It is a *reserved* width: the glyph inside changes with state, and §2.4
/// forbids the row reflowing when it does. All three call sites share this one
/// constant so the columns line up down the tree.
const GLYPH_SLOT_W: f32 = 14.0;

/// Worktree-row indent: one glyph slot in from the project row's own
/// [`SPACE_2XL`] gutter, so a worktree's twisty sits under the project's
/// label rather than under its twisty. `12 + 14 = 26`.
const INDENT_WORKTREE: f32 = SPACE_2XL + GLYPH_SLOT_W;
/// Session-row indent: the same step again, taken from the worktree row's
/// geometry — its glyph slot plus the `SPACE_MD` gap that follows it — so a
/// session's state glyph starts exactly where its worktree's *name* does.
/// `26 + 14 + 6 = 46`. The two rungs are derived from one another; neither is
/// an independent number.
const INDENT_SESSION: f32 = INDENT_WORKTREE + GLYPH_SLOT_W + SPACE_MD;

/// Vertical breathing room around the sidebar's inline empty rows. Taller than
/// any spacing notch on purpose — this is a block of prose standing in for a
/// list, not a row in one (§9.2's sanctioned local exception).
const EMPTY_ROW_PAD_Y: f32 = 24.0;

/// Width of the amber accent bar overlaid on a row that needs input. Overlaid
/// rather than a `border_l` so it never shifts the row's content.
/// (`src/views/appbar.rs` draws the same 3px bar; the two are independent
/// call sites of the same visual idea.)
const ATTENTION_BAR_W: f32 = 3.0;

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
        ROW_H + BRANCH_LINE_H
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
        is_git: bool,
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
        running: bool,
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
    home_running: &[bool],
) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut any_active = false;

    for p in &snap.projects {
        any_active = true;
        let expanded = !ws.project_collapsed(p.idx);
        // Counted by project name, so non-active projects (whose worktree
        // cache is empty until visited) still show their true session count.
        let sessions = &p.sessions;
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
                is_git: p.is_git,
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
            count: home_running.len(),
            activity_dot: false,
        });
        for (i, &running) in home_running.iter().enumerate() {
            rows.push(TreeRow::Terminal {
                idx: i,
                active: ws.terminal_focused() && ws.active_terminal() == Some(i),
                pending_kill: ws.pending_kill_terminal() == Some(i),
                running,
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
    /// The rail header's `+` — opens the add-project wizard (Task 4).
    AddProject,
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
        ActivityState::Working => icons::spinner(ICON_SM, c::GREEN(), tick),
        ActivityState::WaitingForInput => icons::icon(
            "question",
            ICON_SM,
            c::alpha(c::AMBER(), 1.0 - 0.45 * pulse),
        ),
        ActivityState::Done => icons::icon("check", ICON_SM, c::FG_MUTE()),
        ActivityState::Idle => icons::icon("dot", ICON_SM, c::FG_MUTE()),
        ActivityState::Exited => icons::icon("ring", ICON_SM, c::FG_MUTE()),
    };
    div()
        .w(rpx(GLYPH_SLOT_W))
        .flex_none()
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
    let dispatch = Rc::clone(&ctx.dispatch);
    icon_btn(
        SharedString::from(format!("{id}-{key}")),
        glyph,
        CONTROL_H,
        CONTROL_H,
        size,
        c::FG_MUTE(),
        hover_bg,
        Some(hover_fg),
        true,
        move |window, cx| dispatch(action, window, cx),
    )
    .flex_none()
    .into_any_element()
}

/// The `main` tag (`src/gui/rows.rs:253-265`). Shares its slot with the hover
/// icons so the two never compete for width.
fn main_tag() -> AnyElement {
    ui("main", TEXT_MICRO, c::GREEN()).into_any_element()
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
            running,
        } => terminal_row(*idx, *active, *pending_kill, *running, ctx),
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
        .gap(rpx(SPACE_MD))
        .pr(rpx(SPACE_LG))
        .when_some(rollup, |d, st| {
            d.child(state_glyph(st, ctx.tick, ctx.pulse))
        });
    if is_git {
        // Worktrees are git-only.
        right = right.child(tool_button(
            "wt-add",
            idx,
            "plus",
            ICON_SM,
            false,
            ctx,
            RowAction::AddWorktree(idx),
        ));
    } else {
        right = right.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(icons::icon("no-git", ICON_SM, c::FG_MUTE()))
                .child(ui("no git", TEXT_MICRO, c::FG_MUTE())),
        );
    }
    right = right.child(tool_button(
        "proj-scripts",
        idx,
        "cog",
        ICON_SM,
        false,
        ctx,
        RowAction::ProjectScripts(idx),
    ));
    right = right.child(tool_button(
        "proj-remove",
        idx,
        "trash",
        ICON_SM,
        true,
        ctx,
        RowAction::RemoveProject(idx),
    ));
    // Fixed chrome never shrinks — see the comment at the cluster div for
    // why it can't be `flex_1`. The blank middle is claimed by the filler
    // div below instead of an auto margin here.
    right = right.flex_shrink_0();

    div()
        .id(SharedString::from(format!("proj-{idx}")))
        .h(rpx(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(rpx(SPACE_LG))
                .pl(rpx(SPACE_2XL))
                .pr(rpx(SPACE_SM))
                .overflow_hidden()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectProject(idx)))
                .child(div().w(rpx(GLYPH_SLOT_W)).flex_none().child(icons::icon(
                    twist,
                    ICON_XS,
                    c::FG_MUTE(),
                )))
                // No `flex_1` here on purpose: `.truncate()` (`overflow_hidden`
                // + `text_ellipsis`) makes gpui decide the ellipsis at MEASURE
                // time (`gpui/src/elements/text.rs:659-672,739-751`) — whatever
                // width the text measures at becomes its flex-basis, and a
                // `flex_1` parent (basis 0%) never re-grows it afterward. By
                // leaving this cluster's basis at `auto` (its own content size)
                // and letting the filler below absorb the leftover space, the
                // cluster only receives a *shrunk* definite width — and
                // therefore only truncates — once the row is genuinely too
                // narrow for everything.
                .child(
                    ui(name.to_uppercase(), TEXT_BODY, c::FG())
                        .font_weight(FontWeight::BOLD)
                        .truncate(),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(status_dot(DOT_SM, count_color))
                        .child(mono(format!("{count}"), TEXT_SMALL, count_color)),
                ),
        )
        // Clickable filler: the name cluster above can't be `flex_1` (see its
        // comment), so the row's click target would otherwise only cover the
        // cluster's content width, leaving the blank middle dead to clicks.
        // This absorbs the leftover space (basis 0% + grow, so it never
        // steals width from the name) and carries the same action.
        .child(
            div()
                .flex_1()
                .self_stretch()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectProject(idx))),
        )
        .child(right)
        .into_any_element()
}

fn worktree_row(row: &TreeRow, ctx: &RowCtx) -> AnyElement {
    let TreeRow::Worktree {
        proj,
        wt,
        name,
        branch,
        is_main,
        is_git,
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

    // Non-git project root: flag it so the user knows sessions run directly in
    // the project path with no branch isolation / worktrees.
    let no_git = *is_main && !*is_git;

    let mut label = if no_git {
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(rpx(SPACE_SM))
            .overflow_hidden()
            .child(ui(name.clone(), TEXT_TITLE, c::FG_DIM()).truncate())
            .child(icons::icon("no-git", ICON_SM, c::FG_MUTE()).flex_none())
            .child(ui("no git", TEXT_MICRO, c::FG_MUTE()).flex_none())
    } else {
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .overflow_hidden()
            .child(ui(name.clone(), TEXT_TITLE, c::FG_DIM()).truncate())
    };
    if show_branch {
        // Branch chip: a soft-bordered pill under the name.
        // The chip must shrink with the sidebar, not wrap: the row's height is
        // fixed at `row_height`, so a wrapped chip spills over its neighbours.
        label = label.child(
            div()
                .min_w_0()
                .pt(rpx(SPACE_XS))
                .child(keycap_filled(
                    c::BORDER_SOFT(),
                    ui(branch.clone(), TEXT_MICRO, c::FG_DIM()).truncate(),
                ))
                .overflow_hidden(),
        );
    }

    // The `main` tag and the hover action strip share one fixed right-hand
    // slot, so they never render at once and never shift the layout
    // (`src/gui/rows.rs:253-265,396-420`).
    let actions: AnyElement = if hovered {
        let mut strip = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .pr(rpx(SPACE_LG));
        for agent in &ctx.available {
            strip = strip.child(tool_button(
                "wt-spawn",
                format!("{proj}-{wt}-{}", agent.label()),
                agent.icon_name(),
                ICON_SM,
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
                ICON_SM,
                false,
                ctx,
                RowAction::RunScript(proj, wt),
            ));
        }
        strip = strip.child(tool_button(
            "wt-more",
            format!("{proj}-{wt}"),
            "more",
            ICON_SM,
            false,
            ctx,
            RowAction::OpenAgentMenu(Some((proj, wt))),
        ));
        if !*is_main {
            strip = strip.child(tool_button(
                "wt-del",
                format!("{proj}-{wt}"),
                "trash",
                ICON_SM,
                true,
                ctx,
                RowAction::DeleteWorktree(proj, wt),
            ));
        }
        strip.into_any_element()
    } else if *is_main && *is_git {
        div().px(rpx(SPACE_LG)).child(main_tag()).into_any_element()
    } else {
        div().into_any_element()
    };

    div()
        .id(SharedString::from(format!("wt-{proj}-{wt}")))
        .h(rpx(h))
        .w_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .when(*active, |d| d.bg(c::BG_HL()))
        .on_hover({
            let dispatch = Rc::clone(&ctx.dispatch);
            move |hovered, window, cx| {
                let target = if *hovered { Some((proj, wt)) } else { None };
                dispatch(RowAction::HoverWorktree(target), window, cx);
            }
        })
        .child(
            // No `flex_1` on this cluster — see the comment at `project_row`'s
            // name child (`rows.rs`): `.truncate()` measures and fixes its
            // ellipsis point at measure time, so a `flex_1` (basis 0%) parent
            // would strand the name at whatever width it first ellipsized to.
            // Leaving basis at `auto`, with the filler below absorbing the
            // leftover space, means this cluster keeps its content width until
            // the row is actually too narrow for both.
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(rpx(SPACE_MD))
                .pl(rpx(INDENT_WORKTREE))
                .pr(rpx(SPACE_MD))
                .overflow_hidden()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    ctx.on(RowAction::SelectWorktree(proj, wt)),
                )
                .child(div().w(rpx(GLYPH_SLOT_W)).flex_none().child(icons::icon(
                    twist,
                    ICON_XS,
                    c::FG_MUTE(),
                )))
                .child(label),
        )
        // Clickable filler: absorbs the row's blank middle so clicking it
        // selects the worktree, same as clicking the name cluster — see the
        // comment at `project_row`'s filler for why the cluster can't just
        // be `flex_1` itself.
        .child(
            div()
                .flex_1()
                .self_stretch()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    ctx.on(RowAction::SelectWorktree(proj, wt)),
                ),
        )
        .child(
            // The git-suffix, roll-up glyph and tool chrome are fixed-width
            // and pinned to the row's right edge: `flex_shrink_0` so shrink
            // pressure lands on the name cluster above; the filler above
            // claims only the space the name cluster doesn't need.
            div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .when_some(git_suffix.clone(), |d, s| {
                    d.child(ui(s, TEXT_SMALL, c::FG_MUTE()).flex_none())
                })
                .when_some(*rollup, |d, st| {
                    d.child(state_glyph(st, ctx.tick, ctx.pulse))
                })
                .child(actions),
        )
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
    // No `flex_1` here — see `project_row`'s name-child comment (`rows.rs`):
    // `.truncate()` on the context text below fixes its ellipsis at measure
    // time, so a `flex_1` (basis 0%) parent would never let it re-grow. The
    // close button claims the right edge via `ml_auto()` instead, so this
    // cluster keeps its own content width until the row truly runs out.
    let mut meta = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(rpx(SPACE_MD))
        .overflow_hidden()
        .child(icons::icon(agent.icon_name(), ICON_SM, agent_color).flex_none())
        .child(ui(agent.label(), TEXT_BODY, agent_color).flex_none());
    if let Some(ctx_text) = context {
        meta = meta
            .child(ui("·", TEXT_SMALL, c::FG_MUTE()).flex_none())
            .child(ui(ctx_text, TEXT_SMALL, c::FG_MUTE()).truncate());
    }

    // Two-step confirm: the first press arms (red tick), the second kills
    // (`src/gui/rows.rs:519-524`).
    let close = if pending_kill {
        tool_button(
            "sess-kill",
            id.raw(),
            "check",
            ICON_SM,
            true,
            ctx,
            RowAction::KillSession(id),
        )
    } else {
        tool_button(
            "sess-arm",
            id.raw(),
            "close",
            ICON_SM,
            false,
            ctx,
            RowAction::ArmKillSession(id),
        )
    };
    // `AnyElement` isn't `Styled`, so the auto margin that pins it to the row's
    // right edge (freeing the name cluster above to keep its content width)
    // has to go on a thin wrapper div.
    let close = div().flex_none().ml_auto().child(close);

    div()
        .id(SharedString::from(format!("sess-{}", id.raw())))
        .h(rpx(ROW_H))
        .w_full()
        .relative()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .pl(rpx(INDENT_SESSION))
        .pr(rpx(SPACE_LG))
        .when(active, |d| d.bg(c::BG_HL()))
        .when(state == ActivityState::WaitingForInput, |d| {
            d.bg(c::AMBER_ROW_TINT())
        })
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectSession(id)))
        .child(state_glyph(state, ctx.tick, ctx.pulse))
        .child(meta)
        .child(close)
        // Overlaid rather than a `border_l` so the amber accent never shifts
        // the row content (`src/gui/rows.rs:539-566`).
        .when(state == ActivityState::WaitingForInput, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w(rpx(ATTENTION_BAR_W))
                    .bg(c::AMBER()),
            )
        })
        .into_any_element()
}

fn empty_row(title: &'static str, subtitle: &'static str) -> AnyElement {
    div()
        .w_full()
        .py(rpx(EMPTY_ROW_PAD_Y))
        .flex()
        .flex_col()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(ui(title, TEXT_TITLE, c::FG_DIM()))
        .child(ui(subtitle, TEXT_BODY, c::FG_MUTE()))
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
        .h(rpx(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(rpx(SPACE_MD))
                .pl(rpx(SPACE_2XL))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, ctx.on(RowAction::ToggleTerminalsSection))
                .child(icons::icon(twist, ICON_XS, c::FG_MUTE()))
                .child(icons::icon("term", ICON_SM, c::FG_MUTE()))
                .child(mono(tracked("TERMINALS"), TEXT_MICRO, c::FG_MUTE()))
                .child(keycap_filled(
                    c::BORDER_SOFT(),
                    mono(format!("{count}"), TEXT_MICRO, c::FG_MUTE()),
                ))
                .when(activity_dot, |d| d.child(status_dot(DOT_SM, c::CYAN()))),
        )
        .child(tool_button(
            "term-new",
            "home",
            "plus",
            ICON_SM,
            false,
            ctx,
            RowAction::NewHomeTerminal,
        ))
        .into_any_element()
}

fn terminal_row(
    idx: usize,
    active: bool,
    pending_kill: bool,
    running: bool,
    ctx: &RowCtx,
) -> AnyElement {
    // No synthetic "terminal N" name — the icon plus the shell's own title,
    // falling back to `~` (`src/gui/rows.rs:596-600`).
    let context = ctx
        .terminal_text
        .get(idx)
        .cloned()
        .flatten()
        .unwrap_or_else(|| "~".to_string());
    let name_color = if active {
        c::CYAN()
    } else if running {
        c::FG()
    } else {
        c::FG_MUTE()
    };
    let close = if pending_kill {
        tool_button(
            "term-kill",
            idx,
            "check",
            ICON_SM,
            true,
            ctx,
            RowAction::KillTerminal(idx),
        )
    } else {
        tool_button(
            "term-arm",
            idx,
            "close",
            ICON_SM,
            false,
            ctx,
            RowAction::ArmKillTerminal(idx),
        )
    };
    // Wrapper for the same reason as `session_row`'s `close`: `AnyElement`
    // isn't `Styled`, so the auto margin needs a thin div around it.
    let close = div().flex_none().ml_auto().child(close);
    div()
        .id(SharedString::from(format!("term-{idx}")))
        .h(rpx(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .pl(rpx(SPACE_3XL))
        .pr(rpx(SPACE_LG))
        .when(active, |d| d.bg(c::BG_HL()))
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, ctx.on(RowAction::SelectTerminal(idx)))
        .child(
            // No `flex_1` — see `project_row`'s name-child comment (`rows.rs`):
            // `.truncate()` fixes its ellipsis at measure time, so the name
            // cluster keeps `auto` basis and the close button (fixed-width)
            // claims the right edge via `ml_auto()` above instead.
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(rpx(SPACE_MD))
                .overflow_hidden()
                .child(icons::icon("term", ICON_SM, name_color).flex_none())
                .child(ui(context, TEXT_BODY, name_color).truncate()),
        )
        .child(close)
        .into_any_element()
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
                    sessions: vec![sid(1), sid(2)],
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
                    sessions: vec![sid(3)],
                },
            ],
        }
    }

    fn rows_of(snap: &TreeSnapshot, ws: &WorkspaceState, homes: usize) -> Vec<TreeRow> {
        flatten(
            snap,
            ws,
            &ActivityStore::new(),
            &HashMap::new(),
            &vec![false; homes],
        )
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

    /// The project row counts by project, not by worktree: a cold `wt_cache`
    /// gives a non-active project zero worktrees, and its count must still be
    /// its own session count (`sidebar.rs` `by_proj[s.project]`).
    #[test]
    fn a_worktree_cache_miss_still_counts_and_rolls_up_the_projects_sessions() {
        let mut snap = fixture();
        snap.projects[0].worktrees.clear();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(2), ActivityState::Working);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        // Selecting project 0 leaves it expanded; collapse it for the roll-up.
        ws.select_project(0);
        let rows = flatten(&snap, &ws, &activity, &HashMap::new(), &[]);
        let Some(TreeRow::Project {
            count,
            expanded,
            rollup,
            ..
        }) = rows.first()
        else {
            unreachable!("the first row is the project")
        };
        assert!(!expanded, "the project must be collapsed to roll up");
        assert_eq!(*count, snap.projects[0].sessions.len());
        assert_eq!(*rollup, Some(ActivityState::Working));
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
        let rows = flatten(&snap, &ws, &ActivityStore::new(), &suffixes, &[]);
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
