//! Shared types: the top-level `Grove` model, the `Msg` enum dispatched by
//! iced, and PTY-render caching primitives.

use crate::agent::Agent;
use crate::app::App;
use crate::git::Worktree;
use iced::keyboard::{Key, Modifiers};
use iced::widget::canvas;
use iced::{Color, Size};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Which top-level rendering the sidebar uses. `Tree` is the original
/// project → worktree → session hierarchy; `Activity` is a flat list of
/// every session across every worktree, grouped by liveness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarView {
    Tree,
    Activity,
    /// A single persistent local shell rooted at `~`. Not tied to any
    /// worktree, never appears in the session lists, and can't be killed —
    /// only restarted (always back at `~`) if its shell exits.
    Terminal,
}

/// Which of the two PTYs receives keyboard input, scroll, and selection while
/// the right-docked terminal slide-over panel is open. Meaningless (and ignored
/// by `focused_session*`) when the panel is closed. Clicking a PTY sets this to
/// its pane; opening the panel defaults to `Panel` (the user just asked for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusedPane {
    Agent,
    Panel,
}

/// Top-level iced application state.
pub struct Grove {
    pub app: App,
    pub collapsed: HashSet<usize>,
    /// Worktrees whose session children are hidden. Independent of the
    /// project-level `collapsed`.
    pub collapsed_wt: HashSet<(usize, usize)>,
    /// Cache of worktrees per project index. Refilled on project expand /
    /// session spawn/kill — never inside `view()`, since `git worktree list`
    /// is a subprocess and `view()` runs on every 33ms tick.
    pub wt_cache: HashMap<usize, Vec<Worktree>>,
    /// Cached PTY screen snapshots, keyed by the `dirty` Arc's pointer
    /// (stable & unique per Session). Rebuilt only when the session's dirty
    /// flag was set since last build — so switching to a quiet session is
    /// free, and the per-frame parser lock is taken only for sessions that
    /// actually changed.
    pub pty_cache: RefCell<HashMap<usize, PtyCacheEntry>>,
    /// Current PTY dimensions derived from window size. Updated on every
    /// `WindowResized` event and applied to sessions on spawn / select.
    pub pty_rows: u16,
    /// Full workspace column count (panel closed) — used by the home-terminal
    /// tab and as the fallback width.
    pub pty_cols: u16,
    /// Agent-view column count: equals `pty_cols` when the slide-over terminal
    /// panel is closed, the 65% share when it is open.
    pub pty_sess_cols: u16,
    /// Terminal-panel column count: the 35% share used while the panel is open.
    pub pty_panel_cols: u16,
    /// Per-window GUI zoom multiplier. Applied as the iced application scale
    /// factor and reused when deriving PTY rows/cols from the visible area.
    pub ui_zoom: f32,
    /// Last known window size in unzoomed units so zoom changes can recompute
    /// PTY dimensions without waiting for another resize event.
    pub window_size: Size,
    /// Worktree whose split-start agent menu is open.
    pub open_agent_menu: Option<(usize, usize)>,
    /// Mouse-drag selection in the active session's PTY, stored in
    /// scrollback-stable absolute cells (see [`AbsCell`]) so it survives
    /// auto-scrolling and can span more than one viewport. Un-normalized so we
    /// know which end (`.0` anchor / `.1` head) is moving.
    pub pty_selection: Option<(AbsCell, AbsCell)>,
    /// Active selection drag, if the left button is held over the PTY. Drives
    /// the tick-based edge auto-scroll: while set, `Msg::Tick` checks whether
    /// `last_y` sits in the top/bottom edge zone and scrolls + extends.
    pub pty_drag: Option<PtyDrag>,
    /// Monotonically incrementing counter driven by `Msg::Tick` (~30 Hz).
    /// Used to compute cursor blink state: visible when `blink_tick % 30 < 15`
    /// (≈ 500 ms on / 500 ms off).
    pub blink_tick: u32,
    /// Session index awaiting kill confirmation. When set, that session's
    /// close button shows a red tick — clicking it confirms the kill, clicking
    /// anywhere else clears this back to `None`.
    pub pending_kill: Option<usize>,
    /// Worktree currently under the mouse — drives reveal of the per-row
    /// action buttons (play / terminal / more). `None` when no row is hovered.
    pub hovered_wt: Option<(usize, usize)>,
    /// Session-index of the activity-stream row currently under the mouse.
    /// Drives the hover-reveal of the inline spawn chips on session rows in
    /// the activity view. `None` when no session row is hovered.
    pub hovered_activity_row: Option<usize>,
    /// Selected sidebar rendering mode (tree vs activity stream). In-memory
    /// only — no persisted prefs pattern exists for transient view state.
    pub sidebar_view: SidebarView,
    /// User-toggled expansion of the `worktrees · no sessions` activity-view
    /// group. `None` means "use default" (expanded iff non-empty).
    pub activity_no_sessions_expanded: Option<bool>,
    /// Whether the right-docked terminal slide-over panel is open. The panel
    /// belongs to the active session's worktree; toggled by the `term` button
    /// in the session header. Closing it leaves the worktree's shells alive
    /// (they reattach when reopened); they only die when the worktree is
    /// removed.
    pub term_panel_open: bool,
    /// The terminal panel's share of the workspace width, in percent (the agent
    /// view gets `100 - term_panel_portion`). Adjusted live with
    /// Ctrl+Shift+Left/Right between `TERM_PANEL_PORTION_MIN` and
    /// `TERM_PANEL_PORTION_MAX`; drives both the `FillPortion` weights in
    /// `view()` and each PTY's derived column count.
    pub term_panel_portion: u16,
    /// Which PTY (agent vs panel) input is routed to while the panel is open.
    /// Only consulted when `term_panel_open`; defaults to `Panel` on open and
    /// resets to `Agent` on close or active-session change. Clicking either PTY
    /// updates it.
    pub focused_pane: FocusedPane,
}

/// Identifies which on-screen PTY a mouse event originated from. The home
/// terminal tab and the single full-width agent view both use `Agent`; only the
/// right-docked slide-over panel uses `Panel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyPane {
    Agent,
    Panel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyCell {
    pub row: usize,
    pub col: usize,
}

/// A selection endpoint in scrollback-stable coordinates. `a_row` is the line's
/// distance above the live bottom: for visible grid height `h` and scrollback
/// offset `S`, viewport row `r` maps to `a_row = S + (h - 1 - r)` and back via
/// `r = (h - 1) - (a_row - S)`. Larger `a_row` = older content. Storing rows
/// this way keeps the selection pinned to its content as the view scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsCell {
    pub a_row: usize,
    pub col: usize,
}

/// Transient state for an in-progress selection drag over the PTY canvas.
#[derive(Clone, Copy, Debug)]
pub struct PtyDrag {
    /// Last drag cursor y, in unzoomed canvas pixels (clamped to `[0, view_h_px]`
    /// by the canvas program). Used to detect the edge zones.
    pub last_y: f32,
    /// Last drag cursor x, in unzoomed canvas pixels. Used to recompute the
    /// selection column when auto-scroll extends the head.
    pub last_x: f32,
    /// Visible canvas height in unzoomed pixels (`h * CELL_H`).
    pub view_h_px: f32,
}

pub struct PtyCacheEntry {
    /// One row per terminal line. Each row is a run-list of styled segments.
    /// Wrapped in `Arc` so the Canvas program can hold a cheap clone without
    /// copying ~8000 strings per frame.
    pub rows: Arc<Vec<Vec<StyledRun>>>,
    /// Iced canvas cache. The PTY draw skips entirely while warm — we
    /// `clear()` it only when `dirty` flips.
    pub cache: Arc<canvas::Cache>,
    /// Current cursor position (row, col) as reported by vt100. `None` when
    /// the running program has hidden the cursor (e.g. vim, htop).
    pub cursor_pos: Option<(u16, u16)>,
}

#[derive(Clone)]
pub struct StyledRun {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    WindowResized(Size),
    BackendNative,
    BackendTmux,
    ProjectClicked(usize),
    /// Toggle button in the sidebar tree header. Collapses everything except
    /// worktrees that currently contain sessions, or — if already in that
    /// state — expands every project and worktree.
    ToggleCollapseAll,
    /// Switch the sidebar between the tree view, the activity stream, and the
    /// persistent home terminal.
    SidebarSetView(SidebarView),
    /// Relaunch the active home terminal's shell at `~` (e.g. after it exited).
    RestartHomeTerminal,
    /// Spawn an additional home terminal and focus it.
    NewHomeTerminal,
    /// Focus the home terminal at this index.
    SelectHomeTerminal(usize),
    /// Close the home terminal at this index.
    CloseHomeTerminal(usize),
    /// Toggle the collapsed-state of the `worktrees · no sessions` group in
    /// activity view.
    ToggleActivityNoSessionsGroup,
    WorktreeClicked {
        proj: usize,
        wt: usize,
    },
    HoverWorktree(Option<(usize, usize)>),
    /// Mouse entered/left an activity-stream session row (by session index).
    HoverActivityRow(Option<usize>),
    StartSession {
        proj: usize,
        wt: usize,
        agent: Agent,
    },
    /// Spawn a terminal *session* (a sibling tree row) in a worktree. Still used
    /// by the per-worktree-row hover button; the session header's `term` button
    /// no longer uses this — it toggles the slide-over panel instead.
    StartTerminal {
        proj: usize,
        wt: usize,
    },
    /// Toggle the right-docked terminal slide-over for the active session's
    /// worktree. Ensures a shell exists when opening.
    ToggleTermPanel,
    /// Spawn an additional panel shell in the active session's worktree.
    NewWtTerminal,
    /// Focus the panel shell at this index in the active worktree's panel.
    SelectWtTerminal(usize),
    /// Close the panel shell at this index in the active worktree's panel.
    CloseWtTerminal(usize),
    CloseAgentMenu,
    SelectSession(usize),
    KillSession(usize),
    RequestKillSession(usize),
    KeyPress(Key, Modifiers),
    PtyMouseDown(PtyPane, f32, f32),
    PtyMouseDrag(PtyPane, f32, f32),
    PtyMouseUp,
    PtyScroll {
        pane: PtyPane,
        up: bool,
        x: f32,
        y: f32,
    },
    ToggleZen,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    AddProject,
    AddWorktree {
        proj: usize,
    },
    DeleteWorktree {
        proj: usize,
        wt: usize,
    },
    /// Open the remove-project confirmation modal for the given project row.
    RemoveProject {
        proj: usize,
    },
    /// Toggle the "also delete worktrees on disk" checkbox in the
    /// remove-project modal.
    ToggleRemoveWorktrees(bool),
    /// User confirmed the remove-project modal; kicks off async teardown
    /// when worktrees are slated for removal, otherwise finalizes inline.
    ConfirmRemoveProject,
    /// One worktree finished removing (or errored). Carries the per-worktree
    /// outcome plus the remaining queue so the handler can advance.
    WorktreeRemovedStep {
        path: String,
        error: Option<String>,
        remaining: Vec<String>,
    },
    ModalSubmit,
    ModalCancel,
    ModalConfirm(bool),
    ModalPickDir(String),
    ChooseTmux(bool),
    AgentPickerSelect(usize),
    AgentPickerToggleDefault,
    AgentPickerSubmit,
    OpenThemePicker,
    ThemePickerSwitchTab,
    ThemePickerSelect(usize),
    ThemePickerSubmit,
    ThemePickerCancel,
}
