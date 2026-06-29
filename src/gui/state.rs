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
    /// Memoized `list_dirs` result for the add-project path modal, keyed by the
    /// input buffer. `view()` runs every tick; without this the modal would hit
    /// the filesystem (`read_dir`) on every frame.
    pub dir_cache: std::cell::RefCell<Option<(String, Vec<String>)>>,
    /// Per-session activity trackers, keyed by `Session::id` (never reused,
    /// unlike Arc pointer addresses). Refreshed every ~480ms by `Msg::Tick`;
    /// stale keys are pruned on the same pass.
    pub activity: HashMap<u64, super::activity::Tracker>,
    /// Whether the OS window currently has focus — gates the dock bounce.
    pub window_focused: bool,
    /// Last dock badge value pushed, to avoid redundant objc calls.
    pub last_badge: usize,
    /// Sidebar width in logical pixels. Driven by dragging the divider, clamped
    /// to `[SIDEBAR_MIN_W, window cap]`, persisted to `Store.sidebar_width`.
    pub sidebar_width: f32,
    /// Active divider drag, if the left button is held over the resize handle.
    /// While set, a global mouse subscription feeds cursor moves and the
    /// button-release that ends the drag.
    pub sidebar_drag: Option<SidebarDrag>,
    /// Timestamp of the last divider press, for double-click reset detection.
    pub last_divider_press: Option<std::time::Instant>,
    /// Whether the terminal-panel split divider is being dragged. While true, a
    /// global mouse subscription feeds cursor moves and the button-release.
    pub term_panel_dragging: bool,
    /// Timestamp of the last terminal-panel divider press, for double-click
    /// reset detection.
    pub last_term_divider_press: Option<std::time::Instant>,
    /// Live state for the per-project lifecycle-scripts editor, when open.
    /// `Some` exactly when `app.modal` is `Modal::ScriptsEditor`.
    pub scripts_editor: Option<ScriptsEditorState>,
    /// Per-tool install/version status shown in the Settings → Tools section.
    /// Parked on the model (like `scripts_editor`) because detection runs
    /// asynchronously and posts results back via `Msg::ToolVersionsDetected`.
    /// Empty until Settings is first opened.
    pub settings_tools: Vec<ToolStatus>,
    /// Current self-update state — drives the Updates UI and badge.
    pub upgrade: UpgradeState,
    /// State for the changelog modal.
    pub changelog: ChangelogState,
    /// When true, the changelog modal is shown over the normal view.
    pub show_changelog: bool,
    /// How Grove was installed (homebrew, cargo, etc.) — determines the update command.
    pub upgrade_method: crate::upgrade::InstallMethod,
    /// Written by the apply thread, drained on `Tick` to drive `UpgradeState`.
    pub upgrade_progress: std::sync::Arc<std::sync::Mutex<UpgradeProgress>>,
}

/// Install + version status for a single coding-agent tool in the Settings
/// Tools section. Built asynchronously off the UI thread.
#[derive(Clone, Debug)]
pub struct ToolStatus {
    pub agent: Agent,
    pub installed: bool,
    /// Version string from `<program> --version`, or `None` when missing /
    /// undetectable (callers then show "installed").
    pub version: Option<String>,
    /// True while detection is in flight — drives the per-row spinner.
    pub detecting: bool,
}

/// Transient state for an in-progress sidebar divider drag.
#[derive(Clone, Copy, Debug)]
pub struct SidebarDrag {
    /// `sidebar_width - cursor_x` captured on the first cursor move after the
    /// press, so the width tracks the cursor without jumping when the press
    /// lands a few px off the exact divider edge. `None` until that first move.
    pub grab_offset: Option<f32>,
    /// Sidebar width when the drag began, so a press without real movement
    /// (a plain click) skips the PTY resize + persist on release.
    pub start_width: f32,
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

/// Shared progress state written by the background apply thread and drained on
/// each `Msg::Tick`. Both fields are `take`-d by the Tick handler so they are
/// consumed exactly once.
#[derive(Default)]
pub struct UpgradeProgress {
    pub stage: Option<crate::upgrade::Stage>,
    pub finished: Option<Result<(), String>>,
}

/// Drives the changelog modal.
#[derive(Debug, Clone)]
pub enum ChangelogState {
    Idle,
    Loading,
    Loaded(Vec<crate::upgrade::ReleaseNote>),
    Error(String),
}

/// Drives the Updates UI. `Available` carries the resolved release; the apply
/// states drive the progress modal.
#[derive(Debug, Clone)]
pub enum UpgradeState {
    Idle,
    Checking,
    UpToDate,
    Available(crate::upgrade::Release),
    Error(String),
    Updating(crate::upgrade::Stage),
    Updated,
    UpdateFailed(String),
}

/// Which lifecycle script a `ScriptsEditorAction` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptField {
    Setup,
    Run,
    Teardown,
}

/// Live state for the per-project scripts editor overlay. Holds the three
/// `text_editor` buffers (which must persist across frames, so they can't live
/// in the cloneable `Modal`) plus the target project index.
pub struct ScriptsEditorState {
    pub proj: usize,
    pub project_name: String,
    pub setup: iced::widget::text_editor::Content,
    pub run: iced::widget::text_editor::Content,
    pub teardown: iced::widget::text_editor::Content,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    /// OS window gained/lost focus (drives dock-bounce gating and
    /// implicit acknowledgment of the visible session).
    WindowFocusChanged(bool),
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
    /// `(key, modified_key, modifiers)`. `key` is layout-aware but
    /// modifier-independent (so Ctrl shortcuts match across platforms);
    /// `modified_key` carries Shift/AltGr for text entry.
    KeyPress(Key, Key, Modifiers),
    /// A file was dragged onto the window; its path is typed into the
    /// focused session (shell-escaped, trailing space).
    FileDropped(std::path::PathBuf),
    PtyMouseDown(PtyPane, f32, f32),
    PtyMouseDrag(PtyPane, f32, f32),
    PtyMouseUp,
    /// Left button pressed on the sidebar resize handle. Begins a drag (or, on a
    /// second press within the double-click window, resets to the default width).
    SidebarDragStart,
    /// Cursor moved while the divider is held; carries the cursor's logical
    /// x-position (window-relative), which maps to the new sidebar width.
    SidebarDragMove(f32),
    /// Left button released: commit the width (recompute PTYs, persist).
    SidebarDragEnd,
    /// Left button pressed on the terminal-panel split divider. Begins a drag,
    /// or resets to the default split on a double-click.
    TermPanelDragStart,
    /// Cursor moved while the terminal-panel divider is held; carries the
    /// cursor's logical x-position, mapped to the panel's width share.
    TermPanelDragMove(f32),
    /// Left button released: commit the split (recompute PTY columns).
    TermPanelDragEnd,
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
    /// Run the project's `run` script in this worktree (spawns a session tab).
    RunScript {
        proj: usize,
        wt: usize,
    },
    /// Open the per-project lifecycle-scripts editor.
    EditScripts {
        proj: usize,
    },
    /// Edit one of the three script buffers in the scripts editor.
    ScriptsEditorAction(ScriptField, iced::widget::text_editor::Action),
    /// Persist the edited scripts back to the project and close the editor.
    ScriptsEditorSave,
    /// Close the scripts editor without saving.
    ScriptsEditorCancel,
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
    /// Live edit of the path field in the add-project / add-worktree input modal.
    InputPathChanged(String),
    /// Live edit of the optional project-name field in the add-project modal.
    InputNameChanged(String),
    /// No-git add-project decision: initialize git for the just-added project.
    AddProjectInitGit,
    /// No-git add-project decision: keep the project without git.
    AddProjectContinueNoGit,
    /// No-git add-project decision: discard the just-added project.
    AddProjectCancelNoGit,
    ModalPickDir(String),
    ChooseTmux(bool),
    AgentPickerSelect(usize),
    AgentPickerToggleDefault,
    AgentPickerSubmit,
    OpenThemePicker,
    /// Open the Settings modal (cog in the appbar).
    OpenSettings,
    /// Set (or clear, if re-selecting the current) the global default agent
    /// from the Settings Tools section.
    SetDefaultAgent(Agent),
    /// Re-run tool install/version detection for the Settings Tools section.
    RefreshTools,
    /// Async detection finished; carries the fresh per-tool statuses.
    ToolVersionsDetected(Vec<(Agent, ToolStatus)>),
    /// Open the changelog modal (fetches releases off-thread).
    OpenChangelog,
    /// Off-thread release-note fetch completed; carries the notes or an error string.
    ChangelogLoaded(Result<Vec<crate::upgrade::ReleaseNote>, String>),
    /// Close the changelog modal and return to the Settings modal.
    CloseChangelog,
    /// Trigger an off-thread update check. `manual: true` = user-initiated (surfaces
    /// errors inline); `manual: false` = launch/periodic (fails silently, log only).
    CheckForUpdates { manual: bool },
    /// Off-thread check completed; carries the fetched release or an error string.
    /// The `bool` mirrors the `manual` flag from the originating `CheckForUpdates`.
    UpdateCheckResult(Result<crate::upgrade::Release, String>, bool),
    /// User chose to skip the available release version.
    SkipVersion,
    /// User confirmed they want to apply the update.
    StartUpdate,
    /// Restart the app after a successful update.
    RestartApp,
    ThemePickerSwitchTab,
    ThemePickerSelect(usize),
    ThemePickerSubmit,
    ThemePickerCancel,
    // ── first-run onboarding wizard ──────────────────────────────────────
    /// Advance one step. On the project step this registers the project first;
    /// on the theme step it persists the previewed theme; on the session step it
    /// finishes setup and launches the chosen agent.
    OnbNext,
    /// Step back one step.
    OnbBack,
    /// Skip the rest of setup; marks onboarded and restores the pre-preview theme.
    OnbSkip,
    /// Live edit of the project-step path field.
    OnbPathChanged(String),
    /// Live edit of the project-step name field.
    OnbNameChanged(String),
    /// Clicked a directory match in the project step.
    OnbPickDir(String),
    /// Toggle the theme-step dark/light tab.
    OnbThemeTab,
    /// Select (and live-preview) the theme at this index in the theme step.
    OnbThemeSelect(usize),
    /// Select the agent at this index in the session step.
    OnbAgentSelect(usize),
}
