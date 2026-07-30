//! Shared types: the top-level `Grove` model, the `Msg` enum dispatched by
//! iced, and PTY-render caching primitives.

use crate::app::App;
use grove_core::agent::Agent;
use grove_core::git::Worktree;
use iced::keyboard::{Key, Modifiers};
use iced::widget::canvas;
use iced::{Color, Size};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Which of the two PTYs receives keyboard input, scroll, and selection while
/// the right-docked terminal slide-over panel is open. Meaningless (and ignored
/// by `focused_session*`) when the panel is closed. Clicking a PTY sets this to
/// its pane; opening the panel defaults to `Panel` (the user just asked for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusedPane {
    Agent,
    Panel,
}

/// The three modes the tree header's cycle button steps through, in ring
/// order `Collapsed → SessionsOnly → All → Collapsed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeExpand {
    /// Every project row collapsed — only the project list is visible.
    Collapsed,
    /// Projects/worktrees with no sessions collapsed; the rest expanded.
    SessionsOnly,
    /// Everything expanded.
    All,
}

impl TreeExpand {
    /// The mode a click advances to from `self`.
    pub fn next(self) -> Self {
        match self {
            TreeExpand::Collapsed => TreeExpand::SessionsOnly,
            TreeExpand::SessionsOnly => TreeExpand::All,
            TreeExpand::All => TreeExpand::Collapsed,
        }
    }
}

/// Top-level iced application state.
pub struct Grove {
    pub(in crate::gui) app: App,
    pub(in crate::gui) collapsed: HashSet<usize>,
    /// Worktrees whose session children are hidden. Independent of the
    /// project-level `collapsed`.
    pub(in crate::gui) collapsed_wt: HashSet<(usize, usize)>,
    /// Which mode the tree header's cycle button last applied. Drives the
    /// glyph (which shows the *next* action) and advances on each click.
    pub(in crate::gui) tree_expand: TreeExpand,
    /// Cache of worktrees per project index. Refilled on project expand /
    /// session spawn/kill — never inside `view()`, since `git worktree list`
    /// is a subprocess and `view()` runs on every 33ms tick.
    pub(in crate::gui) wt_cache: HashMap<usize, Vec<Worktree>>,
    /// Cached PTY screen snapshots, keyed by the `dirty` Arc's pointer
    /// (stable & unique per Session). Rebuilt only when the session's dirty
    /// flag was set since last build — so switching to a quiet session is
    /// free, and the per-frame parser lock is taken only for sessions that
    /// actually changed.
    pub(in crate::gui) pty_cache: RefCell<HashMap<usize, PtyCacheEntry>>,
    /// PTY geometry and GUI zoom, all derived from the window size.
    pub(in crate::gui) pty_layout: PtyLayout,
    /// Worktree whose split-start agent menu is open.
    pub(in crate::gui) open_agent_menu: Option<(usize, usize)>,
    /// Whether the appbar's attention-queue dropdown is open.
    pub(in crate::gui) attention_open: bool,
    /// Mouse-drag selection in the active session's PTY, stored in
    /// scrollback-stable absolute cells (see [`AbsCell`]) so it survives
    /// auto-scrolling and can span more than one viewport. Un-normalized so we
    /// know which end (`.0` anchor / `.1` head) is moving.
    pub(in crate::gui) pty_selection: Option<(AbsCell, AbsCell)>,
    /// Active selection drag, if the left button is held over the PTY. Drives
    /// the tick-based edge auto-scroll: while set, `Msg::Tick` checks whether
    /// `last_y` sits in the top/bottom edge zone and scrolls + extends.
    pub(in crate::gui) pty_drag: Option<PtyDrag>,
    /// True while the current PTY press is the one that changed focus (tile or
    /// pane). That press only focuses: its release must not fire the
    /// click-to-move-caret, so a first click on a stale session can't poke
    /// its shell.
    pub(in crate::gui) pty_press_focused: bool,
    /// Blink counter and the two idle-until-needed iced animations.
    pub(in crate::gui) anim: Animations,
    /// Session index awaiting kill confirmation. When set, that session's
    /// close button shows a red tick — clicking it confirms the kill, clicking
    /// anywhere else clears this back to `None`.
    pub(in crate::gui) pending_kill: Option<usize>,
    /// Home-terminal index awaiting close confirmation — same two-step
    /// confirm idiom as `pending_kill`, kept separate since it indexes
    /// `App::home_terminals` rather than `App::sessions`.
    pub(in crate::gui) pending_kill_terminal: Option<usize>,
    /// Worktree currently under the mouse — drives reveal of the per-row
    /// action buttons (play / terminal / more). `None` when no row is hovered.
    pub(in crate::gui) hovered_wt: Option<(usize, usize)>,
    /// TRUE `store.projects` index of the archived-projects row under the
    /// mouse, driving that row's hover fill. Same descendant-hover mechanism
    /// as `hovered_wt`: the row has no press action of its own, so
    /// `button::Status` can't supply the hover state.
    pub(in crate::gui) hovered_archived: Option<usize>,
    /// True while the workspace/focus is showing a home terminal rather than
    /// the tree's agent sessions. Flipped by `SelectHomeTerminal` and cleared
    /// by `leave_terminal_tab()`.
    pub(in crate::gui) terminal_focused: bool,
    /// Whether the right-docked terminal slide-over panel is open. The panel
    /// belongs to the active session's worktree; toggled by the `term` button
    /// in the session header. Closing it leaves the worktree's shells alive
    /// (they reattach when reopened); they only die when the worktree is
    /// removed.
    pub(in crate::gui) term_panel_open: bool,
    /// Whether the docked home-terminals section of the sidebar tree is
    /// collapsed. Session-only (not persisted, same as `collapsed`/
    /// `collapsed_wt`); toggled by `Msg::ToggleTerminalsSection`.
    pub(in crate::gui) terminals_collapsed: bool,
    /// The terminal panel's share of the workspace width, in percent (the agent
    /// view gets `100 - term_panel_portion`). Adjusted live with
    /// Ctrl+Shift+Left/Right between `TERM_PANEL_PORTION_MIN` and
    /// `TERM_PANEL_PORTION_MAX`; drives both the `FillPortion` weights in
    /// `view()` and each PTY's derived column count.
    pub(in crate::gui) term_panel_portion: u16,
    /// Which PTY (agent vs panel) input is routed to while the panel is open.
    /// Only consulted when `term_panel_open`; defaults to `Panel` on open and
    /// resets to `Agent` on close or active-session change. Clicking either PTY
    /// updates it.
    pub(in crate::gui) focused_pane: FocusedPane,
    /// Memoized `list_dirs` result for the add-project path modal, keyed by the
    /// input buffer. `view()` runs every tick; without this the modal would hit
    /// the filesystem (`read_dir`) on every frame.
    pub(in crate::gui) dir_cache: std::cell::RefCell<Option<(String, Vec<String>)>>,
    /// True while a native folder-picker dialog is open (add-project /
    /// onboarding "Browse…"). Guards against spawning a second dialog and
    /// dims the Browse button.
    pub(in crate::gui) picker_open: bool,
    /// Per-session activity trackers, keyed by `Session::id` (never reused,
    /// unlike Arc pointer addresses). Refreshed every ~480ms by `Msg::Tick`;
    /// stale keys are pruned on the same pass.
    pub(in crate::gui) activity: HashMap<u64, super::activity::Tracker>,
    /// Background poller for `claude agents --json`, the most authoritative
    /// available signal for a live Claude session's status when it's
    /// supported (see `claude_agents`). Shared across all sessions — one
    /// poll per tick, not one per session. `refresh_activity` consults it
    /// ahead of the hook-state-file and heuristic fallbacks.
    pub(in crate::gui) claude_poller: grove_core::claude_agents::Poller,
    /// Whether the OS window currently has focus — gates the dock bounce.
    pub(in crate::gui) window_focused: bool,
    /// Last dock badge value pushed, to avoid redundant objc calls.
    pub(in crate::gui) last_badge: usize,
    /// Sidebar width in logical pixels. Driven by dragging the divider, clamped
    /// to `[SIDEBAR_MIN_W, window cap]`, persisted to `Store.sidebar_width`.
    pub(in crate::gui) sidebar_width: f32,
    /// Every in-progress pointer drag: sidebar divider, terminal-panel
    /// divider, and grid tile reorder.
    pub(in crate::gui) drag: DragState,
    pub(in crate::gui) grid_view: bool,
    /// Session indices in display order. Built on grid entry; kept in sync
    /// as sessions spawn or die while the grid is open.
    pub(in crate::gui) tile_order: Vec<usize>,
    /// Session index with keyboard focus (`app.sessions[i]`). `None` until
    /// a tile is clicked. All keystrokes route here while set.
    pub(in crate::gui) grid_focused: Option<usize>,
    /// True when zen was entered from grid view; exiting zen re-enters grid.
    pub(in crate::gui) grid_view_before_zen: bool,
    /// True when the terminal tab was entered from grid view; leaving the
    /// terminal (mod+`) re-enters grid. Set only by the paths that swap the
    /// grid out for a terminal — a plain mod+g is not the toggle.
    pub(in crate::gui) grid_view_before_terminal: bool,
    /// Live state for the per-project lifecycle-scripts editor, when open.
    /// `Some` exactly when `app.modal` is `Modal::ScriptsEditor`.
    pub(in crate::gui) scripts_editor: Option<super::scripts_editor::ScriptsEditorState>,
    /// Live state for the two-step add-project wizard, when open. `Some`
    /// exactly when `app.modal` is `Modal::AddProject` — same idiom as
    /// `scripts_editor`.
    pub(in crate::gui) add_project: Option<super::add_project::AddProjectState>,
    /// Live state for the command palette / session launcher, when open.
    /// `Some` exactly when `app.modal` is `Modal::SessionLauncher` — same
    /// idiom as `scripts_editor`.
    pub(in crate::gui) launcher: Option<super::session_launcher::LauncherState>,
    /// Live state for `Modal::ThemeManager`'s EDITOR sub-view, when open.
    /// `Some` exactly when the modal is showing the editor rather than the
    /// list — same idiom as `scripts_editor`.
    pub(in crate::gui) theme_manager_editor:
        Option<super::theme_manager_editor::ThemeManagerEditorState>,
    /// Per-tool install/version status shown in the Settings → Tools section.
    /// Parked on the model (like `scripts_editor`) because detection runs
    /// asynchronously and posts results back via `Msg::ToolVersionsDetected`.
    /// Empty until Settings is first opened.
    pub(in crate::gui) settings_tools: Vec<ToolStatus>,
    /// Current self-update state — drives the Updates UI and badge.
    pub(in crate::gui) upgrade: UpgradeState,
    /// State for the changelog modal.
    pub(in crate::gui) changelog: ChangelogState,
    /// When true, the changelog modal is shown over the normal view.
    pub(in crate::gui) show_changelog: bool,
    /// How Grove was installed (homebrew, cargo, etc.) — determines the update command.
    pub(in crate::gui) upgrade_method: grove_core::upgrade::InstallMethod,
    /// Written by the apply thread, drained on `Tick` to drive `UpgradeState`.
    pub(in crate::gui) upgrade_progress: std::sync::Arc<std::sync::Mutex<UpgradeProgress>>,
    /// Latest git status (dirty/ahead/behind) per worktree, keyed by worktree
    /// path. Written by a background thread spawned on the throttled poll in
    /// `Msg::Tick`; read directly (no message round-trip) by `tree_view` when
    /// rendering each worktree row's suffix. A missing key means "no signal"
    /// (never polled yet, or the last poll failed) and renders no suffix.
    pub(in crate::gui) git_state:
        std::sync::Arc<std::sync::Mutex<HashMap<String, grove_core::git::WorktreeGitState>>>,
    /// When the last git-status poll was kicked off, for the ~5s throttle in
    /// `Msg::Tick`. `None` before the first poll.
    pub(in crate::gui) last_git_poll: Option<std::time::Instant>,
    /// Set while a git-status poll thread is running; guards against
    /// spawning an overlapping poll if the previous one is still in flight.
    pub(in crate::gui) git_poll_inflight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A `rebuild_wt_cache` was requested and its off-thread `git worktree
    /// list` sweep has not been kicked off yet. Collapses a burst of requests
    /// inside one tick into a single sweep.
    pub(in crate::gui) wt_rebuild_pending: bool,
    /// Set while an off-thread worktree sweep is running; guards against
    /// overlapping sweeps, mirroring `git_poll_inflight`. Plain `bool` (not
    /// an atomic) because it is only ever touched from `update()`.
    pub(in crate::gui) wt_rebuild_inflight: bool,
    /// Monotonic generation of `wt_cache`'s index space, bumped by
    /// `rebuild_wt_cache` (the single point where the cache is invalidated —
    /// every add / remove / archive / restore path routes through it).
    ///
    /// Each off-thread sweep is stamped with the generation it was launched
    /// under and `on_wt_cache_rebuilt` drops the whole batch on a mismatch.
    /// Without it, a sweep that raced a project *removal* would fold its
    /// results onto shifted indices — every entry after the removed project
    /// naming a different project than the one actually swept, while still
    /// being in range. Deliberately not a length comparison: filtering the
    /// fan-out to active projects makes lengths differ legitimately.
    pub(in crate::gui) wt_cache_gen: u64,
    /// Live modifier-key state, tracked via `Msg::ModifiersChanged` — see
    /// its doc comment for why this needs its own channel rather than
    /// reading `mods` off `Msg::KeyPress`.
    pub(in crate::gui) live_mods: Modifiers,
}

/// PTY geometry and GUI zoom. Every field here is derived from the window
/// size (plus the terminal-panel split) and recomputed together by
/// `refresh_pty_viewport` — grouped so that relationship is visible.
pub struct PtyLayout {
    /// Current PTY row count derived from window size. Updated on every
    /// `WindowResized` event and applied to sessions on spawn / select.
    pub(in crate::gui) rows: u16,
    /// Full workspace column count (panel closed) — used by the home-terminal
    /// tab and as the fallback width.
    pub(in crate::gui) cols: u16,
    /// Agent-view column count: equals `cols` when the slide-over terminal
    /// panel is closed, the 65% share when it is open.
    pub(in crate::gui) sess_cols: u16,
    /// Terminal-panel column count: the 35% share used while the panel is open.
    pub(in crate::gui) panel_cols: u16,
    /// Per-window GUI zoom multiplier. Applied as the iced application scale
    /// factor and reused when deriving PTY rows/cols from the visible area.
    pub(in crate::gui) zoom: f32,
    /// Countdown (in `Msg::Tick`s) until the debounced `zoom` disk write
    /// fires; `None` when there is no pending write. Reset to
    /// `ZOOM_SAVE_QUIET_TICKS` on every zoom change so a continuous pinch or
    /// held keyboard shortcut only writes once, ~250ms after the last event.
    /// See `set_ui_zoom` and the `Msg::Tick` handler.
    pub(in crate::gui) zoom_save_countdown: Option<u8>,
    /// Last known window size in unzoomed units so zoom changes can recompute
    /// PTY dimensions without waiting for another resize event.
    pub(in crate::gui) window_size: Size,
}

/// Every in-progress pointer drag. All three are mutually exclusive in
/// practice (one pointer), and all three are fed by the same global mouse
/// subscription while live.
#[derive(Clone, Copy, Debug, Default)]
pub struct DragState {
    /// Active sidebar-divider drag, if the left button is held over the resize
    /// handle. While set, a global mouse subscription feeds cursor moves and
    /// the button-release that ends the drag.
    pub(in crate::gui) sidebar_drag: Option<SidebarDrag>,
    /// Timestamp of the last sidebar-divider press, for double-click reset
    /// detection.
    pub(in crate::gui) last_divider_press: Option<std::time::Instant>,
    /// Whether the terminal-panel split divider is being dragged. While true, a
    /// global mouse subscription feeds cursor moves and the button-release.
    pub(in crate::gui) term_panel_dragging: bool,
    /// Timestamp of the last terminal-panel divider press, for double-click
    /// reset detection.
    pub(in crate::gui) last_term_divider_press: Option<std::time::Instant>,
    /// Active grid tile-reorder drag.
    pub(in crate::gui) grid_drag: Option<GridDrag>,
}

/// Animation clocks and counters. Every one of these is idle (zero redraw
/// cost) until something asks for it.
pub struct Animations {
    /// Monotonically incrementing counter driven by `Msg::Tick` (~30 Hz).
    /// Used to compute cursor blink state: visible when `blink_tick % 30 < 15`
    /// (≈ 500 ms on / 500 ms off).
    pub(in crate::gui) blink_tick: u32,
    /// Needs-attention opacity pulse. Idle (`false`, zero redraw cost) until a
    /// session enters `WaitingForInput`; then a repeating auto-reversed
    /// `Animation` drives the amber glyph/scrim alpha via per-frame redraws
    /// (`window::frames()`), instead of the old `blink_tick` triangle waves.
    pub(in crate::gui) attention_anim: iced::animation::Animation<bool>,
    /// Onboarding wizard step-transition animation: quick (200ms, `EaseOut`)
    /// fade + ≤8px slide-up played whenever the wizard's step changes (and on
    /// first show). Restarted via `go_mut(true, ..)` from a fresh idle
    /// instance each time, mirroring `attention_anim`'s shape. Idle (not
    /// animating) costs nothing — gated into the same `frames()` subscription
    /// as `attention_anim`.
    pub(in crate::gui) onb_step_anim: iced::animation::Animation<bool>,
    /// In-flight tile-slide animation: post-swap tile-order indices of the two
    /// swapped tiles, each with the (col, row) cell delta it travelled, plus
    /// when the slide started. Drives a draw-only offset in `grid_workspace`.
    pub(in crate::gui) grid_slide: Option<GridSlide>,
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

/// Active tile-reorder drag in grid view.
#[derive(Clone, Copy, Debug)]
pub struct GridDrag {
    /// Index into `tile_order` of the tile being dragged.
    pub source_idx: usize,
    /// Index into `tile_order` of the tile currently under the cursor.
    pub hover_idx: usize,
}

/// Draw-only tile-slide animation state for a just-completed grid reorder.
/// `tiles` holds `(tile_order_idx, d_col, d_row)` for the two swapped tiles,
/// where the delta is FROM-cell minus TO-cell (where the tile came from, in
/// grid cells) so the offset shrinks to zero as the slide progresses.
#[derive(Clone, Copy, Debug)]
pub struct GridSlide {
    pub tiles: [(usize, i32, i32); 2],
    pub start: std::time::Instant,
}

/// Identifies which on-screen PTY a mouse event originated from. The home
/// terminal tab and the single full-width agent view both use `Agent`; only the
/// right-docked slide-over panel uses `Panel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyPane {
    Agent,
    Panel,
    /// Grid-view tile; carries the index into `app.sessions`.
    Tile(usize),
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
    /// Canvas cache for the blinking cursor block, kept separate from `cache`
    /// so a blink (which flips twice a second) doesn't invalidate the whole
    /// screen's geometry — and so a *steady* cursor costs nothing at all.
    pub cursor_cache: Arc<canvas::Cache>,
    /// The `(cursor_pos, cursor_visible)` pair `cursor_cache` was last drawn
    /// for. `cursor_cache` is cleared only when this changes.
    pub cursor_key: (Option<(u16, u16)>, bool),
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
    pub stage: Option<grove_core::upgrade::Stage>,
    pub finished: Option<Result<(), String>>,
}

/// Drives the changelog modal.
#[derive(Debug, Clone)]
pub enum ChangelogState {
    Idle,
    Loading,
    Loaded(Vec<grove_core::upgrade::ReleaseNote>),
    Error(String),
}

/// Drives the Updates UI. `Available` carries the resolved release; the apply
/// states drive the progress modal.
#[derive(Debug, Clone)]
pub enum UpgradeState {
    Idle,
    Checking,
    UpToDate,
    Available(grove_core::upgrade::Release),
    Error(String),
    Updating(grove_core::upgrade::Stage),
    Updated,
    UpdateFailed(String),
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    /// A `window::frames()` redraw fired while the attention pulse is active.
    /// Carries no state changes — the message itself schedules the next
    /// frame so the animation keeps interpolating.
    AnimationFrame,
    /// OS window gained/lost focus (drives dock-bounce gating and
    /// implicit acknowledgment of the visible session).
    WindowFocusChanged(bool),
    WindowResized(Size),
    /// An off-thread `git worktree list` sweep finished: one worktree list per
    /// swept project, each paired with its TRUE `store.projects` index (the
    /// sweep covers active projects only, so positions are not indices), plus
    /// the `wt_cache_gen` the sweep was launched under so
    /// `on_wt_cache_rebuilt` can tell whether those indices still mean what
    /// they meant at launch. Folded into `wt_cache` by `on_wt_cache_rebuilt`.
    WtCacheRebuilt {
        generation: u64,
        lists: Vec<(usize, Vec<Worktree>)>,
    },
    /// The OS asked to close the window (exit_on_close_request is off; grove
    /// decides whether running native sessions warrant a confirm first).
    CloseRequested(iced::window::Id),
    /// Open the keyboard-shortcuts overlay (status-bar chip / cmd+/).
    OpenShortcutOverlay,
    BackendNative,
    BackendTmux,
    SkipPermissionsEnable,
    SkipPermissionsDisable,
    /// Toggled the "share anonymous usage data" checkbox in settings.
    TelemetryToggle(bool),
    /// Toggled the "let Claude control Chrome" checkbox in settings.
    ChromeToggle(bool),
    /// Toggle the universal "Project themes" setting (Settings → Appearance).
    ProjectThemesToggle(bool),
    ProjectClicked(usize),
    /// Toggle button in the sidebar tree header. Collapses everything except
    /// worktrees that currently contain sessions, or — if already in that
    /// state — expands every project and worktree.
    ToggleCollapseAll,
    /// Toggle collapse of the docked TERMINALS section at the bottom of the
    /// sidebar tree.
    ToggleTerminalsSection,
    /// Relaunch the active home terminal's shell at `~` (e.g. after it exited).
    RestartHomeTerminal,
    /// Spawn an additional home terminal and focus it.
    NewHomeTerminal,
    /// Focus the home terminal at this index.
    SelectHomeTerminal(usize),
    /// Arm the close-confirm for the home terminal at this index (first
    /// press/click of the two-step confirm — mirrors `RequestKillSession`).
    RequestCloseHomeTerminal(usize),
    /// Close the home terminal at this index (the confirmed action — mirrors
    /// `KillSession`). Also reachable directly, e.g. from the second mod+w
    /// press once armed.
    CloseHomeTerminal(usize),
    WorktreeClicked {
        proj: usize,
        wt: usize,
    },
    HoverWorktree(Option<(usize, usize)>),
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
    /// Toggle the appbar's attention-queue dropdown open/closed.
    ToggleAttentionQueue,
    /// Close the appbar's attention-queue dropdown.
    CloseAttentionQueue,
    /// Select the first session currently waiting for input, in tree order.
    JumpToWaitingSession,
    SelectSession(usize),
    KillSession(usize),
    RequestKillSession(usize),
    /// `(key, modified_key, modifiers)`. `key` is layout-aware but
    /// modifier-independent (so Ctrl shortcuts match across platforms);
    /// `modified_key` carries Shift/AltGr for text entry.
    KeyPress(Key, Key, Modifiers),
    /// Live modifier-key state, tracked independently of `KeyPress` (see
    /// `Grove::subscription`'s doc comment) so `Msg::SessionLauncher(session_launcher::Msg::InputChanged)`
    /// can recognize a `global_mods` chord the focused search field doesn't
    /// special-case and ignore the stray edit it would otherwise produce.
    ModifiersChanged(Modifiers),
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
    /// Two-step add-project wizard messages, delegated to
    /// `add_project::update`/`handle_key` (with `add_project::Msg::Open`
    /// intercepted by the parent — see that module's `Msg` doc comment).
    AddProject(super::add_project::Msg),
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
    /// Scripts-editor messages, delegated to `scripts_editor::update` (with
    /// `OpenProjectThemePicker` intercepted by the parent — see that
    /// module's `Msg` doc comment).
    Scripts(super::scripts_editor::Msg),
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
    /// Live edit of the add-worktree input modal's text field.
    InputPathChanged(String),
    // ── two-step add-project modal's native-picker round trip ────────────
    // Shared with the onboarding wizard's project step (`gui::onboarding`),
    // which reuses this exact same "Browse…" affordance — see
    // `add_project::Msg`'s doc comment for why these two stay top-level
    // rather than folding into `add_project::Msg`.
    /// "Browse…" clicked: open the native folder picker off-thread.
    AddProjectBrowse,
    /// Native picker resolved; `None` means the user cancelled.
    AddProjectPicked(Option<std::path::PathBuf>),
    ChooseTmux(bool),
    AgentPickerSelect(usize),
    AgentPickerToggleDefault,
    AgentPickerSubmit,
    /// Open the Settings modal (cog in the appbar).
    OpenSettings,
    /// Set (or clear, if re-selecting the current) the global default agent
    /// from the Settings Tools section.
    SetDefaultAgent(Agent),
    /// Re-run tool install/version detection for the Settings Tools section.
    RefreshTools,
    /// Async detection finished; carries the fresh per-tool statuses.
    ToolVersionsDetected(Vec<(Agent, ToolStatus)>),
    /// Update-check / changelog / self-upgrade messages, handled by
    /// `Grove::on_upgrade`.
    Upgrade(UpgradeMsg),
    /// Theme-picker modal messages, handled by `Grove::on_theme_picker`.
    ThemePicker(ThemePickerMsg),
    /// The OS light/dark setting changed (or was queried at startup). Always
    /// subscribed; only affects the active theme while "follow system" is on.
    SystemThemeChanged(iced::theme::Mode),
    /// First-run onboarding wizard messages, handled by
    /// `Grove::on_onboarding`.
    Onboarding(OnboardingMsg),
    ToggleGridView,
    /// Agent-grid tile drag-and-drop reordering, handled by
    /// `Grove::on_grid_drag`.
    GridDrag(GridDragMsg),
    /// ⤢ expand clicked in tile header: enter zen for this session and
    /// remember to return to grid on exit.
    GridTileZen(usize),

    // ── Command palette (session launcher) ───────────────────────────────
    /// Every message the command palette can emit (open, search, activate,
    /// options/switch/row-actions/settings drill-in, theme sub-pane, update-
    /// actions strip), delegated to a nested match in `Grove::update`'s
    /// `Msg::SessionLauncher` arm. Unlike `ThemeManagerEditor`/`AddProject`,
    /// there is no single free-fn `session_launcher::update` to delegate the
    /// bulk of these to — see `gui::session_launcher`'s module doc comment
    /// for why (the palette reaches deep into `Grove`-only state throughout,
    /// not just at one or two call sites), so every variant is matched
    /// individually at that one arm instead.
    SessionLauncher(super::session_launcher::Msg),
    /// Theme-manager modal messages, handled by `Grove::on_theme_manager`.
    ThemeManager(ThemeManagerMsg),
    /// Project archive/restore messages, handled by `Grove::on_archive`.
    Archive(ArchiveMsg),
}

/// Archive/restore messages (`Modal::ArchiveProject` and
/// `Modal::ArchivedProjects`), dispatched as a family from `Grove::update`
/// into `Grove::on_archive`.
///
/// Every `usize` here is a TRUE index into `store.projects` (as yielded by
/// `Store::active_projects`/`archived_projects`), never a position within a
/// filtered sequence — a renumbered index would archive or delete the wrong
/// project.
#[derive(Debug, Clone)]
pub enum ArchiveMsg {
    /// Kill every session of the gated project via the shared
    /// `App::kill_sessions_for_project`, then recompute the gate.
    KillSessions,
    /// Archive the gated project. A no-op while the gate is still blocked —
    /// the same precondition the disabled Archive button encodes.
    Confirm,
    /// Open the archived-projects list (Settings → Archived projects).
    OpenList,
    /// Un-archive the project at this TRUE index.
    Restore(usize),
    /// Route "delete permanently" into the EXISTING remove-project confirm
    /// flow for this TRUE index. There is one destructive path, not two.
    Delete(usize),
    /// Close the archived-projects list.
    CloseList,
    /// Mouse entered/left an archived-projects row (TRUE index), driving that
    /// row's hover fill.
    Hover(Option<usize>),
}

/// Theme-manager modal messages (`Modal::ThemeManager`), dispatched as a
/// family from `Grove::update` into `Grove::on_theme_manager`.
#[derive(Debug, Clone)]
pub enum ThemeManagerMsg {
    /// Opens `Modal::ThemeManager` (the palette's "Manage themes…" row / ⌘M).
    Open,
    /// `Modal::ThemeManager` LIST view: click (or the equivalent of hover) on
    /// row `i` — just moves the highlight, mirroring `session_launcher::Msg::ThemePaneSelect`.
    Select(usize),
    /// Begins inline rename of the custom theme at row `i`.
    RenameStart(usize),
    /// Live edit of the inline rename buffer.
    RenameChanged(String),
    /// Commits the inline rename buffer via `theme::rename_custom`.
    RenameSubmit,
    /// Discards the inline rename buffer without renaming.
    RenameCancel,
    /// Duplicates the custom theme at row `i` (auto-named via
    /// `theme::duplicate_name`) and selects the copy.
    Duplicate(usize),
    /// Opens the "Delete theme…?" confirmation for row `i`.
    DeleteStart(usize),
    /// Confirms the pending delete, removing the custom theme via
    /// `theme::delete_custom` (falling back to the mode default if it was
    /// the active theme).
    DeleteConfirm,
    /// Cancels the pending delete confirmation.
    DeleteCancel,
    /// "New theme": creates a fresh custom theme seeded from the current
    /// active theme's mode default, auto-named via
    /// `theme::duplicate_name("untitled")`, then opens the EDITOR sub-view
    /// on it directly (same landing as `ThemeManagerEditStart`).
    New,
    /// Closes `Modal::ThemeManager` ("Done" button / Esc from the list).
    Close,
    /// Every message the theme-manager EDITOR sub-view (list row's Edit
    /// button, row select, hex/name/kind edits, preview, save, discard, and
    /// the paste box) can emit, delegated to `theme_manager_editor::update`
    /// (with `theme_manager_editor::Msg::Edit` intercepted by the parent —
    /// see that module's `Msg` doc comment).
    Editor(super::theme_manager_editor::Msg),
}

/// Theme-picker modal messages, dispatched as a family from `Grove::update`
/// into `Grove::on_theme_picker`.
#[derive(Debug, Clone)]
pub enum ThemePickerMsg {
    /// Open the theme picker (Settings → Appearance).
    Open,
    SwitchTab,
    Select(usize),
    /// Project-scoped theme picker only: select the "Default (follow app)"
    /// row, pinning nothing.
    SelectDefault,
    /// Toggle the "follow system appearance" checkbox in the theme picker.
    ToggleSystem(bool),
    Submit,
    Cancel,
}

/// First-run onboarding wizard messages, dispatched as a family from
/// `Grove::update` into `Grove::on_onboarding`.
#[derive(Debug, Clone)]
pub enum OnboardingMsg {
    /// Advance one step. On the project step this registers the project first;
    /// on the theme step it persists the previewed theme; on the session step it
    /// finishes setup and launches the chosen agent.
    Next,
    /// Step back one step.
    Back,
    /// Skip the rest of setup; marks onboarded and restores the pre-preview theme.
    Skip,
    /// Live edit of the project-step path field.
    PathChanged(String),
    /// Live edit of the project-step name field.
    NameChanged(String),
    /// Clicked a directory match in the project step.
    PickDir(String),
    /// Select the agent at this index in the session step.
    AgentSelect(usize),
    /// Select the permissions mode (true = skip prompts) on the session step.
    PermsSelect(bool),
}

/// Update-check, changelog, and self-upgrade messages, dispatched as a family
/// from `Grove::update` into `Grove::on_upgrade`.
#[derive(Debug, Clone)]
pub enum UpgradeMsg {
    /// Open the changelog modal (fetches releases off-thread).
    OpenChangelog,
    /// Off-thread release-note fetch completed; carries the notes or an error string.
    ChangelogLoaded(Result<Vec<grove_core::upgrade::ReleaseNote>, String>),
    /// Close the changelog modal and return to the Settings modal.
    CloseChangelog,
    /// Trigger an off-thread update check. `manual: true` = user-initiated (surfaces
    /// errors inline); `manual: false` = launch/periodic (fails silently, log only).
    CheckForUpdates { manual: bool },
    /// Off-thread check completed; carries the fetched release or an error string.
    /// The `bool` mirrors the `manual` flag from the originating `CheckForUpdates`.
    UpdateCheckResult(Result<grove_core::upgrade::Release, String>, bool),
    /// User chose to skip the available release version.
    SkipVersion,
    /// Copy the available release's GitHub URL to the clipboard.
    CopyReleaseUrl,
    /// User confirmed they want to apply the update.
    StartUpdate,
    /// Restart the app after a successful update.
    RestartApp,
}

/// Agent-grid tile drag-and-drop reordering, dispatched as a family from
/// `Grove::update` into `Grove::on_grid_drag`.
#[derive(Debug, Clone)]
pub enum GridDragMsg {
    /// Tile header was pressed; starts a drag and focuses the tile.
    /// Argument is an index into `tile_order`.
    Start(usize),
    /// Cursor entered a tile while a drag is live.
    /// Argument is an index into `tile_order`.
    Hover(usize),
    /// Left button released: commit the drag (insert at hover slot if source ≠ hover).
    End,
}
