//! WorkspaceState — the single owner of selection, tree presentation and sidebar layout (spec §4); consolidates two iced-era structs mutated from a dozen `update` handlers.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDrag {
    pub source_idx: usize,
    pub hover_idx: usize,
}

/// Session-only proportions for the current column-first topology.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridSizing {
    tile_count: usize,
    topology: (usize, usize),
    column_weights: Vec<f32>,
    row_weights: Vec<Vec<f32>>,
}

impl GridSizing {
    #[must_use]
    fn equal(tile_count: usize) -> Self {
        if tile_count == 0 {
            return Self::default();
        }
        let (cols, rows) = crate::grid::grid_layout(tile_count);
        let row_weights = (0..cols)
            .map(|column| {
                let count = (column..tile_count).step_by(cols).count();
                crate::grid::equal_weights(count)
            })
            .collect();
        Self {
            tile_count,
            topology: (cols, rows),
            column_weights: crate::grid::equal_weights(cols),
            row_weights,
        }
    }

    fn ensure_topology(&mut self, tile_count: usize) -> bool {
        let next = if tile_count == 0 {
            (0, 0)
        } else {
            crate::grid::grid_layout(tile_count)
        };
        let row_shape: Vec<usize> = if tile_count == 0 {
            Vec::new()
        } else {
            (0..next.0)
                .map(|column| (column..tile_count).step_by(next.0).count())
                .collect()
        };
        let current_shape: Vec<usize> = self.row_weights.iter().map(Vec::len).collect();
        if self.tile_count == tile_count && self.topology == next && current_shape == row_shape {
            return false;
        }
        *self = Self::equal(tile_count);
        true
    }

    #[must_use]
    pub fn column_weights(&self) -> &[f32] {
        &self.column_weights
    }

    #[must_use]
    pub fn row_weights(&self, column: usize) -> &[f32] {
        self.row_weights.get(column).map_or(&[], Vec::as_slice)
    }

    fn weights(&self, boundary: GridBoundary) -> Option<&[f32]> {
        match boundary.axis {
            GridAxis::Columns => Some(&self.column_weights),
            GridAxis::Rows => self.row_weights.get(boundary.column?),
        }
        .map(Vec::as_slice)
    }

    fn weights_mut(&mut self, boundary: GridBoundary) -> Option<&mut Vec<f32>> {
        match boundary.axis {
            GridAxis::Columns => Some(&mut self.column_weights),
            GridAxis::Rows => self.row_weights.get_mut(boundary.column?),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridResizeMode {
    pub selected: Option<GridBoundary>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridResizeDrag {
    pub boundary: GridBoundary,
    pub start_coordinate: f32,
    pub span_px: f32,
    pub start_weights: (f32, f32),
    pub minimum_weight: f32,
}

/// Exactly one grid gesture may own the root pointer listeners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridPointerDrag {
    Reorder(GridDrag),
    Resize(GridResizeDrag),
}

#[derive(Clone, Copy, Debug)]
pub struct GridSlide {
    pub tiles: [(usize, i32, i32); 2],
    pub start: std::time::Instant,
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
    terminals_collapsed: bool,
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
            terminals_collapsed: false,
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
    pub fn terminals_collapsed(&self) -> bool {
        self.terminals_collapsed
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

    /// The pending-confirmation shift across a home-terminal removal (`sessions.rs:109-113`); the registry owns the actual removal and respawn, this is the selection half.
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
    pub fn toggle_terminals_collapsed(&mut self) {
        self.terminals_collapsed = !self.terminals_collapsed;
    }
    pub fn set_sidebar_width(&mut self, width: f32, logical_win_w: f32) {
        self.sidebar_width = clamp_sidebar_width(width, logical_win_w);
    }

    /// `update/mod.rs:1139-1141`.
    fn leave_terminal_tab(&mut self) {
        self.terminal_focused = false;
    }

    /// Shared by every path that shows the grid (`mod+g`, terminal toggle, zen-exit restore) so they can't drift; does not set `grid_view` — the caller owns it (`layout.rs:222-252`).
    pub fn enter_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        self.chrome_visible = true;
        let keys: Vec<String> = live.iter().map(|t| t.key.clone()).collect();
        self.tile_order = crate::grid::reconcile_tile_order(&keys, saved)
            .into_iter()
            .filter_map(|i| live.get(i).map(|t| t.id))
            .collect();
        let focus = self
            .active_session
            .filter(|id| self.tile_order.contains(id))
            .or_else(|| self.tile_order.first().copied());
        self.grid_focused = focus;
        if let Some(id) = focus {
            self.set_active_session(Some(id));
            self.acknowledge(id);
        }
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
        self.grid_sizing.ensure_topology(self.tile_order.len());
    }

    /// Counterpart to [`Self::enter_grid`]; likewise leaves `grid_view` to the caller (`layout.rs:257-269`).
    pub fn exit_grid(&mut self) {
        if let Some(id) = self.grid_focused {
            self.set_active_session(Some(id));
            self.leave_terminal_tab();
            // The panel re-anchors to this session's worktree, so a stale `Panel` focus would type into a different worktree's shell.
            self.reset_focused_pane();
        }
        self.pending_grid_persist = Some(self.tile_order.clone());
        self.tile_order.clear();
        self.grid_focused = None;
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
    }

    /// `mod+g`. Port of `on_toggle_grid_view` (`layout.rs:199-216`).
    pub fn toggle_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        self.grid_view = !self.grid_view;
        self.grid_view_before_zen = false;
        if self.grid_view {
            self.leave_terminal_tab();
            self.enter_grid(live, saved);
        } else {
            self.exit_grid();
        }
    }

    /// Port of `on_toggle_zen` (`layout.rs:63-103`), all four branches.
    pub fn toggle_zen(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.chrome_visible {
            self.chrome_visible = true;
            if self.grid_view_before_zen {
                self.grid_view = true;
                self.grid_view_before_zen = false;
                if self.tile_order.is_empty() {
                    self.enter_grid(live, saved);
                }
            }
        } else if self.grid_view {
            if let Some(id) = self
                .grid_focused
                .or(self.active_session)
                .or_else(|| self.tile_order.first().copied())
            {
                self.tile_zen(id);
                return;
            }
            self.grid_view = false;
            self.grid_view_before_zen = true;
            self.chrome_visible = false;
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
        } else {
            self.chrome_visible = false;
        }
    }

    /// A tile's own zen button (`layout.rs:344-356`).
    pub fn tile_zen(&mut self, id: SessionId) {
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.grid_focused = Some(id);
        self.acknowledge(id);
        self.grid_view = false;
        self.grid_view_before_zen = true;
        self.chrome_visible = false;
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
    }

    /// Point `grid_focused` at a (possibly different) tile (`update/mod.rs:1061-1068`; the selection half is the view's).
    pub fn set_grid_focus(&mut self, focus: Option<SessionId>) {
        if self.grid_focused != focus {
            // The resize context is defined relative to the focused tile. Exiting on a focus
            // change is less surprising than leaving its selected split pointed at the old tile.
            self.grid_resize_mode = None;
        }
        self.grid_focused = focus;
    }

    /// `update/mod.rs:1052-1056`.
    pub fn sync_grid_focus(&mut self) {
        if crate::grid::should_sync_grid_focus(self.grid_view, self.grid_view_before_zen) {
            self.set_grid_focus(self.active_session);
        }
    }

    /// Grid-only; no-ops if there's nothing to focus or the move would fall off the edge of the tile layout (`update/mod.rs:1071-1094`).
    pub fn grid_move(&mut self, dx: i32, dy: i32) {
        if self.tile_order.is_empty() || self.grid_resize_mode.is_some() {
            return;
        }
        self.leave_terminal_tab();
        let cur = self
            .grid_focused
            .and_then(|id| self.tile_order.iter().position(|&x| x == id));
        let Some(pos) = cur else {
            let id = self.tile_order[0];
            self.set_active_session(Some(id));
            self.sync_grid_focus();
            self.acknowledge(id);
            return;
        };
        let Some(target) = crate::grid::grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        let id = self.tile_order[target];
        self.set_active_session(Some(id));
        self.sync_grid_focus();
        self.acknowledge(id);
    }

    /// Leaves `grid_focused`/`active_session` untouched — both hold a session, not a tile-order position, so focus stays on the same session after its tile moves (`update/mod.rs:1102-1116`).
    pub fn grid_swap(&mut self, dx: i32, dy: i32) {
        if self.grid_resize_mode.is_some() {
            return;
        }
        let Some(pos) = self
            .grid_focused
            .and_then(|id| self.tile_order.iter().position(|&x| x == id))
        else {
            return;
        };
        let Some(target) = crate::grid::grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        crate::grid::swap_tiles(&mut self.tile_order, pos, target);
        self.grid_slide = Some(GridSlide {
            tiles: crate::grid::slide_offsets(pos, target, self.tile_order.len()),
            start: std::time::Instant::now(),
        });
        self.pending_grid_persist = Some(self.tile_order.clone());
    }

    /// A press on a tile: focus it, make it active, acknowledge, and arm the drag (`layout.rs:308-321`).
    pub fn grid_drag_start(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
        self.grid_resize_mode = None;
        self.grid_pointer_drag = Some(GridPointerDrag::Reorder(GridDrag {
            source_idx: tile_idx,
            hover_idx: tile_idx,
        }));
    }

    pub fn grid_focus_tile(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
    }

    /// A no-op when no drag is armed — the enter event fires regardless (`layout.rs:323-328`).
    pub fn grid_drag_hover(&mut self, tile_idx: usize) {
        if let Some(GridPointerDrag::Reorder(drag)) = self.grid_pointer_drag.as_mut() {
            drag.hover_idx = tile_idx;
        }
    }

    /// Releases the one pointer owner. Returns true only when a reorder changed the persisted order.
    pub fn grid_pointer_end(&mut self) -> bool {
        let Some(pointer) = self.grid_pointer_drag.take() else {
            return false;
        };
        let GridPointerDrag::Reorder(drag) = pointer else {
            return false;
        };
        let (src, dst) = (drag.source_idx, drag.hover_idx);
        if src == dst || src >= self.tile_order.len() || dst >= self.tile_order.len() {
            return false;
        }
        crate::grid::swap_tiles(&mut self.tile_order, src, dst);
        self.grid_slide = Some(GridSlide {
            tiles: crate::grid::slide_offsets(src, dst, self.tile_order.len()),
            start: std::time::Instant::now(),
        });
        self.pending_grid_persist = Some(self.tile_order.clone());
        true
    }

    /// Clears pointer ownership without committing a reorder. Used when a move arrives without
    /// the left button held, which means the platform release happened outside our listener path.
    pub fn grid_pointer_cancel(&mut self) -> bool {
        self.grid_pointer_drag.take().is_some()
    }

    /// Enters the transient keyboard context without changing tile focus or order.
    pub fn enter_grid_resize_mode(&mut self) -> bool {
        if !self.grid_view || self.tile_order.len() < 2 {
            return false;
        }
        self.grid_sizing.ensure_topology(self.tile_order.len());
        let selected = self
            .grid_focused
            .and_then(|id| {
                self.tile_order
                    .iter()
                    .position(|candidate| *candidate == id)
            })
            .and_then(|tile| {
                [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .into_iter()
                    .find_map(|(dx, dy)| {
                        crate::grid::boundary_adjacent_to_tile(tile, self.tile_order.len(), dx, dy)
                    })
            });
        self.grid_pointer_drag = None;
        self.grid_resize_mode = Some(GridResizeMode { selected });
        true
    }

    pub fn exit_grid_resize_mode(&mut self) {
        self.grid_resize_mode = None;
    }

    /// Steps the boundary in the requested direction. The caller supplies an axis-specific floor
    /// derived from the current viewport and terminal metrics.
    pub fn grid_resize_step(
        &mut self,
        dx: i32,
        dy: i32,
        percentage_points: f32,
        minimum_weight: f32,
    ) -> bool {
        if self.grid_resize_mode.is_none() || !percentage_points.is_finite() {
            return false;
        }
        let Some(tile) = self.grid_focused.and_then(|id| {
            self.tile_order
                .iter()
                .position(|candidate| *candidate == id)
        }) else {
            return false;
        };
        let Some(boundary) =
            crate::grid::boundary_adjacent_to_tile(tile, self.tile_order.len(), dx, dy)
        else {
            return false;
        };
        let selection_changed = self
            .grid_resize_mode
            .is_some_and(|mode| mode.selected != Some(boundary));
        if let Some(mode) = self.grid_resize_mode.as_mut() {
            mode.selected = Some(boundary);
        }
        let direction = if dx != 0 { dx } else { dy };
        let signed_step = if direction < 0 {
            -percentage_points
        } else {
            percentage_points
        };
        let delta = signed_step / 100.0;
        let Some(weights) = self.grid_sizing.weights_mut(boundary) else {
            return false;
        };
        crate::grid::transfer_pair(weights, boundary.boundary, delta, minimum_weight)
            || selection_changed
    }

    /// Starts a mouse resize from the current pair. Updates always refer back to this snapshot,
    /// which prevents accumulated error and jump-back after dragging against a clamp.
    pub fn grid_resize_drag_start(
        &mut self,
        boundary: GridBoundary,
        coordinate: f32,
        span_px: f32,
        minimum_weight: f32,
    ) -> bool {
        if !coordinate.is_finite() || !span_px.is_finite() || span_px <= 0.0 {
            return false;
        }
        self.grid_sizing.ensure_topology(self.tile_order.len());
        let Some(weights) = self.grid_sizing.weights(boundary) else {
            return false;
        };
        let Some(&before) = weights.get(boundary.boundary) else {
            return false;
        };
        let Some(&after) = weights.get(boundary.boundary + 1) else {
            return false;
        };
        self.grid_pointer_drag = Some(GridPointerDrag::Resize(GridResizeDrag {
            boundary,
            start_coordinate: coordinate,
            span_px,
            start_weights: (before, after),
            minimum_weight,
        }));
        true
    }

    pub fn grid_resize_drag_update(&mut self, x: f32, y: f32) -> bool {
        let Some(GridPointerDrag::Resize(drag)) = self.grid_pointer_drag else {
            return false;
        };
        let coordinate = match drag.boundary.axis {
            GridAxis::Columns => x,
            GridAxis::Rows => y,
        };
        if !coordinate.is_finite() {
            return false;
        }
        let Some(weights) = self.grid_sizing.weights_mut(drag.boundary) else {
            return false;
        };
        let Some(right) = drag.boundary.boundary.checked_add(1) else {
            return false;
        };
        if right >= weights.len() {
            return false;
        }
        weights[drag.boundary.boundary] = drag.start_weights.0;
        weights[right] = drag.start_weights.1;
        crate::grid::transfer_pair(
            weights,
            drag.boundary.boundary,
            (coordinate - drag.start_coordinate) / drag.span_px,
            drag.minimum_weight,
        )
    }

    pub fn reset_grid_boundary(&mut self, boundary: GridBoundary) -> bool {
        self.grid_pointer_drag = None;
        let Some(weights) = self.grid_sizing.weights_mut(boundary) else {
            return false;
        };
        crate::grid::reset_pair(weights, boundary.boundary)
    }

    #[must_use]
    pub fn grid_resize_label(&self) -> Option<String> {
        let mode = self.grid_resize_mode?;
        Some(match mode.selected {
            Some(GridBoundary {
                axis: GridAxis::Columns,
                boundary,
                ..
            }) => format!("columns {} / {}", boundary + 1, boundary + 2),
            Some(GridBoundary {
                axis: GridAxis::Rows,
                boundary,
                column: Some(column),
            }) => format!(
                "column {} rows {} / {}",
                column + 1,
                boundary + 1,
                boundary + 2
            ),
            Some(GridBoundary {
                axis: GridAxis::Rows,
                column: None,
                ..
            })
            | None => "choose split".to_string(),
        })
    }

    /// Re-derive the grid's view of the session list after sessions were removed behind the GUI's back (`layout.rs:276-306`).
    pub fn reconcile_after_teardown(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.grid_view && !self.grid_view_before_zen {
            self.tile_order.clear();
            self.grid_focused = None;
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
            return;
        }
        let keys: Vec<String> = live.iter().map(|t| t.key.clone()).collect();
        self.tile_order = crate::grid::reconcile_tile_order(&keys, saved)
            .into_iter()
            .filter_map(|i| live.get(i).map(|t| t.id))
            .collect();
        if self
            .grid_focused
            .is_none_or(|id| !self.tile_order.contains(&id))
        {
            self.set_grid_focus(self.tile_order.first().copied());
        }
        if self.active_session.is_none() {
            self.set_active_session(self.grid_focused);
        }
        if self.grid_sizing.ensure_topology(self.tile_order.len()) {
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
        }
        if self.grid_view && self.tile_order.is_empty() {
            self.grid_view = false;
        }
    }

    /// `mod+1..9` inside the grid indexes `tile_order` rather than the sidebar's visible order (`sessions.rs:396-405`); `n` is 0-based.
    pub fn select_tile_by_index(&mut self, n: usize) {
        let Some(&id) = self.tile_order.get(n) else {
            return;
        };
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.sync_grid_focus();
        self.acknowledge(id);
    }

    /// Shared with [`Self::toggle_terminal_tab`]'s enter branch (`update/mod.rs:1008-1013`): leaves the grid so a freshly spawned terminal is visible instead of drawn behind the tiles.
    pub fn exit_grid_for_terminal(&mut self) {
        if self.grid_view {
            self.grid_view_before_terminal = true;
            self.grid_view = false;
            self.exit_grid();
        }
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
mod tests {
    use super::*;

    fn sid(n: u64) -> SessionId {
        SessionId::from_raw(n)
    }

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
                    is_git: true,
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
        assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));
    }

    /// The reported bug: launching from the palette while the grid is up left `grid_focused` on the previous tile, so the keyboard stayed there too.
    #[test]
    fn select_session_carries_the_grid_focus_with_it() {
        let snap = fixture();
        let mut w = WorkspaceState {
            grid_view: true,
            grid_focused: Some(sid(1)),
            ..WorkspaceState::default()
        };

        w.select_session(sid(3), &snap);

        assert_eq!(w.grid_focused(), Some(sid(3)));

        let mut w = WorkspaceState {
            grid_focused: Some(sid(1)),
            ..WorkspaceState::default()
        };
        w.select_session(sid(3), &snap);
        assert_eq!(w.grid_focused(), Some(sid(1)));
    }

    /// `sessions.rs:35-50` + `mod.rs:1164-1183`.
    #[test]
    fn select_worktree_repoints_the_active_session_at_the_first_one() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_worktree(0, 0, &snap);
        assert_eq!(w.active_session(), Some(sid(1)));
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
        assert!(w.worktree_collapsed(0, 0));
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

    #[test]
    fn begin_add_worktree_preserves_all_expansion_state() {
        let mut w = WorkspaceState {
            collapsed: [0, 2].into_iter().collect(),
            collapsed_wt: [(0, 1), (2, 0)].into_iter().collect(),
            open_agent_menu: Some((0, 1)),
            pending_kill: Some(sid(1)),
            pending_kill_terminal: Some(2),
            ..WorkspaceState::default()
        };

        w.begin_add_worktree(2);

        assert_eq!(w.proj_idx(), 2);
        assert!(w.tree_touched);
        assert!(w.project_collapsed(0));
        assert!(w.project_collapsed(2));
        assert!(w.worktree_collapsed(0, 1));
        assert!(w.worktree_collapsed(2, 0));
        assert_eq!(w.open_agent_menu(), None);
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);
    }

    #[test]
    fn focus_added_worktree_opens_only_the_new_target_and_selects_its_session() {
        let mut snap = fixture();
        snap.projects[0].worktrees.push(SnapshotWorktree {
            path: "/a-new".into(),
            name: "a-new".into(),
            branch: "new".into(),
            is_main: false,
            sessions: vec![sid(4)],
        });
        let mut w = WorkspaceState {
            collapsed: [0, 2].into_iter().collect(),
            collapsed_wt: [(0, 0), (0, 1), (0, 2), (2, 0)].into_iter().collect(),
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };

        w.focus_added_worktree(0, 2, &snap);

        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 2));
        assert_eq!(w.active_session(), Some(sid(4)));
        assert!(!w.project_collapsed(0));
        assert!(!w.worktree_collapsed(0, 2));
        assert!(w.project_collapsed(2));
        assert!(w.worktree_collapsed(0, 0));
        assert!(w.worktree_collapsed(0, 1));
        assert!(w.worktree_collapsed(2, 0));

        snap.projects[0].worktrees[2].sessions.clear();
        w.focus_added_worktree(0, 2, &snap);
        assert_eq!(w.active_session(), None);
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

    /// Second mod+w on the same session confirms the kill; a different session re-arms instead (`shortcuts.rs:501-527`).
    #[test]
    fn close_focused_session_confirms_only_on_a_second_press_of_the_same_target() {
        let mut w = WorkspaceState::default();
        assert!(!w.close_focused_session(sid(1)));
        assert_eq!(w.pending_kill(), Some(sid(1)));

        assert!(!w.close_focused_session(sid(2)));
        assert_eq!(w.pending_kill(), Some(sid(2)));

        assert!(w.close_focused_session(sid(2)));
    }

    #[test]
    fn close_focused_terminal_confirms_only_on_a_second_press_of_the_same_target() {
        let mut w = WorkspaceState::default();
        assert!(!w.close_focused_terminal(0));
        assert_eq!(w.pending_kill_terminal(), Some(0));

        assert!(!w.close_focused_terminal(1));
        assert_eq!(w.pending_kill_terminal(), Some(1));

        assert!(w.close_focused_terminal(1));
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

        w.cycle_session(true, &order, &snap);
        assert_eq!(w.active_session(), Some(sid(2)));
        assert!(!w.terminal_focused());
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
        assert!(!w.project_collapsed(0));
        assert!(!w.project_collapsed(2));
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
        assert_eq!(w.tree_expand(), TreeExpand::SessionsOnly);

        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::All);
        assert!(!w.project_collapsed(0));
        assert_eq!(w.open_agent_menu(), None);
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);

        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::Collapsed);
        assert!(w.project_collapsed(0));
        w.toggle_collapse_all(&snap);
        assert_eq!(w.tree_expand(), TreeExpand::All);
    }

    #[test]
    fn rail_mode_toggles_and_clears_transients() {
        let mut w = WorkspaceState {
            open_agent_menu: Some((0, 0)),
            pending_kill: Some(sid(1)),
            hovered_wt: Some((0, 0)),
            ..WorkspaceState::default()
        };
        assert_eq!(w.rail_mode(), RailMode::Tree);
        assert_eq!(w.toggle_rail_mode(), RailMode::Sessions);
        assert_eq!(w.rail_mode(), RailMode::Sessions);
        assert_eq!(w.open_agent_menu(), None);
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.hovered_wt(), None);
        assert_eq!(w.toggle_rail_mode(), RailMode::Tree);
    }

    #[test]
    fn new_skips_an_archived_first_project_when_seeding_proj_idx() {
        use grove_core::storage::Project;
        let project = |name: &str, path: &str, archived: bool| Project {
            name: name.to_string(),
            path: path.to_string(),
            scripts: grove_core::storage::ProjectScripts::default(),
            theme: None,
            archived,
            worktree_dir: None,
        };
        let store = Store {
            projects: vec![project("alpha", "/a", true), project("beta", "/b", false)],
            ..Store::default()
        };
        assert_eq!(WorkspaceState::new(&store, 1280.0).proj_idx(), 1);
    }

    #[test]
    fn rail_mode_round_trips_from_the_store() {
        let store = Store {
            rail_sessions: true,
            ..Store::default()
        };
        assert_eq!(
            WorkspaceState::new(&store, 1600.0).rail_mode(),
            RailMode::Sessions
        );
        assert_eq!(
            WorkspaceState::new(&Store::default(), 1600.0).rail_mode(),
            RailMode::Tree
        );
    }

    #[test]
    fn sync_default_tree_expands_only_sessionful_projects_and_picks_the_first() {
        let mut snap = fixture();
        snap.projects[0].worktrees[0].sessions.clear();
        snap.projects[0].sessions.clear();
        let mut w = WorkspaceState::default();

        w.sync_default_tree(&snap);
        assert!(w.project_collapsed(0));
        assert!(!w.project_collapsed(2));
        assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));

        w.tree_touched = true;
        w.proj_idx = 0;
        w.wt_idx = 0;
        w.sync_default_tree(&snap);
        assert!(w.project_collapsed(0));
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
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

    #[test]
    fn on_project_removed_shifts_every_true_index() {
        let mut w = WorkspaceState {
            proj_idx: 2,
            collapsed: [0, 2, 3].into_iter().collect(),
            collapsed_wt: [(0, 0), (2, 1), (3, 0)].into_iter().collect(),
            hovered_wt: Some((3, 0)),
            ..WorkspaceState::default()
        };
        w.on_project_removed(0);
        assert_eq!(w.proj_idx(), 1);
        assert!(!w.project_collapsed(0));
        assert!(w.project_collapsed(1)); // was 2
        assert!(w.project_collapsed(2)); // was 3
        assert!(w.worktree_collapsed(1, 1)); // was (2, 1)
        assert!(w.worktree_collapsed(2, 0)); // was (3, 0)
        assert_eq!(w.hovered_wt(), Some((2, 0))); // was (3, 0)

        let mut w = WorkspaceState {
            proj_idx: 1,
            collapsed: [1].into_iter().collect(),
            collapsed_wt: [(1, 0)].into_iter().collect(),
            hovered_wt: Some((1, 0)),
            ..WorkspaceState::default()
        };
        w.on_project_removed(1);
        assert_eq!(w.proj_idx(), 0);
        assert!(!w.project_collapsed(1));
        assert!(!w.worktree_collapsed(1, 0));
        assert_eq!(w.hovered_wt(), None);

        let mut w = WorkspaceState {
            proj_idx: 0,
            collapsed: [0].into_iter().collect(),
            ..WorkspaceState::default()
        };
        w.on_project_removed(2);
        assert_eq!(w.proj_idx(), 0);
        assert!(w.project_collapsed(0));
    }

    #[test]
    fn selection_is_one_directional() {
        let snap = fixture();
        let mut w = WorkspaceState::default();
        w.select_worktree(2, 0, &snap);
        w.select_session(sid(1), &snap);
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
        w.select_worktree(0, 1, &snap);
        assert_eq!((w.proj_idx(), w.wt_idx()), (0, 1));
        assert_eq!(w.active_session(), None);
    }

    /// `src/gui/metrics.rs:244-251`.
    #[test]
    fn clamp_sidebar_width_bounds() {
        assert!((clamp_sidebar_width(900.0, 1280.0) - 640.0).abs() < f32::EPSILON);
        assert!((clamp_sidebar_width(900.0, 800.0) - 400.0).abs() < f32::EPSILON);
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

    /// Bare Escape's carve-out (`update/mod.rs:789-804`): each of the four armed states alone makes Escape a dismissal that clears **all** of them.
    #[test]
    fn escape_dismiss_clears_every_armed_state() {
        let mut w = WorkspaceState::default();
        assert!(!w.escape_dismiss(), "nothing armed: Escape reaches the PTY");

        let arm: [fn(&mut WorkspaceState); 4] = [
            |w| w.arm_kill(sid(1)),
            |w| w.arm_kill_terminal(0),
            |w| w.set_open_agent_menu(Some((0, 0))),
            WorkspaceState::toggle_attention_queue,
        ];
        for (i, f) in arm.iter().enumerate() {
            let mut w = WorkspaceState::default();
            f(&mut w);
            assert!(w.escape_dismiss(), "armed state {i} must be dismissed");
            assert!(w.pending_kill().is_none());
            assert!(w.pending_kill_terminal().is_none());
            assert!(w.open_agent_menu().is_none());
            assert!(!w.attention_queue_open());
            assert!(!w.escape_dismiss(), "state {i}: only one Escape is eaten");
        }
    }

    use crate::keymap::Screen;

    fn live(ids: &[u64]) -> Vec<LiveTile> {
        ids.iter()
            .map(|&n| LiveTile {
                id: sid(n),
                key: format!("p::/w{n}"),
            })
            .collect()
    }

    fn keys(ids: &[u64]) -> Vec<String> {
        ids.iter().map(|n| format!("p::/w{n}")).collect()
    }

    /// The `chromeless_grid_is_not_a_nameable_screen` guard: every grid-entry path sets `chrome_visible` first (`layout.rs:222-227`).
    #[test]
    fn entering_the_grid_from_zen_is_never_a_chromeless_grid() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Zen);

        w.toggle_grid(&l, &[]);
        assert!(w.chrome_visible());
        assert!(!w.zen());
        assert_eq!(w.screen(), Screen::Grid);
    }

    /// `layout.rs:63-79` — zen entered from the grid restores it, with the tile order it had.
    #[test]
    fn zen_entered_from_the_grid_returns_to_the_same_grid() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let before = w.tile_order().to_vec();
        assert_eq!(before, vec![sid(1), sid(2), sid(3)]);

        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Zen);
        assert!(w.grid_view_before_zen());
        assert_eq!(w.active_session(), Some(sid(1)));

        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Grid);
        assert!(!w.grid_view_before_zen());
        assert_eq!(w.tile_order(), before.as_slice());
    }

    /// `layout.rs:98-102` — the other entry point round-trips to the single-session workspace, not to a grid.
    #[test]
    fn zen_entered_from_the_workspace_returns_to_the_workspace() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Zen);
        assert!(!w.grid_view_before_zen());
        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Workspace);
        assert!(w.tile_order().is_empty());
    }

    /// An empty grid still drops out of grid view rather than stacking zen on a chrome-less grid (`layout.rs:88-95`).
    #[test]
    fn zen_from_an_empty_grid_still_leaves_grid_view() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&[], &[]);
        assert_eq!(w.screen(), Screen::Grid);
        w.toggle_zen(&[], &[]);
        assert_eq!(w.screen(), Screen::Zen);
        assert!(!w.grid_view());
        assert!(w.grid_view_before_zen());
    }

    /// `layout.rs:204-206` — a manual `mod+g` cancels the restore intent, so a later zen-exit does not resurrect a grid.
    #[test]
    fn a_manual_grid_toggle_cancels_the_zen_restore_intent() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.toggle_zen(&l, &[]);
        assert!(w.grid_view_before_zen());

        w.toggle_grid(&l, &[]);
        assert!(!w.grid_view_before_zen());
        assert_eq!(w.screen(), Screen::Grid);

        w.toggle_grid(&l, &[]);
        assert_eq!(w.screen(), Screen::Workspace);
        w.toggle_zen(&l, &[]);
        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Workspace);
    }

    /// `layout.rs:258-262`.
    #[test]
    fn exit_grid_carries_the_focused_tile_into_the_active_session() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.set_grid_focus(Some(sid(3)));
        w.toggle_grid(&l, &[]);
        assert_eq!(w.active_session(), Some(sid(3)));
        assert!(w.tile_order().is_empty());
        assert_eq!(w.grid_focused(), None);
        assert_eq!(
            w.take_grid_order_to_persist(),
            Some(vec![sid(1), sid(2), sid(3)])
        );
    }

    /// `layout.rs:222-252` — the saved order wins, new sessions append, and the active session keeps its tile focused.
    #[test]
    fn enter_grid_rebuilds_the_order_from_the_saved_keys() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState {
            active_session: Some(sid(2)),
            ..WorkspaceState::default()
        };
        w.enter_grid(&l, &keys(&[3, 1]));
        assert_eq!(w.tile_order(), [sid(3), sid(1), sid(2)]);
        assert_eq!(w.grid_focused(), Some(sid(2)));
        assert_eq!(w.take_pending_acks(), vec![sid(2)]);
    }

    /// `layout.rs:276-306`.
    #[test]
    fn teardown_drops_dead_tiles_and_refocuses_the_first() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.set_grid_focus(Some(sid(2)));

        w.reconcile_after_teardown(&live(&[1, 3]), &keys(&[1, 2, 3]));
        assert_eq!(w.tile_order(), [sid(1), sid(3)]);
        assert_eq!(w.grid_focused(), Some(sid(1)));
        assert!(w.grid_view());
    }

    #[test]
    fn teardown_keeps_a_still_live_focus() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.set_grid_focus(Some(sid(3)));
        w.reconcile_after_teardown(&live(&[1, 3]), &keys(&[1, 2, 3]));
        assert_eq!(w.grid_focused(), Some(sid(3)));
    }

    #[test]
    fn teardown_with_nothing_left_falls_out_of_grid_view() {
        let l = live(&[1]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.reconcile_after_teardown(&[], &keys(&[1]));
        assert!(w.tile_order().is_empty());
        assert_eq!(w.grid_focused(), None);
        assert!(!w.grid_view());
        assert_eq!(w.screen(), Screen::Workspace);
    }

    #[test]
    fn teardown_outside_the_grid_just_clears_the_bookkeeping() {
        let mut w = WorkspaceState {
            tile_order: vec![sid(1)],
            grid_focused: Some(sid(1)),
            ..WorkspaceState::default()
        };
        w.reconcile_after_teardown(&live(&[1]), &[]);
        assert!(w.tile_order().is_empty());
        assert_eq!(w.grid_focused(), None);
    }

    #[test]
    fn reconcile_after_teardown_adds_a_session_spawned_while_the_grid_is_up() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);

        w.reconcile_after_teardown(&live(&[1, 2, 3]), &[]);
        assert_eq!(w.tile_order(), [sid(1), sid(2), sid(3)]);
    }

    /// `update/mod.rs:1071-1094` over `grid::grid_neighbor`. 3 tiles → cols=2: left column 0/2, right column 1.
    #[test]
    fn grid_move_walks_the_neighbor_and_takes_the_session_with_it() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let _ = w.take_pending_acks();
        assert_eq!(w.grid_focused(), Some(sid(1))); // tile 0

        w.grid_move(1, 0); // → tile 1
        assert_eq!(w.grid_focused(), Some(sid(2)));
        assert_eq!(w.active_session(), Some(sid(2)));
        assert_eq!(w.take_pending_acks(), vec![sid(2)]);

        w.grid_move(-1, 0); // back to tile 0
        assert_eq!(w.grid_focused(), Some(sid(1)));
        w.grid_move(0, 1); // down to tile 2
        assert_eq!(w.grid_focused(), Some(sid(3)));
    }

    #[test]
    fn grid_move_off_the_edge_or_on_an_empty_grid_is_a_no_op() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        w.grid_move(-1, 0);
        assert_eq!(w.grid_focused(), Some(sid(1)));
        w.grid_move(1, 0);
        w.grid_move(0, 1);
        assert_eq!(w.grid_focused(), Some(sid(2)));

        let mut empty = WorkspaceState::default();
        empty.grid_move(1, 0);
        assert_eq!(empty.grid_focused(), None);
    }

    /// `update/mod.rs:1102-1116` — the tile moves, the focused **session** does not change.
    #[test]
    fn grid_swap_moves_the_tile_and_keeps_focus_on_its_session() {
        let l = live(&[1, 2, 3, 4]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        assert_eq!(w.grid_focused(), Some(sid(1)));

        w.grid_swap(1, 0);
        assert_eq!(w.tile_order(), [sid(2), sid(1), sid(3), sid(4)]);
        assert_eq!(w.grid_focused(), Some(sid(1)));
        assert_eq!(w.active_session(), Some(sid(1)));
        let Some(slide) = w.grid_slide() else {
            unreachable!("a swap records a slide");
        };
        assert_eq!(slide.tiles, [(1, -1, 0), (0, 1, 0)]);
        assert_eq!(
            w.take_grid_order_to_persist(),
            Some(vec![sid(2), sid(1), sid(3), sid(4)])
        );

        w.grid_swap(1, 0);
        assert_eq!(w.tile_order(), [sid(2), sid(1), sid(3), sid(4)]);
    }

    /// `sessions.rs:396-405`.
    #[test]
    fn mod_n_in_the_grid_indexes_the_tile_order() {
        let l = live(&[1, 2, 3]);
        let mut w = WorkspaceState::default();
        w.enter_grid(&l, &keys(&[3, 2, 1]));
        w.grid_view = true;
        w.select_tile_by_index(1);
        assert_eq!(w.active_session(), Some(sid(2)));
        assert_eq!(w.grid_focused(), Some(sid(2)));
        w.select_tile_by_index(9);
        assert_eq!(w.active_session(), Some(sid(2)));
    }

    /// `shortcuts.rs:528-557` + `update/mod.rs:472-500`.
    #[test]
    fn the_terminal_tab_restores_the_grid_only_when_entered_from_it() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);

        let out = w.toggle_terminal_tab(true, &l, &[]);
        assert!(!out.spawn_home_terminal);
        assert!(w.terminal_focused());
        assert!(!w.grid_view());
        assert!(w.grid_view_before_terminal());

        w.toggle_terminal_tab(true, &l, &[]);
        assert!(!w.terminal_focused());
        assert!(w.grid_view());
        assert!(!w.grid_view_before_terminal());

        w.toggle_grid(&l, &[]);
        assert_eq!(w.screen(), Screen::Workspace);
        w.toggle_terminal_tab(true, &l, &[]);
        w.toggle_terminal_tab(true, &l, &[]);
        assert!(!w.grid_view());
    }

    /// Recorded ambiguity 3 (`update/mod.rs:472-475`): in zen `mod+t` is a pure content swap.
    #[test]
    fn the_terminal_tab_never_touches_the_chrome() {
        let l = live(&[1]);
        let mut w = WorkspaceState::default();
        w.toggle_zen(&l, &[]);
        assert!(!w.chrome_visible());
        w.toggle_terminal_tab(true, &l, &[]);
        assert!(!w.chrome_visible());
        assert!(w.terminal_focused());
        w.toggle_terminal_tab(true, &l, &[]);
        assert!(!w.chrome_visible());
        assert_eq!(w.screen(), Screen::Zen);
    }

    /// `update/mod.rs:493-497` — the transition only reports the spawn.
    #[test]
    fn the_first_terminal_tab_entry_asks_for_a_spawn() {
        let mut w = WorkspaceState {
            pending_kill: Some(sid(1)),
            pending_kill_terminal: Some(0),
            ..WorkspaceState::default()
        };
        let out = w.toggle_terminal_tab(false, &[], &[]);
        assert!(out.spawn_home_terminal);
        assert_eq!(w.pending_kill(), None);
        assert_eq!(w.pending_kill_terminal(), None);
    }

    /// `sessions.rs:62-88`.
    #[test]
    fn the_panel_refuses_to_open_without_a_worktree_to_anchor_to() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        assert!(!w.toggle_term_panel(false));
        assert!(!w.term_panel_open());
        assert_eq!(w.focused_pane(), FocusedPane::Agent);

        assert!(w.toggle_term_panel(true));
        assert!(w.term_panel_open());
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
        assert!(w.panel_focused());

        assert!(!w.toggle_term_panel(false));
        assert!(!w.term_panel_open());
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
        assert!(!w.panel_focused());
    }

    /// The reported bug: the panel used to be one global bool, so leaving it open in session A leaked into session B on switch.
    #[test]
    fn term_panel_open_is_tracked_per_session() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        assert!(w.toggle_term_panel(true));
        assert!(w.term_panel_open());

        w.active_session = Some(sid(2));
        assert!(!w.term_panel_open());

        w.active_session = Some(sid(1));
        assert!(w.term_panel_open());

        assert!(!w.toggle_term_panel(true));
        assert!(!w.term_panel_open());
        w.active_session = Some(sid(2));
        assert!(!w.term_panel_open());
    }

    /// `layout.rs:533-542`.
    #[test]
    fn the_panel_portion_steps_are_clamped_and_no_op_at_the_bounds() {
        let mut w = WorkspaceState::default();
        assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION);
        #[allow(clippy::cast_possible_wrap)]
        let step = TERM_PANEL_PORTION_STEP as i16;
        w.adjust_term_panel_portion(step);
        assert_eq!(w.term_panel_portion(), 45);
        for _ in 0..20 {
            w.adjust_term_panel_portion(step);
        }
        assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MAX);
        for _ in 0..40 {
            w.adjust_term_panel_portion(-step);
        }
        assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MIN);

        w.set_term_panel_portion(999);
        assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MAX);
        w.set_term_panel_portion(0);
        assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MIN);
    }

    /// `src/gui/metrics.rs:482-501`, ported verbatim.
    #[test]
    fn term_portion_for_cursor_maps_and_clamps() {
        let win = 1280.0;
        let sidebar = RAIL_W;
        let work_left = sidebar + crate::views::tokens::DIVIDER_DRAG_HIT_W;
        let work_w = win - work_left;
        let mid = work_left + work_w * 0.5;
        assert_eq!(term_portion_for_cursor(mid, win, sidebar), 50);
        assert_eq!(
            term_portion_for_cursor(win - 1.0, win, sidebar),
            TERM_PANEL_PORTION_MIN
        );
        assert_eq!(
            term_portion_for_cursor(work_left + 1.0, win, sidebar),
            TERM_PANEL_PORTION_MAX
        );
    }

    /// `pty_input.rs:128-158`.
    #[test]
    fn focus_pane_only_counts_while_the_panel_is_open_and_ignores_tiles() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        w.focus_pane(PtyPane::Panel);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);

        w.toggle_term_panel(true);
        w.focus_pane(PtyPane::Agent);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
        w.focus_pane(PtyPane::Panel);
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
        w.focus_pane(PtyPane::Tile(sid(1)));
        assert_eq!(w.focused_pane(), FocusedPane::Panel);

        w.focus_pane(PtyPane::Agent);
        w.reset_focused_pane();
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
    }

    /// `pty_input.rs:170-178` — the fallback that stops a shell-less panel from eating every keystroke.
    #[test]
    fn a_worktree_with_no_panel_shell_routes_input_to_the_agent() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        assert_eq!(w.input_target(false), PtyPane::Agent);

        w.toggle_term_panel(true);
        assert!(w.panel_focused());
        assert_eq!(w.input_target(false), PtyPane::Agent);
        assert_eq!(w.input_target(true), PtyPane::Panel);

        w.focus_pane(PtyPane::Agent);
        assert_eq!(w.input_target(true), PtyPane::Agent);

        w.toggle_term_panel(false);
        assert_eq!(w.input_target(true), PtyPane::Agent);
    }

    /// `layout.rs:308-342` — press focuses and arms, enter tracks, release commits.
    #[test]
    fn a_tile_drag_focuses_on_press_and_swaps_on_release() {
        let l = live(&[1, 2, 3, 4]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let _ = w.take_pending_acks();
        let _ = w.take_grid_order_to_persist();

        w.grid_drag_start(2);
        assert_eq!(w.grid_focused(), Some(sid(3)));
        assert_eq!(w.active_session(), Some(sid(3)));
        assert_eq!(w.take_pending_acks(), vec![sid(3)]);
        assert_eq!(
            w.grid_reorder_drag(),
            Some(GridDrag {
                source_idx: 2,
                hover_idx: 2
            })
        );

        w.grid_drag_hover(0);
        assert_eq!(w.grid_reorder_drag().map(|d| d.hover_idx), Some(0));

        assert!(w.grid_pointer_end());
        assert_eq!(w.tile_order(), [sid(3), sid(2), sid(1), sid(4)]);
        assert_eq!(w.grid_focused(), Some(sid(3)));
        assert!(w.grid_pointer_drag().is_none());
        assert!(w.grid_slide().is_some());
        assert!(w.take_grid_order_to_persist().is_some());
    }

    #[test]
    fn a_hover_with_no_armed_drag_is_a_no_op_and_a_null_drag_does_not_swap() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let _ = w.take_grid_order_to_persist();

        w.grid_drag_hover(1);
        assert!(w.grid_reorder_drag().is_none());
        assert!(!w.grid_pointer_end());
        assert_eq!(w.tile_order(), [sid(1), sid(2)]);

        w.grid_drag_start(1);
        assert!(!w.grid_pointer_end());
        assert_eq!(w.tile_order(), [sid(1), sid(2)]);
        assert!(w.grid_slide().is_none());
        assert!(w.take_grid_order_to_persist().is_none());
    }

    #[test]
    fn resize_drag_owns_pointer_without_reordering_or_moving_focus() {
        let l = live(&[1, 2, 3, 4]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let focused = w.grid_focused();
        let order = w.tile_order().to_vec();
        let boundary = GridBoundary {
            axis: GridAxis::Columns,
            boundary: 0,
            column: None,
        };

        assert!(w.grid_resize_drag_start(boundary, 200.0, 800.0, 0.1));
        assert!(matches!(
            w.grid_pointer_drag(),
            Some(GridPointerDrag::Resize(_))
        ));
        assert!(w.grid_reorder_drag().is_none());
        assert!(w.grid_resize_drag_update(280.0, 0.0));
        assert!((w.grid_sizing().column_weights()[0] - 0.6).abs() < 1e-6);
        assert!(!w.grid_pointer_end());
        assert_eq!(w.tile_order(), order);
        assert_eq!(w.grid_focused(), focused);
        assert!(w.take_grid_order_to_persist().is_none());
    }

    #[test]
    fn resize_drag_uses_its_original_baseline_after_a_clamp() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let boundary = GridBoundary {
            axis: GridAxis::Columns,
            boundary: 0,
            column: None,
        };
        assert!(w.grid_resize_drag_start(boundary, 100.0, 100.0, 0.2));
        assert!(w.grid_resize_drag_update(1000.0, 0.0));
        assert!((w.grid_sizing().column_weights()[0] - 0.8).abs() < 1e-6);
        assert!(w.grid_resize_drag_update(110.0, 0.0));
        assert!((w.grid_sizing().column_weights()[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn a_missing_left_button_cancels_pointer_ownership_without_committing_reorder() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&live(&[1, 2]), &[]);
        let _ = w.take_grid_order_to_persist();
        let original = w.tile_order().to_vec();
        w.grid_drag_start(0);
        w.grid_drag_hover(1);

        assert!(w.grid_pointer_cancel());
        assert_eq!(w.tile_order(), original);
        assert!(w.grid_pointer_drag().is_none());
        assert!(!w.grid_pointer_cancel());
        assert!(w.take_grid_order_to_persist().is_none());
    }

    #[test]
    fn keyboard_resize_selects_the_focused_adjacent_boundary_and_never_reorders() {
        let l = live(&[1, 2, 3, 4]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);
        let order = w.tile_order().to_vec();
        assert!(w.enter_grid_resize_mode());
        assert_eq!(w.grid_resize_label().as_deref(), Some("columns 1 / 2"));
        assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
        assert!((w.grid_sizing().column_weights()[0] - 0.55).abs() < 1e-6);
        assert_eq!(w.tile_order(), order);
        assert_eq!(w.grid_focused(), Some(sid(1)));

        assert!(w.grid_resize_step(0, 1, 1.0, 0.1));
        assert_eq!(
            w.grid_resize_label().as_deref(),
            Some("column 1 rows 1 / 2")
        );
        assert!((w.grid_sizing().row_weights(0)[0] - 0.51).abs() < 1e-6);
        w.exit_grid_resize_mode();
        assert!(w.grid_resize_mode().is_none());
    }

    #[test]
    fn keyboard_resize_reports_a_new_selection_even_when_transfer_is_clamped() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
        assert!(w.enter_grid_resize_mode());

        // An equal-share floor clamps both pairs, but changing axis still changes the status hint.
        assert!(!w.grid_resize_step(1, 0, 5.0, 0.5));
        assert_eq!(w.grid_resize_label().as_deref(), Some("columns 1 / 2"));
        assert!(w.grid_resize_step(0, 1, 5.0, 0.5));
        assert_eq!(
            w.grid_resize_label().as_deref(),
            Some("column 1 rows 1 / 2")
        );
        // Repeating the same clamped request changes neither selection nor weights.
        assert!(!w.grid_resize_step(0, 1, 5.0, 0.5));
    }

    #[test]
    fn changing_grid_focus_exits_keyboard_resize_mode() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&live(&[1, 2]), &[]);
        assert!(w.enter_grid_resize_mode());

        w.set_grid_focus(Some(sid(2)));
        assert_eq!(w.grid_focused(), Some(sid(2)));
        assert!(w.grid_resize_mode().is_none());
    }

    #[test]
    fn topology_change_resets_all_session_only_weights_and_exits_resize() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
        assert!(w.enter_grid_resize_mode());
        assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
        assert_ne!(w.grid_sizing().column_weights(), &[0.5, 0.5]);

        w.reconcile_after_teardown(&live(&[1, 2, 3]), &[]);
        assert_eq!(w.grid_sizing().column_weights(), &[0.5, 0.5]);
        assert_eq!(w.grid_sizing().row_weights(0), &[0.5, 0.5]);
        assert_eq!(w.grid_sizing().row_weights(1), &[1.0]);
        assert!(w.grid_resize_mode().is_none());
        assert!(w.grid_pointer_drag().is_none());
    }

    #[test]
    fn resetting_one_split_leaves_every_other_split_unchanged() {
        let mut w = WorkspaceState::default();
        w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
        assert!(w.enter_grid_resize_mode());
        assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
        assert!(w.grid_resize_step(0, 1, 5.0, 0.1));
        let rows_before = w.grid_sizing().row_weights(0).to_vec();
        assert!(w.reset_grid_boundary(GridBoundary {
            axis: GridAxis::Columns,
            boundary: 0,
            column: None,
        }));
        assert_eq!(w.grid_sizing().column_weights(), &[0.5, 0.5]);
        assert_eq!(w.grid_sizing().row_weights(0), rows_before);
    }

    #[test]
    fn leaving_the_grid_for_zen_clears_the_transient_resize_context() {
        let tiles = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&tiles, &[]);
        assert!(w.enter_grid_resize_mode());
        w.toggle_zen(&tiles, &[]);
        assert!(!w.grid_view());
        assert!(w.grid_resize_mode().is_none());
        assert!(w.grid_pointer_drag().is_none());
    }

    /// `layout.rs:257-269` — leaving the grid re-anchors the panel, so a stale `Panel` intent cannot type into another worktree's shell.
    #[test]
    fn exit_grid_re_anchors_the_focused_pane() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        w.toggle_term_panel(true);
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
        w.toggle_grid(&l, &[]);
        w.toggle_grid(&l, &[]);
        assert_eq!(w.focused_pane(), FocusedPane::Panel);

        w.toggle_term_panel(false);
        w.toggle_grid(&l, &[]);
        w.toggle_grid(&l, &[]);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
    }
}
