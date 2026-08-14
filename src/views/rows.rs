//! Sidebar row model: pure helpers plus the flattening of project → worktree →
//! session into `Vec<TreeRow>`. Rows are emitted pre-resolved (ported from
//! `src/gui/view/sidebar.rs:227-237`, `src/gui/rows.rs:268`).

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
use crate::views::components::{
    card, click_row_on, diff_chip, icon_btn, keycap_filled, mono, status_dot, status_dot_hollow,
    tracked, ui, RowDensity,
};

/// Sidebar row height (`src/gui/metrics.rs:7`).
pub const ROW_H: f32 = 28.0;

/// Extra height a branch-chip worktree row gains; must match the agent-menu overlay's [`row_height`] math.
const BRANCH_LINE_H: f32 = 14.0;

/// Reserved width for a row's leading glyph, shared across all call sites so columns align down the tree.
const GLYPH_SLOT_W: f32 = 14.0;

const INDENT_WORKTREE: f32 = SPACE_2XL + GLYPH_SLOT_W;
/// Derived from [`INDENT_WORKTREE`] so a session's glyph lines up under its worktree's name.
const INDENT_SESSION: f32 = INDENT_WORKTREE + GLYPH_SLOT_W + SPACE_MD;

const EMPTY_ROW_PAD_Y: f32 = 24.0;

#[must_use]
pub fn worktree_shows_branch(is_main: bool, branch: &str, name: &str) -> bool {
    !is_main && branch != name && !branch.is_empty()
}

#[must_use]
pub fn row_height(show_branch: bool) -> f32 {
    if show_branch {
        ROW_H + BRANCH_LINE_H
    } else {
        ROW_H
    }
}

/// Reimplemented rather than shared with `crate::app::path_basename` — grove-core/the iced app stay read-only.
#[must_use]
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

/// Strips glyphs the UI font can't render; `None` when nothing useful is left.
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

/// gpui rows are built once per repaint from an already-materialized title, so unlike the iced build's per-tick cache, no cache is ported here.
#[must_use]
pub fn terminal_context(raw_title: &str, label: &str) -> Option<String> {
    sanitize_ui_text(&remove_all_ci(raw_title, label))
}

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

/// The two states must never share copy — each has a different fix.
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

/// One row of the sidebar, carrying everything its renderer needs.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeRow {
    Project {
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
    /// Sessions rail card; unlike [`TreeRow::Session`] it carries its own worktree/project since it's shown cross-project.
    SessionCard {
        id: SessionId,
        agent: Agent,
        /// Falls back to the session's own label when the agent has set no OSC title.
        title: String,
        worktree: String,
        project: String,
        agent_label: String,
        elapsed: String,
        active: bool,
        pending_kill: bool,
        state: ActivityState,
        /// Lines added/removed in the worktree's uncommitted diff against
        /// `HEAD`, or `None` while the poll has no entry for this worktree.
        /// `Some((0, 0))` is the clean worktree, which draws one neutral chip
        /// rather than a `+0 -0` pair; `None` draws **nothing** — see
        /// [`DiffDisplay`].
        diff: Option<(u32, u32)>,
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
    /// This row's rendered height. Worktrees showing a branch chip are taller
    /// (`src/gui/rows.rs:268`); the sessions rail's card is a different shape
    /// altogether and declares [`SESSION_CARD_H`]. Still the single height
    /// source — the renderer sets the same token on the card's box, so the two
    /// cannot drift.
    #[must_use]
    pub fn height(&self) -> f32 {
        match self {
            Self::Worktree {
                name,
                branch,
                is_main,
                ..
            } => row_height(worktree_shows_branch(*is_main, branch, name)),
            Self::SessionCard { .. } => SESSION_CARD_H,
            _ => ROW_H,
        }
    }
}

/// `git_suffix` is the off-thread git-state map, already rendered to text by `grove_core::git::git_state_suffix`.
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
        let sessions = &p.sessions;
        rows.push(TreeRow::Project {
            idx: p.idx,
            name: p.name.clone(),
            count: sessions.len(),
            expanded,
            is_git: p.is_git,
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
                rollup: (!wt_expanded)
                    .then(|| most_urgent(w.sessions.iter().map(|&id| activity.state_of(id))))
                    .flatten(),
                git_suffix: git_suffix.get(normalize_wt_path(&w.path)).cloned(),
            });
            if !wt_expanded {
                continue;
            }
            for &id in &w.sessions {
                rows.push(TreeRow::Session {
                    id,
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

    push_terminals(&mut rows, ws, home_running);

    rows
}

/// The docked TERMINALS section, appended to whichever content mode the rail
/// is in. Shared so the two flatteners cannot drift.
fn push_terminals(rows: &mut Vec<TreeRow>, ws: &WorkspaceState, home_running: &[bool]) {
    if ws.terminals_collapsed() {
        return;
    }
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

/// Attention band, low number first: needs-you → working → idle → done → exited.
fn attention_band(state: ActivityState) -> u8 {
    match state {
        ActivityState::WaitingForInput => 0,
        ActivityState::Working => 1,
        ActivityState::Idle => 2,
        ActivityState::Done => 3,
        ActivityState::Exited => 4,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub project: String,
    pub wt_path: String,
    pub label: String,
    pub agent: Agent,
    /// Card headline falls back to `label` when `None`.
    pub title: Option<String>,
    pub spawned_at: std::time::Instant,
}

/// A trailing slash is the one difference between the poll's cache key and a session's spawn path; not `canonicalize` — no syscalls in a repaint.
#[must_use]
pub fn normalize_wt_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

/// `Unknown` (no poll data yet) is distinct from `Clean` (poll confirms no uncommitted work) — drawing `clean` for `Unknown` would be a false claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffDisplay {
    Unknown,
    Clean,
    Counts(u32, u32),
}

/// [`DiffDisplay`] for a card's joined diff counts.
#[must_use]
pub fn diff_display(diff: Option<(u32, u32)>) -> DiffDisplay {
    match diff {
        None => DiffDisplay::Unknown,
        Some((0, 0)) => DiffDisplay::Clean,
        Some((added, removed)) => DiffDisplay::Counts(added, removed),
    }
}

/// Age as one unit: `12s`, `12m`, `2h`, `3d`.
#[must_use]
pub fn elapsed_short(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Sessions across all projects, flat, sorted attention-first then most-recent within a band; a session with no `info` entry sorts oldest.
#[must_use]
pub fn flatten_sessions(
    snap: &TreeSnapshot,
    ws: &WorkspaceState,
    activity: &ActivityStore,
    info: &HashMap<SessionId, SessionInfo>,
    git: &HashMap<String, grove_core::git::WorktreeGitState>,
    home_running: &[bool],
) -> Vec<TreeRow> {
    // Keyed by project name, not the worktree walk, so projects with an unpopulated worktree cache still count.
    let mut sessions: Vec<(u8, SessionId, ActivityState)> = snap
        .projects
        .iter()
        .flat_map(|p| p.sessions.iter().copied())
        .map(|id| {
            let state = activity.state_of(id);
            (attention_band(state), id, state)
        })
        .collect();
    // Most recent first inside a band; monotonic ids break ties.
    sessions.sort_by(|a, b| {
        let (sa, sb) = (
            info.get(&a.1).map(|i| i.spawned_at),
            info.get(&b.1).map(|i| i.spawned_at),
        );
        a.0.cmp(&b.0)
            .then_with(|| sb.cmp(&sa))
            .then_with(|| b.1.cmp(&a.1))
    });

    let worktrees: HashMap<&str, &str> = snap
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .map(|w| (normalize_wt_path(&w.path), w.name.as_str()))
        .collect();
    let now = std::time::Instant::now();

    let mut rows: Vec<TreeRow> = sessions
        .into_iter()
        .map(|(_, id, state)| {
            let meta = info.get(&id);
            let wt_path = meta.map_or("", |i| i.wt_path.as_str());
            let worktree = worktrees
                .get(normalize_wt_path(wt_path))
                .map_or_else(|| path_basename(wt_path), |w| (*w).to_string());
            // Missing entry means the poll hasn't answered yet, not a clean worktree.
            let diff = git
                .get(normalize_wt_path(wt_path))
                .map(|g| (g.added, g.removed));
            TreeRow::SessionCard {
                id,
                agent: meta.map_or(Agent::Terminal, |i| i.agent),
                title: meta
                    .map(|i| i.title.clone().unwrap_or_else(|| i.label.clone()))
                    .unwrap_or_default(),
                worktree,
                project: meta.map(|i| i.project.clone()).unwrap_or_default(),
                agent_label: meta.map(|i| i.label.clone()).unwrap_or_default(),
                elapsed: meta.map_or_else(String::new, |i| {
                    elapsed_short(now.saturating_duration_since(i.spawned_at))
                }),
                active: !ws.terminal_focused() && ws.active_session() == Some(id),
                pending_kill: ws.pending_kill() == Some(id),
                state,
                diff,
            }
        })
        .collect();

    if rows.is_empty() {
        let (title, subtitle) = sidebar_empty_copy(snap.total_projects, snap.projects.len())
            .unwrap_or(("No sessions yet", "Start one from a worktree in the tree."));
        rows.push(TreeRow::Empty { title, subtitle });
    }

    push_terminals(&mut rows, ws, home_running);

    rows
}

/// Derived from [`flatten`]'s output rather than a second tree walk, to avoid drift; this is `mod+1..9`'s index space.
#[must_use]
pub fn visible_session_order(rows: &[TreeRow]) -> Vec<SessionId> {
    rows.iter()
        .filter_map(|r| match r {
            TreeRow::Session { id, .. } | TreeRow::SessionCard { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// Returns `(proj, wt, top, is_main)`; `6.0` is the tree area's top padding minus the menu's own 2px lift.
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

/// What a row click asks the [`crate::views::sidebar::Sidebar`] to do; rows never reach into state themselves.
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
    OpenDiff(SessionId),
    ArmKillSession(SessionId),
    KillSession(SessionId),
    SelectTerminal(usize),
    ArmKillTerminal(usize),
    KillTerminal(usize),
    NewHomeTerminal,
    ToggleTerminalsSection,
    ToggleCollapseAll,
    ToggleRailMode,
    ToggleGridView,
    AddProject,
    LaunchInWorktree,
}

pub type Dispatch = Rc<dyn Fn(RowAction, &mut Window, &mut App)>;

pub struct RowCtx {
    pub tick: u64,
    /// `[0, 1]`, 0 = opaque, 1 = max dim.
    pub pulse: f32,
    pub hovered_wt: Option<(usize, usize)>,
    pub available: Vec<Agent>,
    pub session_text: HashMap<SessionId, (Agent, Option<String>)>,
    /// Positional, like the rows themselves.
    pub terminal_text: Vec<Option<String>>,
    pub dispatch: Dispatch,
}

impl RowCtx {
    fn on(&self, action: RowAction) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
        let dispatch = Rc::clone(&self.dispatch);
        move |_, window, cx| dispatch(action, window, cx)
    }
}

/// WaitingForInput dims rather than hides, so the row layout never moves.
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

fn main_tag() -> AnyElement {
    ui("main", TEXT_MICRO, c::GREEN()).into_any_element()
}

/// Reads only from `row` and `ctx` — never back into the tree.
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
        TreeRow::SessionCard { .. } => session_card(row, ctx),
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
                // No flex_1: truncate() fixes its ellipsis at measure time, so basis 0% would never re-grow it.
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
        // Clickable filler: the name cluster can't be flex_1, so this absorbs the blank middle to keep it clickable.
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
        // Must shrink, not wrap: row height is fixed at `row_height`, and a wrapped chip would spill over its neighbours.
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

    // `main` tag and hover strip share one fixed slot so they never render at once.
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
            // No flex_1 — same truncate/measure-time footgun as `project_row`'s name child.
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
    // No flex_1 — same truncate/measure-time footgun; close button uses ml_auto instead.
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

    // Two-step confirm: first press arms (red tick), second kills.
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
        // Overlaid rather than a border_l so the amber accent never shifts row content.
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

/// Colour is never alone — every card spells the state out beside the dot it tints.
fn state_accent(state: ActivityState) -> (gpui::Hsla, &'static str) {
    match state {
        ActivityState::WaitingForInput => (c::AMBER(), "needs you"),
        ActivityState::Working => (c::GREEN(), "working"),
        ActivityState::Done => (c::BLUE(), "done"),
        ActivityState::Idle => (c::FG_MUTE(), "idle"),
        ActivityState::Exited => (c::FG_MUTE(), "exited"),
    }
}

/// Filled-versus-hollow is a shape difference so it survives greyscale.
fn card_state_mark(state: ActivityState, accent: gpui::Hsla, tick: u64) -> AnyElement {
    let inner: AnyElement = match state {
        ActivityState::Working => icons::spinner(ICON_SM, accent, tick).into_any_element(),
        ActivityState::Idle | ActivityState::Exited => {
            status_dot_hollow(DOT_SM, accent).into_any_element()
        }
        ActivityState::WaitingForInput | ActivityState::Done => {
            status_dot(DOT_SM, accent).into_any_element()
        }
    };
    div()
        .w(rpx(STATUS_DOT_COL_W))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
        .into_any_element()
}

/// Nothing while unanswered (honest about not knowing) vs a `clean` chip once polled. Uses an ASCII hyphen, not U+2212 — bundled fonts have no minus glyph.
pub fn diff_chips(diff: Option<(u32, u32)>) -> AnyElement {
    let row = div().flex().flex_none().items_center().gap(rpx(SPACE_SM));
    match diff_display(diff) {
        DiffDisplay::Unknown => row.into_any_element(),
        DiffDisplay::Clean => row
            .child(diff_chip("clean", c::FG_MUTE()))
            .into_any_element(),
        DiffDisplay::Counts(added, removed) => row
            .child(diff_chip(format!("+{added}"), c::GREEN()))
            .child(diff_chip(format!("-{removed}"), c::RED()))
            .into_any_element(),
    }
}

/// State treatments are always a fill or overlay, never a size change — the card's box is pinned to [`SESSION_CARD_H`] so nothing reflows the list.
fn session_card(row: &TreeRow, ctx: &RowCtx) -> AnyElement {
    let TreeRow::SessionCard {
        id,
        agent,
        title,
        worktree,
        project,
        agent_label,
        elapsed,
        active,
        pending_kill,
        state,
        diff,
    } = row
    else {
        return div().into_any_element();
    };
    let id = *id;
    let (accent, word) = state_accent(*state);
    let waiting = *state == ActivityState::WaitingForInput;
    let title_color = match state {
        ActivityState::Exited => c::FG_MUTE(),
        ActivityState::Idle => c::FG_DIM(),
        _ => c::FG(),
    };

    // Two-step confirm, same as the tree's row.
    let close = if *pending_kill {
        tool_button(
            "card-kill",
            id.raw(),
            "check",
            ICON_SM,
            true,
            ctx,
            RowAction::KillSession(id),
        )
    } else {
        tool_button(
            "card-arm",
            id.raw(),
            "close",
            ICON_SM,
            false,
            ctx,
            RowAction::ArmKillSession(id),
        )
    };

    // No flex_1 — same truncate/measure-time footgun; kill button uses ml_auto.
    let headline = div()
        .flex()
        .w_full()
        .h(rpx(CARD_LINE_H))
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(icons::icon(agent.icon_name(), ICON_SM, accent).flex_none())
        .child(
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .child(ui(title.clone(), TEXT_BODY, title_color).truncate()),
        )
        .child(div().flex().flex_none().ml_auto().child(close));

    let context_line = div()
        .flex()
        .w_full()
        .h(rpx(CARD_LINE_SM_H))
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .child(mono(worktree.clone(), TEXT_SMALL, c::FG_DIM()).truncate()),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .ml_auto()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(card_state_mark(*state, accent, ctx.tick))
                .child(ui(word, TEXT_MICRO, accent)),
        );

    let meta = div()
        .flex()
        .w_full()
        .h(rpx(CARD_LINE_SM_H))
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(
            div().flex().min_w_0().overflow_hidden().child(
                ui(
                    format!("{project} · {agent_label} · {elapsed}"),
                    TEXT_SMALL,
                    c::FG_MUTE(),
                )
                .truncate(),
            ),
        )
        .child({
            let chips = diff_chips(*diff);
            match diff_display(*diff) {
                DiffDisplay::Counts(..) => {
                    let dispatch = Rc::clone(&ctx.dispatch);
                    div()
                        .id(("diff-chip-open", id.raw()))
                        .flex()
                        .flex_none()
                        .ml_auto()
                        .rounded(rpx(RADIUS_CONTROL))
                        .cursor_pointer()
                        .hover(|s| s.bg(c::BG_HOVER()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            dispatch(RowAction::OpenDiff(id), window, cx);
                        })
                        .child(chips)
                        .into_any_element()
                }
                _ => div()
                    .flex()
                    .flex_none()
                    .ml_auto()
                    .child(chips)
                    .into_any_element(),
            }
        });

    let dispatch = Rc::clone(&ctx.dispatch);
    let body = click_row_on(
        SharedString::from(format!("card-{}", id.raw())),
        *active,
        RowDensity::Card,
        move |window, cx| dispatch(RowAction::SelectSession(id), window, cx),
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .py(rpx(SPACE_LG))
            .gap(rpx(ROW_LINE_GAP))
            .child(headline)
            .child(context_line)
            .child(meta),
    )
    .h_full();

    card(vec![body.into_any_element()])
        .h(rpx(SESSION_CARD_H))
        .flex_none()
        .relative()
        .overflow_hidden()
        // Fills stack least-to-most urgent: armed kill > selection > attention tint. None changes the box.
        .when(waiting, |d| d.bg(c::AMBER_ROW_TINT()))
        .when(*active, |d| {
            d.bg(c::SEL_TINT_SOFT()).border_color(c::SEL_RING())
        })
        .when(*pending_kill, |d| {
            d.bg(c::RED_WASH()).border_color(c::RED())
        })
        .when(waiting, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w(rpx(ATTENTION_BAR_W))
                    .bg(c::alpha(c::AMBER(), 1.0 - 0.45 * ctx.pulse)),
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
    // No synthetic "terminal N" name — the shell's own title, falling back to `~`.
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
    // AnyElement isn't Styled, so the ml_auto margin needs a thin div wrapper.
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
            // No flex_1 — same truncate/measure-time footgun as `project_row`.
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

    #[test]
    fn branch_chip_only_for_non_main_worktrees_with_a_distinct_branch() {
        assert!(worktree_shows_branch(false, "feature", "wt"));
        assert!(!worktree_shows_branch(true, "feature", "wt"));
        assert!(!worktree_shows_branch(false, "wt", "wt"));
        assert!(!worktree_shows_branch(false, "", "wt"));
    }

    #[test]
    fn a_branch_chip_makes_the_row_fourteen_pixels_taller() {
        assert!((row_height(false) - 28.0).abs() < f32::EPSILON);
        assert!((row_height(true) - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn path_basename_handles_trailing_slashes_roots_and_odd_input() {
        assert_eq!(path_basename("/a/b/c"), "c");
        assert_eq!(path_basename("/a/b/c/"), "c");
        assert_eq!(path_basename("/"), "/");
        assert_eq!(path_basename(""), "");
        assert_eq!(path_basename("plain"), "plain");
        assert_eq!(path_basename("/a/b/../c"), "c");
    }

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
        assert_eq!(sanitize_ui_text("café"), Some("café".into()));
    }

    #[test]
    fn remove_all_ci_is_case_insensitive_and_utf8_safe() {
        assert_eq!(remove_all_ci("Claude claude CLAUDE x", "claude"), "   x");
        assert_eq!(remove_all_ci("caféXcafé", "X"), "cafécafé");
        assert_eq!(remove_all_ci("abc", ""), "abc");
        assert_eq!(remove_all_ci("abc", "zzz"), "abc");
        assert_eq!(remove_all_ci("", "a"), "");
    }

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

    fn sid(n: u64) -> SessionId {
        SessionId::from_raw(n)
    }

    /// Indices 0 and 2 are active; index 1 is archived.
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

    fn info(project: &str, wt_path: &str, spawned_at: std::time::Instant) -> SessionInfo {
        SessionInfo {
            project: project.into(),
            wt_path: wt_path.into(),
            label: "claude 1".into(),
            agent: Agent::Claude,
            title: None,
            spawned_at,
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

    fn shape(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Project { idx, name, .. } => format!("P{idx}:{name}"),
                TreeRow::Worktree { proj, wt, name, .. } => format!("W{proj}.{wt}:{name}"),
                TreeRow::Session { id, .. } => format!("S{}", id.raw()),
                TreeRow::SessionCard { id, .. } => format!("C{}", id.raw()),
                TreeRow::Empty { title, .. } => format!("E:{title}"),
                TreeRow::TerminalsHeader { count, .. } => format!("TH{count}"),
                TreeRow::Terminal { idx, .. } => format!("T{idx}"),
            })
            .collect()
    }

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

    #[test]
    fn the_sessions_mode_is_flat_and_sorted_attention_first() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(2), ActivityState::WaitingForInput);
        activity.set_state_for_test(sid(3), ActivityState::Working);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(shape(&rows), vec!["C2", "C3", "C1"]);
    }

    #[test]
    fn the_sessions_mode_puts_the_newest_first_inside_a_band() {
        let snap = fixture();
        let base = std::time::Instant::now();
        let spawned = HashMap::from([
            (sid(1), info("alpha", "/a", base)),
            (
                sid(2),
                info("alpha", "/a", base + std::time::Duration::from_secs(2)),
            ),
            (
                sid(3),
                info("gamma", "/g", base + std::time::Duration::from_secs(1)),
            ),
        ]);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(
            &snap,
            &ws,
            &ActivityStore::new(),
            &spawned,
            &HashMap::new(),
            &[],
        );
        assert_eq!(shape(&rows), vec!["C2", "C3", "C1"]);
    }

    #[test]
    fn the_sessions_mode_keeps_the_terminals_tail_and_shows_an_empty_state() {
        let mut snap = fixture();
        for p in &mut snap.projects {
            p.sessions.clear();
            for w in &mut p.worktrees {
                w.sessions.clear();
            }
        }
        let ws = WorkspaceState::default();
        let rows = flatten_sessions(
            &snap,
            &ws,
            &ActivityStore::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[false],
        );
        assert_eq!(shape(&rows), vec!["E:No sessions yet", "TH1", "T0"]);
    }

    #[test]
    fn a_session_card_joins_its_worktree_and_its_diff_stat_by_worktree_path() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let now = std::time::Instant::now();
        let session_info = HashMap::from([
            (sid(1), info("alpha", "/a", now)),
            (sid(2), info("alpha", "/a", now)),
            (sid(3), info("gamma", "/g", now)),
        ]);
        let git = HashMap::from([(
            "/a".to_string(),
            grove_core::git::WorktreeGitState {
                dirty: true,
                ahead: 0,
                behind: 0,
                added: 128,
                removed: 9,
            },
        )]);
        let found: Vec<String> =
            flatten_sessions(&snap, &ws, &ActivityStore::new(), &session_info, &git, &[])
                .iter()
                .filter_map(|r| match r {
                    TreeRow::SessionCard {
                        worktree,
                        project,
                        diff,
                        ..
                    } => Some(format!("{project}/{worktree} {:?}", diff_display(*diff))),
                    _ => None,
                })
                .collect();
        assert_eq!(
            found,
            vec![
                "gamma/gamma Unknown".to_string(),
                "alpha/alpha Counts(128, 9)".to_string(),
                "alpha/alpha Counts(128, 9)".to_string(),
            ]
        );
    }

    #[test]
    fn a_missing_git_entry_shows_no_chip_while_a_zero_entry_shows_clean() {
        assert_eq!(diff_display(None), DiffDisplay::Unknown);
        assert_eq!(diff_display(Some((0, 0))), DiffDisplay::Clean);
        assert_eq!(diff_display(Some((3, 1))), DiffDisplay::Counts(3, 1));
    }

    #[test]
    fn the_git_join_normalizes_a_trailing_slash_on_the_worktree_path() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let now = std::time::Instant::now();
        let session_info = HashMap::from([(sid(3), info("gamma", "/g/", now))]);
        let git = HashMap::from([(
            "/g".to_string(),
            grove_core::git::WorktreeGitState {
                dirty: true,
                ahead: 0,
                behind: 0,
                added: 5,
                removed: 2,
            },
        )]);
        let diffs: Vec<Option<(u32, u32)>> =
            flatten_sessions(&snap, &ws, &ActivityStore::new(), &session_info, &git, &[])
                .iter()
                .filter_map(|r| match r {
                    TreeRow::SessionCard { diff, .. } => Some(*diff),
                    _ => None,
                })
                .collect();
        assert!(diffs.contains(&Some((5, 2))));
    }

    #[test]
    fn a_session_card_declares_the_cards_height_not_the_row_height() {
        let card = TreeRow::SessionCard {
            id: sid(1),
            agent: Agent::Claude,
            title: "reviewing the rail".into(),
            worktree: "alpha".into(),
            project: "alpha".into(),
            agent_label: "claude 1".into(),
            elapsed: "2m".into(),
            active: false,
            pending_kill: false,
            state: ActivityState::Idle,
            diff: Some((0, 0)),
        };
        assert!((card.height() - SESSION_CARD_H).abs() < f32::EPSILON);
        assert!(card.height() > ROW_H, "a card is taller than a tree row");
        // Arming a kill is a fill, never a size (§2.4).
        let mut armed = card.clone();
        if let TreeRow::SessionCard { pending_kill, .. } = &mut armed {
            *pending_kill = true;
        }
        assert!((armed.height() - card.height()).abs() < f32::EPSILON);
    }

    #[test]
    fn a_cards_headline_is_the_osc_title_and_falls_back_to_the_label() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let now = std::time::Instant::now();
        let mut titled = info("alpha", "/a", now);
        titled.title = Some("refactoring the rail".into());
        let session_info = HashMap::from([(sid(1), titled), (sid(2), info("alpha", "/a", now))]);
        let found: Vec<(u64, String, Agent)> = flatten_sessions(
            &snap,
            &ws,
            &ActivityStore::new(),
            &session_info,
            &HashMap::new(),
            &[],
        )
        .iter()
        .filter_map(|r| match r {
            TreeRow::SessionCard {
                id, title, agent, ..
            } => Some((id.raw(), title.clone(), *agent)),
            _ => None,
        })
        .collect();
        assert!(found.contains(&(1, "refactoring the rail".to_string(), Agent::Claude)));
        assert!(found.contains(&(2, "claude 1".to_string(), Agent::Claude)));
        assert!(found.contains(&(3, String::new(), Agent::Terminal)));
    }

    #[test]
    fn elapsed_short_shows_exactly_one_unit() {
        use std::time::Duration;
        assert_eq!(elapsed_short(Duration::from_secs(0)), "0s");
        assert_eq!(elapsed_short(Duration::from_secs(59)), "59s");
        assert_eq!(elapsed_short(Duration::from_mins(1)), "1m");
        assert_eq!(elapsed_short(Duration::from_secs(12 * 60 + 30)), "12m");
        assert_eq!(elapsed_short(Duration::from_secs(3599)), "59m");
        assert_eq!(elapsed_short(Duration::from_hours(1)), "1h");
        assert_eq!(elapsed_short(Duration::from_secs(86_399)), "23h");
        assert_eq!(elapsed_short(Duration::from_hours(24)), "1d");
    }

    #[test]
    fn the_visible_order_counts_cards_as_sessions() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(
            &snap,
            &ws,
            &ActivityStore::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[],
        );
        assert_eq!(visible_session_order(&rows).len(), 3);
    }

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
        ws.select_project(0);
        let rows = rows_of(&snap, &ws, 0);
        let Some(TreeRow::Project { expanded, .. }) = rows.first() else {
            unreachable!("the first row is the project")
        };
        assert!(!expanded);
    }

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

    #[test]
    fn a_worktree_cache_miss_still_counts_and_rolls_up_the_projects_sessions() {
        let mut snap = fixture();
        snap.projects[0].worktrees.clear();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(2), ActivityState::Working);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
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

    #[test]
    fn the_agent_menu_anchors_below_its_worktree_row() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = rows_of(&snap, &ws, 0);
        let Some((proj, wt, top, is_main)) = agent_menu_top(&rows, (0, 1)) else {
            unreachable!("the open worktree is in the tree")
        };
        assert_eq!((proj, wt), (0, 1));
        assert!(!is_main);
        assert!((top - (6.0 + 112.0 + 42.0)).abs() < f32::EPSILON);

        let Some((.., top, is_main)) = agent_menu_top(&rows, (0, 0)) else {
            unreachable!("the main worktree is in the tree")
        };
        assert!(is_main);
        assert!((top - (6.0 + 28.0 + 28.0)).abs() < f32::EPSILON);

        ws.select_project(0);
        let collapsed = rows_of(&snap, &ws, 0);
        assert!(agent_menu_top(&collapsed, (0, 1)).is_none());
    }

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
        let tall = rows_of(&snap, &ws, 0)
            .iter()
            .filter(|r| (r.height() - ROW_H).abs() > f32::EPSILON)
            .count();
        assert_eq!(tall, 1);
    }
}
