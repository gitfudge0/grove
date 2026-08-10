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
//! | `focused_pane` | `Grove::focused_pane` (`src/gui/state.rs:110`) |
//! | `grid_view` / `grid_focused` / `tile_order` | `Grove::{grid_view, grid_focused, tile_order}` (`src/gui/state.rs:112-117`) |
//! | `chrome_visible` | `App::chrome_visible` (`src/app/mod.rs`) |
//! | `grid_view_before_zen` / `grid_view_before_terminal` | `Grove::{grid_view_before_zen, grid_view_before_terminal}` |
//! | `term_panel_open` / `term_panel_portion` | `Grove::{term_panel_open, term_panel_portion}` (`src/gui/state.rs:282-289`) — `term_panel_open` is per session here (`panel_open_sessions`, a set of the sessions left with their panel open), not the single global bool iced had; `term_panel_portion` (the width) stays global |
//!
//! **Plan 07 Task 2 Step 1 deviation.** The Plan 06 stub carried both a
//! `chrome_visible`-shaped idea and a `zen` bool. Only one can be the stored
//! truth, and it is **`chrome_visible`**: [`crate::keymap::screen_from_flags`]
//! (already ported, with the "zen wins over grid" invariant) is written in
//! those terms, as is every oracle line in `src/gui/update/layout.rs`. The
//! `zen` field is deleted; [`WorkspaceState::zen`] survives as its negation.
//!
//! **Plan 07 Task 2 deviation — selection.** iced clears `Grove::pty_selection`
//! inside several of these transitions, because there it is one app-global
//! field. In grove-gpui each [`crate::views::terminal_view::TerminalView`] owns
//! its own selection (`terminal_view.rs:50`), so there is nothing global to
//! clear and the transitions do not try; a focus-changing press clears the
//! selection at the view that owns it (`terminal_view.rs:273`).
//!
//! **Plan 07 Task 2 deviation — persistence.** `persist_grid_order`
//! (`layout.rs:481-489`) needs the `Store`, which these pure transitions do not
//! hold. They stage the order in [`WorkspaceState::take_grid_order_to_persist`]
//! instead, exactly
//! as [`WorkspaceState::acknowledge`] defers to the `ActivityStore`.
//!
//! `sync_wt_to_session` / `sync_session_to_wt` (`src/gui/update/mod.rs:1143`,
//! `:1164`) are **deleted, not ported** (carried amendment 5). Their observable
//! outcomes survive inside [`WorkspaceState::select_session`] and
//! [`WorkspaceState::select_worktree`] as a single forward pass each; nothing
//! here writes `active_session` and then re-reads it to fix up `wt_idx`.

use std::collections::{HashMap, HashSet};

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
    /// Default — a fresh sidebar only expands what has something to show
    /// (see [`WorkspaceState::sync_default_tree`]).
    #[default]
    SessionsOnly,
    /// Everything expanded.
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

/// Default terminal-panel share of the workspace width, in percent
/// (`src/gui/metrics.rs:38`); the agent view gets `100 - TERM_PANEL_PORTION`.
pub const TERM_PANEL_PORTION: u16 = 40;
/// Bounds and step (percent of the workspace) for resizing the panel
/// (`src/gui/metrics.rs:42-44`).
pub const TERM_PANEL_PORTION_MIN: u16 = 20;
pub const TERM_PANEL_PORTION_MAX: u16 = 75;
pub const TERM_PANEL_PORTION_STEP: u16 = 5;

/// Terminal-panel width share (percent) for a divider dragged to logical
/// cursor x. The panel is docked on the right, so a cursor further left grows
/// it. Clamped to `[TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX]`.
/// Port of `src/gui/metrics.rs:257-263`.
#[must_use]
pub fn term_portion_for_cursor(cursor_x: f32, logical_win_w: f32, sidebar_w: f32) -> u16 {
    let work_left = sidebar_w + crate::views::sidebar::SIDEBAR_DIVIDER_W;
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

/// Which PTY input is routed to while the worktree terminal panel is open
/// (`src/gui/state.rs:14-22`). Survives in gpui as the **persisted intent**
/// that decides which `FocusHandle` to focus on open / re-anchor (carried
/// amendment 8); the keystrokes themselves follow gpui focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusedPane {
    #[default]
    Agent,
    Panel,
}

/// The origin of a PTY click, as `focus_pane` sees it (`src/gui/state.rs`'s
/// `PtyPane`). `Tile` is carried so the call sites can share one enum; tile
/// focus is `grid_focused`'s job and `focus_pane` ignores it
/// (`pty_input.rs:146-158`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyPane {
    Agent,
    Panel,
    // Vocabulary entry: the doc above is the contract — the enum exists so every
    // click origin can be named with one type, and `focus_pane` deliberately
    // ignores a tile origin. Constructed by `#[cfg(test)]` code only.
    #[allow(dead_code)]
    Tile(SessionId),
}

/// An in-progress tile drag (`src/gui/state.rs`'s `GridDrag`): both fields are
/// positions into `tile_order`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDrag {
    pub source_idx: usize,
    pub hover_idx: usize,
}

/// A recorded tile slide: the two tiles that swapped, each with the
/// `(d_col, d_row)` offset pointing back where it came from, plus the start
/// instant `slide_progress` is measured from (`src/gui/state.rs`'s `GridSlide`).
#[derive(Clone, Copy, Debug)]
pub struct GridSlide {
    pub tiles: [(usize, i32, i32); 2],
    pub start: std::time::Instant,
}

/// One live session as the grid transitions see it: its id and its stable
/// cross-restart key ([`crate::grid::session_grid_key`]), in registry order.
#[derive(Clone, Debug)]
pub struct LiveTile {
    pub id: SessionId,
    pub key: String,
}

/// What [`WorkspaceState::toggle_terminal_tab`] needs the caller to do after
/// the pure transition has run — the spawn itself is the view's job
/// (`update/mod.rs:493-497`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTabOutcome {
    /// The tab is now showing and there was no home terminal to show, so one
    /// must be spawned.
    pub spawn_home_terminal: bool,
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
    /// Every session belonging to this project, keyed by project **name** —
    /// independent of the worktree cache, which is empty until a project is
    /// visited (`src/gui/view/sidebar.rs` `by_proj[s.project]`).
    pub sessions: Vec<SessionId>,
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
    /// Monotonic "last focused" stamp per session, missing until the session is
    /// first focused. In memory only — recency is a property of this run, not
    /// something to persist. Lives here rather than on the registry because
    /// `active_session` is owned here: every focus change already routes
    /// through [`Self::set_active_session`], so nothing can move focus without
    /// stamping.
    used: HashMap<SessionId, u64>,
    /// Monotonic counter behind [`Self::used`].
    used_seq: u64,
    proj_idx: usize,
    wt_idx: usize,
    terminal_focused: bool,
    active_terminal: Option<usize>,
    // tree presentation
    collapsed: HashSet<usize>,
    collapsed_wt: HashSet<(usize, usize)>,
    tree_expand: TreeExpand,
    /// Set once the user manually changes tree expansion — the cycle button,
    /// or a project/worktree row's own collapse toggle. Gates
    /// [`Self::sync_default_tree`], which otherwise re-derives the default
    /// every frame until then.
    tree_touched: bool,
    terminals_collapsed: bool,
    // transient row affordances
    hovered_wt: Option<(usize, usize)>,
    open_agent_menu: Option<(usize, usize)>,
    pending_kill: Option<SessionId>,
    pending_kill_terminal: Option<usize>,
    // layout
    sidebar_width: f32,
    /// The sidebar's flattened session order, refreshed each time the rail
    /// rebuilds its rows. Cached here so the attention queue can be resolved
    /// in **tree order** without the `ActivityStore` reaching into a view.
    visible_order: Vec<SessionId>,
    /// Whether the appbar's attention dropdown is open
    /// (`Grove::attention_queue_open`, `update/mod.rs:619-627`).
    attention_queue_open: bool,
    /// Sessions the user has focused since the last drain. See
    /// [`Self::acknowledge`].
    pending_acks: Vec<SessionId>,
    // ── the four screens (Plan 07) ──────────────────────────────────────
    focused_pane: FocusedPane,
    grid_view: bool,
    /// The stored truth; `zen()` is its negation (see the module doc).
    chrome_visible: bool,
    /// The grid's tiles, in display order. `SessionId`s, not indices — see
    /// [`crate::grid`]'s deviation note.
    tile_order: Vec<SessionId>,
    grid_focused: Option<SessionId>,
    /// Zen was entered from the grid, so exiting zen restores it
    /// (`layout.rs:69-79`).
    grid_view_before_zen: bool,
    /// The terminal tab was entered from the grid, likewise
    /// (`shortcuts.rs:548-556`).
    grid_view_before_terminal: bool,
    grid_drag: Option<GridDrag>,
    grid_slide: Option<GridSlide>,
    /// The tile order awaiting a `Store::grid_order` write; drained by the
    /// view that holds the `Store`.
    pending_grid_persist: Option<Vec<SessionId>>,
    /// The sessions whose panel was left open. Membership, not a bare bool,
    /// is the state: "open" is per session (user's report — leaving the
    /// panel open in session A must not leak into session B when B is
    /// entered), so there is nothing to save and restore on a switch. A
    /// session's open-ness is just whether it is in this set; a never-touched
    /// session (or one with no active session at all) reads as closed
    /// because it is absent. Entries are dropped in [`Self::on_session_removed`]
    /// so the set cannot accumulate dead ids.
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
            grid_drag: None,
            grid_slide: None,
            pending_grid_persist: None,
            panel_open_sessions: HashSet::new(),
            term_panel_portion: TERM_PANEL_PORTION,
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
    /// `id`'s last-focused stamp, `0` if it has never been focused — the order
    /// key the palette's switch drill-in sorts on
    /// ([`crate::launcher::order_switch_sessions`]).
    pub fn used(&self, id: SessionId) -> u64 {
        self.used.get(&id).copied().unwrap_or(0)
    }

    /// The **only** writer of `active_session`: every focus change stamps the
    /// session it lands on, so recency can never drift from selection. Clearing
    /// focus (`None`) stamps nothing and does not burn a sequence number.
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
    /// Bare Escape's carve-out with **no** modal open (`update/mod.rs:789-804`):
    /// dismisses the armed kill-confirm, the open agent menu and the attention
    /// dropdown, and reports whether it dismissed anything. `false` means the
    /// key must reach the PTY untouched — many TUI programs need a real Escape,
    /// so it is never swallowed unconditionally.
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
    // Exercised only by this module's `#[cfg(test)]` assertions; rustc's
    // non-test pass cannot see that use.
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
    /// Zen is exactly "the chrome is hidden" — see the module doc.
    pub fn zen(&self) -> bool {
        !self.chrome_visible
    }
    /// The coarse screen the key contexts are chosen from
    /// (`shortcuts.rs:387-392`).
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
    // Exercised only by this module's `#[cfg(test)]` assertions; rustc's
    // non-test pass cannot see that use.
    #[allow(dead_code)]
    pub fn grid_view_before_terminal(&self) -> bool {
        self.grid_view_before_terminal
    }
    pub fn grid_drag(&self) -> Option<GridDrag> {
        self.grid_drag
    }
    pub fn grid_slide(&self) -> Option<GridSlide> {
        self.grid_slide
    }
    /// Derived, not stored: open-ness lives in [`Self::panel_open_sessions`],
    /// keyed by the *active* session, so switching sessions can never leave a
    /// stale bool behind to sync.
    pub fn term_panel_open(&self) -> bool {
        self.active_session
            .is_some_and(|id| self.panel_open_sessions.contains(&id))
    }
    pub fn term_panel_portion(&self) -> u16 {
        self.term_panel_portion
    }
    /// The tile order to write to `Store::grid_order`, if one is pending.
    /// Drained by the view that owns the `Store` (see the module doc); it
    /// carries the order itself rather than a bare dirty bit because
    /// [`Self::exit_grid`] persists **before** tearing `tile_order` down
    /// (`layout.rs:264-267`).
    pub fn take_grid_order_to_persist(&mut self) -> Option<Vec<SessionId>> {
        self.pending_grid_persist.take()
    }
    pub fn attention_queue_open(&self) -> bool {
        self.attention_queue_open
    }

    /// `update/mod.rs:619-627`. Plan 08 also closes it when a modal opens
    /// (`:795-801`); there are no modals yet.
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

    // ── transitions ─────────────────────────────────────────────────────

    /// Every focus transition acknowledges the session it lands on
    /// (spec §4: "Attention is never event-driven").
    ///
    /// Acknowledgment has two halves — `Tracker::acknowledge` and truncating
    /// the hook state file (`update/mod.rs:697-707`; the file must be
    /// truncated too, or a stale `needs-you` resurfaces the moment the user
    /// looks away). **Deviation from the plan's sketch:** both halves are
    /// applied by
    /// [`crate::entities::activity_store::ActivityStore::acknowledge`], which
    /// owns the trackers and holds the registry handle that knows the file
    /// path. `WorkspaceState` owns neither and its transitions are pure
    /// (`&mut self`, no `Context`), so it records the id here instead. The
    /// store observes this entity and drains within the same frame — every
    /// existing call site already notifies in the same update — so the
    /// observable behavior is unchanged.
    pub fn acknowledge(&mut self, id: SessionId) {
        if !self.pending_acks.contains(&id) {
            self.pending_acks.push(id);
        }
    }

    /// Drained by the `ActivityStore`'s observer.
    pub fn take_pending_acks(&mut self) -> Vec<SessionId> {
        std::mem::take(&mut self.pending_acks)
    }

    /// The sidebar's flattened session order — the order the attention queue,
    /// `mod+N` and next/prev cycling all share.
    #[must_use]
    pub fn visible_session_order(&self) -> &[SessionId] {
        &self.visible_order
    }

    /// Published by the rail every time it rebuilds its rows. Deliberately
    /// does **not** notify: it is derived data that changed *because* a
    /// repaint was already happening.
    pub fn set_visible_order(&mut self, order: Vec<SessionId>) {
        self.visible_order = order;
    }

    /// `src/gui/update/sessions.rs:225-246` folded together with
    /// `sync_wt_to_session`'s outcome (`update/mod.rs:1143-1156`): one forward
    /// pass, no read-back.
    pub fn select_session(&mut self, id: SessionId, snap: &TreeSnapshot) {
        self.open_agent_menu = None;
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        // Selecting a session closes the attention dropdown (`sessions.rs:229`).
        self.attention_queue_open = false;
        self.set_active_session(Some(id));
        self.terminal_focused = false;
        // In the grid the highlighted tile is what holds the keyboard, so a
        // selection that leaves `grid_focused` behind (palette launch, palette
        // switch-to-session) lands the keys on the *previous* tile — the same
        // sync `select_tile_by_index` does.
        self.sync_grid_focus();
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
        self.tree_touched = true;
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
        self.set_active_session(worktree.sessions.first().copied());
    }

    /// `sessions.rs:22-33` + `switch_active_project` (`mod.rs:1121-1130`).
    /// The worktree-cache hand-off that function also performs belongs to
    /// [`crate::entities::project_tree::ProjectTree`], not to selection.
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
        self.tree_touched = true;
        self.tree_expand = self.tree_expand.next();
        self.apply_tree_expand(snap);
    }

    /// Re-derive the default tree presentation every frame until the user
    /// touches it manually: expand only what has sessions, and — the first
    /// time there is a session anywhere — point the highlight at the first
    /// (project, worktree) that has one. Sessions restore asynchronously
    /// (see the module's `TreeSnapshot` doc), so the first snapshot may
    /// legitimately be empty; a one-shot apply at construction would miss
    /// the sessions that show up a frame later. Never touches
    /// `active_session`.
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

    /// `sessions.rs:270-280` without the index dance: a stable [`SessionId`]
    /// means the *other* sessions never move.
    pub fn on_session_removed(&mut self, id: SessionId) {
        if self.pending_kill == Some(id) {
            self.pending_kill = None;
        }
        if self.active_session == Some(id) {
            self.set_active_session(None);
        }
        // Otherwise `id`'s open/closed membership would sit in the set
        // forever, keyed to a session that can never become active again.
        self.panel_open_sessions.remove(&id);
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
    /// Second press on the same target confirms; a different target re-arms.
    /// Returns `true` when the caller should kill `target`
    /// (`shortcuts.rs:501-527`'s `close_focused_session_decision`).
    pub fn close_focused_session(&mut self, target: SessionId) -> bool {
        if self.pending_kill == Some(target) {
            true
        } else {
            self.arm_kill(target);
            false
        }
    }
    /// Terminal counterpart of [`Self::close_focused_session`].
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
    pub fn toggle_terminals_collapsed(&mut self) {
        self.terminals_collapsed = !self.terminals_collapsed;
    }
    pub fn set_sidebar_width(&mut self, width: f32, logical_win_w: f32) {
        self.sidebar_width = clamp_sidebar_width(width, logical_win_w);
    }

    // ── the four screens (Plan 07 Task 2) ───────────────────────────────

    /// `update/mod.rs:1139-1141`.
    fn leave_terminal_tab(&mut self) {
        self.terminal_focused = false;
    }

    /// Build `tile_order` from the persisted order and pick the tile that takes
    /// keyboard focus. Shared by every path that shows the grid, so they can't
    /// drift (`mod+g`, the terminal toggle, the zen-exit restore). Does not set
    /// `grid_view` — the caller owns it. Port of `layout.rs:222-252`.
    pub fn enter_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        // Zen hides the chrome, but `mod+g` (and the terminal toggle's grid
        // restore) stay reachable there. A grid with no appbar or statusbar
        // isn't a screen `screen_from_flags` can even name — it reports Zen —
        // so showing the grid always ends zen rather than stacking the two.
        self.chrome_visible = true;
        let keys: Vec<String> = live.iter().map(|t| t.key.clone()).collect();
        self.tile_order = crate::grid::reconcile_tile_order(&keys, saved)
            .into_iter()
            .filter_map(|i| live.get(i).map(|t| t.id))
            .collect();
        // Open with a focused tile so the directional shortcuts work on the
        // first keypress. Keep the active session's tile if it has one —
        // yanking focus elsewhere on entry would be a surprise — otherwise
        // focus the first tile.
        let focus = self
            .active_session
            .filter(|id| self.tile_order.contains(id))
            .or_else(|| self.tile_order.first().copied());
        self.grid_focused = focus;
        if let Some(id) = focus {
            self.set_active_session(Some(id));
            self.acknowledge(id);
        }
        self.grid_drag = None;
    }

    /// Carry the focused tile into the single-session workspace and tear the
    /// grid bookkeeping down. Counterpart to [`Self::enter_grid`]; likewise
    /// leaves `grid_view` to the caller. Port of `layout.rs:257-269`.
    pub fn exit_grid(&mut self) {
        if let Some(id) = self.grid_focused {
            self.set_active_session(Some(id));
            self.leave_terminal_tab();
            // The panel re-anchors to this session's worktree, so a stale
            // `Panel` focus would type into a different worktree's shell.
            self.reset_focused_pane();
        }
        self.pending_grid_persist = Some(self.tile_order.clone());
        self.tile_order.clear();
        self.grid_focused = None;
        self.grid_drag = None;
    }

    /// `mod+g`. Port of `on_toggle_grid_view` (`layout.rs:199-216`).
    pub fn toggle_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        self.grid_view = !self.grid_view;
        // A manual grid toggle cancels the "restore grid on zen exit" intent;
        // leaving it set would later re-enter grid with no tiles built.
        self.grid_view_before_zen = false;
        if self.grid_view {
            // A home terminal is invisible behind the tiles, and would keep
            // stealing mod+w / keystrokes from the focused tile.
            self.leave_terminal_tab();
            self.enter_grid(live, saved);
        } else {
            self.exit_grid();
        }
    }

    /// Port of `on_toggle_zen` (`layout.rs:63-103`), all four branches.
    pub fn toggle_zen(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.chrome_visible {
            // Exiting zen.
            self.chrome_visible = true;
            if self.grid_view_before_zen {
                // Zen was entered from grid view: restore grid.
                self.grid_view = true;
                self.grid_view_before_zen = false;
                // Anything that emptied `tile_order` while zenned (a kill, a
                // grid toggle) would restore a blank grid with dead keys.
                if self.tile_order.is_empty() {
                    self.enter_grid(live, saved);
                }
            }
        } else if self.grid_view {
            // Entering zen from the grid: focus the selected tile so zen shows
            // that one session, matching the tile's zen button.
            if let Some(id) = self
                .grid_focused
                .or(self.active_session)
                .or_else(|| self.tile_order.first().copied())
            {
                self.tile_zen(id);
                return;
            }
            // An empty grid has no tile to zen into. Still drop out of grid the
            // way `tile_zen` does, so zen never stacks on top of a chrome-less
            // grid; exiting zen restores it.
            self.grid_view = false;
            self.grid_view_before_zen = true;
            self.chrome_visible = false;
        } else {
            // Entering zen from the single-session workspace: the active
            // session is already focused, just hide the chrome.
            self.chrome_visible = false;
        }
    }

    /// A tile's own zen button. Port of `on_grid_tile_zen`
    /// (`layout.rs:344-356`).
    pub fn tile_zen(&mut self, id: SessionId) {
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.grid_focused = Some(id);
        self.acknowledge(id);
        // Temporarily exit grid so zen has a single-session workspace.
        self.grid_view = false;
        self.grid_view_before_zen = true;
        self.chrome_visible = false;
    }

    /// Point `grid_focused` at a (possibly different) tile
    /// (`update/mod.rs:1061-1068`; the selection half is the view's, see the
    /// module doc).
    pub fn set_grid_focus(&mut self, focus: Option<SessionId>) {
        self.grid_focused = focus;
    }

    /// `update/mod.rs:1052-1056`.
    pub fn sync_grid_focus(&mut self) {
        if crate::grid::should_sync_grid_focus(self.grid_view, self.grid_view_before_zen) {
            self.set_grid_focus(self.active_session);
        }
    }

    /// Move keyboard focus between grid tiles directionally. Grid-only; no-ops
    /// if there's nothing to focus or the move would fall off the edge of the
    /// tile layout. Port of `grid_move` (`update/mod.rs:1071-1094`).
    pub fn grid_move(&mut self, dx: i32, dy: i32) {
        if self.tile_order.is_empty() {
            return;
        }
        // Focusing a tile means the agent side owns input again.
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

    /// Swap the focused tile with its neighbor. Leaves `grid_focused` /
    /// `active_session` untouched — both hold a session, not a tile-order
    /// position, so focus stays on the same session after its tile moves.
    /// Port of `grid_swap` (`update/mod.rs:1102-1116`).
    pub fn grid_swap(&mut self, dx: i32, dy: i32) {
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

    /// A press on a tile: focus it, make it active, acknowledge, and arm the
    /// drag. Port of `on_grid_drag_start` (`layout.rs:308-321`).
    pub fn grid_drag_start(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
        self.grid_drag = Some(GridDrag {
            source_idx: tile_idx,
            hover_idx: tile_idx,
        });
    }

    /// A press on a tile's PTY body: focus it, make it active, and
    /// acknowledge it — but do not arm a drag. Body-click focus
    /// (`GridAction::Focus`); keeps `grid_focused`, the active session and
    /// acknowledgment in step with the gpui focus the terminal view just
    /// took, without arming a drag.
    pub fn grid_focus_tile(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
    }

    /// The pointer entered a tile. A no-op when no drag is armed — the enter
    /// event fires regardless (`layout.rs:323-328`).
    pub fn grid_drag_hover(&mut self, tile_idx: usize) {
        if let Some(drag) = self.grid_drag.as_mut() {
            drag.hover_idx = tile_idx;
        }
    }

    /// The pointer was released: swap if it moved, record the slide, stage the
    /// persist. Port of `on_grid_drag_end` (`layout.rs:330-342`).
    pub fn grid_drag_end(&mut self) {
        let Some(drag) = self.grid_drag.take() else {
            return;
        };
        let (src, dst) = (drag.source_idx, drag.hover_idx);
        if src == dst || src >= self.tile_order.len() || dst >= self.tile_order.len() {
            return;
        }
        crate::grid::swap_tiles(&mut self.tile_order, src, dst);
        self.grid_slide = Some(GridSlide {
            tiles: crate::grid::slide_offsets(src, dst, self.tile_order.len()),
            start: std::time::Instant::now(),
        });
        self.pending_grid_persist = Some(self.tile_order.clone());
    }

    /// Re-derive the grid's view of the session list after sessions were
    /// removed behind the GUI's back. Port of `reconcile_grid_after_teardown`
    /// (`layout.rs:276-306`).
    pub fn reconcile_after_teardown(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.grid_view && !self.grid_view_before_zen {
            self.tile_order.clear();
            self.grid_focused = None;
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
        if self.grid_view && self.tile_order.is_empty() {
            // Nothing left to tile — fall back to the normal workspace.
            self.grid_view = false;
        }
    }

    /// `mod+1..9` inside the grid indexes `tile_order` rather than the
    /// sidebar's visible order (`sessions.rs:396-405`). `n` is 0-based.
    pub fn select_tile_by_index(&mut self, n: usize) {
        let Some(&id) = self.tile_order.get(n) else {
            return;
        };
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.sync_grid_focus();
        self.acknowledge(id);
    }

    /// Shared with [`Self::toggle_terminal_tab`]'s enter branch
    /// (`update/mod.rs:1008-1013`): leaves the grid so a freshly spawned
    /// terminal is actually visible instead of drawn behind the tiles.
    pub fn exit_grid_for_terminal(&mut self) {
        if self.grid_view {
            self.grid_view_before_terminal = true;
            self.grid_view = false;
            self.exit_grid();
        }
    }

    /// `mod+t`. Port of `terminal_toggle_decision` (`shortcuts.rs:528-557`) +
    /// `on_toggle_terminal` (`update/mod.rs:472-500`). **Never touches
    /// `chrome_visible`** (recorded ambiguity 3): in zen it is a pure content
    /// swap. Returns what the caller still has to do.
    pub fn toggle_terminal_tab(
        &mut self,
        has_home_terminals: bool,
        live: &[LiveTile],
        saved: &[String],
    ) -> TerminalTabOutcome {
        if self.terminal_focused {
            // Leaving the tab restores the grid only when the tab was entered
            // from it.
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
                // A terminal drawn behind the tiles would be invisible.
                self.grid_view_before_terminal = true;
                self.grid_view = false;
                self.exit_grid();
            }
            self.terminal_focused = true;
            // Focus moved, so any armed confirm-to-kill is stale.
            self.pending_kill = None;
            self.pending_kill_terminal = None;
            TerminalTabOutcome {
                // First use with no terminals yet: make one rather than
                // showing an empty tab.
                spawn_home_terminal: !has_home_terminals,
            }
        }
    }

    /// The session bar's `term` toggle. Port of `on_toggle_term_panel`
    /// (`sessions.rs:62-88`). Refuses to open with no worktree to anchor to,
    /// and — with the panel's open-ness now per session — with no active
    /// session to anchor to either: there is nothing for the toggle to
    /// record membership against. Returns the panel's new open state.
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
        // Focusing the just-opened panel is the natural default — that's why
        // the user opened it. Click the agent to switch. Closing leaves the
        // agent as the only interactive PTY.
        self.focused_pane = if now_open {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
        now_open
    }

    /// Ctrl+Shift+←/→. Port of `adjust_term_panel_portion`
    /// (`layout.rs:533-542`): clamped, and a no-op when unchanged.
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

    /// The divider drag's live update (`layout.rs:184-193`); `portion` comes
    /// from [`term_portion_for_cursor`], which already clamps.
    pub fn set_term_panel_portion(&mut self, portion: u16) {
        self.term_panel_portion = portion.clamp(TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX);
    }

    /// Reset the input-focus target after the active session (and hence the
    /// panel's worktree) changes: focus the panel when it's open (the just
    /// re-anchored terminal), otherwise the agent. Port of
    /// `reset_focused_pane` (`pty_input.rs:128-137`).
    pub fn reset_focused_pane(&mut self) {
        self.focused_pane = if self.term_panel_open() {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
    }

    /// Apply a click/scroll's origin pane to the input-focus target. A `Panel`
    /// click only takes effect while the panel is open; a `Tile` origin is
    /// ignored (tile focus is `grid_focused`'s job). Port of `focus_pane`
    /// (`pty_input.rs:146-158`).
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

    /// Whether input routes to the panel PTY: only while the panel is open
    /// *and* the panel pane holds the intent (`pty_input.rs:1180-1186`).
    pub fn panel_focused(&self) -> bool {
        self.term_panel_open() && matches!(self.focused_pane, FocusedPane::Panel)
    }

    /// Which PTY a keystroke reaches. The fallback at `pty_input.rs:170-178`
    /// is the load-bearing half: a worktree whose panel has **no shell** routes
    /// to the agent rather than silently swallowing input. In gpui this decides
    /// which `FocusHandle` the workspace focuses; the keystrokes themselves
    /// then follow gpui focus (carried amendment 8). Now also the decider for
    /// the zen-mode keyboard focus toggle (`keymap::FocusSidePanel` /
    /// `FocusAgentPane`) and for routing `mod+w` to a focused panel shell
    /// (`Workspace::close_focused`).
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
        // TRUE project index 2, worktree position 0.
        assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));
    }

    /// The reported bug: launching from the palette while the grid is up left
    /// `grid_focused` on the previous tile, so the keyboard stayed there too.
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

        // Outside the grid it stays untouched (`should_sync_grid_focus`).
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

    /// Second mod+w on the same session confirms the kill; a different
    /// session re-arms instead (`shortcuts.rs:501-527`).
    #[test]
    fn close_focused_session_confirms_only_on_a_second_press_of_the_same_target() {
        let mut w = WorkspaceState::default();
        assert!(!w.close_focused_session(sid(1)));
        assert_eq!(w.pending_kill(), Some(sid(1)));

        // A different target re-arms rather than killing.
        assert!(!w.close_focused_session(sid(2)));
        assert_eq!(w.pending_kill(), Some(sid(2)));

        // Same target twice in a row confirms.
        assert!(w.close_focused_session(sid(2)));
    }

    /// Terminal counterpart of the above.
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
        assert_eq!(w.tree_expand(), TreeExpand::SessionsOnly);
    }

    /// The startup fix: a fresh sidebar only expands what has sessions, and
    /// highlights the first (project, worktree) that has one — once the user
    /// touches the tree manually, later snapshots stop moving it.
    #[test]
    fn sync_default_tree_expands_only_sessionful_projects_and_picks_the_first() {
        let mut snap = fixture();
        // Project 0 (alpha) has no sessions; project 2 (gamma) does.
        snap.projects[0].worktrees[0].sessions.clear();
        snap.projects[0].sessions.clear();
        let mut w = WorkspaceState::default();

        w.sync_default_tree(&snap);
        assert!(w.project_collapsed(0));
        assert!(!w.project_collapsed(2));
        assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));

        // Once touched, a later call is a no-op — even one that would
        // otherwise re-expand/re-point everything.
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

    /// Bare Escape's carve-out (`update/mod.rs:789-804`): each of the four
    /// armed states alone makes Escape a dismissal that clears **all** of
    /// them, and with none armed the key is left for the PTY.
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
            // A second Escape has nothing left and falls through.
            assert!(!w.escape_dismiss(), "state {i}: only one Escape is eaten");
        }
    }

    // ── Plan 07 Task 2: the four screens ────────────────────────────────

    use crate::keymap::Screen;

    /// `n` live sessions with ids 1..=n and stable keys `p::/w{id}`.
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

    /// The `chromeless_grid_is_not_a_nameable_screen` guard, now enforced on
    /// the transitions rather than only on the classifier: every grid-entry
    /// path sets `chrome_visible` first (`layout.rs:222-227`).
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

    /// `layout.rs:63-79` — zen entered from the grid restores it, with the
    /// tile order it had.
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
        // Zen shows exactly the focused tile's session.
        assert_eq!(w.active_session(), Some(sid(1)));

        w.toggle_zen(&l, &[]);
        assert_eq!(w.screen(), Screen::Grid);
        assert!(!w.grid_view_before_zen());
        assert_eq!(w.tile_order(), before.as_slice());
    }

    /// `layout.rs:98-102` — the other entry point round-trips to the
    /// single-session workspace, not to a grid.
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

    /// An empty grid still drops out of grid view rather than stacking zen on
    /// a chrome-less grid (`layout.rs:88-95`).
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

    /// `layout.rs:204-206` — a manual `mod+g` cancels the restore intent, so a
    /// later zen-exit does not resurrect a grid.
    #[test]
    fn a_manual_grid_toggle_cancels_the_zen_restore_intent() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]); // grid
        w.toggle_zen(&l, &[]); // zen, remembering the grid
        assert!(w.grid_view_before_zen());

        w.toggle_grid(&l, &[]); // manual mod+g while zenned
        assert!(!w.grid_view_before_zen());
        assert_eq!(w.screen(), Screen::Grid);

        w.toggle_grid(&l, &[]); // back out of the grid
        assert_eq!(w.screen(), Screen::Workspace);
        w.toggle_zen(&l, &[]);
        w.toggle_zen(&l, &[]);
        // No resurrected grid.
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
        // The order was staged for `Store::grid_order` before the teardown.
        assert_eq!(
            w.take_grid_order_to_persist(),
            Some(vec![sid(1), sid(2), sid(3)])
        );
    }

    /// `layout.rs:222-252` — the saved order wins, new sessions append, and
    /// the active session keeps its tile focused.
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

        // Session 2 was torn down behind the GUI's back.
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

    /// A session spawned while the grid is up has no path that adds it to
    /// `tile_order` other than re-running the reconcile the registry observer
    /// now triggers (`Workspace::sync_grid_tiles`). Guard the mechanism: a
    /// third, newly-live tile must appear, appended after the two the grid
    /// already knows about.
    #[test]
    fn reconcile_after_teardown_adds_a_session_spawned_while_the_grid_is_up() {
        let l = live(&[1, 2]);
        let mut w = WorkspaceState::default();
        w.toggle_grid(&l, &[]);

        // Session 3 was spawned behind the grid's back.
        w.reconcile_after_teardown(&live(&[1, 2, 3]), &[]);
        assert_eq!(w.tile_order(), [sid(1), sid(2), sid(3)]);
    }

    /// `update/mod.rs:1071-1094` over `grid::grid_neighbor`. 3 tiles → cols=2:
    /// left column 0/2, right column 1.
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
        // Vertical moves need the naive target: from tile 1, down is index 3.
        w.grid_move(1, 0);
        w.grid_move(0, 1);
        assert_eq!(w.grid_focused(), Some(sid(2)));

        let mut empty = WorkspaceState::default();
        empty.grid_move(1, 0);
        assert_eq!(empty.grid_focused(), None);
    }

    /// `update/mod.rs:1102-1116` — the tile moves, the focused **session**
    /// does not change.
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
        // The slide was recorded post-swap: tile at position 1 came from 0.
        let Some(slide) = w.grid_slide() else {
            unreachable!("a swap records a slide");
        };
        assert_eq!(slide.tiles, [(1, -1, 0), (0, 1, 0)]);
        assert_eq!(
            w.take_grid_order_to_persist(),
            Some(vec![sid(2), sid(1), sid(3), sid(4)])
        );

        // A swap off the edge is a no-op: the focused session now sits at
        // position 1 (column 1 of 2), so there is nothing to its right.
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
        // Out of range is a no-op, never a clamp.
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

        // From the plain workspace there is no grid to come back to.
        w.toggle_grid(&l, &[]);
        assert_eq!(w.screen(), Screen::Workspace);
        w.toggle_terminal_tab(true, &l, &[]);
        w.toggle_terminal_tab(true, &l, &[]);
        assert!(!w.grid_view());
    }

    /// Recorded ambiguity 3 (`update/mod.rs:472-475`): in zen `mod+t` is a
    /// pure content swap.
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

        // Closing always works, worktree or not, and hands input back.
        assert!(!w.toggle_term_panel(false));
        assert!(!w.term_panel_open());
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
        assert!(!w.panel_focused());
    }

    /// The reported bug: the panel used to be one global bool, so leaving it
    /// open in session A leaked into session B on switch. Membership in
    /// `panel_open_sessions` is per session now, so a switch shows whatever
    /// *that* session was left with — closed for one that was never touched.
    #[test]
    fn term_panel_open_is_tracked_per_session() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        assert!(w.toggle_term_panel(true));
        assert!(w.term_panel_open());

        // B has never touched the panel: closed, not A's leftover state.
        w.active_session = Some(sid(2));
        assert!(!w.term_panel_open());

        // Back to A: still open, exactly as A left it.
        w.active_session = Some(sid(1));
        assert!(w.term_panel_open());

        // Closing A's panel while A is active clears only A's membership.
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
        let work_left = sidebar + crate::views::sidebar::SIDEBAR_DIVIDER_W;
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
        // Panel closed: a click cannot move the intent.
        w.focus_pane(PtyPane::Panel);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);

        w.toggle_term_panel(true);
        w.focus_pane(PtyPane::Agent);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
        w.focus_pane(PtyPane::Panel);
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
        // A tile origin is `grid_focused`'s business, not the pane's.
        w.focus_pane(PtyPane::Tile(sid(1)));
        assert_eq!(w.focused_pane(), FocusedPane::Panel);

        // Re-anchoring after the active session changes re-focuses the panel.
        w.focus_pane(PtyPane::Agent);
        w.reset_focused_pane();
        assert_eq!(w.focused_pane(), FocusedPane::Panel);
    }

    /// `pty_input.rs:170-178` — the fallback that stops a shell-less panel
    /// from eating every keystroke.
    #[test]
    fn a_worktree_with_no_panel_shell_routes_input_to_the_agent() {
        let mut w = WorkspaceState {
            active_session: Some(sid(1)),
            ..WorkspaceState::default()
        };
        assert_eq!(w.input_target(false), PtyPane::Agent);

        w.toggle_term_panel(true);
        assert!(w.panel_focused());
        // Open and focused, but this worktree has no shell yet.
        assert_eq!(w.input_target(false), PtyPane::Agent);
        // Once one exists, the panel wins.
        assert_eq!(w.input_target(true), PtyPane::Panel);

        // Clicking the agent hands input back even with a shell present.
        w.focus_pane(PtyPane::Agent);
        assert_eq!(w.input_target(true), PtyPane::Agent);

        // Closing the panel routes to the agent regardless.
        w.toggle_term_panel(false);
        assert_eq!(w.input_target(true), PtyPane::Agent);
    }

    /// `layout.rs:308-342` — press focuses and arms, enter tracks, release
    /// commits.
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
            w.grid_drag(),
            Some(GridDrag {
                source_idx: 2,
                hover_idx: 2
            })
        );

        w.grid_drag_hover(0);
        assert_eq!(w.grid_drag().map(|d| d.hover_idx), Some(0));

        w.grid_drag_end();
        assert_eq!(w.tile_order(), [sid(3), sid(2), sid(1), sid(4)]);
        // Focus follows the session, not the slot.
        assert_eq!(w.grid_focused(), Some(sid(3)));
        assert!(w.grid_drag().is_none());
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
        assert!(w.grid_drag().is_none());
        w.grid_drag_end();
        assert_eq!(w.tile_order(), [sid(1), sid(2)]);

        // Pressing and releasing on the same tile is a focus, not a reorder.
        w.grid_drag_start(1);
        w.grid_drag_end();
        assert_eq!(w.tile_order(), [sid(1), sid(2)]);
        assert!(w.grid_slide().is_none());
        assert!(w.take_grid_order_to_persist().is_none());
    }

    /// `layout.rs:257-269` — leaving the grid re-anchors the panel, so a stale
    /// `Panel` intent cannot type into another worktree's shell.
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
        // Still open, so the panel is re-focused rather than dropped.
        assert_eq!(w.focused_pane(), FocusedPane::Panel);

        w.toggle_term_panel(false);
        w.toggle_grid(&l, &[]);
        w.toggle_grid(&l, &[]);
        assert_eq!(w.focused_pane(), FocusedPane::Agent);
    }
}
