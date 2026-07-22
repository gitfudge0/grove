use crate::agent::Agent;
use crate::git::{self, Worktree};
use crate::session::Session;
use crate::storage::{self, Project, Store};
use crate::theme;
use crate::tmux;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// Fallback dark/light themes for "system" mode when the user hasn't picked
/// one explicitly yet (thematic pair: `tokyonight` and its day companion).
const DEFAULT_DARK_THEME: &str = "tokyonight";
const DEFAULT_LIGHT_THEME: &str = "tokyonight-day";

/// Which theme the picker modal is editing: the global app theme, or a
/// single project's pinned "Project theme" (see `Store::project_themes_enabled`
/// / `storage::Project::theme`). Project scope never touches the global
/// active theme — hovering/selecting only updates local picker state, and
/// submitting writes `Project::theme` instead of `theme::set` + `Store::theme`.
#[derive(Clone, PartialEq)]
pub enum ThemePickerScope {
    App,
    /// Keyed by project name (unique, like the rest of this feature) rather
    /// than index — indices can shift under project add/remove while a
    /// picker is theoretically open, names don't.
    Project(String),
}

pub fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i32 + delta).rem_euclid(len as i32) as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub created: std::time::Instant,
}

impl Toast {
    /// How long a toast stays up before auto-dismissing: errors linger
    /// twice as long as informational messages.
    pub fn ttl(kind: ToastKind) -> std::time::Duration {
        match kind {
            ToastKind::Info => std::time::Duration::from_secs(4),
            ToastKind::Error => std::time::Duration::from_secs(8),
        }
    }

    /// Whether the toast should be dismissed as of `now`. Pure so expiry is
    /// unit-testable without waiting.
    pub fn expired_at(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.created) >= Self::ttl(self.kind)
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum Pane {
    Projects,
    Worktrees,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveBackend {
    Native,
    Tmux,
}

pub fn effective_backend_for(tmux_available: bool, tmux_enabled: Option<bool>) -> EffectiveBackend {
    if tmux_available && tmux_enabled == Some(true) {
        EffectiveBackend::Tmux
    } else {
        EffectiveBackend::Native
    }
}

pub fn needs_tmux_choice(tmux_available: bool, tmux_enabled: Option<bool>) -> bool {
    tmux_available && tmux_enabled.is_none()
}

/// The one-time modal (if any) to show on launch, in priority order: the
/// first-run onboarding wizard takes precedence over the tmux/native choice
/// (the wizard's environment step already surfaces tmux), which in turn only
/// appears once. Pure so the precedence is unit-testable without iced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstRunModal {
    Onboarding,
    TmuxChoice,
    None,
}

pub fn first_run_modal(
    onboarded: bool,
    tmux_available: bool,
    tmux_enabled: Option<bool>,
) -> FirstRunModal {
    if !onboarded {
        FirstRunModal::Onboarding
    } else if needs_tmux_choice(tmux_available, tmux_enabled) {
        FirstRunModal::TmuxChoice
    } else {
        FirstRunModal::None
    }
}

/// The ordered steps of the first-run onboarding wizard. `Welcome` orients the
/// user, `Environment` reports detected tools, `Project` registers the first
/// project, and `Session` launches the first agent. The flow no longer varies
/// with tmux availability — new sessions default to tmux when it's present,
/// native otherwise (set on completion in [`App::onboard_finish`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardStep {
    Welcome,
    Environment,
    Project,
    Session,
}

impl OnboardStep {
    pub const ALL: [OnboardStep; 4] = [
        OnboardStep::Welcome,
        OnboardStep::Environment,
        OnboardStep::Project,
        OnboardStep::Session,
    ];

    /// The wizard's fixed step sequence.
    pub fn flow() -> &'static [OnboardStep] {
        &Self::ALL
    }

    pub fn index_in(self) -> usize {
        Self::flow().iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<OnboardStep> {
        Self::flow().get(self.index_in() + 1).copied()
    }

    pub fn prev(self) -> Option<OnboardStep> {
        self.index_in().checked_sub(1).map(|i| Self::flow()[i])
    }

    /// Short label shown in the progress rail.
    pub fn label(self) -> &'static str {
        match self {
            OnboardStep::Welcome => "welcome",
            OnboardStep::Environment => "environment",
            OnboardStep::Project => "project",
            OnboardStep::Session => "session",
        }
    }
}

/// Build the initial onboarding modal.
pub fn onboarding_modal() -> Modal {
    Modal::Onboarding {
        step: OnboardStep::Welcome,
        path: String::new(),
        dir_sel: 0,
        name: None,
        note: None,
        added_proj: None,
        agent_sel: 0,
        perms_skip: false,
        name_focused: false,
    }
}

#[derive(Clone)]
pub enum Modal {
    None,
    /// Single-field text prompt; today only the worktree-name input.
    Input {
        title: String,
        buffer: String,
        /// Inline validation message, shown in red under the field. Cleared on
        /// the next edit.
        note: Option<String>,
    },
    Confirm {
        title: String,
        prompt: String,
        destructive: bool,
        kind: ConfirmKind,
    },
    /// Two-step add-project flow. Step 1 (`PickSource`) offers the native
    /// folder picker, drag-and-drop, and a typed path with tab-completion;
    /// step 2 (`Details`) shows the chosen folder, the name, the upfront git
    /// probe, and the init-git choice inline.
    /// Nothing is persisted until the final submit.
    AddProject {
        step: AddProjectStep,
        /// Step 1: the typed path buffer. Step 2: the canonicalized folder.
        path: String,
        /// Directory-match cursor for the step-1 autocomplete list.
        dir_sel: usize,
        /// Project-name override. Left empty, the folder basename is used
        /// (shown as the field's placeholder). Edits survive a round-trip
        /// through "change".
        name: String,
        git: GitProbe,
        /// "Initialize git repository" checkbox (meaningful when `NotRepo`).
        init_git: bool,
        /// Inline validation message, cleared on the next edit.
        note: Option<String>,
    },
    /// Two-stage project removal: confirmation (with an optional checkbox to
    /// also delete worktrees on disk) followed by a progress view while the
    /// worktrees are torn down.
    RemoveProject {
        idx: usize,
        name: String,
        project_path: String,
        /// Non-main worktree paths discovered when the modal opened.
        worktrees: Vec<String>,
        also_remove_worktrees: bool,
        in_progress: bool,
        done: usize,
        current: String,
        errors: Vec<String>,
    },
    Message(String),
    TmuxChoice,
    AgentPicker {
        project: String,
        wt_path: String,
        sel: usize,
    },
    /// Recents-first command palette (Agent View "+ New session", mod+n, grid
    /// pill). Three states, all driven by this payload:
    /// - root: `input` empty and `browse_all` false — recents + actions list.
    /// - typing/browse-all: `input` non-empty OR `browse_all` true — every
    ///   project×worktree combo, fuzzy-filtered by `input` (unfiltered if empty
    ///   and `browse_all` is what got us here via "+ new session…").
    /// - options: `options` is `Some` — a plain list of agents to launch the
    ///   selected row's project/worktree with.
    SessionLauncher {
        input: String,
        /// Selected row index into whatever list is currently rendered (root's
        /// combined recents+actions list, or the typing/browse-all list).
        selected: usize,
        /// Identity of the row at `selected`, captured whenever `selected` is
        /// written (see `PaletteRowIdentity`) — activation resolves by this,
        /// not by `selected` alone. `None` only defensively, for states this
        /// field predates having always been threaded through.
        selected_identity: Option<PaletteRowIdentity>,
        /// Set once the root "+ new session…" action row is activated: forces
        /// the unfiltered every-combo list even while `input` is empty.
        browse_all: bool,
        options: Option<LauncherOptions>,
        /// "Switch to session…" drill-in: `Some(selected)` lists every active
        /// session (index into `App::sessions`-derived display order = the
        /// selection cursor within that list). `None` outside this state.
        switch: Option<usize>,
        /// The `id` of the session at `switch`'s position, captured whenever
        /// `switch` is written — same principle as `selected_identity`,
        /// applied to this drill-in's own list (`Session::id` is the stable
        /// key here rather than `PaletteRowIdentity`, since a switch-drill-in
        /// row already *is* a session, with its own never-reused id).
        switch_identity: Option<u64>,
        /// Inline contextual actions revealed by Tab under a highlighted
        /// `Recent`/`Combo` row (root or typing/browse-all list). `None` when
        /// no action strip is open.
        row_actions: Option<RowActionsState>,
        /// "Settings…" drill-in: `Some` shows the scoped settings list in
        /// place of the root/typing list. Entered via the root "Settings…"
        /// action row (Enter or Tab); `None` outside this state.
        settings: Option<LauncherSettings>,
    },
    ThemePicker {
        sel_dark: usize,
        sel_light: usize,
        tab: crate::theme::ThemeKind,
        original: crate::theme::Theme,
        /// When true, closing the picker (apply or cancel) reopens
        /// `Modal::Settings` instead of `Modal::None` — the picker was entered
        /// from the Settings Appearance section.
        return_to_settings: bool,
        /// Whether the "follow system appearance" checkbox is checked. When
        /// true, submitting sets `App::theme_follow_system` instead of
        /// pinning the selected list entry.
        follow_system: bool,
        /// Whether this picker edits the global app theme or one project's
        /// pinned "Project theme".
        scope: ThemePickerScope,
        /// Project scope only: the "Default (follow app)" row is selected
        /// (equivalent to `Project::theme == None`). Picking a concrete
        /// theme from the list clears this.
        project_use_default: bool,
    },
    /// The consolidated Settings modal (appearance, terminal, tools). Opened
    /// from the appbar cog. All controls persist immediately; there is no
    /// apply/cancel footer.
    Settings,
    /// Lightweight keyboard-shortcut reference (mod+/). Esc or mod+/ closes.
    ShortcutOverlay,
    /// Worktree teardown: runs the project's teardown script (if any) in a
    /// modal-embedded PTY, then performs `git worktree remove`. The live PTY
    /// session and stage live in `App::teardown`.
    Teardown,
    /// Per-project lifecycle-scripts editor. The editable buffers and target
    /// project live in the GUI model (`Grove::scripts_editor`); this just marks
    /// the modal open.
    ScriptsEditor,
    /// Apply-in-progress overlay. Mirrors the one-deep modal pattern; the
    /// live stage is tracked in `Grove.upgrade` and polled every `Msg::Tick`.
    Updating,
    /// First-run onboarding wizard. Self-contained, multi-step state: the
    /// project step mirrors the add-project path input (`path`/`dir_sel`/`name`/
    /// `note`), and the session step tracks the agent selection. `added_proj`
    /// is the index of the project registered during the wizard, so the
    /// session step can launch into it.
    Onboarding {
        step: OnboardStep,
        path: String,
        dir_sel: usize,
        name: Option<String>,
        note: Option<String>,
        added_proj: Option<usize>,
        agent_sel: usize,
        /// Session-step permissions selection: `true` = skip permission
        /// prompts. Persisted as an explicit store value on finish; "safe"
        /// (`false`) is preselected.
        perms_skip: bool,
        /// Project step only: Tab alternates keyboard focus between the path
        /// and name fields. `false` = path (the default on entering the step).
        name_focused: bool,
    },
}

/// The command palette's "options" state: a resolved (project, worktree)
/// selection with a plain list of agents to pick from before launch.
#[derive(Clone)]
pub struct LauncherOptions {
    pub proj: usize,
    /// Index into `launcher_worktrees(proj)`.
    pub wt: usize,
    /// Index into `App::available_agents`.
    pub agent: usize,
    /// The row-actions strip this options state was entered from (always set —
    /// `options` is only ever entered via the strip's "Launch session…"
    /// action). Esc restores this strip instead of dropping to bare root.
    pub origin: RowActionsState,
}

/// The command palette's inline contextual-action strip, revealed by Tab
/// under a highlighted `Recent`/`Combo` row. Identifies the row by
/// `(proj, wt_path, agent)` rather than a list index, so it stays valid even
/// if the rendered row list is rebuilt out from under it (e.g. `browse_all`
/// flips, or the query changes) — the strip is simply not found/collapsed
/// rather than silently acting on a different row. `agent` is part of the
/// identity because recent launches are deduped by `(project, wt_path,
/// agent)` (see `push_recent_launch`), so two Recent rows can share a
/// worktree with different agents. `action` is the selected action within
/// the strip (`0` = primary/"Launch session…", `1` = danger/"Delete
/// worktree").
#[derive(Clone)]
pub struct RowActionsState {
    pub proj: usize,
    pub wt_path: String,
    pub agent: crate::agent::Agent,
    pub action: usize,
}

/// Same "identify by content, not list position" principle as
/// `RowActionsState`, applied to `SessionLauncher::selected` — the main
/// palette's own selection cursor. Captured whenever `selected` is written
/// (`Grove::set_palette_selected`) and re-resolved against a freshly rebuilt
/// row list at activation time (`Grove::resolve_selected` /
/// `resolve_row_by_identity`) rather than trusting the raw index: a query
/// edit, a recency-driven re-sort, or the root's no-recents worktree fallback
/// swapping in between two keystrokes can't make Enter silently activate a
/// different row than the one the user was looking at.
///
/// `proj` is an index rather than the project's name: a project can only be
/// removed via its own confirmation modal (`Modal::RemoveProject`), and
/// `Modal` holds exactly one variant at a time, so a project can't be
/// removed out from under an *open* `SessionLauncher` — its project indices
/// are stable for the palette's whole lifetime.
#[derive(Clone, PartialEq, Debug)]
pub enum PaletteRowIdentity {
    Session {
        proj: usize,
        wt_path: String,
        agent: crate::agent::Agent,
    },
    NewSession,
    TerminalHome,
    TerminalWt,
    AddProject,
    SwitchToSession,
    Settings,
    Setting(crate::gui::update::SettingRow),
}

/// Which pane the palette's Settings drill-in is showing. `Root` is the
/// sectioned all-settings list; the others are one-level-deeper pickers for
/// enum-shaped settings, entered via `Grove::activate_setting`. Esc pops one
/// level back to `Root` (and `Root`'s own Esc closes the whole drill-in).
#[derive(Clone, Copy)]
pub enum SettingsPane {
    Root,
    /// Theme picker with live preview, mirroring `Modal::ThemePicker`:
    /// `original` restores the pre-entry theme when Esc backs out without
    /// applying. `kind` is which theme list is currently shown (System mode
    /// still lists the dark set, like the real picker's own tab). `follow_system`
    /// is a local draft of "follow system appearance" — toggled and previewed
    /// by the System segment, only written to `Store::theme_follow_system` on
    /// Enter, exactly like `Modal::ThemePicker::follow_system`.
    Theme {
        original: crate::theme::Theme,
        kind: theme::ThemeKind,
        follow_system: bool,
    },
    Backend,
    Permissions,
    DefaultAgent,
    /// Project-scoped theme picker entered from a session row's actions strip.
    /// `preview` is the pane's whole state: Some = highlighted theme (live-
    /// previewed on that project's tiles only), None = "Use app theme".
    /// Nothing persists until Enter commits.
    ProjectTheme {
        proj: usize,
        kind: theme::ThemeKind,
        preview: Option<crate::theme::Theme>,
    },
}

/// "Settings" drill-in state for the session-launcher palette: a `pane`
/// (root list or a one-level-deeper enum picker) plus a selection cursor into
/// whatever that pane currently renders.
#[derive(Clone, Copy)]
pub struct LauncherSettings {
    pub pane: SettingsPane,
    /// Selection cursor into the current pane's rendered list.
    pub selected: usize,
    /// Root pane only: the App-size row is in inline-edit mode (←/→ or the
    /// on-row stepper adjust zoom; ⏎ or Esc leaves the mode without popping
    /// out of the drill-in).
    pub resizing: bool,
    /// Root pane only: the update-available actions strip expanded under the
    /// Check-for-updates row — `Some(selected strip index)` while open. ⏎ on
    /// that row expands this instead of re-firing a check whenever an update
    /// is already known to be available; Esc closes just the strip, not the
    /// drill-in.
    pub update_actions: Option<usize>,
}

/// Stage of an in-progress worktree teardown.
#[derive(Clone, Copy, PartialEq)]
pub enum TeardownStage {
    /// The teardown script is running in `session`.
    RunningScript,
    /// Script finished; `git worktree remove` is executing.
    Removing,
    /// Done — `error` is `Some` if removal failed.
    Done { failed: bool },
}

/// State for a worktree deletion in progress. Holds the live teardown PTY (if a
/// teardown script is configured) so the modal can render it; kept out of the
/// cloneable `Modal` because `Session` isn't `Clone`.
pub struct Teardown {
    pub wt_path: String,
    pub project_path: String,
    pub session: Option<Session>,
    pub stage: TeardownStage,
    pub message: String,
    /// Set once the blocking `git worktree remove` has been kicked off, so a
    /// `Removing` frame paints before the UI thread blocks on it.
    pub removal_started: bool,
}

/// Which pane of the two-step add-project modal is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddProjectStep {
    PickSource,
    Details,
}

/// Result of probing the chosen folder for a git repository when the
/// add-project modal enters its details step.
#[derive(Clone)]
pub enum GitProbe {
    Repo { branch: String },
    NotRepo,
}

#[derive(Clone)]
pub enum ConfirmKind {
    RemoveProject(usize),
    RemoveWorktree(String), // wt path
    InitAndAddWorktree {
        name: String,
    },
    /// Close grove despite running native sessions.
    Quit,
}

pub struct App {
    pub store: Store,
    pub worktrees: Vec<Worktree>,
    pub focus: Pane,
    pub proj_idx: usize,
    pub wt_idx: usize,
    pub modal: Modal,
    pub sessions: Vec<Session>,
    pub active_session: Option<usize>,
    /// The shells behind the `terminal` tab, each rooted at `~`. The first is
    /// spawned lazily when the tab is first opened; the user can add more.
    /// These live outside `sessions` so they never show up in the tree /
    /// activity lists and aren't reachable by the session-cycling or kill
    /// machinery. Always non-empty while the terminal tab is in use (closing
    /// the last one immediately respawns a fresh shell).
    pub home_terminals: Vec<Session>,
    /// Index into `home_terminals` of the terminal the tab is showing.
    pub active_terminal: Option<usize>,
    /// Monotonic counter behind each terminal's internal label (`terminal 1`,
    /// `terminal 2`, …). The label isn't shown in the UI — rows display only
    /// the icon and the shell's contextual title — but it stays stable and
    /// unique per terminal so it can be stripped from that title and preserved
    /// across a restart.
    pub home_terminal_seq: usize,
    /// Worktree-scoped terminals for the right-docked slide-over panel, keyed by
    /// absolute worktree path. Each panel can hold several shells (the panel's
    /// tab strip). These live outside `sessions` so they never appear as
    /// sidebar/tree/activity rows — they belong *inside* a session's worktree,
    /// not beside it. Entries are dropped (shells killed) when the worktree is
    /// removed via [`kill_wt_terminals`].
    pub wt_terminals: HashMap<String, Vec<Session>>,
    /// Active shell index within each worktree's panel, keyed by worktree path.
    pub wt_active_terminal: HashMap<String, usize>,
    /// Monotonic counter behind each panel terminal's internal label.
    pub wt_terminal_seq: usize,
    /// Transient top-right notification (e.g. copy confirmation).
    pub toast: Option<Toast>,
    /// Total worktrees across all projects, cached so the renderer doesn't
    /// shell out to `git` for every project on every frame.
    pub worktree_count: usize,
    /// Zen mode: when false, the top banner and the sessions sidebar are
    /// hidden on the session page so the PTY can use the full frame.
    pub chrome_visible: bool,
    /// Whether tmux was available on PATH when Grove started.
    pub tmux_available: bool,
    /// Agents whose binaries were found on PATH and are executable.
    /// Ordered to match `Agent::ALL`; `sel` in `Modal::AgentPicker` indexes
    /// into this slice. Always contains at least `Terminal`.
    /// Re-scanned each time the picker is opened so newly-installed tools
    /// appear without restarting Grove.
    pub(crate) available_agents: Vec<Agent>,
    /// Completion message from a background job (e.g. `.worktreeinclude`
    /// generation). Set by the worker thread, drained on the GUI tick.
    pub bg_status: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// In-progress worktree teardown, when `modal` is `Modal::Teardown`.
    pub teardown: Option<Teardown>,
    /// Whether the active theme should track the OS light/dark setting
    /// (mirrors `store.theme_follow_system`, kept in sync on submit).
    pub theme_follow_system: bool,
    /// Last-known OS appearance, seeded by an `iced::system::theme()` query
    /// at startup and kept current by the `system::theme_changes()`
    /// subscription. Only consulted while `theme_follow_system` is set.
    pub system_theme_mode: iced::theme::Mode,
}

impl App {
    pub fn set_toast(&mut self, message: impl Into<String>) {
        self.toast_with_kind(message, ToastKind::Info);
    }

    pub fn set_error_toast(&mut self, message: impl Into<String>) {
        self.toast_with_kind(message, ToastKind::Error);
    }

    fn toast_with_kind(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            created: std::time::Instant::now(),
        });
    }

    /// Re-scan PATH and update `available_agents`. Called before opening the
    /// agent picker so that tools installed while Grove is running are visible.
    pub(crate) fn refresh_available_agents(&mut self) {
        self.available_agents = Agent::ALL
            .iter()
            .copied()
            .filter(|a| a.available())
            .collect();
    }
}

impl App {
    pub fn new() -> Result<Self> {
        let store = storage::load()?;
        let tmux_available = tmux::available();
        // Apply the saved theme before building the initial modal so the
        // onboarding wizard seeds its theme selection from the active theme.
        // In "follow system" mode the real OS appearance arrives later via
        // the seeded `iced::system::theme()` task, but that first frame still
        // needs a concrete theme, so we seed from the saved dark theme (the
        // `SystemThemeChanged` handler corrects this once the query answers).
        let theme_follow_system = store.theme_follow_system;
        if theme_follow_system {
            let name = store.theme_dark.as_deref().unwrap_or(DEFAULT_DARK_THEME);
            theme::set_by_name(name);
        } else if let Some(name) = store.theme.as_deref() {
            theme::set_by_name(name);
        }
        let initial_modal =
            match first_run_modal(store.onboarded, tmux_available, store.tmux_enabled) {
                FirstRunModal::Onboarding => onboarding_modal(),
                FirstRunModal::TmuxChoice => Modal::TmuxChoice,
                FirstRunModal::None => Modal::None,
            };
        // Existing tmux sessions keep their backend even when the saved
        // preference now chooses native sessions for new launches.
        let sessions = if tmux_available {
            discover_sessions()
        } else {
            Vec::new()
        };
        let mut app = App {
            store,
            worktrees: vec![],
            focus: Pane::Projects,
            proj_idx: 0,
            wt_idx: 0,
            modal: initial_modal,
            sessions,
            active_session: None,
            home_terminals: Vec::new(),
            active_terminal: None,
            home_terminal_seq: 0,
            wt_terminals: HashMap::new(),
            wt_active_terminal: HashMap::new(),
            wt_terminal_seq: 0,
            toast: None,
            worktree_count: 0,
            chrome_visible: true,
            tmux_available,
            available_agents: Agent::ALL
                .iter()
                .copied()
                .filter(|a| a.available())
                .collect(),
            bg_status: std::sync::Arc::new(std::sync::Mutex::new(None)),
            teardown: None,
            theme_follow_system,
            system_theme_mode: iced::theme::Mode::None,
        };
        app.refresh_worktrees();
        Ok(app)
    }

    pub fn effective_backend(&self) -> EffectiveBackend {
        effective_backend_for(self.tmux_available, self.store.tmux_enabled)
    }

    pub fn use_tmux(&self) -> bool {
        self.effective_backend() == EffectiveBackend::Tmux
    }

    /// Running sessions that would die with the process: native-backend agent
    /// sessions only. tmux-backed sessions survive a quit and don't count.
    pub fn native_sessions_running(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.tmux_name().is_none() && s.is_running())
            .count()
    }

    pub fn set_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled && !self.tmux_available {
            self.set_error_toast("tmux not found; using native sessions");
            return Ok(());
        }
        self.store.tmux_enabled = Some(enabled);
        storage::save(&self.store)?;
        if enabled {
            self.discover_tmux_sessions();
            self.set_toast("tmux enabled for new sessions");
        } else {
            self.set_toast("tmux disabled for new sessions");
        }
        Ok(())
    }

    pub fn choose_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_tmux_enabled(enabled)?;
        self.modal = Modal::None;
        Ok(())
    }

    pub fn skip_permissions_enabled(&self) -> bool {
        self.store
            .dangerously_skip_permissions_enabled
            .unwrap_or(true)
    }

    pub fn set_skip_permissions_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.dangerously_skip_permissions_enabled = Some(enabled);
        storage::save(&self.store)?;
        self.set_toast(if enabled {
            "permission bypass enabled for new sessions"
        } else {
            "permission bypass disabled for new sessions"
        });
        Ok(())
    }

    /// Whether the universal "Project themes" toggle is on (Settings →
    /// Appearance).
    pub fn project_themes_enabled(&self) -> bool {
        self.store.project_themes_enabled
    }

    pub fn set_project_themes_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.project_themes_enabled = enabled;
        storage::save(&self.store)
    }

    /// Resolve the theme a PTY belonging to `project_name` should render its
    /// *content* in: `None` when Project themes is off, the project has no
    /// pinned theme, or its pinned name no longer matches a builtin — in
    /// every such case the caller falls back to the global active theme.
    pub fn project_theme_override(&self, project_name: &str) -> Option<theme::Theme> {
        // Live preview: while the project-scoped theme picker is open for
        // this project, its current highlight wins over the persisted pin —
        // this is the only path that lets a PTY preview a theme that hasn't
        // been saved yet. It never touches the global active theme (no
        // `theme::set` here), so other projects' tiles and app chrome are
        // unaffected. Cancel/submit both drop back to the persisted state
        // (cancel via the modal closing + an explicit cache invalidation,
        // submit because the pin is now saved).
        if let Modal::ThemePicker {
            scope: ThemePickerScope::Project(name),
            tab,
            sel_dark,
            sel_light,
            project_use_default,
            ..
        } = &self.modal
        {
            if name == project_name {
                if *project_use_default {
                    return None; // preview the global theme
                }
                let sel = match tab {
                    theme::ThemeKind::Dark => *sel_dark,
                    theme::ThemeKind::Light => *sel_light,
                };
                return theme::themes_of(*tab).get(sel).copied();
            }
        }
        // Same live-preview idea for the in-palette project-theme pane
        // (Settings drill-in entered from a session row's actions strip):
        // `preview` is the pane's whole draft state, so it wins outright —
        // `None` means "preview the global theme", matching
        // `project_use_default` above.
        if let Modal::SessionLauncher {
            settings:
                Some(LauncherSettings {
                    pane: SettingsPane::ProjectTheme { proj, preview, .. },
                    ..
                }),
            ..
        } = &self.modal
        {
            if self.store.projects.get(*proj).map(|p| p.name.as_str()) == Some(project_name) {
                return *preview;
            }
        }
        if !self.store.project_themes_enabled {
            return None;
        }
        self.store
            .projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| p.theme.as_deref())
            .and_then(theme::by_name)
    }

    pub fn telemetry_enabled(&self) -> bool {
        self.store.telemetry_enabled.unwrap_or(true)
    }

    pub fn set_telemetry_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.telemetry_enabled = Some(enabled);
        storage::save(&self.store)?;
        crate::telemetry::set_enabled(enabled);
        Ok(())
    }

    fn discover_tmux_sessions(&mut self) {
        if !self.tmux_available {
            return;
        }
        let known: HashSet<String> = self
            .sessions
            .iter()
            .filter_map(|s| s.tmux_name().map(str::to_string))
            .collect();
        let discovered: Vec<Session> = discover_sessions()
            .into_iter()
            .filter(|s| s.tmux_name().map_or(true, |name| !known.contains(name)))
            .collect();
        for s in discovered {
            let at = self.session_insert_index(&s);
            if self.active_session.map_or(false, |i| at <= i) {
                self.active_session = self.active_session.map(|i| i + 1);
            }
            self.sessions.insert(at, s);
        }
    }

    /// Recompute the cross-project worktree total. Call after any change that
    /// adds or removes projects or worktrees.
    pub fn recount_worktrees(&mut self) {
        // `self.worktrees` already holds the selected project's worktrees;
        // reuse it instead of shelling out to `git` for that project again.
        let proj_idx = self.proj_idx;
        let selected = self.worktrees.iter().filter(|w| !w.is_main).count();
        self.worktree_count = self
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == proj_idx {
                    selected
                } else {
                    git::list_worktrees(&p.path)
                        .iter()
                        .filter(|w| !w.is_main)
                        .count()
                }
            })
            .sum();
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.store.projects.get(self.proj_idx)
    }

    pub fn refresh_worktrees(&mut self) {
        self.worktrees = match self.selected_project() {
            Some(p) => git::list_worktrees(&p.path),
            None => vec![],
        };
        if self.wt_idx >= self.worktrees.len() {
            self.wt_idx = self.worktrees.len().saturating_sub(1);
        }
        self.recount_worktrees();
    }

    pub fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane;
    }

    pub fn start_add(&mut self) {
        match self.focus {
            Pane::Projects => self.start_add_project(),
            Pane::Worktrees => {
                if self.selected_project().is_some() {
                    self.modal = Modal::Input {
                        title: "Worktree name".into(),
                        buffer: String::new(),
                        note: None,
                    };
                }
            }
        }
    }

    /// Open the two-step add-project modal at its pick-source step.
    pub fn start_add_project(&mut self) {
        self.modal = Modal::AddProject {
            step: AddProjectStep::PickSource,
            path: "~/".into(),
            dir_sel: 0,
            name: String::new(),
            git: GitProbe::NotRepo,
            init_git: true,
            note: None,
        };
    }

    pub fn start_delete(&mut self) {
        match self.focus {
            Pane::Projects => {
                if let Some(p) = self.selected_project() {
                    self.modal = Modal::Confirm {
                        title: "Remove project?".into(),
                        prompt: format!(
                            "'{}' will be unregistered. Files on disk stay put.",
                            p.name
                        ),
                        destructive: true,
                        kind: ConfirmKind::RemoveProject(self.proj_idx),
                    };
                }
            }
            Pane::Worktrees => {
                if let Some(wt) = self.worktrees.get(self.wt_idx) {
                    if wt.is_main {
                        self.modal =
                            Modal::Message("Can't remove the project's main checkout".into());
                        return;
                    }
                    let path = wt.path.clone();
                    self.modal = Modal::Confirm {
                        title: "Remove worktree?".into(),
                        prompt: format!("git worktree remove --force {}", path),
                        destructive: true,
                        kind: ConfirmKind::RemoveWorktree(path),
                    };
                }
            }
        }
    }

    /// Index of the default agent in the picker, or 0 if none is set.
    fn picker_sel(&self) -> usize {
        self.store
            .default_agent
            .and_then(|a| self.available_agents.iter().position(|&x| x == a))
            .unwrap_or(0)
    }

    /// Launch the default agent for `wt_path`, or open the picker if no
    /// default is configured or the saved default is no longer available.
    pub(crate) fn launch_or_pick(&mut self, project: String, wt_path: String) -> Option<usize> {
        self.refresh_available_agents();
        let default = self
            .store
            .default_agent
            .filter(|a| self.available_agents.contains(a));
        if let Some(agent) = default {
            let label = path_basename(&wt_path);
            let args = agent.launch_args(self.skip_permissions_enabled());
            self.spawn_session(
                label,
                project,
                wt_path.clone(),
                agent,
                args,
                &wt_path,
                false,
            )
        } else {
            if let Some(saved) = self.store.default_agent {
                if !self.available_agents.contains(&saved) {
                    self.set_toast(format!("{} not found; pick an agent", saved.label()));
                }
            }
            self.modal = Modal::AgentPicker {
                project,
                wt_path,
                sel: self.picker_sel(),
            };
            None
        }
    }

    /// Compute the index at which a newly spawned session should be inserted
    /// so the sessions list stays grouped by project, and worktrees within a
    /// project follow the project's actual worktree order (rather than the
    /// order sessions happened to be created in).
    fn session_insert_index(&self, s: &Session) -> usize {
        let proj_path = self
            .store
            .projects
            .iter()
            .find(|p| p.name == s.project)
            .map(|p| p.path.clone());
        let wt_order: Vec<String> = match proj_path {
            Some(p) => git::list_worktrees(&p)
                .into_iter()
                .map(|w| w.path)
                .collect(),
            None => Vec::new(),
        };
        let new_pos = wt_order.iter().position(|p| p == &s.wt_path);

        let proj_block: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, x)| x.project == s.project)
            .map(|(i, _)| i)
            .collect();
        if proj_block.is_empty() {
            return self.sessions.len();
        }
        if let Some(new_pos) = new_pos {
            for &i in &proj_block {
                let other_pos = wt_order.iter().position(|p| p == &self.sessions[i].wt_path);
                if other_pos.map_or(false, |o| o > new_pos) {
                    return i;
                }
            }
        }
        proj_block.last().unwrap() + 1
    }

    /// Spawn an agent in a new embedded PTY session and focus it. Returns the
    /// index in `self.sessions` where the new session was inserted, or `None`
    /// if the spawn failed (an error modal is set).
    ///
    /// When `at_end` is false the session is grouped by project and sorted by
    /// worktree (so the insert can be mid-vector). When `at_end` is true it is
    /// appended after all existing sessions — used by the Agent View launcher
    /// so a freshly launched session always lands last in the grid.
    pub fn spawn_session(
        &mut self,
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: Vec<String>,
        cwd: &str,
        at_end: bool,
    ) -> Option<usize> {
        match Session::spawn(
            label.clone(),
            project,
            wt_path,
            agent,
            &args,
            cwd,
            self.use_tmux(),
        ) {
            Ok(s) => {
                let at = if at_end {
                    self.sessions.len()
                } else {
                    self.session_insert_index(&s)
                };
                let project = s.project.clone();
                let wt_path = s.wt_path.clone();
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.set_toast(format!("started {label}"));
                crate::gui::launcher::push_recent_launch(
                    &mut self.store.recent_launches,
                    crate::storage::RecentLaunch {
                        project,
                        wt_path,
                        agent,
                    },
                );
                let _ = crate::storage::save(&self.store);
                Some(at)
            }
            Err(e) => {
                self.modal = Modal::Message(format!("Failed to start agent: {e}"));
                None
            }
        }
    }

    /// Spawn a lifecycle script (`setup`/`run`) as a focused session tab under
    /// `wt_path`. No-op when the snippet is empty/whitespace.
    pub fn spawn_script_session(
        &mut self,
        stage: &str,
        project: String,
        wt_path: String,
        script: &str,
    ) {
        let script = script.trim();
        if script.is_empty() {
            return;
        }
        match Session::spawn_script(
            stage.to_string(),
            project,
            wt_path.clone(),
            script,
            &wt_path,
        ) {
            Ok(s) => {
                let at = self.session_insert_index(&s);
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.set_toast(format!("running {stage} script"));
            }
            Err(e) => {
                self.set_error_toast(format!("{stage} script failed: {e}"));
            }
        }
    }

    /// Run the project's `run` script in the given worktree, if configured.
    /// Runs as an additional shell in that worktree's terminal panel rather
    /// than a sibling session tab — like an ad hoc terminal, its output
    /// shouldn't clutter the tree or take over the agent view.
    pub fn run_worktree_script(&mut self, wt_path: &str, rows: u16, cols: u16) {
        let Some(p) = self.selected_project().cloned() else {
            return;
        };
        match p.scripts.run.as_deref() {
            Some(script) if !script.trim().is_empty() => {
                match Session::spawn_script(
                    "run".to_string(),
                    p.name.clone(),
                    wt_path.to_string(),
                    script,
                    wt_path,
                ) {
                    Ok(mut s) => {
                        s.resize(rows, cols);
                        let v = self.wt_terminals.entry(wt_path.to_string()).or_default();
                        v.push(s);
                        self.wt_active_terminal
                            .insert(wt_path.to_string(), v.len() - 1);
                    }
                    Err(e) => self.set_error_toast(format!("run script failed: {e}")),
                }
            }
            _ => self.set_toast("no run script configured for this project"),
        }
    }

    fn create_worktree(&mut self, p: &Project, name: &str) {
        let wt_path = match git::add_worktree(&p.path, &p.name, name) {
            Ok(path) => path,
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                self.modal = Modal::Message(format!("Add worktree failed: {e}"));
                return;
            }
        };
        if let Err(e) = git::copy_worktree_includes(&p.path, &wt_path) {
            self.set_error_toast(format!("worktreeinclude: {e}"));
        }
        crate::telemetry::track("worktree_created", vec![]);
        self.refresh_worktrees();
        // Launch the agent first, then the setup script (if any) so the setup
        // tab is spawned last and is the one focused by default — when a setup
        // script exists, the user wants to watch it run before touching the
        // agent. Both tabs coexist.
        self.launch_or_pick(p.name.clone(), wt_path.clone());
        if let Some(setup) = p.scripts.setup.clone() {
            self.spawn_script_session("setup", p.name.clone(), wt_path, &setup);
        }
    }

    /// Absolute path of the home directory, falling back to `/` if it can't be
    /// resolved. Used as the home terminal's working directory.
    fn home_dir() -> String {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into())
    }

    /// The home terminal the tab is currently showing, if any.
    pub fn active_home_terminal(&self) -> Option<&Session> {
        self.active_terminal
            .and_then(|i| self.home_terminals.get(i))
    }

    /// Ensure at least one home terminal exists (spawning the first on demand),
    /// resize them all to the current pane, and make sure something is selected.
    pub fn ensure_home_terminal(&mut self, rows: u16, cols: u16) {
        if self.home_terminals.is_empty() {
            self.spawn_home_terminal(rows, cols);
        } else {
            for s in &mut self.home_terminals {
                s.resize(rows, cols);
            }
        }
        if self.active_terminal.is_none() && !self.home_terminals.is_empty() {
            self.active_terminal = Some(0);
        }
    }

    /// Spawn an additional home terminal and focus it.
    pub fn new_home_terminal(&mut self, rows: u16, cols: u16) {
        self.spawn_home_terminal(rows, cols);
    }

    /// Replace the active terminal's shell in place with a fresh one at `~`,
    /// keeping its slot and label. Used to recover an exited terminal.
    pub fn restart_active_terminal(&mut self, rows: u16, cols: u16) {
        let Some(i) = self.active_terminal else {
            return;
        };
        if i >= self.home_terminals.len() {
            return;
        }
        let label = self.home_terminals[i].label.clone();
        // Only swap once the replacement is live: on spawn failure
        // `build_home_terminal` toasts and we keep the (usually exited)
        // terminal in place rather than leaving an empty slot.
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            let mut old = std::mem::replace(&mut self.home_terminals[i], s);
            old.kill();
        }
    }

    /// Close the terminal at `idx`. The terminal count may reach zero; the
    /// workspace shows an empty state and `ensure_home_terminal`/
    /// `Msg::NewHomeTerminal` spawn a fresh one on demand.
    pub fn close_home_terminal(&mut self, idx: usize) {
        if idx >= self.home_terminals.len() {
            return;
        }
        let mut s = self.home_terminals.remove(idx);
        s.kill();
        self.active_terminal = match self.active_terminal {
            Some(a) if a == idx => {
                if self.home_terminals.is_empty() {
                    None
                } else {
                    Some(idx.min(self.home_terminals.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
    }

    fn spawn_home_terminal(&mut self, rows: u16, cols: u16) {
        self.home_terminal_seq += 1;
        let label = format!("terminal {}", self.home_terminal_seq);
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            self.home_terminals.push(s);
            self.active_terminal = Some(self.home_terminals.len() - 1);
        }
    }

    /// Build a native home-terminal session at `~`, sized to the pane. Always
    /// native: a local convenience shell, not a worktree-backed agent that
    /// needs to survive grove restarts via tmux. Returns `None` (and toasts) on
    /// spawn failure.
    fn build_home_terminal(&mut self, label: String, rows: u16, cols: u16) -> Option<Session> {
        let home = Self::home_dir();
        let args = Agent::Terminal.launch_args(false);
        match Session::spawn(
            label,
            String::new(),
            home.clone(),
            Agent::Terminal,
            &args,
            &home,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                Some(s)
            }
            Err(e) => {
                self.set_error_toast(format!("terminal failed: {e}"));
                None
            }
        }
    }

    // ── per-worktree terminal panel ────────────────────────────────────────

    /// The shells of the panel for `wt_path` (empty if none spawned yet).
    pub fn wt_terminals_for(&self, wt_path: &str) -> &[Session] {
        self.wt_terminals
            .get(wt_path)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The active shell of the panel for `wt_path`, if any.
    pub fn active_wt_terminal(&self, wt_path: &str) -> Option<&Session> {
        let i = *self.wt_active_terminal.get(wt_path)?;
        self.wt_terminals.get(wt_path)?.get(i)
    }

    /// Active shell index within the panel for `wt_path`.
    pub fn active_wt_terminal_idx(&self, wt_path: &str) -> Option<usize> {
        self.wt_active_terminal.get(wt_path).copied()
    }

    /// Ensure the panel for `wt_path` has at least one shell, resize them all,
    /// and select something. Mirrors `ensure_home_terminal` but rooted in the
    /// worktree rather than `~`.
    pub fn ensure_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        match self.wt_terminals.get_mut(wt_path) {
            Some(v) if !v.is_empty() => {
                for s in v {
                    s.resize(rows, cols);
                }
            }
            // Missing or present-but-empty: spawn the first shell (which also
            // sets the active index).
            _ => self.spawn_wt_terminal(wt_path, rows, cols),
        }
        if self.wt_active_terminal.get(wt_path).is_none()
            && !self.wt_terminals_for(wt_path).is_empty()
        {
            self.wt_active_terminal.insert(wt_path.to_string(), 0);
        }
    }

    /// Spawn an additional panel shell for `wt_path` and focus it.
    pub fn new_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.spawn_wt_terminal(wt_path, rows, cols);
    }

    /// Focus the panel shell at `idx` for `wt_path`.
    pub fn select_wt_terminal(&mut self, wt_path: &str, idx: usize, rows: u16, cols: u16) {
        if let Some(v) = self.wt_terminals.get_mut(wt_path) {
            if idx < v.len() {
                v[idx].resize(rows, cols);
                self.wt_active_terminal.insert(wt_path.to_string(), idx);
            }
        }
    }

    /// Close the panel shell at `idx` for `wt_path`. Unlike the home terminal
    /// this does *not* respawn when the last one closes — an empty panel is a
    /// valid state (the panel shows its empty/start affordance).
    pub fn close_wt_terminal(&mut self, wt_path: &str, idx: usize) {
        let Some(v) = self.wt_terminals.get_mut(wt_path) else {
            return;
        };
        if idx >= v.len() {
            return;
        }
        let mut s = v.remove(idx);
        s.kill();
        let new_active = match self.wt_active_terminal.get(wt_path).copied() {
            Some(a) if a == idx => {
                if v.is_empty() {
                    None
                } else {
                    Some(idx.min(v.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        match new_active {
            Some(a) => {
                self.wt_active_terminal.insert(wt_path.to_string(), a);
            }
            None => {
                self.wt_active_terminal.remove(wt_path);
            }
        }
    }

    /// Kill and drop every panel shell for `wt_path`. Called when the owning
    /// worktree/session is removed so no orphaned shells survive.
    pub fn kill_wt_terminals(&mut self, wt_path: &str) {
        if let Some(mut v) = self.wt_terminals.remove(wt_path) {
            for s in &mut v {
                s.kill();
            }
        }
        self.wt_active_terminal.remove(wt_path);
    }

    fn spawn_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.wt_terminal_seq += 1;
        let label = format!("wt-terminal {}", self.wt_terminal_seq);
        let args = Agent::Terminal.launch_args(false);
        match Session::spawn(
            label,
            String::new(),
            wt_path.to_string(),
            Agent::Terminal,
            &args,
            wt_path,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                let v = self.wt_terminals.entry(wt_path.to_string()).or_default();
                v.push(s);
                self.wt_active_terminal
                    .insert(wt_path.to_string(), v.len() - 1);
            }
            Err(e) => {
                self.set_error_toast(format!("terminal failed: {e}"));
            }
        }
    }

    /// Open the project-removal modal for the project at `idx`. Discovers
    /// the project's non-main worktrees up front so the modal can show
    /// "Also delete N worktrees on disk" without re-shelling-out per frame.
    pub fn open_remove_project_modal(&mut self, idx: usize) {
        let Some(p) = self.store.projects.get(idx).cloned() else {
            return;
        };
        let worktrees: Vec<String> = git::list_worktrees(&p.path)
            .into_iter()
            .filter(|w| !w.is_main)
            .map(|w| w.path)
            .collect();
        self.modal = Modal::RemoveProject {
            idx,
            name: p.name,
            project_path: p.path,
            worktrees,
            also_remove_worktrees: false,
            in_progress: false,
            done: 0,
            current: String::new(),
            errors: Vec::new(),
        };
    }

    /// Kill any sessions belonging to the named project. Used when removing
    /// a project so its sessions don't linger in the sidebar as orphans.
    pub fn kill_sessions_for_project(&mut self, project: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].project == project {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
    }

    /// Finalize project removal after any worktree teardown has completed.
    /// Kills lingering sessions, drops the project from the store, and
    /// persists. Returns a status string for the caller to display.
    pub fn finalize_remove_project(&mut self, idx: usize) -> Result<String> {
        if idx >= self.store.projects.len() {
            return Ok(String::new());
        }
        let name = self.store.projects[idx].name.clone();
        self.kill_sessions_for_project(&name);
        let removed = self.store.projects.remove(idx);
        storage::save(&self.store)?;
        if self.proj_idx >= self.store.projects.len() {
            self.proj_idx = self.store.projects.len().saturating_sub(1);
        }
        self.refresh_worktrees();
        Ok(format!("removed project {}", removed.name))
    }

    /// Kill any sessions whose worktree path matches `wt_path` and remove them
    /// from the sessions list. Adjusts `active_session` so it still points at a
    /// valid session (or `None` when the list is empty).
    pub fn kill_sessions_for_wt(&mut self, wt_path: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].wt_path == wt_path {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
        // The worktree is going away — drop its panel shells too.
        self.kill_wt_terminals(wt_path);
    }

    /// Begin tearing down `path`: kill its sessions, then either run the
    /// project's teardown script in a modal PTY (advancing to removal when it
    /// exits) or remove the worktree immediately. Opens `Modal::Teardown`.
    fn start_teardown(&mut self, p: &Project, path: String) {
        self.kill_sessions_for_wt(&path);
        let script = p
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let session = script.and_then(|s| {
            Session::spawn_script("teardown".into(), p.name.clone(), path.clone(), s, &path).ok()
        });
        self.modal = Modal::Teardown;
        if session.is_some() {
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session,
                stage: TeardownStage::RunningScript,
                message: "running teardown script…".into(),
                removal_started: false,
            });
        } else {
            // No teardown script (or it failed to spawn): the next poll paints
            // "removing worktree…" then runs the blocking git removal.
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session: None,
                stage: TeardownStage::Removing,
                message: "removing worktree…".into(),
                removal_started: false,
            });
        }
    }

    /// Drive an in-progress teardown forward. Called every GUI tick. When the
    /// teardown script's PTY exits, performs the git removal.
    pub fn poll_teardown(&mut self) {
        let Some(td) = self.teardown.as_mut() else {
            return;
        };
        // Script finished → switch to the "removing…" stage. Removal itself
        // waits for the next poll so that frame paints before we block.
        if td.stage == TeardownStage::RunningScript
            && td.session.as_ref().is_none_or(|s| !s.is_running())
        {
            td.stage = TeardownStage::Removing;
            td.session = None;
            td.message = "removing worktree…".into();
            return;
        }
        if td.stage == TeardownStage::Removing && !td.removal_started {
            td.removal_started = true;
            self.do_teardown_removal();
        }
    }

    /// Run `git worktree remove` for the active teardown and transition to
    /// `Done`. Drops the (exited) teardown PTY session.
    fn do_teardown_removal(&mut self) {
        let Some(td) = self.teardown.as_mut() else {
            return;
        };
        td.stage = TeardownStage::Removing;
        td.session = None;
        let wt_path = td.wt_path.clone();
        let project_path = td.project_path.clone();
        let err = git::remove_worktree(&project_path, &wt_path)
            .err()
            .map(|e| e.to_string());
        if let Some(td) = self.teardown.as_mut() {
            td.stage = TeardownStage::Done {
                failed: err.is_some(),
            };
            td.message = match &err {
                Some(e) => format!("removal failed: {e}"),
                None => "worktree deleted".into(),
            };
        }
        match &err {
            Some(e) => self.set_error_toast(format!("teardown err: {e}")),
            None => self.set_toast(format!("removed worktree {wt_path}")),
        }
        self.refresh_worktrees();
    }

    /// Skip a still-running teardown script: kill it and proceed to removal.
    pub fn skip_teardown_script(&mut self) {
        if let Some(td) = self.teardown.as_mut() {
            if td.stage == TeardownStage::RunningScript {
                if let Some(s) = td.session.as_mut() {
                    s.kill();
                }
                self.do_teardown_removal();
            }
        }
    }

    /// Dismiss the teardown modal. Only meaningful once removal has finished.
    pub fn close_teardown(&mut self) {
        self.teardown = None;
        self.modal = Modal::None;
    }

    pub fn picker_move(&mut self, delta: i32) {
        if let Modal::AgentPicker { sel, .. } = &mut self.modal {
            *sel = cycle(*sel, delta, self.available_agents.len());
        }
    }

    pub fn picker_toggle_default(&mut self) -> Result<()> {
        let Modal::AgentPicker { sel, .. } = &self.modal else {
            return Ok(());
        };
        let Some(agent) = self.available_agents.get(*sel).copied() else {
            return Ok(());
        };
        if self.store.default_agent == Some(agent) {
            self.store.default_agent = None;
            self.set_toast(format!("cleared default agent ({})", agent.label()));
        } else {
            self.store.default_agent = Some(agent);
            self.set_toast(format!("default agent: {}", agent.label()));
        }
        storage::save(&self.store)?;
        Ok(())
    }

    pub fn picker_submit(&mut self) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::AgentPicker {
            project,
            wt_path,
            sel,
        } = modal
        else {
            return;
        };
        let Some(agent) = self.available_agents.get(sel).copied() else {
            return;
        };
        let label = path_basename(&wt_path);
        let args = agent.launch_args(self.skip_permissions_enabled());
        self.spawn_session(
            label,
            project,
            wt_path.clone(),
            agent,
            args,
            &wt_path,
            false,
        );
    }

    pub fn open_settings(&mut self) {
        self.modal = Modal::Settings;
    }

    /// Set the global default agent, or clear it when re-selecting the current
    /// default (mirrors `picker_toggle_default`). Only installed tools should
    /// reach here — the Settings UI hides the action on missing tools.
    pub fn set_default_agent(&mut self, agent: Agent) -> Result<()> {
        if self.store.default_agent == Some(agent) {
            self.store.default_agent = None;
            self.set_toast(format!("cleared default agent ({})", agent.label()));
        } else {
            self.store.default_agent = Some(agent);
            self.set_toast(format!("default agent: {}", agent.label()));
        }
        storage::save(&self.store)?;
        Ok(())
    }

    pub fn open_theme_picker(&mut self, return_to_settings: bool) {
        let original = theme::current();
        let tab = original.kind;
        let sel = theme::themes_of(tab)
            .iter()
            .position(|t| t.name == original.name)
            .unwrap_or(0);
        let (sel_dark, sel_light) = match tab {
            theme::ThemeKind::Dark => (sel, 0),
            theme::ThemeKind::Light => (0, sel),
        };
        self.modal = Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            original,
            return_to_settings,
            follow_system: self.theme_follow_system,
            scope: ThemePickerScope::App,
            project_use_default: false,
        };
    }

    /// Open the theme picker scoped to a single project's pinned "Project
    /// theme" (opened from the Project Settings modal's "Project theme"
    /// row). Unlike the app-scoped picker, hovering/selecting here never
    /// touches the global active theme — only `theme_picker_submit` writes
    /// anything, and it writes `Project::theme`, not `theme::set`.
    pub fn open_project_theme_picker(&mut self, proj: usize) {
        let Some(project) = self.store.projects.get(proj) else {
            return;
        };
        let name = project.name.clone();
        let original = theme::current();
        let pinned = project.theme.as_deref().and_then(theme::by_name);
        let project_use_default = pinned.is_none();
        let tab = pinned.map(|t| t.kind).unwrap_or(original.kind);
        let sel = pinned
            .and_then(|t| theme::themes_of(tab).iter().position(|x| x.name == t.name))
            .unwrap_or(0);
        let (sel_dark, sel_light) = match tab {
            theme::ThemeKind::Dark => (sel, 0),
            theme::ThemeKind::Light => (0, sel),
        };
        self.modal = Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            original,
            return_to_settings: false,
            follow_system: false,
            scope: ThemePickerScope::Project(name),
            project_use_default,
        };
    }

    /// The theme name to use for `mode` under "follow system" — the user's
    /// saved dark/light theme, falling back to the built-in defaults.
    pub fn resolve_system_theme_name(&self, mode: iced::theme::Mode) -> &str {
        match mode {
            iced::theme::Mode::Light => self
                .store
                .theme_light
                .as_deref()
                .unwrap_or(DEFAULT_LIGHT_THEME),
            iced::theme::Mode::Dark | iced::theme::Mode::None => self
                .store
                .theme_dark
                .as_deref()
                .unwrap_or(DEFAULT_DARK_THEME),
        }
    }

    /// Re-applies the active theme from `system_theme_mode` when following
    /// the OS setting. No-op otherwise.
    pub fn apply_system_theme(&mut self) {
        if self.theme_follow_system {
            let name = self
                .resolve_system_theme_name(self.system_theme_mode)
                .to_string();
            theme::set_by_name(&name);
        }
    }

    pub fn theme_picker_move(&mut self, delta: i32) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            project_use_default,
            ..
        } = &mut self.modal
        else {
            return;
        };
        let themes = theme::themes_of(*tab);
        if themes.is_empty() {
            return;
        }
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        *sel = cycle(*sel, delta, themes.len());
        match scope {
            ThemePickerScope::App => {
                *follow_system = false;
                theme::set(themes[*sel]);
            }
            ThemePickerScope::Project(_) => {
                // Project scope only edits local picker state — never the
                // global active theme.
                *project_use_default = false;
            }
        }
    }

    pub fn theme_picker_switch_tab(&mut self) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            ..
        } = &mut self.modal
        else {
            return;
        };
        *tab = match *tab {
            theme::ThemeKind::Dark => theme::ThemeKind::Light,
            theme::ThemeKind::Light => theme::ThemeKind::Dark,
        };
        if *scope != ThemePickerScope::App {
            return;
        }
        let themes = theme::themes_of(*tab);
        let sel = match tab {
            theme::ThemeKind::Dark => *sel_dark,
            theme::ThemeKind::Light => *sel_light,
        };
        // Switching tabs alone is just browsing, not a selection — leave
        // `follow_system` as the user set it via the checkbox. But if it's
        // checked, keep the preview showing the resolved system theme rather
        // than snapping to the tab's list selection (which would visually
        // contradict the still-checked checkbox).
        if *follow_system {
            let name = self
                .resolve_system_theme_name(self.system_theme_mode)
                .to_string();
            theme::set_by_name(&name);
        } else if let Some(t) = themes.get(sel) {
            theme::set(*t);
        }
    }

    /// Project scope only: select the "Default (follow app)" row, deselecting
    /// any concrete theme in the list. No-op in app scope.
    pub fn theme_picker_select_default(&mut self) {
        if let Modal::ThemePicker {
            scope: ThemePickerScope::Project(_),
            project_use_default,
            ..
        } = &mut self.modal
        {
            *project_use_default = true;
        }
    }

    pub fn theme_picker_submit(&mut self) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            return_to_settings,
            follow_system,
            scope,
            project_use_default,
            ..
        } = modal
        else {
            return Ok(());
        };
        if let ThemePickerScope::Project(name) = scope {
            // Project scope never touches the global active theme or
            // `Store::theme*` — it only pins/clears this project's override.
            let chosen = if project_use_default {
                None
            } else {
                let themes = theme::themes_of(tab);
                let sel = match tab {
                    theme::ThemeKind::Dark => sel_dark,
                    theme::ThemeKind::Light => sel_light,
                };
                themes.get(sel).map(|t| t.name.to_string())
            };
            // The project may have been removed while the picker was open
            // (e.g. via a remove-project flow elsewhere) — only save/toast
            // when the write actually lands somewhere.
            match self.store.projects.iter_mut().find(|p| p.name == name) {
                Some(p) => {
                    p.theme = chosen.clone();
                    storage::save(&self.store)?;
                    let label = chosen.unwrap_or_else(|| "default".to_string());
                    self.set_toast(format!("project theme: {label}"));
                }
                None => {
                    self.set_error_toast(format!("project \"{name}\" no longer exists"));
                }
            }
            // Return to the Project Settings modal (`Modal::ScriptsEditor`);
            // its live editor state lives outside `Modal` in the GUI layer
            // and survives this round-trip untouched.
            self.modal = Modal::ScriptsEditor;
            return Ok(());
        }
        if return_to_settings {
            self.modal = Modal::Settings;
        }
        self.theme_follow_system = follow_system;
        self.store.theme_follow_system = follow_system;
        if follow_system {
            self.apply_system_theme();
            storage::save(&self.store)?;
            self.set_toast("theme: system".to_string());
            return Ok(());
        }
        let themes = theme::themes_of(tab);
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        let Some(chosen) = themes.get(sel).copied() else {
            storage::save(&self.store)?;
            return Ok(());
        };
        theme::set(chosen);
        self.store.theme = Some(chosen.name.to_string());
        match chosen.kind {
            theme::ThemeKind::Dark => self.store.theme_dark = Some(chosen.name.to_string()),
            theme::ThemeKind::Light => self.store.theme_light = Some(chosen.name.to_string()),
        }
        storage::save(&self.store)?;
        self.set_toast(format!("theme: {}", chosen.name));
        Ok(())
    }

    pub fn theme_picker_cancel(&mut self) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        if let Modal::ThemePicker {
            original,
            return_to_settings,
            scope,
            ..
        } = modal
        {
            if let ThemePickerScope::Project(_) = scope {
                // Project scope never previewed into the global theme, so
                // there's nothing to restore — just return to Project Settings.
                self.modal = Modal::ScriptsEditor;
                return;
            }
            theme::set(original);
            if return_to_settings {
                self.modal = Modal::Settings;
            }
        }
    }

    // ── onboarding wizard ──────────────────────────────────────────────────

    /// Move the wizard to `step` (no-op if onboarding isn't open).
    fn onboard_goto(&mut self, step: OnboardStep) {
        if let Modal::Onboarding { step: s, .. } = &mut self.modal {
            *s = step;
        }
    }

    /// Advance the wizard one step. The project step validates and registers
    /// the project before advancing. The session step is terminal —
    /// [`onboard_finish`](Self::onboard_finish) handles it.
    pub fn onboard_next(&mut self) {
        let Modal::Onboarding { step, .. } = &self.modal else {
            return;
        };
        let step = *step;
        match step {
            // Plain forward steps: walk to the next one.
            OnboardStep::Welcome | OnboardStep::Environment => {
                if let Some(next) = step.next() {
                    self.onboard_goto(next);
                }
            }
            OnboardStep::Project => self.onboard_submit_project(),
            OnboardStep::Session => {}
        }
    }

    /// Step back. Never un-registers a project added on the way forward; the
    /// project step recognizes it's already added and skips re-adding.
    pub fn onboard_back(&mut self) {
        let prev = match &self.modal {
            Modal::Onboarding { step, .. } => step.prev(),
            _ => None,
        };
        if let Some(prev) = prev {
            self.onboard_goto(prev);
        }
    }

    /// Register the project from the path field, then advance to the session
    /// step. On validation failure the inline note is set and the step stays
    /// put. A project already added (e.g. after stepping back and forward)
    /// just advances. Unlike the normal add-project flow this is quiet — no
    /// git probe or init-git choice.
    fn onboard_submit_project(&mut self) {
        let (already, path, name) = match &self.modal {
            Modal::Onboarding {
                added_proj,
                path,
                name,
                ..
            } => (added_proj.is_some(), path.clone(), name.clone()),
            _ => return,
        };
        if already {
            self.onboard_goto(OnboardStep::Session);
            return;
        }
        match self.onboard_add_project(&path, name) {
            Ok(idx) => {
                if let Modal::Onboarding {
                    added_proj, note, ..
                } = &mut self.modal
                {
                    *added_proj = Some(idx);
                    *note = None;
                }
                self.onboard_goto(OnboardStep::Session);
            }
            Err(e) => {
                if let Modal::Onboarding { note, .. } = &mut self.modal {
                    *note = Some(e);
                }
            }
        }
    }

    /// Register a project quietly (no follow-up modals), returning its index.
    /// Mirrors the validation in [`submit_input`](Self::submit_input)'s
    /// add-project branch.
    fn onboard_add_project(&mut self, path: &str, name: Option<String>) -> Result<usize, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err("enter a path, or skip setup".into());
        }
        let pb = std::path::PathBuf::from(shellexpand_tilde(trimmed));
        if !pb.is_dir() {
            return Err("not a directory".into());
        }
        let project_name = name
            .unwrap_or_else(|| {
                pb.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string()
            })
            .trim()
            .to_string();
        if project_name.is_empty() {
            return Err("name required".into());
        }
        if self.store.projects.iter().any(|p| p.name == project_name) {
            return Err(format!("project '{project_name}' already exists"));
        }
        let abs = std::fs::canonicalize(&pb)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        self.register_project(project_name, abs)
            .map_err(|e| e.to_string())
    }

    /// Live-update the project-step path field, mirroring the add-project input:
    /// clears the inline note, resets the directory cursor, and once the path
    /// resolves to a real directory, prefills the name from its basename.
    pub fn onboard_set_path(&mut self, value: String) {
        let resolved = std::path::PathBuf::from(shellexpand_tilde(value.trim()));
        let base = resolved
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string);
        let is_dir = resolved.is_dir();
        if let Modal::Onboarding {
            path,
            dir_sel,
            note,
            name,
            ..
        } = &mut self.modal
        {
            *path = value;
            *dir_sel = 0;
            *note = None;
            if is_dir {
                if name.is_none() {
                    *name = Some(base.unwrap_or_else(|| "project".into()));
                }
            } else {
                *name = None;
            }
        }
    }

    pub fn onboard_set_name(&mut self, value: String) {
        if let Modal::Onboarding { name, .. } = &mut self.modal {
            *name = Some(value);
        }
    }

    /// Fill the path field from a clicked directory match (trailing slash so the
    /// next keystroke descends).
    pub fn onboard_pick_dir(&mut self, dir: String) {
        self.onboard_set_path(format!("{dir}/"));
    }

    /// Move the directory-match cursor in the project step.
    pub fn onboard_dir_move(&mut self, delta: i32) {
        let entries = match &self.modal {
            Modal::Onboarding { path, .. } => list_dirs(path).len(),
            _ => return,
        };
        if entries == 0 {
            return;
        }
        if let Modal::Onboarding { dir_sel, .. } = &mut self.modal {
            *dir_sel = cycle(*dir_sel, delta, entries);
        }
    }

    /// Reset Tab's toggle target to the path field. Called whenever the
    /// project step is (re-)entered, so a stale toggle from a previous visit
    /// doesn't leave the first Tab press landing on the name field.
    pub fn onboard_reset_project_focus(&mut self) {
        if let Modal::Onboarding { name_focused, .. } = &mut self.modal {
            *name_focused = false;
        }
    }

    /// Tab in the project step: alternate focus between the path and name
    /// fields. Returns `true` if focus moved to the name field (the caller
    /// then skips path-completion); `false` if it's on the path field (where
    /// the caller runs the existing directory completion). No name field
    /// (path not yet a valid directory) means there's nothing to alternate
    /// to, so this always reports the path field.
    pub fn onboard_toggle_project_focus(&mut self) -> bool {
        if let Modal::Onboarding {
            name, name_focused, ..
        } = &mut self.modal
        {
            if name.is_none() {
                *name_focused = false;
                return false;
            }
            *name_focused = !*name_focused;
            return *name_focused;
        }
        false
    }

    /// Complete the path from the selected directory match (Tab in the project
    /// step).
    pub fn onboard_dir_pick(&mut self) {
        let pick = match &self.modal {
            Modal::Onboarding { path, dir_sel, .. } => list_dirs(path).into_iter().nth(*dir_sel),
            _ => None,
        };
        if let Some(dir) = pick {
            self.onboard_set_path(format!("{dir}/"));
        }
    }

    pub fn onboard_agent_select(&mut self, i: usize) {
        if let Modal::Onboarding { agent_sel, .. } = &mut self.modal {
            *agent_sel = i;
        }
    }

    pub fn onboard_set_perms(&mut self, skip: bool) {
        if let Modal::Onboarding { perms_skip, .. } = &mut self.modal {
            *perms_skip = skip;
        }
    }

    /// Skip the wizard: mark onboarded, persist, and close. The first-run
    /// gate won't show it again.
    pub fn onboard_skip(&mut self) -> Result<()> {
        self.store.onboarded = true;
        storage::save(&self.store)?;
        self.modal = Modal::None;
        Ok(())
    }

    /// Finish the wizard: mark onboarded, default new sessions to tmux when
    /// available (native otherwise), close, and return the
    /// `(project index, agent)` to launch a first session into — or `None` if
    /// no project was added or no agent is available.
    pub fn onboard_finish(&mut self) -> Result<Option<(usize, Agent)>> {
        let (added_proj, agent_sel, perms_skip) = match &self.modal {
            Modal::Onboarding {
                added_proj,
                agent_sel,
                perms_skip,
                ..
            } => (*added_proj, *agent_sel, *perms_skip),
            _ => (None, 0, false),
        };
        let agent = self.available_agents.get(agent_sel).copied();
        self.store.onboarded = true;
        // New sessions default to the tmux backend when it's available, native
        // otherwise; changeable later via the palette's Settings pane.
        self.store.tmux_enabled = Some(self.tmux_available);
        self.store.dangerously_skip_permissions_enabled = Some(perms_skip);
        storage::save(&self.store)?;
        self.modal = Modal::None;
        Ok(match (added_proj, agent) {
            (Some(p), Some(a)) => Some((p, a)),
            _ => None,
        })
    }

    /// Replace the input-modal buffer from a live `text_input` edit.
    pub fn set_input_path(&mut self, s: String) {
        if let Modal::Input { buffer, note, .. } = &mut self.modal {
            *buffer = s;
            *note = None;
        }
    }

    pub fn submit_input(&mut self) -> Result<()> {
        let value = match &self.modal {
            Modal::Input { buffer, .. } => buffer.trim().to_string(),
            _ => return Ok(()),
        };
        if value.is_empty() {
            return Ok(());
        }
        self.modal = Modal::None;
        if !git::valid_worktree_name(&value) {
            self.modal =
                Modal::Message("Invalid name: use letters, digits, '-', '_' or '.'".into());
            return Ok(());
        }
        let Some(p) = self.selected_project().cloned() else {
            return Ok(());
        };
        if !git::is_repo(&p.path) {
            self.modal = Modal::Confirm {
                title: "Initialize Git repo?".into(),
                prompt: format!(
                    "'{}' is not a Git repo. Run `git init`, then create worktree '{}'.",
                    p.path, value
                ),
                destructive: false,
                kind: ConfirmKind::InitAndAddWorktree { name: value },
            };
            return Ok(());
        }
        self.create_worktree(&p, &value);
        Ok(())
    }

    // ── two-step add-project modal ───────────────────────────────────────

    fn add_project_note(&mut self, msg: String) {
        if let Modal::AddProject { note, .. } = &mut self.modal {
            *note = Some(msg);
        }
    }

    /// Live edit of the step-1 path buffer.
    pub fn add_project_set_path(&mut self, s: String) {
        if let Modal::AddProject {
            step: AddProjectStep::PickSource,
            path,
            dir_sel,
            note,
            ..
        } = &mut self.modal
        {
            *path = s;
            *dir_sel = 0;
            *note = None;
        }
    }

    /// Live edit of the step-2 name field.
    pub fn add_project_set_name(&mut self, s: String) {
        if let Modal::AddProject { name, note, .. } = &mut self.modal {
            *name = s;
            *note = None;
        }
    }

    pub fn add_project_dir_move(&mut self, delta: i32) {
        let Modal::AddProject {
            step: AddProjectStep::PickSource,
            path,
            dir_sel,
            ..
        } = &mut self.modal
        else {
            return;
        };
        let entries = list_dirs(path);
        if entries.is_empty() {
            *dir_sel = 0;
            return;
        }
        *dir_sel = cycle(*dir_sel, delta, entries.len());
    }

    pub fn add_project_dir_pick(&mut self) {
        if let Modal::AddProject {
            step: AddProjectStep::PickSource,
            path,
            dir_sel,
            ..
        } = &mut self.modal
        {
            let entries = list_dirs(path);
            if let Some(pick) = entries.get(*dir_sel) {
                *path = format!("{pick}/");
                *dir_sel = 0;
            }
        }
    }

    /// Step-1 Enter: feed the typed buffer into the choose funnel. Guarded to
    /// the pick-source step so a doubled Enter (the text_input's on_submit plus
    /// the key subscription) can't fall through and submit the details step.
    pub fn add_project_choose_typed(&mut self) {
        let Modal::AddProject {
            step: AddProjectStep::PickSource,
            path,
            ..
        } = &self.modal
        else {
            return;
        };
        let pb = std::path::PathBuf::from(shellexpand_tilde(path.trim()));
        self.add_project_choose(pb);
    }

    /// Single funnel for all three folder sources (native picker, drop, typed
    /// path): validate, canonicalize, probe git upfront, and advance to the
    /// details step. On failure an inline note is set and the step stays put.
    pub fn add_project_choose(&mut self, pb: std::path::PathBuf) {
        if !matches!(self.modal, Modal::AddProject { .. }) {
            return;
        }
        if !pb.is_dir() {
            self.add_project_note("not a folder; choose a directory".into());
            return;
        }
        let abs = match std::fs::canonicalize(&pb) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(e) => {
                self.add_project_note(format!("cannot resolve path: {e}"));
                return;
            }
        };
        let probe = if git::is_repo(&abs) {
            GitProbe::Repo {
                branch: git::current_branch(&abs),
            }
        } else {
            GitProbe::NotRepo
        };
        if let Modal::AddProject {
            step,
            path,
            git,
            note,
            ..
        } = &mut self.modal
        {
            *step = AddProjectStep::Details;
            *path = abs;
            *git = probe;
            *note = None;
        }
    }

    /// "change" from the details step: back to pick-source with the buffer
    /// primed to the current folder. The (possibly edited) name is kept so a
    /// round-trip doesn't lose it.
    pub fn add_project_change_source(&mut self) {
        if let Modal::AddProject { step, note, .. } = &mut self.modal {
            *step = AddProjectStep::PickSource;
            *note = None;
        }
    }

    /// Final submit from the details step: validate, optionally `git init`,
    /// then register the project. Nothing is persisted until every check has
    /// passed.
    pub fn submit_add_project(&mut self) -> Result<()> {
        let (path, name, git, init_git) = match &self.modal {
            Modal::AddProject {
                step: AddProjectStep::Details,
                path,
                name,
                git,
                init_git,
                ..
            } => (
                path.clone(),
                name.trim().to_string(),
                git.clone(),
                *init_git,
            ),
            _ => return Ok(()),
        };
        // The name field is a pure override: left empty, the folder's basename
        // is used (mirrored by the field's placeholder in the view).
        let name = if name.is_empty() {
            path_basename(&path)
        } else {
            name
        };
        if name.is_empty() {
            self.add_project_note("name required".into());
            return Ok(());
        }
        if self.store.projects.iter().any(|p| p.name == name) {
            self.add_project_note(format!("project '{name}' already exists"));
            return Ok(());
        }
        if let Some(p) = self.store.projects.iter().find(|p| p.path == path) {
            self.add_project_note(format!("folder already added as '{}'", p.name));
            return Ok(());
        }
        if matches!(git, GitProbe::NotRepo) && init_git {
            if let Err(e) = git::init_if_needed(&path) {
                self.add_project_note(format!("git init failed: {e}"));
                return Ok(());
            }
        }
        self.modal = Modal::None;
        self.register_project(name, path)?;
        Ok(())
    }

    /// Persist a new project and select it. Quiet — no follow-up modals.
    fn register_project(&mut self, name: String, path: String) -> Result<usize> {
        self.store.projects.push(Project {
            name: name.clone(),
            path,
            scripts: Default::default(),
            theme: None,
        });
        storage::save(&self.store)?;
        self.proj_idx = self.store.projects.len() - 1;
        self.set_toast(format!("added {name}"));
        self.refresh_worktrees();
        Ok(self.proj_idx)
    }

    pub fn submit_confirm(&mut self, yes: bool) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::Confirm { kind, .. } = modal else {
            return Ok(());
        };
        if !yes {
            return Ok(());
        }
        match kind {
            ConfirmKind::RemoveProject(idx) => {
                // Route through the same teardown as the remove-project modal
                // so the project's sessions are killed, not orphaned.
                let msg = self.finalize_remove_project(idx)?;
                if !msg.is_empty() {
                    self.set_toast(msg);
                }
            }
            ConfirmKind::RemoveWorktree(path) => {
                if let Some(p) = self.selected_project().cloned() {
                    self.start_teardown(&p, path);
                }
            }
            ConfirmKind::InitAndAddWorktree { name } => {
                let Some(p) = self.selected_project().cloned() else {
                    return Ok(());
                };
                if let Err(e) = git::init_if_needed(&p.path) {
                    self.modal = Modal::Message(format!("Git init failed: {e}"));
                    return Ok(());
                }
                self.create_worktree(&p, &name);
            }
            // Handled at the GUI layer (needs iced::exit); never reaches here.
            ConfirmKind::Quit => {}
        }
        Ok(())
    }
}

/// Re-attach to any tmux sessions grove left running from a previous launch.
/// Sessions that fail to attach are silently dropped.
fn discover_sessions() -> Vec<Session> {
    tmux::list_grove_sessions()
        .into_iter()
        .filter(|d| tmux::has_session(&d.name))
        .filter_map(|d| Session::attach_existing(d).ok())
        .collect()
}

/// Last path component, or the whole string if there is no separator.
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

pub fn list_dirs(buffer: &str) -> Vec<String> {
    let expanded = shellexpand_tilde(buffer);
    let (dir, prefix) = if expanded.is_empty() {
        (std::path::PathBuf::from("."), String::new())
    } else if expanded.ends_with('/') {
        (
            std::path::PathBuf::from(expanded.trim_end_matches('/')),
            String::new(),
        )
    } else {
        let pb = std::path::PathBuf::from(&expanded);
        let parent = pb
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = pb
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let parent = if parent.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            parent
        };
        (parent, name)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') || prefix.starts_with('.'))
        .filter(|n| n.starts_with(&prefix))
        .map(|n| format!("{}/{}", dir.display(), n))
        .collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out
}

fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        effective_backend_for, first_run_modal, needs_tmux_choice, onboarding_modal, App,
        EffectiveBackend, FirstRunModal, HashMap, Modal, OnboardStep, Pane, Session, Store,
    };
    use super::{Toast, ToastKind};
    use crate::session::{SessionBackend, SessionStatus};
    use std::time::{Duration, Instant};

    /// Build a minimal `App` around the given sessions, bypassing `App::new`
    /// (which reads/writes the real on-disk config) so tests stay hermetic.
    fn test_app(sessions: Vec<Session>) -> App {
        App {
            store: Store::default(),
            worktrees: vec![],
            focus: Pane::Projects,
            proj_idx: 0,
            wt_idx: 0,
            modal: Modal::None,
            sessions,
            active_session: None,
            home_terminals: Vec::new(),
            active_terminal: None,
            home_terminal_seq: 0,
            wt_terminals: HashMap::new(),
            wt_active_terminal: HashMap::new(),
            wt_terminal_seq: 0,
            toast: None,
            worktree_count: 0,
            chrome_visible: true,
            tmux_available: false,
            available_agents: vec![],
            bg_status: std::sync::Arc::new(std::sync::Mutex::new(None)),
            teardown: None,
            theme_follow_system: false,
            system_theme_mode: iced::theme::Mode::None,
        }
    }

    /// Spawn a cheap real session (a `true` shell script — exits immediately)
    /// and force its backend/status to the scenario under test. `Session`'s
    /// PTY/child fields are private to `session.rs` and can't be constructed
    /// directly, but `backend` and `status` are `pub`, so overwriting them
    /// after a real spawn is the lightest way to get a `Session` in an
    /// arbitrary (backend, running) state without a real tmux session.
    fn spawn_test_session(status: SessionStatus, tmux: bool) -> Session {
        let mut s = Session::spawn_script("t".into(), "p".into(), ".".into(), "true", ".")
            .expect("spawn test session");
        if tmux {
            s.backend = SessionBackend::Tmux {
                name: "test".into(),
            };
        }
        *s.status.lock().unwrap() = status;
        s
    }

    #[test]
    fn native_sessions_running_counts_only_running_native() {
        let app = test_app(vec![
            spawn_test_session(SessionStatus::Running, false), // native, running: counts
            spawn_test_session(SessionStatus::Exited(Some(0)), false), // native, exited: no
            spawn_test_session(SessionStatus::Running, true),  // tmux, running: no
        ]);
        assert_eq!(app.native_sessions_running(), 1);
    }

    #[test]
    fn toast_ttl_is_kind_dependent() {
        assert_eq!(Toast::ttl(ToastKind::Info), Duration::from_secs(4));
        assert_eq!(Toast::ttl(ToastKind::Error), Duration::from_secs(8));
    }

    #[test]
    fn toast_expiry_follows_ttl() {
        let t0 = Instant::now();
        let info = Toast {
            message: "copied".into(),
            kind: ToastKind::Info,
            created: t0,
        };
        let error = Toast {
            message: "failed".into(),
            kind: ToastKind::Error,
            created: t0,
        };
        assert!(!info.expired_at(t0 + Duration::from_secs(3)));
        assert!(info.expired_at(t0 + Duration::from_secs(4)));
        assert!(!error.expired_at(t0 + Duration::from_secs(7)));
        assert!(error.expired_at(t0 + Duration::from_secs(8)));
    }

    #[test]
    fn onboarding_takes_precedence_until_completed() {
        // Fresh install: onboarding wins even when a tmux choice is also pending.
        assert_eq!(
            first_run_modal(false, true, None),
            FirstRunModal::Onboarding
        );
        assert_eq!(
            first_run_modal(false, false, None),
            FirstRunModal::Onboarding
        );
        // Once onboarded, the tmux choice falls through (only when pending).
        assert_eq!(first_run_modal(true, true, None), FirstRunModal::TmuxChoice);
        assert_eq!(first_run_modal(true, true, Some(true)), FirstRunModal::None);
        assert_eq!(first_run_modal(true, false, None), FirstRunModal::None);
    }

    #[test]
    fn onboard_step_navigation_is_bounded() {
        assert_eq!(OnboardStep::Welcome.prev(), None);
        assert_eq!(OnboardStep::Environment.next(), Some(OnboardStep::Project));
        assert_eq!(OnboardStep::Project.prev(), Some(OnboardStep::Environment));
        assert_eq!(OnboardStep::Project.next(), Some(OnboardStep::Session));
        assert_eq!(OnboardStep::Session.next(), None);
        // index round-trips through the flow in order.
        for (i, s) in OnboardStep::flow().iter().enumerate() {
            assert_eq!(s.index_in(), i);
        }
    }

    #[test]
    fn onboard_tab_toggles_project_focus_only_when_name_field_exists() {
        let mut app = test_app(vec![]);
        app.modal = onboarding_modal();
        // Path not yet resolved to a directory: no name field to toggle to.
        assert!(!app.onboard_toggle_project_focus());
        assert!(!app.onboard_toggle_project_focus());
        // Once a name is inferred, Tab alternates path <-> name.
        if let Modal::Onboarding { name, .. } = &mut app.modal {
            *name = Some("repo".into());
        }
        assert!(app.onboard_toggle_project_focus());
        assert!(!app.onboard_toggle_project_focus());
        assert!(app.onboard_toggle_project_focus());
        // Losing the name field again snaps back to the path field.
        if let Modal::Onboarding { name, .. } = &mut app.modal {
            *name = None;
        }
        assert!(!app.onboard_toggle_project_focus());
    }

    #[test]
    fn onboard_reset_project_focus_clears_stale_toggle() {
        let mut app = test_app(vec![]);
        app.modal = onboarding_modal();
        if let Modal::Onboarding {
            name, name_focused, ..
        } = &mut app.modal
        {
            *name = Some("repo".into());
            *name_focused = true;
        }
        app.onboard_reset_project_focus();
        let Modal::Onboarding { name_focused, .. } = &app.modal else {
            unreachable!()
        };
        assert!(!name_focused);
    }

    #[test]
    fn tmux_unavailable_uses_native_for_any_preference() {
        assert_eq!(effective_backend_for(false, None), EffectiveBackend::Native);
        assert_eq!(
            effective_backend_for(false, Some(true)),
            EffectiveBackend::Native
        );
        assert_eq!(
            effective_backend_for(false, Some(false)),
            EffectiveBackend::Native
        );
    }

    #[test]
    fn tmux_available_uses_saved_preference() {
        assert_eq!(
            effective_backend_for(true, Some(true)),
            EffectiveBackend::Tmux
        );
        assert_eq!(
            effective_backend_for(true, Some(false)),
            EffectiveBackend::Native
        );
    }

    #[test]
    fn tmux_available_without_preference_requires_choice() {
        assert_eq!(effective_backend_for(true, None), EffectiveBackend::Native);
        assert!(needs_tmux_choice(true, None));
        assert!(!needs_tmux_choice(true, Some(true)));
        assert!(!needs_tmux_choice(true, Some(false)));
        assert!(!needs_tmux_choice(false, None));
    }
}
