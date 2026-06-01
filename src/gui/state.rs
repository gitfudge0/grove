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
    pub pty_cols: u16,
    /// Per-window GUI zoom multiplier. Applied as the iced application scale
    /// factor and reused when deriving PTY rows/cols from the visible area.
    pub ui_zoom: f32,
    /// Last known window size in unzoomed units so zoom changes can recompute
    /// PTY dimensions without waiting for another resize event.
    pub window_size: Size,
    /// Worktree whose split-start agent menu is open.
    pub open_agent_menu: Option<(usize, usize)>,
    /// Mouse-drag selection in the active session's PTY, in (row, col) cells.
    /// Un-normalized so we know which end is moving.
    pub pty_selection: Option<(PtyCell, PtyCell)>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyCell {
    pub row: usize,
    pub col: usize,
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
    /// Switch the sidebar between the tree view and the activity stream.
    SidebarSetView(SidebarView),
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
    StartTerminal {
        proj: usize,
        wt: usize,
    },
    CloseAgentMenu,
    SelectSession(usize),
    KillSession(usize),
    RequestKillSession(usize),
    KeyPress(Key, Modifiers),
    PtyMouseDown(f32, f32),
    PtyMouseDrag(f32, f32),
    PtyMouseUp,
    PtyScroll {
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
