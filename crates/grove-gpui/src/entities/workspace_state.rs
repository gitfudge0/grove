//! `WorkspaceState` — the single owner of selection, tree presentation and
//! sidebar layout (spec §4).
//!
//! There is **no `WorkspaceState` type in the iced code**: this is the
//! consolidation of state that lives in two structs there and is mutated from a
//! dozen `update` handlers. The field-by-field provenance:
//!
//! | field | iced origin |
//! |---|---|
//! | `active_session` | `App::active_session` (`src/app/mod.rs:86`) — an `Option<usize>` index there, a stable [`SessionId`] here |
//! | `proj_idx` | `App::proj_idx` (`src/app/mod.rs:82`) |
//! | `wt_idx` | `App::wt_idx` (`src/app/mod.rs:83`) |
//! | `terminal_focused` | `Grove::terminal_focused` (`src/gui/state.rs:108`) |
//! | `active_terminal` | `App::active_terminal` (`src/app/mod.rs:95`) |
//! | `collapsed` | `Grove::collapsed` (`src/gui/state.rs:50`) |
//! | `collapsed_wt` | `Grove::collapsed_wt` (`src/gui/state.rs:53`) |
//! | `tree_expand` | `Grove::tree_expand` (`src/gui/state.rs:56`) |
//! | `terminals_collapsed` | `Grove::terminals_collapsed` (`src/gui/state.rs:118`) |
//! | `hovered_wt` | `Grove::hovered_wt` (`src/gui/state.rs:99`) |
//! | `open_agent_menu` | `Grove::open_agent_menu` (`src/gui/state.rs:66`) |
//! | `pending_kill` | `Grove::pending_kill` (`src/gui/state.rs:89`) |
//! | `pending_kill_terminal` | `Grove::pending_kill_terminal` (`src/gui/state.rs:94`) |
//! | `sidebar_width` | `Grove::sidebar_width` (`src/gui/state.rs:154`) |
//! | `focused_pane` / `grid_view` / `zen` | `Grove::{focused_pane, grid_view, ...}` — declared so Plan 07 does not re-open the single-owner question |
//!
//! `sync_wt_to_session` / `sync_session_to_wt` (`src/gui/update/mod.rs:1143`,
//! `:1164`) are **deleted, not ported** (carried amendment 5). Their observable
//! outcomes survive inside [`WorkspaceState::select_session`] and
//! [`WorkspaceState::select_worktree`] as a single forward pass each; nothing
//! here writes `active_session` and then re-reads it to fix up `wt_idx`.

// Several accessors and Plan 07 fields have no caller until their consumer
// lands (Tasks 5-7, Plan 07).
#![allow(dead_code)]

use std::collections::HashSet;

use gpui::Context;
use grove_core::storage::Store;

use crate::entities::session_registry::SessionId;

/// Default sidebar width, also the divider double-click reset target
/// (`src/gui/metrics.rs:9`).
pub const RAIL_W: f32 = 320.0;
/// Lower bound for the drag-resizable sidebar (`src/gui/metrics.rs:11`).
pub const SIDEBAR_MIN_W: f32 = 220.0;
/// Minimum workspace width the sidebar must leave behind
/// (`src/gui/metrics.rs:14`).
pub const WORKSPACE_MIN_W: f32 = 400.0;

/// Clamp a requested sidebar width (logical px) to its valid range for the
/// current window. Port of `src/gui/metrics.rs:244-251`: lower bound
/// [`SIDEBAR_MIN_W`], upper bound the smaller of half the window and "window
/// minus a usable workspace", but never below the lower bound — so the upper
/// bound wins outright on a narrow window.
///
/// Pulled forward from Task 5 (which TDDs it) because
/// [`WorkspaceState::new`] seeds through it.
pub fn clamp_sidebar_width(width: f32, logical_win_w: f32) -> f32 {
    let upper = (logical_win_w * 0.5)
        .min(logical_win_w - WORKSPACE_MIN_W)
        .max(SIDEBAR_MIN_W);
    width.clamp(SIDEBAR_MIN_W, upper)
}

/// The three modes the tree header's cycle button steps through, in ring order
/// `Collapsed → SessionsOnly → All → Collapsed`. Ported verbatim from
/// `src/gui/state.rs:27-44`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeExpand {
    /// Every project row collapsed — only the project list is visible.
    Collapsed,
    /// Projects/worktrees with no sessions collapsed; the rest expanded.
    SessionsOnly,
    /// Everything expanded.
    #[default]
    All,
}

impl TreeExpand {
    /// The mode a click advances to from `self`.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Collapsed => Self::SessionsOnly,
            Self::SessionsOnly => Self::All,
            Self::All => Self::Collapsed,
        }
    }
}

/// Which PTY input is routed to while the worktree terminal panel is open
/// (`src/gui/state.rs:14-22`). Plan 07 owns the routing; the field exists here
/// so the single-owner rule is not violated later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusedPane {
    #[default]
    Agent,
    Panel,
}

// ── the borrowed tree snapshot the pure transitions read ────────────────────

/// One worktree as the transitions see it: its path plus the sessions that
/// live in it, in registry insertion order.
#[derive(Clone, Debug, Default)]
pub struct SnapshotWorktree {
    /// Absolute path — the worktree's stable identity.
    pub path: String,
    /// Displayed name: the project name for the main worktree, otherwise
    /// `path_basename(path)` (`src/gui/view/sidebar.rs:285-289`).
    pub name: String,
    pub branch: String,
    pub is_main: bool,
    pub sessions: Vec<SessionId>,
}

/// One **active** project, carrying its TRUE `store.projects` index — the
/// index space `collapsed`/`proj_idx` are keyed on (`storage.rs:174`).
#[derive(Clone, Debug, Default)]
pub struct SnapshotProject {
    pub idx: usize,
    pub name: String,
    /// `git::is_repo(project.path)`, memoized for 5s by
    /// [`crate::entities::project_tree::ProjectTree`] (`sidebar.rs:26-54`).
    pub is_git: bool,
    /// Whether the project has a non-blank run script (`sidebar.rs:280-284`).
    pub has_run: bool,
    /// The project's worktrees, or **empty on a cache miss** — never a panic
    /// (`sidebar.rs:272-278`).
    pub worktrees: Vec<SnapshotWorktree>,
}

/// Everything the selection transitions need to know about the world, so they
/// stay pure functions testable without a gpui `App`.
#[derive(Clone, Debug, Default)]
pub struct TreeSnapshot {
    /// Active projects only, TRUE indices preserved.
    pub projects: Vec<SnapshotProject>,
    /// `store.projects.len()`, archived included — the empty-state input.
    pub total_projects: usize,
}

impl TreeSnapshot {
    fn project(&self, idx: usize) -> Option<&SnapshotProject> {
        self.projects.iter().find(|p| p.idx == idx)
    }

    /// `(true project index, worktree position)` of the worktree owning `id`.
    fn locate(&self, id: SessionId) -> Option<(usize, usize)> {
        self.projects.iter().find_map(|p| {
            p.worktrees
                .iter()
                .position(|w| w.sessions.contains(&id))
                .map(|wi| (p.idx, wi))
        })
    }

    fn worktree(&self, proj: usize, wt: usize) -> Option<&SnapshotWorktree> {
        self.project(proj).and_then(|p| p.worktrees.get(wt))
    }
}

/// Wrap-around index step (`src/app/util.rs:5-10`).
fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    {
        (cur as i32 + delta).rem_euclid(len as i32) as usize
    }
}

pub struct WorkspaceState {
    // selection — spec §4's single-owner set
    active_session: Option<SessionId>,
    proj_idx: usize,
    wt_idx: usize,
    terminal_focused: bool,
    active_terminal: Option<usize>,
    // tree presentation
    collapsed: HashSet<usize>,
    collapsed_wt: HashSet<(usize, usize)>,
    tree_expand: TreeExpand,
    terminals_collapsed: bool,
    // transient row affordances
    hovered_wt: Option<(usize, usize)>,
    open_agent_menu: Option<(usize, usize)>,
    pending_kill: Option<SessionId>,
    pending_kill_terminal: Option<usize>,
    // layout
    sidebar_width: f32,
    // Plan 07 owns these; declared so the single-owner rule is not violated.
    focused_pane: FocusedPane,
    grid_view: bool,
    zen: bool,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            active_session: None,
            proj_idx: 0,
            wt_idx: 0,
            terminal_focused: false,
            active_terminal: None,
            collapsed: HashSet::new(),
            collapsed_wt: HashSet::new(),
            tree_expand: TreeExpand::default(),
            terminals_collapsed: false,
            hovered_wt: None,
            open_agent_menu: None,
            pending_kill: None,
            pending_kill_terminal: None,
            sidebar_width: RAIL_W,
            focused_pane: FocusedPane::default(),
            grid_view: false,
            zen: false,
        }
    }
}

impl WorkspaceState {
    /// Seed from persisted settings. `logical_win_w` is the current window
    /// width; the stored width is clamped against it exactly as the iced build
    /// clamps on every drag (`layout.rs:105-160`).
    pub fn new(store: &Store, logical_win_w: f32) -> Self {
        Self {
            sidebar_width: clamp_sidebar_width(
                store.sidebar_width.unwrap_or(RAIL_W),
                logical_win_w,
            ),
            ..Self::default()
        }
    }

    // ── readout ─────────────────────────────────────────────────────────

    pub fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }
    pub fn proj_idx(&self) -> usize {
        self.proj_idx
    }
    pub fn wt_idx(&self) -> usize {
        self.wt_idx
    }
    pub fn terminal_focused(&self) -> bool {
        self.terminal_focused
    }
    pub fn active_terminal(&self) -> Option<usize> {
        self.active_terminal
    }
    pub fn tree_expand(&self) -> TreeExpand {
        self.tree_expand
    }
    pub fn terminals_collapsed(&self) -> bool {
        self.terminals_collapsed
    }
    pub fn hovered_wt(&self) -> Option<(usize, usize)> {
        self.hovered_wt
    }
    pub fn open_agent_menu(&self) -> Option<(usize, usize)> {
        self.open_agent_menu
    }
    pub fn pending_kill(&self) -> Option<SessionId> {
        self.pending_kill
    }
    pub fn pending_kill_terminal(&self) -> Option<usize> {
        self.pending_kill_terminal
    }
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }
    pub fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }
    pub fn grid_view(&self) -> bool {
        self.grid_view
    }
    pub fn zen(&self) -> bool {
        self.zen
    }
    pub fn project_collapsed(&self, proj: usize) -> bool {
        self.collapsed.contains(&proj)
    }
    pub fn worktree_collapsed(&self, proj: usize, wt: usize) -> bool {
        self.collapsed_wt.contains(&(proj, wt))
    }

    // ── transitions ─────────────────────────────────────────────────────

    /// Every focus transition acknowledges the session it lands on
    /// (spec §4: "Attention is never event-driven"). One call site set for
    /// Plan 06 to fill, rather than five to find.
    pub fn acknowledge(&mut self, _id: SessionId) {
        // Plan 06: truncates the attention state file.
    }

    /// `src/gui/update/sessions.rs:225-246` folded together with
    /// `sync_wt_to_session`'s outcome (`update/mod.rs:1143-1156`): one forward
    /// pass, no read-back.
    pub fn select_session(&mut self, id: SessionId, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.active_session = Some(id);
        self.terminal_focused = false;
        self.acknowledge(id);
        if let Some((pi, wi)) = snap.locate(id) {
            self.proj_idx = pi;
            self.wt_idx = wi;
        }
    }

    /// `sessions.rs:35-50` plus `sync_session_to_wt`'s outcome
    /// (`update/mod.rs:1164-1183`). `proj`/`wt` are a TRUE project index and a
    /// worktree position.
    pub fn select_worktree(&mut self, proj: usize, wt: usize, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.proj_idx = proj;
        self.wt_idx = wt;
        if !self.collapsed_wt.remove(&(proj, wt)) {
            self.collapsed_wt.insert((proj, wt));
        }
        // The worktree may not be cached yet — iced bails before touching
        // `active_session` in that case (`mod.rs:1172`).
        let Some(worktree) = snap.worktree(proj, wt) else {
            return;
        };
        let already_here = self
            .active_session
            .is_some_and(|id| worktree.sessions.contains(&id));
        if already_here {
            return;
        }
        self.active_session = worktree.sessions.first().copied();
    }

    /// `sessions.rs:22-33` + `switch_active_project` (`mod.rs:1121-1130`).
    /// The worktree-cache hand-off that function also performs belongs to
    /// [`crate::entities::project_tree::ProjectTree`], not to selection.
    pub fn select_project(&mut self, proj: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        if !self.collapsed.remove(&proj) {
            self.collapsed.insert(proj);
        }
        self.proj_idx = proj;
    }

    /// `sessions.rs:90-103`. `count` is `home_terminals.len()`.
    pub fn select_home_terminal(&mut self, i: usize, count: usize) {
        if i >= count {
            return;
        }
        self.active_terminal = Some(i);
        self.terminal_focused = true;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
    }

    /// The pending-confirmation shift across a home-terminal removal
    /// (`sessions.rs:109-113`). The registry owns the actual removal and the
    /// respawn; this is the selection half.
    pub fn close_home_terminal(&mut self, i: usize, remaining: usize) {
        self.pending_kill_terminal = match self.pending_kill_terminal {
            Some(p) if p == i => None,
            Some(p) if p > i => Some(p - 1),
            other => other,
        };
        // `App::close_home_terminal` (`src/app/terminals.rs:61-76`).
        self.active_terminal = match self.active_terminal {
            Some(a) if a == i => {
                if remaining == 0 {
                    None
                } else {
                    Some(i.min(remaining - 1))
                }
            }
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
        if self.active_terminal.is_none() {
            // Nothing left to show on the terminal tab — staying focused there
            // would swallow every keystroke (`sessions.rs:115-118`).
            self.terminal_focused = false;
        }
    }

    /// `sessions.rs:365-405`, walking `order` (Task 4's
    /// `visible_session_order`) rather than the raw session vector.
    pub fn cycle_session(&mut self, next: bool, order: &[SessionId], snap: &TreeSnapshot) {
        if order.is_empty() {
            return;
        }
        // Coming back from the terminal tab, the first press just reveals the
        // session that was already active — advancing off a session the user
        // cannot see is disorienting (`sessions.rs:376-383`).
        if self.terminal_focused {
            if let Some(cur) = self.active_session {
                self.select_session(cur, snap);
                return;
            }
            self.terminal_focused = false;
        }
        let delta = if next { 1 } else { -1 };
        let pos = match self.active_session.and_then(|id| {
            order
                .iter()
                .position(|&candidate| candidate == id)
                .map(|p| cycle(p, delta, order.len()))
        }) {
            Some(p) => p,
            None if next => 0,
            None => order.len() - 1,
        };
        let Some(&id) = order.get(pos) else {
            return;
        };
        self.select_session(id, snap);
    }

    /// `update/mod.rs:1213-1242`. Archived projects are skipped: the sets are
    /// keyed on TRUE indices, and `snap.projects` is already the active list.
    pub fn apply_tree_expand(&mut self, snap: &TreeSnapshot) {
        self.collapsed.clear();
        self.collapsed_wt.clear();
        match self.tree_expand {
            TreeExpand::All => {}
            TreeExpand::Collapsed => {
                for p in &snap.projects {
                    self.collapsed.insert(p.idx);
                }
            }
            TreeExpand::SessionsOnly => {
                for p in &snap.projects {
                    if !p.worktrees.iter().any(|w| !w.sessions.is_empty()) {
                        self.collapsed.insert(p.idx);
                    }
                    for (wi, w) in p.worktrees.iter().enumerate() {
                        if w.sessions.is_empty() {
                            self.collapsed_wt.insert((p.idx, wi));
                        }
                    }
                }
            }
        }
    }

    /// `sessions.rs:14-20`.
    pub fn toggle_collapse_all(&mut self, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.tree_expand = self.tree_expand.next();
        self.apply_tree_expand(snap);
    }

    /// `sessions.rs:270-280` without the index dance: a stable [`SessionId`]
    /// means the *other* sessions never move.
    pub fn on_session_removed(&mut self, id: SessionId) {
        if self.pending_kill == Some(id) {
            self.pending_kill = None;
        }
        if self.active_session == Some(id) {
            self.active_session = None;
        }
    }

    // ── transient affordances ───────────────────────────────────────────

    pub fn set_hovered_wt(&mut self, hovered: Option<(usize, usize)>) {
        self.hovered_wt = hovered;
    }
    pub fn set_open_agent_menu(&mut self, open: Option<(usize, usize)>) {
        self.open_agent_menu = open;
    }
    pub fn arm_kill(&mut self, id: SessionId) {
        self.pending_kill = Some(id);
        self.pending_kill_terminal = None;
    }
    pub fn arm_kill_terminal(&mut self, i: usize) {
        self.pending_kill_terminal = Some(i);
        self.pending_kill = None;
    }
    pub fn disarm_kill(&mut self) {
        self.pending_kill = None;
        self.pending_kill_terminal = None;
    }
    pub fn toggle_terminals_collapsed(&mut self) {
        self.terminals_collapsed = !self.terminals_collapsed;
    }
    pub fn set_sidebar_width(&mut self, width: f32, logical_win_w: f32) {
        self.sidebar_width = clamp_sidebar_width(width, logical_win_w);
    }

    /// Every mutation reaches views through the entity, so the one place that
    /// knows a repaint is due is the caller that had `&mut Context`.
    pub fn notify(cx: &mut Context<Self>) {
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(n: u64) -> SessionId {
        SessionId::from_raw(n)
    }

    /// Two active projects (TRUE indices 0 and 2 — index 1 is archived):
    /// - p0 "alpha": wt0 `/a` with sessions 1,2; wt1 `/a-x` with no sessions
    /// - p2 "gamma": wt0 `/g` with session 3
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
                    is_git: true,
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

    /// `sessions.rs:225-246` + `mod.rs:1143-1156`.
    #[test]
    fn select_session_moves_the_highlight_and_clears_the_kill_arms() {
        let snap = fixture();
        let mut w = WorkspaceState {
            terminal_focused: true,
            pending_kill: Some(sid(9)),
            pending_kill_terminal: Some(4),
            open_agent_menu: Some((0, 0)),
            ..WorkspaceState::default()
        };

        w.select_session(sid(3), &snap);

        assert_eq!(w.active_session(), Some(sid(3)));
        assert!(!w.terminal_focused());
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);
        assert_eq!(w.open_agent_menu(), None);
        // TRUE project index 2, worktree position 0.
        assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));
    }

    /// `sessions.rs:35-50` + `mod.rs:1164-1183`.
    #[test]
    fn select_worktree_repoints_the_active_session_at_the_first_one() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_worktree(0, 0, &snap);
        assert_eq!(w.active_session(), Some(sid(1)));
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
        // First click collapses (the set starts empty = expanded).
        assert!(w.worktree_collapsed(0, 0));
        // Second click expands again.
        w.select_worktree(0, 0, &snap);
        assert!(!w.worktree_collapsed(0, 0));
    }

    #[test]
    fn select_worktree_leaves_a_session_already_in_that_worktree_alone() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_session(sid(2), &snap);
        w.select_worktree(0, 0, &snap);
        assert_eq!(w.active_session(), Some(sid(2)));
    }

    #[test]
    fn select_worktree_clears_the_active_session_when_the_worktree_has_none() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_session(sid(1), &snap);
        w.select_worktree(0, 1, &snap);
        assert_eq!(w.active_session(), None);
    }

    /// `sessions.rs:22-33` + `mod.rs:1121-1130`.
    #[test]
    fn select_project_toggles_collapse_and_switches_the_active_project() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_project(2);
        assert_eq!(w.proj_idx(), 2);
        assert!(w.project_collapsed(2));
        w.select_project(2);
        assert!(!w.project_collapsed(2));
        let _ = snap;
    }

    /// `sessions.rs:90-103`.
    #[test]
    fn select_home_terminal_is_bounds_checked_and_clears_both_kill_arms() {
        let mut w = WorkspaceState {
            pending_kill: Some(sid(1)),
            pending_kill_terminal: Some(0),
            ..WorkspaceState::default()
        };

        w.select_home_terminal(5, 2);
        assert_eq!(w.active_terminal(), None);
        assert!(!w.terminal_focused());

        w.select_home_terminal(1, 2);
        assert_eq!(w.active_terminal(), Some(1));
        assert!(w.terminal_focused());
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);
    }

    /// `sessions.rs:109-113`.
    #[test]
    fn close_home_terminal_shifts_the_pending_confirmation_across_the_removal() {
        let mut w = WorkspaceState {
            pending_kill_terminal: Some(1),
            ..WorkspaceState::default()
        };
        w.close_home_terminal(1, 2);
        assert_eq!(w.pending_kill_terminal(), None);

        w.pending_kill_terminal = Some(2);
        w.close_home_terminal(1, 2);
        assert_eq!(w.pending_kill_terminal(), Some(1));

        w.pending_kill_terminal = Some(0);
        w.close_home_terminal(1, 2);
        assert_eq!(w.pending_kill_terminal(), Some(0));
    }

    #[test]
    fn closing_the_last_home_terminal_leaves_the_terminal_tab() {
        let mut w = WorkspaceState::default();
        w.select_home_terminal(0, 1);
        w.close_home_terminal(0, 0);
        assert_eq!(w.active_terminal(), None);
        assert!(!w.terminal_focused());
    }

    /// `sessions.rs:365-405`.
    #[test]
    fn cycle_session_wraps_in_visible_order() {
        let snap = fixture();
        let order = [sid(1), sid(2), sid(3)];
        let mut w = WorkspaceState::default();

        // No active session: next starts at the head, prev at the tail.
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(1)));
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(2)));
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(3)));
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(1)));
        w.cycle_session(false, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(3)));
    }

    #[test]
    fn cycle_session_from_the_terminal_tab_returns_to_the_last_agent_session() {
        let snap = fixture();
        let order = [sid(1), sid(2), sid(3)];
        let mut w = WorkspaceState::default();
        w.select_session(sid(2), &snap);
        w.select_home_terminal(0, 1);
        assert!(w.terminal_focused());

        // First press only reveals the session that was already active.
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(2)));
        assert!(!w.terminal_focused());
        // The next press advances for real.
        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(3)));
    }

    #[test]
    fn cycle_session_on_an_empty_order_is_a_no_op() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.cycle_session(true, &[], &snap);
        assert_eq!(w.active_session(), None);
    }

    /// `mod.rs:1213-1242`.
    #[test]
    fn apply_tree_expand_all_clears_both_sets() {
        let snap = fixture();
        let mut w = WorkspaceState {
            tree_expand: TreeExpand::All,
            ..WorkspaceState::default()
        };
        w.collapsed.insert(0);
        w.collapsed_wt.insert((0, 1));
        w.apply_tree_expand(&snap);
        assert!(!w.project_collapsed(0));
        assert!(!w.worktree_collapsed(0, 1));
    }

    #[test]
    fn apply_tree_expand_collapsed_collapses_every_active_project_only() {
        let snap = fixture();
        let mut w = WorkspaceState {
            tree_expand: TreeExpand::Collapsed,
            ..WorkspaceState::default()
        };
        w.apply_tree_expand(&snap);
        assert!(w.project_collapsed(0));
        assert!(w.project_collapsed(2));
        // Index 1 is archived — never recorded, TRUE indices preserved.
        assert!(!w.project_collapsed(1));
    }

    #[test]
    fn apply_tree_expand_sessions_only_collapses_the_sessionless() {
        let snap = fixture();
        let mut w = WorkspaceState {
            tree_expand: TreeExpand::SessionsOnly,
            ..WorkspaceState::default()
        };
        w.apply_tree_expand(&snap);
        // alpha has a sessionful worktree; gamma does too.
        assert!(!w.project_collapsed(0));
        assert!(!w.project_collapsed(2));
        // /a has sessions, /a-x does not.
        assert!(!w.worktree_collapsed(0, 0));
        assert!(w.worktree_collapsed(0, 1));
        assert!(!w.worktree_collapsed(2, 0));
    }

    #[test]
    fn sessions_only_collapses_a_project_with_no_sessionful_worktree() {
        let mut snap = fixture();
        snap.projects[1].worktrees[0].sessions.clear();
        let mut w = WorkspaceState {
            tree_expand: TreeExpand::SessionsOnly,
            ..WorkspaceState::default()
        };
        w.apply_tree_expand(&snap);
        assert!(w.project_collapsed(2));
        assert!(w.worktree_collapsed(2, 0));
    }

    /// `sessions.rs:14-20`.
    #[test]
    fn toggle_collapse_all_advances_the_ring_and_clears_transients() {
        let snap = fixture();
        let mut w = WorkspaceState {
            open_agent_menu: Some((0, 0)),
            pending_kill: Some(sid(1)),
            pending_kill_terminal: Some(0),
            ..WorkspaceState::default()
        };
        assert_eq!(w.tree_expand(), TreeExpand::All);

        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::Collapsed);
        assert!(w.project_collapsed(0));
        assert_eq!(w.open_agent_menu(), None);
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);

        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::SessionsOnly);
        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::All);
    }

    /// `sessions.rs:270-280` — the index dance this design removes.
    #[test]
    fn removing_a_session_only_clears_the_active_one() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_session(sid(2), &snap);
        w.on_session_removed(sid(1));
        assert_eq!(w.active_session(), Some(sid(2)));
        w.on_session_removed(sid(2));
        assert_eq!(w.active_session(), None);
    }

    /// Spec §4's negative: selection flows one way. Selecting a session sets
    /// `proj_idx`/`wt_idx` from the snapshot in the same pass; selecting a
    /// worktree never writes them back from `active_session`.
    #[test]
    fn selection_is_one_directional() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        // Point the highlight at gamma, then select a session in alpha.
        w.select_worktree(2, 0, &snap);
        w.select_session(sid(1), &snap);
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
        // Now select alpha's empty worktree: the highlight moves there and the
        // session clears — it is NOT dragged back to /a by a second pass.
        w.select_worktree(0, 1, &snap);
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 1));
        assert_eq!(w.active_session(), None);
    }

    /// `src/gui/metrics.rs:244-251`.
    #[test]
    fn clamp_sidebar_width_bounds() {
        // 1280 window → cap 640 (half wins over 1280-400=880).
        assert!((clamp_sidebar_width(900.0, 1280.0) - 640.0).abs() < f32::EPSILON);
        // 800 window → cap 400 (half = 400, 800-400 = 400).
        assert!((clamp_sidebar_width(900.0, 800.0) - 400.0).abs() < f32::EPSILON);
        // 500 window → both bounds collapse onto the 220 floor.
        assert!((clamp_sidebar_width(900.0, 500.0) - 220.0).abs() < f32::EPSILON);
        assert!((clamp_sidebar_width(10.0, 500.0) - 220.0).abs() < f32::EPSILON);
        assert!((clamp_sidebar_width(300.0, 1280.0) - 300.0).abs() < f32::EPSILON);
    }

    #[test]
    fn new_seeds_the_sidebar_width_through_the_clamp() {
        let store = Store {
            sidebar_width: Some(9000.0),
            ..Store::default()
        };
        let w = WorkspaceState::new(&store, 1280.0);
        assert!((w.sidebar_width() - 640.0).abs() < f32::EPSILON);

        let w = WorkspaceState::new(&Store::default(), 1280.0);
        assert!((w.sidebar_width() - RAIL_W).abs() < f32::EPSILON);
    }
}
