//! The sidebar's row model: pure helpers each row renders from, and the flattening that turns the project → worktree → session tree into the single `Vec<TreeRow>` the view scrolls.
//! Rows are emitted pre-resolved (carried amendment 2) to avoid the iced build's per-frame O(projects × worktrees × sessions) lookups; rows are not uniformly tall, so [`row_height`] is the single height function both the renderer and the agent-menu overlay must call.

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

/// The extra height a worktree row gains when it shows a branch chip: the
/// chip's own line. [`row_height`] and the renderer must both use this — the
/// agent-menu overlay is positioned from [`row_height`], so a renderer that
/// disagreed would misplace the menu (§8.1).
const BRANCH_LINE_H: f32 = 14.0;

/// Reserved width for every row's leading glyph slot: the glyph changes with
/// state but §2.4 forbids the row reflowing, so all call sites share this one constant to keep columns aligned.
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
    /// The sessions rail's card (mock D11 "diff-stat"). Not a denser
    /// [`TreeRow::Session`]: the tree's row identifies a session *inside* a
    /// worktree it is already nested under, while this one is the only thing
    /// on screen that says which worktree, which project and how much work is
    /// sitting in it — so every one of those facts is resolved onto the row by
    /// [`flatten_sessions`] rather than looked up while rendering.
    SessionCard {
        id: SessionId,
        /// Which agent is running here — the card's leading glyph, the same
        /// per-agent mark the tree's session row draws.
        agent: Agent,
        /// The card's headline: the terminal's OSC title, falling back to the
        /// session's own label when the agent has set none. Resolved in
        /// [`flatten_sessions`] like every other field on this row, never read
        /// out of an entity while rendering.
        title: String,
        /// The worktree's name — the second line's leading run.
        worktree: String,
        project: String,
        /// The worktree's branch, shown on the meta line in place of the
        /// session label — suppressed (empty display) when it duplicates
        /// [`Self::SessionCard::worktree`] ([`worktree_shows_branch`]), same
        /// rule as the tree's branch chip. Falls back to `""` when the
        /// worktree isn't in the snapshot's cache yet.
        branch: String,
        /// Time since the session's activity state last changed
        /// ([`ActivityStore::since_of`]), already formatted
        /// ([`elapsed_short`]) — falls back to time since `spawned_at` only
        /// for a session `since_of` doesn't know about yet. Shown on the
        /// second line, trailing the state word.
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
    /// A static, non-collapsible section label above one of the sessions
    /// rail's four sections (`NEEDS YOU`, `REVIEW`, `WORKING`, `IDLE`). No chevron, no toggle, no activity dot — unlike
    /// [`Self::TerminalsHeader`] it never responds to a click.
    SectionHeader { label: &'static str },
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
                git_suffix: git_suffix.get(normalize_wt_path(&w.path)).cloned(),
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

    push_terminals(&mut rows, ws, home_running);

    rows
}

/// The docked TERMINALS section, appended to whichever content mode the rail
/// is in. Shared so the two flatteners cannot drift.
fn push_terminals(rows: &mut Vec<TreeRow>, ws: &WorkspaceState, home_running: &[bool]) {
    if ws.terminals_collapsed() {
        return;
    }
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

/// The registry facts a session card is built from, lifted out by the view so
/// [`flatten_sessions`] can join them without reaching into an entity (module
/// doc, carried amendment 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// `Project::name` — the card's provenance, mandatory because the sessions
    /// list is cross-project.
    pub project: String,
    /// Absolute worktree path: the key that joins a session to its branch and
    /// to its git state.
    pub wt_path: String,
    /// `SessionMeta::label` (`claude 1`).
    pub label: String,
    /// Which agent runs in this session — the card's leading glyph.
    pub agent: Agent,
    /// The terminal's OSC title, already stripped and sanitized by
    /// [`session_context`], or `None` when the agent has set none. The card's
    /// headline falls back to [`Self::label`] in that case, so the two never
    /// come from two different resolvers.
    pub title: Option<String>,
    pub spawned_at: std::time::Instant,
}

/// The one string form of a worktree path that both sides of the git-state
/// join agree on.
///
/// The poll's cache is keyed by the path `git worktree list` printed, while a
/// session card joins on the path the session was **spawned** in
/// ([`SessionInfo::wt_path`]) — the same directory, but not necessarily the
/// same string. A single trailing slash is the difference the two sources can
/// actually produce, and it silently turned every card's diff into a cache
/// miss. Deliberately not `canonicalize`: that stats the filesystem, and this
/// runs per row per frame (module doc — no syscalls in a repaint).
#[must_use]
pub fn normalize_wt_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

/// What a card's trailing diff cluster shows. The distinction between
/// [`Self::Unknown`] and [`Self::Clean`] is the whole point: before the first
/// poll lands (or for a worktree the poll does not cover) there is *no
/// answer*, and drawing the `clean` chip there states one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffDisplay {
    /// No cache entry — draw nothing at all.
    Unknown,
    /// A real entry reporting no uncommitted work.
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

/// A duration as the rail's one-or-two-character age: `12s`, `12m`, `2h`,
/// `3d`. One unit only — the card has room for an age, not for a duration.
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

/// One session as [`flatten_sessions`] resolves it before sorting: its id, its
/// state, its `since_of` (last state transition) and its `active_of` (last
/// genuine `Working` tick). The last two are kept apart because the sections
/// key off different clocks.
type SessionRow = (
    SessionId,
    ActivityState,
    Option<std::time::Instant>,
    Option<std::time::Instant>,
);

/// Build the rail's **sessions** content mode: every session in every
/// project, flat and unnested, split into four labelled sections, followed by
/// the same docked TERMINALS tail [`flatten`] emits. Each section is preceded
/// by its own header, emitted only when that section is non-empty.
///
/// **`NEEDS YOU`** (`WaitingForInput` only) and **`REVIEW`** (`Done` only) are
/// deliberately separate sections rather than one merged "needs attention"
/// pile, even though both once lived in a single `NEEDS YOU` band ranked by
/// [`crate::activity::attention_rank`]. `WaitingForInput` is an agent stuck mid-task: transient
/// and self-clearing, one keystroke frees it. `Done` is a finished turn nobody
/// has read yet: sticky, and it accumulates the longer it goes unacknowledged.
/// Merging them turned the needs-you group into a review pile — a handful of
/// `Done` cards sitting on top of, or crowding out, the one agent that is
/// actually blocked waiting on the user. Splitting them means a single
/// `WaitingForInput` session can never be buried under a stack of `Done`
/// cards: it always has its own section, always first.
///
/// Both of these sections sort by [`ActivityStore::since_of`] ascending —
/// longest-blocked / longest-unread first — then id ascending, so a session
/// that has been waiting (or sitting unread) the longest stays on top and new
/// arrivals append at the bottom rather than shoving it down. Neither
/// comparator needs [`crate::activity::attention_rank`] any more: every card within a section
/// already shares that section's single state, so the rank would be a
/// constant. [`crate::activity::attention_rank`] itself stays in `src/activity.rs` — it still
/// backs `urgency_rank`/[`most_urgent`] for the collapsed-row roll-up.
///
/// Everything else splits into **`WORKING`** and **`IDLE`** by
/// [`ActivityStore::active_of`] against [`crate::activity::IDLE_DWELL`]:
/// `Working` is always `WORKING`, and a non-`Waiting`/non-`Done` live session
/// stays in `WORKING` while its last genuine `Working` tick is younger than
/// the dwell. `IDLE` is everything left over: a session quiet past the dwell,
/// and `Exited` unconditionally — the process is gone, no clock can argue it
/// back into flight, regardless of how fresh `last_active` looks. The dwell
/// governs **position only** — the card's colour, glyph and state word stay
/// instantly responsive to the real state.
///
/// Both `WORKING` and `IDLE` sort by [`ActivityStore::active_of`] descending —
/// most-recently-*working* first — and that key deliberately excludes both
/// [`ActivityState`] and [`ActivityStore::since_of`]. `since_of`/`state_since`
/// re-stamps on every state transition, including a routine `Working` -\>
/// `Idle` flap, so sorting by it would just move the churn from band-jumping
/// to top-of-list-jumping — the exact defect this split exists to kill.
/// `active_of`/`last_active` only advances while a session is genuinely
/// `Working` (or when the user acknowledges it), so a state transition alone —
/// a flap — can repaint a card without ever moving its row, and the dwell
/// keeps that flap from bouncing the row between the two sections either.
/// Within `IDLE`, `Exited` sorts after every `Idle` card regardless of clock —
/// a dead process is definitionally the least-recently-active thing on the
/// rail, not something whose stale `last_active` should let it outrank a live
/// one.
///
/// `info` is the per-session registry data the view resolved ([`SessionInfo`]);
/// `git` is the off-thread git-state cache keyed by worktree path. Both are
/// joined **here**, so the emitted [`TreeRow::SessionCard`]s carry every fact
/// their renderer needs and nothing looks back into state per frame. A session
/// with no [`ActivityStore::since_of`] (`NEEDS YOU` / `REVIEW`) or
/// [`ActivityStore::active_of`] (`WORKING` / `IDLE`) entry sorts as oldest; a
/// clockless non-`Working` session is `IDLE`, having no evidence of work.
#[must_use]
pub fn flatten_sessions(
    snap: &TreeSnapshot,
    ws: &WorkspaceState,
    activity: &ActivityStore,
    info: &HashMap<SessionId, SessionInfo>,
    git: &HashMap<String, grove_core::git::WorktreeGitState>,
    home_running: &[bool],
) -> Vec<TreeRow> {
    // `SnapshotProject::sessions` is keyed by project *name*, so it covers
    // projects whose worktree cache has never been populated — the worktree
    // walk would silently drop those.
    // The fourth field is `active_of` (last genuine `Working` tick), kept
    // separate from `since_of` (third field, last state transition) because
    // the sections deliberately key off different clocks below.
    let now = std::time::Instant::now();
    let all: Vec<SessionRow> = snap
        .projects
        .iter()
        .flat_map(|p| p.sessions.iter().copied())
        .map(|id| {
            (
                id,
                activity.state_of(id),
                activity.since_of(id),
                activity.active_of(id),
            )
        })
        .collect();

    // `NEEDS YOU` and `REVIEW` are split by state alone: `WaitingForInput` is
    // transient and self-clearing (one keystroke frees it), `Done` is sticky
    // and accumulates until acknowledged. Merging them buries the one truly
    // blocked agent under a pile of finished-but-unread turns.
    let needs_you_state = |s: ActivityState| matches!(s, ActivityState::WaitingForInput);
    let review_state = |s: ActivityState| matches!(s, ActivityState::Done);

    let mut needs_you: Vec<_> = all
        .iter()
        .filter(|(_, s, ..)| needs_you_state(*s))
        .collect();
    let mut review: Vec<_> = all.iter().filter(|(_, s, ..)| review_state(*s)).collect();

    // Position-only dwell: `Exited` is always `IDLE` (the process is gone, so
    // no clock can put it back in flight), `Working` is always `WORKING`, and
    // any other live session (i.e. `Idle` — `Waiting`/`Done` are already
    // carved out above) holds its `WORKING` slot while its last genuine
    // `Working` tick is younger than `IDLE_DWELL`. A session with no
    // `active_of` at all has never been seen working: `IDLE`.
    let working_state = |s: ActivityState, active: Option<std::time::Instant>| match s {
        ActivityState::Exited => false,
        ActivityState::Working => true,
        _ => active.is_some_and(|a| now.saturating_duration_since(a) < crate::activity::IDLE_DWELL),
    };

    let rest = |(_, s, ..): &&SessionRow| !needs_you_state(*s) && !review_state(*s);
    let mut working: Vec<_> = all
        .iter()
        .filter(|row @ (_, s, _, a)| rest(row) && working_state(*s, *a))
        .collect();
    let mut idle: Vec<_> = all
        .iter()
        .filter(|row @ (_, s, _, a)| rest(row) && !working_state(*s, *a))
        .collect();

    // Longest-blocked / longest-unread first: `since` ascending, `None` (no
    // clock yet) sorts as oldest via `Option`'s derived order (`None` <
    // `Some(_)`). id breaks exact ties deterministically so `sort_by` never
    // sees a non-transitive comparator. `attention_rank` is deliberately not
    // part of this comparator: every card in one of these sections already
    // shares that section's single state, so the rank would be constant.
    let by_since_asc = |a: &&SessionRow, b: &&SessionRow| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0));
    needs_you.sort_by(by_since_asc);
    review.sort_by(by_since_asc);

    // Most-recently-*working* first, in both sections: `active_of` (field 3,
    // `last_active`) descending, `None` sorting as oldest (last here) via the
    // same derived `Option` order, reversed. Deliberately NOT
    // `since_of`/`state_since` (field 2): that clock re-stamps on every state
    // transition, including a routine `Working` -\> `Idle` flap, so keying
    // these sections on it would just relocate the churn the split exists to
    // kill — from band-jumping to top-of-list-jumping. `active_of` only
    // advances on a genuine `Working` tick (or an acknowledgment), so a state
    // transition alone can never move a row.
    let by_active_desc =
        |a: &&SessionRow, b: &&SessionRow| b.3.cmp(&a.3).then_with(|| b.0.cmp(&a.0));
    working.sort_by(by_active_desc);
    // Inside IDLE, `Exited` sorts after every `Idle` card regardless of
    // clock: a leading `matches!(.., Exited)` bool key (`false < true`, so
    // `Exited` sorts last) takes priority over `active_of`, which is only a
    // tiebreak among the non-Exited cards and among the Exited ones.
    idle.sort_by(|a, b| {
        matches!(a.1, ActivityState::Exited)
            .cmp(&matches!(b.1, ActivityState::Exited))
            .then_with(|| by_active_desc(a, b))
    });

    let mut ordered: Vec<(&'static str, Vec<(SessionId, ActivityState)>)> = Vec::with_capacity(4);
    for (label, section) in [
        ("NEEDS YOU", &needs_you),
        ("REVIEW", &review),
        ("WORKING", &working),
        ("IDLE", &idle),
    ] {
        if !section.is_empty() {
            ordered.push((label, section.iter().map(|(id, s, ..)| (*id, *s)).collect()));
        }
    }

    // Worktree path → worktree name, built once: the card names the worktree
    // its session lives in, and the session only knows the path it was
    // spawned in.
    let worktrees: HashMap<&str, (&str, &str)> = snap
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .map(|w| {
            (
                normalize_wt_path(&w.path),
                (w.name.as_str(), w.branch.as_str()),
            )
        })
        .collect();

    let mut rows: Vec<TreeRow> = Vec::with_capacity(all.len() + ordered.len());
    let card = |(id, state): (SessionId, ActivityState)| {
        let meta = info.get(&id);
        let wt_path = meta.map_or("", |i| i.wt_path.as_str());
        // A worktree the snapshot has not cached yet still has a name in
        // its path — better than a blank run.
        let worktree = worktrees
            .get(normalize_wt_path(wt_path))
            .map_or_else(|| path_basename(wt_path), |(name, _)| (*name).to_string());
        // Same fallback degradation as the worktree name above: a worktree
        // the snapshot hasn't cached yet has no known branch either.
        let branch = worktrees
            .get(normalize_wt_path(wt_path))
            .map_or_else(String::new, |(_, b)| (*b).to_string());
        // A **missing** entry is not a clean worktree: it is the poll not
        // having answered yet. Both sides of the join are normalized so a
        // trailing slash cannot manufacture a miss.
        let diff = git
            .get(normalize_wt_path(wt_path))
            .map(|g| (g.added, g.removed));
        // Age is time since the state last changed — last activity — not
        // since the session spawned; only a session `since_of` doesn't know
        // about yet falls back to the spawn-time clock.
        let elapsed = activity.since_of(id).map_or_else(
            || {
                meta.map_or_else(String::new, |i| {
                    elapsed_short(now.saturating_duration_since(i.spawned_at))
                })
            },
            |since| elapsed_short(now.saturating_duration_since(since)),
        );
        TreeRow::SessionCard {
            id,
            agent: meta.map_or(Agent::Terminal, |i| i.agent),
            // The OSC title is what the agent is *doing*; the label is
            // only who it is. Prefer the former, fall back to the latter
            // — an agent that never set a title still gets a headline.
            title: meta
                .map(|i| i.title.clone().unwrap_or_else(|| i.label.clone()))
                .unwrap_or_default(),
            worktree,
            project: meta.map(|i| i.project.clone()).unwrap_or_default(),
            branch,
            elapsed,
            // Same rule as the tree: a session must not look active while
            // a home terminal is on screen (`sidebar.rs:338`).
            active: !ws.terminal_focused() && ws.active_session() == Some(id),
            pending_kill: ws.pending_kill() == Some(id),
            state,
            diff,
        }
    };
    for (label, section) in ordered {
        rows.push(TreeRow::SectionHeader { label });
        rows.extend(section.into_iter().map(card));
    }

    if rows.is_empty() {
        let (title, subtitle) = sidebar_empty_copy(snap.total_projects, snap.projects.len())
            .unwrap_or(("No sessions yet", "Start one from a worktree in the tree."));
        rows.push(TreeRow::Empty { title, subtitle });
    }

    push_terminals(&mut rows, ws, home_running);

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
            TreeRow::Session { id, .. } | TreeRow::SessionCard { id, .. } => Some(*id),
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
    /// The sessions rail card's `+N −M` chip: open the diff viewer for this
    /// session's worktree.
    OpenDiff(SessionId),
    ArmKillSession(SessionId),
    KillSession(SessionId),
    SelectTerminal(usize),
    ArmKillTerminal(usize),
    KillTerminal(usize),
    NewHomeTerminal,
    ToggleTerminalsSection,
    ToggleCollapseAll,
    /// The rail header's content-mode button — swaps the tree for the flat
    /// cross-project session list, and back.
    ToggleRailMode,
    /// The rail header's grid button — the same toggle `mod+g` performs, in
    /// and out of the agent grid.
    ToggleGridView,
    /// The rail header's `+` in Tree mode — opens the add-project wizard
    /// (Task 4).
    AddProject,
    /// The rail header's `+` in Sessions mode — opens the command palette
    /// scoped to worktrees, i.e. "launch a session from which worktree?".
    LaunchInWorktree,
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
        TreeRow::SessionCard { .. } => session_card(row, ctx),
        TreeRow::Empty { title, subtitle } => empty_row(title, subtitle),
        TreeRow::SectionHeader { label } => section_header(label),
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

/// A state's accent colour and its **word**. Colour is never alone (§2.3):
/// every card spells the state out beside the dot it tints.
fn state_accent(state: ActivityState) -> (gpui::Hsla, &'static str) {
    match state {
        ActivityState::WaitingForInput => (c::AMBER(), "needs you"),
        ActivityState::Working => (c::GREEN(), "working"),
        ActivityState::Done => (c::BLUE(), "done"),
        ActivityState::Idle => (c::FG_MUTE(), "idle"),
        ActivityState::Exited => (c::FG_MUTE(), "exited"),
    }
}

/// The state's mark in the [`STATUS_DOT_COL_W`] column: the spinner while
/// working, a hollow dot where nothing is running, a filled one otherwise.
/// Filled-versus-hollow is a *shape* difference, so present-versus-absent
/// survives greyscale (see [`status_dot_hollow`]).
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

/// The trailing diff stat: `+N` and `-N`, one neutral `clean` chip when the
/// poll says the worktree has no uncommitted work, and **nothing at all**
/// while it has not answered — an empty slot is honest about not knowing,
/// where `clean` would be a claim. The delete chip's sign is an **ASCII**
/// hyphen, not U+2212 — the bundled fonts have no minus glyph (§9.3, R7).
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

/// The sessions rail's card (mock D11). One [`card`] holding one
/// [`RowDensity::Card`] row of three text lines: the agent's glyph with the
/// session title as the headline; the worktree with the state and age; and
/// `project · branch` with the diff stat underneath.
///
/// Every state treatment is a *fill or an overlay*, never a size: the card's
/// box is pinned to [`SESSION_CARD_H`] (the same token [`TreeRow::height`]
/// reports), so arming a kill or gaining the attention bar cannot reflow the
/// list under the cursor.
fn session_card(row: &TreeRow, ctx: &RowCtx) -> AnyElement {
    let TreeRow::SessionCard {
        id,
        agent,
        title,
        worktree,
        project,
        branch,
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

    // Two-step confirm, exactly as the tree's row does it: the first press
    // arms (red tick), the second kills. It holds its slot in both states, so
    // arming never moves anything.
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

    // The title keeps `auto` basis and the kill button claims the right edge
    // with `ml_auto` — see `project_row`'s name-child comment: a
    // `.truncate()` inside a `flex_1` (basis 0%) parent never re-grows.
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

    // The worktree line: what this session is working in, what it is doing
    // right now, and how long it has been in that state. The diff stat sits on
    // the meta line below, next to the rest of the card's metadata.
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
                .child(ui(word, TEXT_MICRO, accent))
                .child(ui(elapsed.clone(), TEXT_MICRO, c::FG_MUTE())),
        );

    // The meta line: `project · branch` — the branch is suppressed
    // (`worktree_shows_branch`) when it duplicates the worktree name already
    // shown on the line above, same rule as the tree's branch chip. The diff
    // stat sits at its trailing edge; its chips are `TEXT_MICRO` in a
    // `keycap_filled` shell, shorter than the `TEXT_SMALL` run they sit
    // beside, so the line still fits `CARD_LINE_SM_H` and the height
    // `TreeRow::height` reports stays true.
    let meta = div()
        .flex()
        .w_full()
        .h(rpx(CARD_LINE_SM_H))
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(
            div().flex().min_w_0().overflow_hidden().child(
                ui(
                    if worktree_shows_branch(false, branch, worktree) {
                        format!("{project} · {branch}")
                    } else {
                        project.clone()
                    },
                    TEXT_SMALL,
                    c::FG_MUTE(),
                )
                .truncate(),
            ),
        )
        .child({
            let chips = diff_chips(*diff);
            // Only an actual `+N -M` pair is worth opening the viewer for —
            // a clean worktree has nothing to show, and the chip's own shell
            // already draws nothing while the poll is unknown.
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
        // Fills stack from least to most urgent, so the last one wins: an
        // armed kill outranks the selection, which outranks the attention
        // tint. None of them changes the box.
        .when(waiting, |d| d.bg(c::AMBER_ROW_TINT()))
        .when(*active, |d| {
            d.bg(c::SEL_TINT_SOFT()).border_color(c::SEL_RING())
        })
        .when(*pending_kill, |d| {
            d.bg(c::RED_WASH()).border_color(c::RED())
        })
        // Overlaid rather than a `border_l` so the amber accent never shifts
        // the card's content (`src/gui/rows.rs:539-566`).
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

/// A static, non-collapsible section label above one of the sessions rail's
/// four sections (`NEEDS YOU`, `REVIEW`, `WORKING`, `IDLE`). Same typography/colour as [`terminals_header`]'s label
/// (mono, [`TEXT_MICRO`], [`c::FG_MUTE`]) but with no chevron, no toggle
/// action and no activity dot — it never collapses.
fn section_header(label: &'static str) -> AnyElement {
    div()
        .id("sessions-section-header")
        .h(rpx(ROW_H))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_MD))
                .pl(rpx(SPACE_2XL))
                .child(mono(tracked(label), TEXT_MICRO, c::FG_MUTE())),
        )
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

    /// A compact shape description, so order assertions read as the tree.
    /// An instant `d` in the past. `Instant` has no past literal, and a bare
    /// `now - d` is an unchecked subtraction the lint rejects. On a monotonic
    /// clock too young to subtract from, the fallback is *now* — which makes
    /// every "this is stale" assertion below fail loudly rather than pass by
    /// accident.
    fn ago(d: std::time::Duration) -> std::time::Instant {
        std::time::Instant::now()
            .checked_sub(d)
            .unwrap_or_else(std::time::Instant::now)
    }

    fn shape(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Project { idx, name, .. } => format!("P{idx}:{name}"),
                TreeRow::Worktree { proj, wt, name, .. } => format!("W{proj}.{wt}:{name}"),
                TreeRow::Session { id, .. } => format!("S{}", id.raw()),
                TreeRow::SessionCard { id, .. } => format!("C{}", id.raw()),
                TreeRow::Empty { title, .. } => format!("E:{title}"),
                TreeRow::SectionHeader { label } => format!("H:{label}"),
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

    /// The sessions content mode: labelled sections, each headed and each in
    /// its own order. The fixture only has three sessions, so this exercises
    /// three of the four sections (`NEEDS YOU`, `WORKING`, `IDLE`); `REVIEW`
    /// is covered by the dedicated tests below.
    #[test]
    fn the_sessions_mode_splits_into_headed_sections() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        // s1 idle (default, no tracker at all — clockless, so IDLE), s2
        // needs-you, s3 working (always WORKING).
        activity.set_state_for_test(sid(2), ActivityState::WaitingForInput);
        activity.set_state_for_test(sid(3), ActivityState::Working);
        activity.set_state_since_for_test(sid(3), std::time::Instant::now());
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(
            shape(&rows),
            vec!["H:NEEDS YOU", "C2", "H:WORKING", "C3", "H:IDLE", "C1"]
        );
    }

    /// `WaitingForInput` and `Done` land in NEEDS YOU and REVIEW
    /// respectively, and never in each other's section.
    #[test]
    fn a_waiting_card_and_a_done_card_land_in_separate_sections() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(1), ActivityState::WaitingForInput);
        activity.set_state_for_test(sid(2), ActivityState::Done);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        // s3 has no tracker at all (clockless), so it falls to IDLE.
        assert_eq!(
            shape(&rows),
            vec!["H:NEEDS YOU", "C1", "H:REVIEW", "C2", "H:IDLE", "C3"]
        );
    }

    /// The regression this split exists to fix: a single blocked
    /// `WaitingForInput` session must never get buried under a pile of
    /// `Done` reviews, no matter how much longer those reviews have been
    /// sitting unread. `NEEDS YOU` is a whole section ahead of `REVIEW`, not
    /// merely a higher rank within one shared section.
    #[test]
    fn a_waiting_session_never_gets_buried_under_done_reviews() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        let base = std::time::Instant::now();
        // s1, s3: Done, unread far longer than s2's wait.
        activity.set_state_for_test(sid(1), ActivityState::Done);
        activity.set_state_since_for_test(sid(1), base);
        activity.set_state_for_test(sid(3), ActivityState::Done);
        activity.set_state_since_for_test(sid(3), base + std::time::Duration::from_secs(5));
        // s2: WaitingForInput, but only just started waiting.
        activity.set_state_for_test(sid(2), ActivityState::WaitingForInput);
        activity.set_state_since_for_test(sid(2), base + std::time::Duration::from_secs(100));
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(
            shape(&rows),
            vec!["H:NEEDS YOU", "C2", "H:REVIEW", "C1", "C3"]
        );
    }

    /// REVIEW orders longest-unread first, same rule as NEEDS YOU: `since_of`
    /// ascending.
    #[test]
    fn the_review_zone_sorts_longest_unread_first() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        let base = std::time::Instant::now();
        activity.set_state_for_test(sid(1), ActivityState::Done);
        activity.set_state_since_for_test(sid(1), base);
        activity.set_state_for_test(sid(3), ActivityState::Done);
        activity.set_state_since_for_test(sid(3), base + std::time::Duration::from_secs(10));
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        // s2 has no tracker at all (clockless), so it falls to IDLE.
        assert_eq!(shape(&rows), vec!["H:REVIEW", "C1", "C3", "H:IDLE", "C2"]);
    }

    /// No `NEEDS YOU` header — not even an empty one — when nothing is
    /// waiting: only the sections that actually have rows are headed.
    #[test]
    fn the_needs_you_header_is_absent_when_that_section_is_empty() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(2), ActivityState::Working);
        activity.set_state_for_test(sid(3), ActivityState::Idle);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        let shape = shape(&rows);
        assert!(!shape.contains(&"H:NEEDS YOU".to_string()));
        // s2 (Working) and s3 (Idle, but `set_state_for_test` gives it a
        // fresh `last_active` inside the dwell) are WORKING; s1 has no
        // tracker at all, so it is clockless and IDLE.
        assert_eq!(shape, vec!["H:WORKING", "C3", "C2", "H:IDLE", "C1"]);
    }

    /// No `REVIEW` header when nothing is `Done`.
    #[test]
    fn the_review_header_is_absent_when_that_section_is_empty() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(2), ActivityState::WaitingForInput);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert!(!shape(&rows).contains(&"H:REVIEW".to_string()));
    }

    /// No `IDLE` header when every live session is in flight, and no
    /// `WORKING` header when none of them is.
    #[test]
    fn the_working_and_idle_headers_are_each_absent_when_their_section_is_empty() {
        let snap = fixture();
        let now = std::time::Instant::now();
        // Everything Working: WORKING only.
        let mut activity = ActivityStore::new();
        for n in 1..=3 {
            activity.set_state_for_test(sid(n), ActivityState::Working);
            activity.set_last_active_for_test(sid(n), now);
        }
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        let shape_working = shape(&rows);
        assert!(!shape_working.contains(&"H:IDLE".to_string()));
        assert_eq!(shape_working[0], "H:WORKING");

        // Everything long-quiet: IDLE only.
        let mut activity = ActivityStore::new();
        let stale = ago(crate::activity::IDLE_DWELL + std::time::Duration::from_secs(1));
        for n in 1..=3 {
            activity.set_state_for_test(sid(n), ActivityState::Idle);
            activity.set_last_active_for_test(sid(n), stale);
        }
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        let shape_idle = shape(&rows);
        assert!(!shape_idle.contains(&"H:WORKING".to_string()));
        assert_eq!(shape_idle[0], "H:IDLE");
    }

    /// `Exited` is IDLE unconditionally: the process is gone, so even a
    /// last_active from a moment ago must not park it in WORKING.
    #[test]
    fn an_exited_session_is_idle_even_with_a_fresh_last_active() {
        let snap = fixture();
        let now = std::time::Instant::now();
        let mut activity = ActivityStore::new();
        activity.set_state_for_test(sid(1), ActivityState::Exited);
        activity.set_last_active_for_test(sid(1), now);
        activity.set_state_for_test(sid(2), ActivityState::Working);
        activity.set_last_active_for_test(sid(2), now);
        activity.set_state_for_test(sid(3), ActivityState::Exited);
        activity.set_last_active_for_test(sid(3), now);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(shape(&rows), vec!["H:WORKING", "C2", "H:IDLE", "C3", "C1"]);
    }

    /// Inside IDLE, `Exited` sorts after every `Idle` card regardless of
    /// clock: a dead process (fresh `last_active`) must not outrank a still
    /// merely-quiet-past-the-dwell live session.
    #[test]
    fn exited_sorts_below_idle_inside_the_idle_section() {
        let snap = fixture();
        let now = std::time::Instant::now();
        let mut activity = ActivityStore::new();
        // s1: Idle, past the dwell — a genuinely stale live session.
        activity.set_state_for_test(sid(1), ActivityState::Idle);
        activity.set_last_active_for_test(
            sid(1),
            ago(crate::activity::IDLE_DWELL + std::time::Duration::from_secs(1)),
        );
        // s2: Working, so it lands in WORKING rather than muddying IDLE.
        activity.set_state_for_test(sid(2), ActivityState::Working);
        activity.set_last_active_for_test(sid(2), now);
        // s3: Exited with a *fresh* last_active — must still sort below s1.
        activity.set_state_for_test(sid(3), ActivityState::Exited);
        activity.set_last_active_for_test(sid(3), now);
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(shape(&rows), vec!["H:WORKING", "C2", "H:IDLE", "C1", "C3"]);
    }

    /// The invariant that matters most: WORKING / IDLE placement and ordering
    /// key on `active_of`/`last_active`, never on the raw state or on
    /// `since_of`/`state_since`. A `Working` -\> `Idle` flap re-stamps
    /// `state_since` on every real tick but never touches `last_active`, so it
    /// must neither reorder a row nor move it to another section — the card
    /// repaints in place. `IDLE_DWELL` is what buys the second half of that:
    /// with placement keyed on `state == Working` alone, the flapped session
    /// (fresh `last_active`, state now `Idle`) would drop straight into IDLE
    /// and the row index below would change.
    #[test]
    fn a_working_to_idle_flap_with_a_fresh_last_active_never_moves_a_row() {
        let snap = fixture();
        let mut activity = ActivityStore::new();
        let base = std::time::Instant::now();
        activity.set_state_for_test(sid(1), ActivityState::Working);
        activity.set_state_since_for_test(sid(1), base);
        activity.set_last_active_for_test(sid(1), base);
        activity.set_state_for_test(sid(2), ActivityState::Idle);
        activity.set_state_since_for_test(sid(2), base + std::time::Duration::from_secs(1));
        activity.set_last_active_for_test(sid(2), base + std::time::Duration::from_secs(1));
        activity.set_state_for_test(sid(3), ActivityState::Working);
        activity.set_state_since_for_test(sid(3), base + std::time::Duration::from_secs(2));
        activity.set_last_active_for_test(sid(3), base + std::time::Duration::from_secs(2));
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let before = shape(&flatten_sessions(
            &snap,
            &ws,
            &activity,
            &HashMap::new(),
            &HashMap::new(),
            &[],
        ));
        // All three are inside `IDLE_DWELL`, so the whole rail is WORKING.
        assert_eq!(before, vec!["H:WORKING", "C3", "C2", "C1"]);

        // Flip s1 Working -> Idle exactly as the real classification tick
        // does: `state_since` re-stamps to a later instant (later than s2's
        // and s3's) since the state actually changed, but `last_active` is
        // left untouched — Idle never advances it.
        activity.set_state_for_test(sid(1), ActivityState::Idle);
        activity.set_state_since_for_test(sid(1), base + std::time::Duration::from_secs(3));
        let after = shape(&flatten_sessions(
            &snap,
            &ws,
            &activity,
            &HashMap::new(),
            &HashMap::new(),
            &[],
        ));
        assert_eq!(before, after);
    }

    /// The other side of the dwell: once `last_active` is older than
    /// `IDLE_DWELL`, a non-`Working` session does fall through to IDLE.
    #[test]
    fn a_stale_last_active_falls_through_to_idle() {
        let snap = fixture();
        let now = std::time::Instant::now();
        let mut activity = ActivityStore::new();
        // s1: Idle but worked on a moment ago — still WORKING.
        activity.set_state_for_test(sid(1), ActivityState::Idle);
        activity.set_last_active_for_test(sid(1), now);
        // s2: Idle and quiet for longer than the dwell — IDLE.
        activity.set_state_for_test(sid(2), ActivityState::Idle);
        activity.set_last_active_for_test(
            sid(2),
            ago(crate::activity::IDLE_DWELL + std::time::Duration::from_secs(1)),
        );
        // s3: Idle and quiet for far longer — IDLE, below s2 (older clock).
        activity.set_state_for_test(sid(3), ActivityState::Idle);
        activity.set_last_active_for_test(sid(3), ago(crate::activity::IDLE_DWELL * 20));
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let rows = flatten_sessions(&snap, &ws, &activity, &HashMap::new(), &HashMap::new(), &[]);
        assert_eq!(shape(&rows), vec!["H:WORKING", "C1", "H:IDLE", "C2", "C3"]);
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

    /// The card's join: `wt_path` → the snapshot's worktree name, and → the
    /// git poll's uncommitted diff counts.
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
                // `/g` has no entry — unknown, *not* clean.
                "gamma/gamma Unknown".to_string(),
                "alpha/alpha Counts(128, 9)".to_string(),
                "alpha/alpha Counts(128, 9)".to_string(),
            ]
        );
    }

    /// The bug the sessions rail shipped with: a cache miss and a genuinely
    /// clean worktree both rendered the neutral `clean` chip, so a rail that
    /// was never polled looked like a repo with no work in it.
    #[test]
    fn a_missing_git_entry_shows_no_chip_while_a_zero_entry_shows_clean() {
        assert_eq!(diff_display(None), DiffDisplay::Unknown);
        assert_eq!(diff_display(Some((0, 0))), DiffDisplay::Clean);
        assert_eq!(diff_display(Some((3, 1))), DiffDisplay::Counts(3, 1));
    }

    /// Both sides of the git join agree on one string form, so a trailing
    /// slash on either cannot manufacture a miss.
    #[test]
    fn the_git_join_normalizes_a_trailing_slash_on_the_worktree_path() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let now = std::time::Instant::now();
        // The registry spawned this session with a trailing slash…
        let session_info = HashMap::from([(sid(3), info("gamma", "/g/", now))]);
        // …while the poll keyed its answer without one.
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

    /// The card is a fixed box, and `height()` is the only place that says so.
    #[test]
    fn a_session_card_declares_the_cards_height_not_the_row_height() {
        let card = TreeRow::SessionCard {
            id: sid(1),
            agent: Agent::Claude,
            title: "reviewing the rail".into(),
            worktree: "alpha".into(),
            project: "alpha".into(),
            branch: "main".into(),
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

    /// The headline is the agent's OSC title; a session whose agent never set
    /// one falls back to its own label rather than showing an empty line.
    #[test]
    fn a_cards_headline_is_the_osc_title_and_falls_back_to_the_label() {
        let snap = fixture();
        let mut ws = WorkspaceState::default();
        ws.toggle_terminals_collapsed();
        let now = std::time::Instant::now();
        let mut titled = info("alpha", "/a", now);
        titled.title = Some("refactoring the rail".into());
        let session_info = HashMap::from([
            (sid(1), titled),
            // No OSC title — this one must fall back to `label`.
            (sid(2), info("alpha", "/a", now)),
        ]);
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
        // A session the registry has no entry for still gets a glyph.
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

    /// The cards are the sessions mode's `mod+1..9` index space, so the
    /// visible order must see them exactly as it sees the tree's rows.
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
