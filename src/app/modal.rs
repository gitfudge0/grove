use super::{OnboardStep, ThemePickerScope};
use grove_core::session::Session;

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
    /// Two-step add-project flow. Marker only — the wizard's state
    /// (step/path/dir_sel/name/git-probe/init_git/note) lives in
    /// `Grove::add_project` (`Some` exactly when this variant is active); see
    /// `crate::gui::add_project`.
    AddProject,
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
    /// pill). Marker only — the palette's transient presentation state
    /// (input/selection/options/switch/row-actions/settings — three states:
    /// root, typing/browse-all, and options, see
    /// `crate::gui::session_launcher::LauncherState`'s doc comment) lives in
    /// `Grove::launcher` (`Some` exactly when this variant is active).
    SessionLauncher,
    ThemePicker {
        sel_dark: usize,
        sel_light: usize,
        tab: grove_core::theme::ThemeKind,
        original: grove_core::theme::Theme,
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
    /// Dedicated custom-theme management modal (⌘M / "Manage themes…" from
    /// the palette's Theme pane). This struct is the LIST sub-view's state
    /// (per-row Rename/Duplicate/Delete/Edit, global "New theme"); the paste-
    /// first EDITOR sub-view's state lives separately in
    /// `Grove::theme_manager_editor` (a `text_editor::Content` can't live in
    /// this cloneable `Modal` — same reason `ScriptsEditorState` lives
    /// outside it). The editor is showing whenever that field is `Some`.
    ThemeManager {
        /// Index into `theme::all_custom_themes()`.
        selected: usize,
        /// Inline rename in progress: `(original_name, live_buffer)`.
        rename: Option<(String, String)>,
        /// Inline error under the row being renamed (e.g. name collision).
        rename_error: Option<String>,
        /// Custom theme pending a delete confirmation, by name.
        pending_delete: Option<String>,
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
