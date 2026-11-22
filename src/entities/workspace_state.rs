//! WorkspaceState — the single owner of selection, tree presentation and sidebar layout (spec §4); consolidates two iced-era structs mutated from a dozen `update` handlers.

mod grid_navigation;

mod grid_sizing;

pub use grid_sizing::{
    GridDrag, GridPointerDrag, GridResizeDrag, GridResizeMode, GridSizing, GridSlide,
};

use std::collections::{HashMap, HashSet};

use grove_core::storage::Store;

use crate::entities::session_registry::SessionId;
use crate::grid::{GridAxis, GridBoundary};

/// Default sidebar width, also the divider double-click reset target (`src/gui/metrics.rs:9`).
pub const RAIL_W: f32 = 320.0;
/// Lower bound for the drag-resizable sidebar (`src/gui/metrics.rs:11`).
pub const SIDEBAR_MIN_W: f32 = 220.0;
/// Minimum workspace width the sidebar must leave behind (`src/gui/metrics.rs:14`).
pub const WORKSPACE_MIN_W: f32 = 400.0;

/// Port of `src/gui/metrics.rs:244-251`: clamps to `[SIDEBAR_MIN_W, min(half window, window - WORKSPACE_MIN_W)]`, so the upper bound wins outright on a narrow window.
pub fn clamp_sidebar_width(width: f32, logical_win_w: f32) -> f32 {
    let upper = (logical_win_w * 0.5)
        .min(logical_win_w - WORKSPACE_MIN_W)
        .max(SIDEBAR_MIN_W);
    width.clamp(SIDEBAR_MIN_W, upper)
}

/// The three modes the tree header's cycle button steps through, in ring order `Collapsed → SessionsOnly → All → Collapsed` (`src/gui/state.rs:27-44`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeExpand {
    Collapsed,
    #[default]
    SessionsOnly,
    All,
}

impl TreeExpand {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Collapsed | Self::SessionsOnly => Self::All,
            Self::All => Self::Collapsed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RailMode {
    #[default]
    Tree,
    Sessions,
}

impl RailMode {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Tree => Self::Sessions,
            Self::Sessions => Self::Tree,
        }
    }
}

/// Default terminal-panel share of the workspace width, in percent (`src/gui/metrics.rs:38`); the agent view gets `100 - TERM_PANEL_PORTION`.
pub const TERM_PANEL_PORTION: u16 = 40;
/// Bounds and step (percent of the workspace) for resizing the panel (`src/gui/metrics.rs:42-44`).
pub const TERM_PANEL_PORTION_MIN: u16 = 20;
pub const TERM_PANEL_PORTION_MAX: u16 = 75;
pub const TERM_PANEL_PORTION_STEP: u16 = 5;
/// Keyboard grid-resize steps, in percentage points of the whole axis.
pub const GRID_RESIZE_STEP_PCT: f32 = 5.0;
pub const GRID_RESIZE_FINE_STEP_PCT: f32 = 1.0;

/// Port of `src/gui/metrics.rs:257-263`: the panel is docked on the right, so a cursor further left grows it; clamped to `[TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX]`.
#[must_use]
pub fn term_portion_for_cursor(cursor_x: f32, logical_win_w: f32, sidebar_w: f32) -> u16 {
    let work_left = sidebar_w + crate::views::tokens::DIVIDER_DRAG_HIT_W;
    let work_w = (logical_win_w - work_left).max(1.0);
    let frac = ((logical_win_w - cursor_x) / work_w).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation)]
    let pct = (frac * 100.0).round() as i32;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    {
        pct.clamp(
            i32::from(TERM_PANEL_PORTION_MIN),
            i32::from(TERM_PANEL_PORTION_MAX),
        ) as u16
    }
}

/// Survives in gpui as the **persisted intent** that decides which `FocusHandle` to focus on open / re-anchor (carried amendment 8); the keystrokes themselves follow gpui focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusedPane {
    #[default]
    Agent,
    Panel,
}

/// `Tile` is carried so the call sites can share one enum; tile focus is `grid_focused`'s job and `focus_pane` ignores it (`pty_input.rs:146-158`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyPane {
    Agent,
    Panel,
    // Constructed by `#[cfg(test)]` code only — hence the `#[allow(dead_code)]`.
    #[allow(dead_code)]
    Tile(SessionId),
}

#[derive(Clone, Debug)]
pub struct LiveTile {
    pub id: SessionId,
    pub key: String,
}

/// What [`WorkspaceState::toggle_terminal_tab`] needs the caller to do after the pure transition has run — the spawn itself is the view's job (`update/mod.rs:493-497`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTabOutcome {
    pub spawn_home_terminal: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SnapshotWorktree {
    pub path: String,
    /// Displayed name: the project name for the main worktree, otherwise `path_basename(path)` (`src/gui/view/sidebar.rs:285-289`).
    pub name: String,
    pub branch: String,
    pub is_main: bool,
    pub sessions: Vec<SessionId>,
}

/// One **active** project, carrying its TRUE `store.projects` index — the index space `collapsed`/`proj_idx` are keyed on (`storage.rs:174`).
#[derive(Clone, Debug, Default)]
pub struct SnapshotProject {
    pub idx: usize,
    pub name: String,
    /// `git::is_repo(project.path)`, memoized for 5s by [`crate::entities::project_tree::ProjectTree`] (`sidebar.rs:26-54`).
    pub is_git: bool,
    /// Whether the project has a non-blank run script (`sidebar.rs:280-284`).
    pub has_run: bool,
    /// The project's worktrees, or **empty on a cache miss** — never a panic (`sidebar.rs:272-278`).
    pub worktrees: Vec<SnapshotWorktree>,
    pub sessions: Vec<SessionId>,
}

#[derive(Clone, Debug, Default)]
pub struct TreeSnapshot {
    pub projects: Vec<SnapshotProject>,
    pub total_projects: usize,
}

impl TreeSnapshot {
    fn project(&self, idx: usize) -> Option<&SnapshotProject> {
        self.projects.iter().find(|p| p.idx == idx)
    }

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
    active_session: Option<SessionId>,
    used: HashMap<SessionId, u64>,
    used_seq: u64,
    proj_idx: usize,
    wt_idx: usize,
    terminal_focused: bool,
    active_terminal: Option<usize>,
    collapsed: HashSet<usize>,
    collapsed_wt: HashSet<(usize, usize)>,
    tree_expand: TreeExpand,
    tree_touched: bool,
    rail_mode: RailMode,
    hovered_wt: Option<(usize, usize)>,
    open_agent_menu: Option<(usize, usize)>,
    pending_kill: Option<SessionId>,
    pending_kill_terminal: Option<usize>,
    sidebar_width: f32,
    visible_order: Vec<SessionId>,
    /// Whether the appbar's attention dropdown is open (`Grove::attention_queue_open`, `update/mod.rs:619-627`).
    attention_queue_open: bool,
    pending_acks: Vec<SessionId>,
    focused_pane: FocusedPane,
    grid_view: bool,
    chrome_visible: bool,
    tile_order: Vec<SessionId>,
    grid_focused: Option<SessionId>,
    /// Zen was entered from the grid, so exiting zen restores it (`layout.rs:69-79`).
    grid_view_before_zen: bool,
    /// The terminal tab was entered from the grid, likewise (`shortcuts.rs:548-556`).
    grid_view_before_terminal: bool,
    grid_pointer_drag: Option<GridPointerDrag>,
    grid_sizing: GridSizing,
    grid_resize_mode: Option<GridResizeMode>,
    grid_slide: Option<GridSlide>,
    pending_grid_persist: Option<Vec<SessionId>>,
    /// Membership, not a bare bool, is the state — per-session open-ness so session A's panel state never leaks into session B; dropped in [`Self::on_session_removed`] to avoid accumulating dead ids.
    panel_open_sessions: HashSet<SessionId>,
    term_panel_portion: u16,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            active_session: None,
            used: HashMap::new(),
            used_seq: 0,
            proj_idx: 0,
            wt_idx: 0,
            terminal_focused: false,
            active_terminal: None,
            collapsed: HashSet::new(),
            collapsed_wt: HashSet::new(),
            tree_expand: TreeExpand::default(),
            tree_touched: false,
            rail_mode: RailMode::default(),
            hovered_wt: None,
            open_agent_menu: None,
            pending_kill: None,
            pending_kill_terminal: None,
            sidebar_width: RAIL_W,
            visible_order: Vec::new(),
            attention_queue_open: false,
            pending_acks: Vec::new(),
            focused_pane: FocusedPane::default(),
            grid_view: false,
            chrome_visible: true,
            tile_order: Vec::new(),
            grid_focused: None,
            grid_view_before_zen: false,
            grid_view_before_terminal: false,
            grid_pointer_drag: None,
            grid_sizing: GridSizing::default(),
            grid_resize_mode: None,
            grid_slide: None,
            pending_grid_persist: None,
            panel_open_sessions: HashSet::new(),
            term_panel_portion: TERM_PANEL_PORTION,
        }
    }
}

impl WorkspaceState {
    /// Seed from persisted settings; the stored width is clamped against `logical_win_w` exactly as the iced build clamps on every drag (`layout.rs:105-160`).
    pub fn new(store: &Store, logical_win_w: f32) -> Self {
        Self {
            sidebar_width: clamp_sidebar_width(
                store.sidebar_width.unwrap_or(RAIL_W),
                logical_win_w,
            ),
            rail_mode: if store.rail_sessions {
                RailMode::Sessions
            } else {
                RailMode::Tree
            },
            // TRUE index of the first *active* project, not bare `0` — `store.projects[0]` may be archived, and `ProjectTree` seeds from the same `active_projects().next()` project, so disagreeing here desyncs them from frame one.
            proj_idx: store.active_projects().next().map_or(0, |(i, _)| i),
            ..Self::default()
        }
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }
    pub fn used(&self, id: SessionId) -> u64 {
        self.used.get(&id).copied().unwrap_or(0)
    }

    /// The **only** writer of `active_session`, so recency can never drift from selection; clearing focus (`None`) stamps nothing and does not burn a sequence number.
    fn set_active_session(&mut self, id: Option<SessionId>) {
        self.active_session = id;
        if let Some(id) = id {
            self.used_seq += 1;
            self.used.insert(id, self.used_seq);
        }
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
    pub fn rail_mode(&self) -> RailMode {
        self.rail_mode
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
    /// Bare Escape's no-modal carve-out (`update/mod.rs:789-804`): reports whether it dismissed anything — `false` means the key must reach the PTY untouched, since many TUI programs need a real Escape.
    pub fn escape_dismiss(&mut self) -> bool {
        if !crate::modal::escape_should_dismiss(
            self.pending_kill.is_some(),
            self.pending_kill_terminal.is_some(),
            self.open_agent_menu.is_some(),
            self.attention_queue_open,
        ) {
            return false;
        }
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.open_agent_menu = None;
        self.attention_queue_open = false;
        true
    }
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }
    // Exercised only by this module's `#[cfg(test)]` assertions.
    #[allow(dead_code)]
    pub fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }
    pub fn grid_view(&self) -> bool {
        self.grid_view
    }
    pub fn chrome_visible(&self) -> bool {
        self.chrome_visible
    }
    pub fn zen(&self) -> bool {
        !self.chrome_visible
    }
    /// The coarse screen the key contexts are chosen from (`shortcuts.rs:387-392`).
    pub fn screen(&self) -> crate::keymap::Screen {
        crate::keymap::screen_from_flags(self.chrome_visible, self.grid_view)
    }
    pub fn tile_order(&self) -> &[SessionId] {
        &self.tile_order
    }
    pub fn grid_focused(&self) -> Option<SessionId> {
        self.grid_focused
    }
    pub fn grid_view_before_zen(&self) -> bool {
        self.grid_view_before_zen
    }
    // Exercised only by this module's `#[cfg(test)]` assertions.
    #[allow(dead_code)]
    pub fn grid_view_before_terminal(&self) -> bool {
        self.grid_view_before_terminal
    }
    #[cfg(test)]
    pub fn grid_pointer_drag(&self) -> Option<GridPointerDrag> {
        self.grid_pointer_drag
    }
    pub fn grid_pointer_active(&self) -> bool {
        self.grid_pointer_drag.is_some()
    }
    pub fn grid_reorder_drag(&self) -> Option<GridDrag> {
        match self.grid_pointer_drag {
            Some(GridPointerDrag::Reorder(drag)) => Some(drag),
            Some(GridPointerDrag::Resize(_)) | None => None,
        }
    }
    pub fn grid_sizing(&self) -> &GridSizing {
        &self.grid_sizing
    }
    pub fn grid_resize_mode(&self) -> Option<GridResizeMode> {
        self.grid_resize_mode
    }
    pub fn grid_slide(&self) -> Option<GridSlide> {
        self.grid_slide
    }
    pub fn term_panel_open(&self) -> bool {
        self.active_session
            .is_some_and(|id| self.panel_open_sessions.contains(&id))
    }
    pub fn term_panel_portion(&self) -> u16 {
        self.term_panel_portion
    }
    /// Drained by the view that owns the `Store`; carries the order itself, not a bare dirty bit, because [`Self::exit_grid`] persists before tearing `tile_order` down (`layout.rs:264-267`).
    pub fn take_grid_order_to_persist(&mut self) -> Option<Vec<SessionId>> {
        self.pending_grid_persist.take()
    }
    pub fn attention_queue_open(&self) -> bool {
        self.attention_queue_open
    }

    /// `update/mod.rs:619-627`. Plan 08 also closes it when a modal opens (`:795-801`); there are no modals yet.
    pub fn toggle_attention_queue(&mut self) {
        self.attention_queue_open = !self.attention_queue_open;
    }
    pub fn close_attention_queue(&mut self) {
        self.attention_queue_open = false;
    }
    pub fn project_collapsed(&self, proj: usize) -> bool {
        self.collapsed.contains(&proj)
    }
    pub fn worktree_collapsed(&self, proj: usize, wt: usize) -> bool {
        self.collapsed_wt.contains(&(proj, wt))
    }

    /// `WorkspaceState` only records the id here — acknowledgment's other half (truncating the hook state file) is applied by [`crate::entities::activity_store::ActivityStore::acknowledge`], which owns the registry handle these pure `&mut self` transitions do not (spec §4).
    pub fn acknowledge(&mut self, id: SessionId) {
        if !self.pending_acks.contains(&id) {
            self.pending_acks.push(id);
        }
    }

    pub fn take_pending_acks(&mut self) -> Vec<SessionId> {
        std::mem::take(&mut self.pending_acks)
    }

    #[must_use]
    pub fn visible_session_order(&self) -> &[SessionId] {
        &self.visible_order
    }

    pub fn set_visible_order(&mut self, order: Vec<SessionId>) {
        self.visible_order = order;
    }

    /// `src/gui/update/sessions.rs:225-246` folded with `sync_wt_to_session`'s outcome (`update/mod.rs:1143-1156`): one forward pass, no read-back.
    pub fn select_session(&mut self, id: SessionId, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        // Selecting a session closes the attention dropdown (`sessions.rs:229`).
        self.attention_queue_open = false;
        self.set_active_session(Some(id));
        self.terminal_focused = false;
        self.sync_grid_focus();
        self.acknowledge(id);
        if let Some((pi, wi)) = snap.locate(id) {
            self.proj_idx = pi;
            self.wt_idx = wi;
        }
    }

    /// `sessions.rs:35-50` plus `sync_session_to_wt`'s outcome (`update/mod.rs:1164-1183`); `proj`/`wt` are a TRUE project index and a worktree position.
    pub fn select_worktree(&mut self, proj: usize, wt: usize, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.proj_idx = proj;
        self.wt_idx = wt;
        self.tree_touched = true;
        if !self.collapsed_wt.remove(&(proj, wt)) {
            self.collapsed_wt.insert((proj, wt));
        }
        // The worktree may not be cached yet — iced bails before touching `active_session` in that case (`mod.rs:1172`).
        let Some(worktree) = snap.worktree(proj, wt) else {
            return;
        };
        let already_here = self
            .active_session
            .is_some_and(|id| worktree.sessions.contains(&id));
        if already_here {
            return;
        }
        self.set_active_session(worktree.sessions.first().copied());
    }

    /// `sessions.rs:22-33` + `switch_active_project` (`mod.rs:1121-1130`); the worktree-cache hand-off that function also performs belongs to [`crate::entities::project_tree::ProjectTree`], not to selection.
    /// Clear the visible canvas selection when changing workspace or opening an overview.
    pub fn clear_canvas_selection(&mut self) {
        self.set_active_session(None);
        self.active_terminal = None;
        self.terminal_focused = false;
    }

    pub fn select_project(&mut self, proj: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.tree_touched = true;
        if !self.collapsed.remove(&proj) {
            self.collapsed.insert(proj);
        }
        self.proj_idx = proj;
    }

    /// Prepares the add-worktree flow without treating its trigger as a tree-row
    /// toggle. The subsequent success transition owns opening the new worktree.
    pub fn begin_add_worktree(&mut self, proj: usize) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.tree_touched = true;
        self.proj_idx = proj;
    }

    /// Selects a newly-created worktree while preserving every unrelated tree
    /// expansion choice. Unlike [`Self::select_worktree`], this deliberately
    /// opens the target rather than toggling it.
    pub fn focus_added_worktree(&mut self, proj: usize, wt: usize, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.tree_touched = true;
        self.proj_idx = proj;
        self.wt_idx = wt;
        self.collapsed.remove(&proj);
        self.collapsed_wt.remove(&(proj, wt));
        self.set_active_session(
            snap.worktree(proj, wt)
                .and_then(|worktree| worktree.sessions.first().copied()),
        );
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

    /// The pending-confirmation shift across a home-terminal removal (`sessions.rs:109-113`); the registry owns the actual removal, this is the selection half.
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
            // Nothing left to show on the terminal tab — staying focused there would swallow every keystroke (`sessions.rs:115-118`).
            self.terminal_focused = false;
        }
    }

    /// `sessions.rs:365-405`, walking `order` (Task 4's `visible_session_order`) rather than the raw session vector.
    pub fn cycle_session(&mut self, next: bool, order: &[SessionId], snap: &TreeSnapshot) {
        if order.is_empty() {
            return;
        }
        // Coming back from the terminal tab, the first press just reveals the session that was already active — advancing off one the user cannot see is disorienting (`sessions.rs:376-383`).
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

    /// `update/mod.rs:1213-1242`. Archived projects are skipped: the sets are keyed on TRUE indices, and `snap.projects` is already the active list.
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
        self.tree_touched = true;
        self.tree_expand = self.tree_expand.next();
        self.apply_tree_expand(snap);
    }

    pub fn sync_default_tree(&mut self, snap: &TreeSnapshot) {
        if self.tree_touched {
            return;
        }
        self.apply_tree_expand(snap);
        let first_with_sessions = snap.projects.iter().find_map(|p| {
            p.worktrees
                .iter()
                .position(|w| !w.sessions.is_empty())
                .map(|wi| (p.idx, wi))
        });
        if let Some((proj, wt)) = first_with_sessions {
            self.proj_idx = proj;
            self.wt_idx = wt;
        }
    }

    /// `sessions.rs:270-280` without the index dance: a stable [`SessionId`] means the *other* sessions never move.
    pub fn on_session_removed(&mut self, id: SessionId) {
        if self.pending_kill == Some(id) {
            self.pending_kill = None;
        }
        if self.active_session == Some(id) {
            self.set_active_session(None);
        }
        self.panel_open_sessions.remove(&id);
    }

    /// `store.projects.remove(idx)` shifts every TRUE index above `idx` down by one, so the caller must call this after the store mutation (before/alongside `TreeInvalidated`) or `proj_idx`/`collapsed`/`collapsed_wt`/`hovered_wt` go on pointing at stale indices.
    pub fn on_project_removed(&mut self, idx: usize) {
        let shift = |i: usize| if i > idx { i - 1 } else { i };
        self.proj_idx = match self.proj_idx {
            i if i > idx => i - 1,
            i if i == idx => 0,
            i => i,
        };
        self.collapsed = self
            .collapsed
            .iter()
            .filter(|&&i| i != idx)
            .map(|&i| shift(i))
            .collect();
        self.collapsed_wt = self
            .collapsed_wt
            .iter()
            .filter(|&&(p, _)| p != idx)
            .map(|&(p, w)| (shift(p), w))
            .collect();
        self.hovered_wt = match self.hovered_wt {
            Some((p, _)) if p == idx => None,
            Some((p, w)) => Some((shift(p), w)),
            None => None,
        };
    }

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
    /// Second press on the same target confirms; a different target re-arms (`shortcuts.rs:501-527`'s `close_focused_session_decision`).
    pub fn close_focused_session(&mut self, target: SessionId) -> bool {
        if self.pending_kill == Some(target) {
            true
        } else {
            self.arm_kill(target);
            false
        }
    }
    pub fn close_focused_terminal(&mut self, target: usize) -> bool {
        if self.pending_kill_terminal == Some(target) {
            true
        } else {
            self.arm_kill_terminal(target);
            false
        }
    }
    pub fn disarm_kill(&mut self) {
        self.pending_kill = None;
        self.pending_kill_terminal = None;
    }
    pub fn toggle_rail_mode(&mut self) -> RailMode {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        self.hovered_wt = None;
        self.rail_mode = self.rail_mode.toggled();
        self.rail_mode
    }
    pub fn set_sidebar_width(&mut self, width: f32, logical_win_w: f32) {
        self.sidebar_width = clamp_sidebar_width(width, logical_win_w);
    }

    /// `update/mod.rs:1139-1141`.
    fn leave_terminal_tab(&mut self) {
        self.terminal_focused = false;
    }

    /// `mod+t`. **Never touches `chrome_visible`** (recorded ambiguity 3) — in zen it is a pure content swap (`shortcuts.rs:528-557`, `update/mod.rs:472-500`).
    pub fn toggle_terminal_tab(
        &mut self,
        has_home_terminals: bool,
        live: &[LiveTile],
        saved: &[String],
    ) -> TerminalTabOutcome {
        if self.terminal_focused {
            self.leave_terminal_tab();
            if self.grid_view_before_terminal {
                self.grid_view_before_terminal = false;
                self.grid_view = true;
                self.enter_grid(live, saved);
            }
            TerminalTabOutcome {
                spawn_home_terminal: false,
            }
        } else {
            if self.grid_view {
                self.grid_view_before_terminal = true;
                self.grid_view = false;
                self.exit_grid();
            }
            self.terminal_focused = true;
            self.pending_kill = None;
            self.pending_kill_terminal = None;
            TerminalTabOutcome {
                spawn_home_terminal: !has_home_terminals,
            }
        }
    }

    /// Refuses to open with no worktree to anchor to, and — panel open-ness being per session — with no active session either, since there'd be nothing to record membership against (`sessions.rs:62-88`).
    pub fn toggle_term_panel(&mut self, has_worktree: bool) -> bool {
        self.open_agent_menu = None;
        let Some(id) = self.active_session else {
            return false;
        };
        let currently_open = self.panel_open_sessions.contains(&id);
        if !currently_open && !has_worktree {
            return false;
        }
        let now_open = !currently_open;
        if now_open {
            self.panel_open_sessions.insert(id);
        } else {
            self.panel_open_sessions.remove(&id);
        }
        self.focused_pane = if now_open {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
        now_open
    }

    /// Ctrl+Shift+←/→ (`layout.rs:533-542`): clamped, and a no-op when unchanged.
    pub fn adjust_term_panel_portion(&mut self, delta: i16) {
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let next = (self.term_panel_portion as i16 + delta)
            .clamp(TERM_PANEL_PORTION_MIN as i16, TERM_PANEL_PORTION_MAX as i16)
            as u16;
        if next == self.term_panel_portion {
            return;
        }
        self.term_panel_portion = next;
    }

    /// The divider drag's live update (`layout.rs:184-193`); `portion` comes from [`term_portion_for_cursor`], which already clamps.
    pub fn set_term_panel_portion(&mut self, portion: u16) {
        self.term_panel_portion = portion.clamp(TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX);
    }

    /// Focus the panel when it's open (the just re-anchored terminal), otherwise the agent (`pty_input.rs:128-137`).
    pub fn reset_focused_pane(&mut self) {
        self.focused_pane = if self.term_panel_open() {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
    }

    /// A `Panel` click only takes effect while the panel is open; a `Tile` origin is ignored (tile focus is `grid_focused`'s job) (`pty_input.rs:146-158`).
    pub fn focus_pane(&mut self, pane: PtyPane) {
        if !self.term_panel_open() {
            return;
        }
        self.focused_pane = match pane {
            PtyPane::Agent => FocusedPane::Agent,
            PtyPane::Panel => FocusedPane::Panel,
            PtyPane::Tile(_) => return,
        };
    }

    /// Whether input routes to the panel PTY: only while the panel is open *and* the panel pane holds the intent (`pty_input.rs:1180-1186`).
    pub fn panel_focused(&self) -> bool {
        self.term_panel_open() && matches!(self.focused_pane, FocusedPane::Panel)
    }

    /// The fallback at `pty_input.rs:170-178` is load-bearing: a worktree whose panel has **no shell** routes to the agent rather than silently swallowing input.
    pub fn input_target(&self, has_panel_shell: bool) -> PtyPane {
        if self.panel_focused() && has_panel_shell {
            PtyPane::Panel
        } else {
            PtyPane::Agent
        }
    }
}

#[cfg(test)]
mod tests;
