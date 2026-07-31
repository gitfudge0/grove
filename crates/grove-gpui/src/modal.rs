//! The pure modal state machine: the single slot, the per-modal Escape
//! verdicts, the key-context strings, and the quit-confirm clobber rule.
//!
//! No gpui types live here. `views::modals` translates gpui keystrokes into
//! [`ModalKey`]/[`ModalMods`], asks this module for a [`ModalKeyVerdict`], and
//! performs it. Ported from `src/app/modal.rs:5-186` (the variant set) and
//! `src/gui/update/modals.rs:69-336,645-702` (the lifecycle and the keyboard
//! table).
//!
//! # `should_forward`, `MODAL_OPEN` and `PALETTE_OPEN` have no counterpart here
//!
//! The iced front end needs three statics and an event-forwarding predicate
//! (`src/gui/update/pty_input.rs:299-362`) because a focused `text_input`
//! captures keys and never tells the app. gpui's dispatch is structural
//! instead, so all three of `should_forward`'s carve-outs are paid for by
//! construction and **neither static is ported** (carried decision 3):
//!
//! | iced carve-out | gpui replacement |
//! |---|---|
//! | Escape, forwarded despite capture (`pty_input.rs:349-352`) | `InputState::escape()` calls `cx.propagate()` (vendored `input/state.rs:1685`) unless `clean_on_escape` is set. `ModalInput` **never** sets it, so Escape reaches the layer from inside a focused field. |
//! | A global-mods chord while a modal is open (`pty_input.rs:357-359`) | Each modal declares its own `key_context` (see [`ModalKind::key_context`]) and binds its chords there as gpui **actions**. An action never arrives at the `Input` as text. |
//! | ←/→ while the palette is open (`pty_input.rs:353-356`) | A `"<ModalContext> > Input"` descendant binding registered **after** `gpui_component::init`, which out-ranks gpui-component's plain `"Input"` binding at the same dispatch node. Enabled per modal by `wants_arrows`. (The plan called for capture-phase interception; that does not work at this gpui rev — see `views::modals::input`'s module doc for the dispatch-order proof.) |
//!
//! The drift guard [`bound_chords`] + its test is what keeps the second row
//! honest: a context may not bind a chord this module's verdict table ignores.

// The full variant set and verdict table are ported ahead of the views that
// consume them (Tasks 3-6 fill them in one wave at a time).
#![allow(dead_code)]

// ── the payload types ported from `src/app/modal.rs` ─────────────────────

/// What a `Confirm` modal is actually confirming (`src/app/modal.rs:177-186`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    RemoveProject(usize),
    /// Worktree path.
    RemoveWorktree(String),
    InitAndAddWorktree {
        name: String,
    },
    /// Close grove despite running native sessions.
    Quit,
}

/// Stage of an in-progress worktree teardown (`src/app/modal.rs:152-161`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownStage {
    /// The teardown script is running in the modal's embedded PTY.
    RunningScript,
    /// Script finished; `git worktree remove` is executing.
    Removing,
    /// Done — `failed` is set if removal failed.
    Done { failed: bool },
}

/// The onboarding wizard's step sequence (`src/app/onboarding.rs:37-42`).
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
}

/// Whether a theme picker edits the app theme or one project's pinned theme
/// (`src/app/theme_picker.rs:17-23`). Keyed by project **name**, not index —
/// indices shift under add/remove, names do not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemePickerScope {
    App,
    Project(String),
}

/// The scripts editor's three buffers. Pure `String`s, so they survive the
/// ScriptsEditor → ThemePicker → ScriptsEditor round trip inside the slot
/// itself (the documented `open_child` exception, `modals.rs:660-668`); the
/// gpui `InputState`s are re-seeded from them on re-mount.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptsEditorState {
    pub project_path: String,
    pub setup: String,
    pub run: String,
    pub teardown: String,
}

/// Where a `ThemePicker` goes when it closes (`src/app/modal.rs:79-82` plus
/// the ScriptsEditor round trip).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemePickerReturn {
    /// Close outright.
    Close,
    /// Reopen `Settings` — the picker was entered from the Appearance section.
    Settings,
    /// Reopen the scripts editor with its buffers intact.
    ScriptsEditor(Box<ScriptsEditorState>),
}

// ── the slot's variant set ───────────────────────────────────────────────

/// One variant per modal, carrying exactly the state that variant needs.
///
/// Unlike iced's `Modal`, the `Grove`-owned child state (`add_project`,
/// `launcher`, `scripts_editor`, `theme_manager_editor`) lives **inline** here.
/// That turns `set_modal`'s clear-by-default discipline (`modals.rs:645-651`)
/// into a type property: replacing the slot drops the old state and forgetting
/// to clear it is impossible (carried decision 4).
#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
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
    /// Two-step add-project wizard (Task 4 owns the inner state).
    AddProject(Box<AddProjectState>),
    /// Two-stage project removal: confirmation (with the optional
    /// "also delete worktrees on disk" checkbox) then a progress view.
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
    /// Blocking archive gate. `sessions` is one row per SESSION (never per
    /// worktree — a single worktree can hold several), deliberately NOT
    /// filtered to running sessions so the gate's count can never disagree
    /// with what `kill_sessions_for_project` would kill, and recomputed after
    /// every kill (`modals.rs:703-745`).
    ArchiveProject {
        idx: usize,
        name: String,
        /// `(worktree display name, agent label, is_running)`.
        sessions: Vec<(String, String, bool)>,
    },
    /// Marker: every row derives live from `store.archived_projects()`.
    ArchivedProjects,
    Message(String),
    TmuxChoice,
    AgentPicker {
        project: String,
        wt_path: String,
        sel: usize,
    },
    /// The recents-first command palette (Task 5 owns the inner state).
    SessionLauncher(Box<LauncherSlotState>),
    ThemePicker {
        sel_dark: usize,
        sel_light: usize,
        /// `true` = the dark tab is showing.
        dark_tab: bool,
        /// Theme name to restore if the picker is cancelled.
        original: String,
        follow_system: bool,
        scope: ThemePickerScope,
        /// Project scope only: the "Default (follow app)" row is selected.
        project_use_default: bool,
        return_to: ThemePickerReturn,
    },
    /// Custom-theme management. The paste-first editor sub-view is open
    /// whenever `editor` is `Some`.
    ThemeManager {
        /// Index into `theme::all_custom_themes()`.
        selected: usize,
        /// Inline rename in progress: `(original_name, live_buffer)`.
        rename: Option<(String, String)>,
        /// Inline error under the row being renamed (e.g. a name collision).
        rename_error: Option<String>,
        /// Custom theme pending a delete confirmation, by name.
        pending_delete: Option<String>,
        /// The multiline editor's buffer, when the editor sub-view is showing.
        editor: Option<String>,
    },
    /// Every control persists immediately; there is no apply/cancel footer.
    Settings,
    /// Registry-generated shortcut reference. Esc or its own chord closes.
    ShortcutOverlay,
    /// Worktree teardown. The live PTY lives on the view; only the stage and
    /// the paths are pure.
    Teardown {
        wt_path: String,
        project_path: String,
        stage: TeardownStage,
        message: String,
        /// Set once the blocking `git worktree remove` has been kicked off, so
        /// a `Removing` frame paints before the removal runs
        /// (`src/app/modal.rs:171-174`). In gpui the removal is on the
        /// background executor, which makes this a paint-ordering detail
        /// rather than a hack — kept so the stage sequence stays observable.
        removal_started: bool,
    },
    /// Per-project lifecycle-scripts editor.
    ScriptsEditor(Box<ScriptsEditorState>),
    /// Apply-in-progress overlay (Plan 09 fills the live stages).
    Updating,
    /// Release notes. Overlays Settings and returns to it on dismiss
    /// (`src/gui/update/upgrade.rs:127-149`).
    Changelog {
        return_to_settings: bool,
    },
    /// First-run onboarding wizard. Rendered full-viewport as a screen
    /// replacement, never through the scrim (`view/modals/mod.rs:107-110`).
    Onboarding {
        step: OnboardStep,
        path: String,
        dir_sel: usize,
        name: Option<String>,
        note: Option<String>,
        added_proj: Option<usize>,
        agent_sel: usize,
        /// `true` = skip permission prompts. "safe" (`false`) is preselected.
        perms_skip: bool,
        /// Project step only: `false` = the path field has focus.
        name_focused: bool,
    },
}

/// The add-project wizard's two steps (`src/gui/add_project.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddProjectStep {
    #[default]
    PickSource,
    Details,
}

/// Placeholder for the add-project wizard's state; Task 4 fills it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddProjectState {
    pub step: AddProjectStep,
    /// Step 1: the typed path buffer. Step 2: the canonicalized folder.
    pub path: String,
    /// Directory-match cursor for the step-1 autocomplete list.
    pub dir_sel: usize,
    /// Project-name override. Left empty, the folder basename is used.
    pub name: String,
    /// Inline validation message, cleared on the next edit.
    pub note: Option<String>,
    /// "Initialize git repository" checkbox (meaningful only when the probe
    /// said the folder is not a repo).
    pub init_git: bool,
    /// The upfront git probe, encoded: `Some(branch)` = a repo on that branch,
    /// `None` = not a repo. Re-probed on every folder choice, never persisted.
    pub git_branch: Option<String>,
}

/// Which list state the palette is showing (`session_launcher/state.rs:13-58`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherView {
    /// Recents + actions.
    #[default]
    Root,
    /// Every project x worktree combo, fuzzy-filtered by the query.
    BrowseAll,
    /// The switch-to-session drill-in: sessions, then home terminals.
    Switch,
    /// The Tab-revealed row-action strip.
    RowActions,
    /// The scoped settings drill-in.
    Settings,
}

/// The palette's slot-side state. The row model and the ranking live in
/// [`crate::launcher`], which is pure and gpui-free.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherSlotState {
    pub query: String,
    pub sel: usize,
    pub view: LauncherView,
    /// Scroll offset of the visible window, in rows.
    pub offset: usize,
    /// The row the drill-ins act on, by identity rather than index — a query
    /// edit or a recency re-sort must not activate a different row
    /// (`session_launcher/state.rs:28-48`).
    pub anchor: Option<crate::launcher::RowIdentity>,
}

/// The slot's discriminant — what the key table, the key contexts and the
/// drift guard are indexed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModalKind {
    Input,
    Confirm,
    AddProject,
    RemoveProject,
    ArchiveProject,
    ArchivedProjects,
    Message,
    TmuxChoice,
    AgentPicker,
    SessionLauncher,
    ThemePicker,
    ThemeManager,
    Settings,
    ShortcutOverlay,
    Teardown,
    ScriptsEditor,
    Updating,
    Changelog,
    Onboarding,
}

impl ModalKind {
    /// Every kind, for the table-driven tests and the drift guards.
    pub const ALL: [ModalKind; 19] = [
        ModalKind::Input,
        ModalKind::Confirm,
        ModalKind::AddProject,
        ModalKind::RemoveProject,
        ModalKind::ArchiveProject,
        ModalKind::ArchivedProjects,
        ModalKind::Message,
        ModalKind::TmuxChoice,
        ModalKind::AgentPicker,
        ModalKind::SessionLauncher,
        ModalKind::ThemePicker,
        ModalKind::ThemeManager,
        ModalKind::Settings,
        ModalKind::ShortcutOverlay,
        ModalKind::Teardown,
        ModalKind::ScriptsEditor,
        ModalKind::Updating,
        ModalKind::Changelog,
        ModalKind::Onboarding,
    ];

    /// The gpui key-context string this modal's root element declares
    /// (spec §4: each modal is its own entity with its own context).
    pub fn key_context(self) -> &'static str {
        match self {
            ModalKind::Input => "ModalInput",
            ModalKind::Confirm => "ModalConfirm",
            ModalKind::AddProject => "ModalAddProject",
            ModalKind::RemoveProject => "ModalRemoveProject",
            ModalKind::ArchiveProject => "ModalArchiveProject",
            ModalKind::ArchivedProjects => "ModalArchivedProjects",
            ModalKind::Message => "ModalMessage",
            ModalKind::TmuxChoice => "ModalTmuxChoice",
            ModalKind::AgentPicker => "ModalAgentPicker",
            ModalKind::SessionLauncher => "ModalSessionLauncher",
            ModalKind::ThemePicker => "ModalThemePicker",
            ModalKind::ThemeManager => "ModalThemeManager",
            ModalKind::Settings => "ModalSettings",
            ModalKind::ShortcutOverlay => "ModalShortcutOverlay",
            ModalKind::Teardown => "ModalTeardown",
            ModalKind::ScriptsEditor => "ModalScriptsEditor",
            ModalKind::Updating => "ModalUpdating",
            ModalKind::Changelog => "ModalChangelog",
            ModalKind::Onboarding => "ModalOnboarding",
        }
    }

    /// Onboarding is rendered as a **screen replacement**: full-viewport, no
    /// sidebar, no statusbar, no scrim (`view/modals/mod.rs:107-110`).
    pub fn is_screen_replacement(self) -> bool {
        matches!(self, ModalKind::Onboarding)
    }

    /// The palette top-drops instead of centering
    /// (`view/modals/mod.rs:114-121`).
    pub fn top_drops(self) -> bool {
        matches!(self, ModalKind::SessionLauncher)
    }

    /// Whether this modal's text field claims ←/→ (carried decision 2: the
    /// `PALETTE_OPEN` carve-out). Everything else lets the caret have them.
    pub fn wants_arrows(self) -> bool {
        matches!(self, ModalKind::SessionLauncher | ModalKind::AddProject)
    }

    /// Whether this modal claims Tab from its single-line fields. Multiline
    /// buffers leave it clear and Tab indents (carried decision 2).
    pub fn wants_tab(self) -> bool {
        matches!(
            self,
            ModalKind::Onboarding | ModalKind::AddProject | ModalKind::ThemePicker
        )
    }
}

impl Modal {
    pub fn kind(&self) -> ModalKind {
        match self {
            Modal::Input { .. } => ModalKind::Input,
            Modal::Confirm { .. } => ModalKind::Confirm,
            Modal::AddProject(_) => ModalKind::AddProject,
            Modal::RemoveProject { .. } => ModalKind::RemoveProject,
            Modal::ArchiveProject { .. } => ModalKind::ArchiveProject,
            Modal::ArchivedProjects => ModalKind::ArchivedProjects,
            Modal::Message(_) => ModalKind::Message,
            Modal::TmuxChoice => ModalKind::TmuxChoice,
            Modal::AgentPicker { .. } => ModalKind::AgentPicker,
            Modal::SessionLauncher(_) => ModalKind::SessionLauncher,
            Modal::ThemePicker { .. } => ModalKind::ThemePicker,
            Modal::ThemeManager { .. } => ModalKind::ThemeManager,
            Modal::Settings => ModalKind::Settings,
            Modal::ShortcutOverlay => ModalKind::ShortcutOverlay,
            Modal::Teardown { .. } => ModalKind::Teardown,
            Modal::ScriptsEditor(_) => ModalKind::ScriptsEditor,
            Modal::Updating => ModalKind::Updating,
            Modal::Changelog { .. } => ModalKind::Changelog,
            Modal::Onboarding { .. } => ModalKind::Onboarding,
        }
    }

    /// The quit confirm, built from the running **native** session count
    /// (`modals.rs:338-366`). tmux-backed sessions survive grove and are not
    /// counted by the caller.
    pub fn quit_confirm(native_running: usize) -> Modal {
        let noun = if native_running == 1 {
            "session"
        } else {
            "sessions"
        };
        Modal::Confirm {
            title: "Quit Grove?".into(),
            prompt: format!("{native_running} running {noun} will end. quit anyway?"),
            destructive: true,
            kind: ConfirmKind::Quit,
        }
    }
}

// ── the pure key alphabet ────────────────────────────────────────────────

/// The subset of keystrokes the modal table reasons about. The layer maps
/// gpui's `Keystroke` onto this; anything unmapped is `FallThrough`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKey {
    Escape,
    Enter,
    Tab,
    Space,
    Up,
    Down,
    Left,
    Right,
    Char(char),
}

/// Modifier snapshot. `platform` is the global-shortcut modifier (Cmd on
/// macOS, Ctrl+Shift elsewhere) already resolved by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModalMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

impl ModalMods {
    pub const NONE: ModalMods = ModalMods {
        ctrl: false,
        alt: false,
        shift: false,
        platform: false,
    };
    pub const CTRL: ModalMods = ModalMods {
        ctrl: true,
        alt: false,
        shift: false,
        platform: false,
    };
}

/// Extra facts the verdict needs that the slot does not own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyCtx {
    /// An upgrade is mid-flight, so `Updating`'s Escape is refused
    /// (`modals.rs:250-256`).
    pub update_in_flight: bool,
    /// This keystroke *is* the shortcut overlay's own registry chord
    /// (`modals.rs:301-308`).
    pub is_shortcut_overlay_chord: bool,
}

/// What the layer must do with a keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalKeyVerdict {
    /// Route through [`ModalSlot::cancel`] — which is *not* always "close"
    /// (Teardown, RemoveProject).
    Close,
    /// The modal's primary action (Enter on `Input`, `AgentPicker`).
    Submit,
    /// Move the modal's selection cursor by `n` rows.
    Move(i32),
    /// A modal-specific action the view performs.
    Custom(ModalAction),
    /// Claimed by this modal and deliberately does nothing.
    Ignore,
    /// Not claimed: the modal's own sub-widget (a delegate wizard, the
    /// palette, the multiline editor) or the focused `Input` gets it.
    FallThrough,
}

/// The modal-specific half of [`ModalKeyVerdict::Custom`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalAction {
    /// `Confirm`: resolve yes/no.
    Confirm(bool),
    /// `ArchiveProject`: `y` routes through the gate re-check so it cannot
    /// bypass a disabled Archive button (`modals.rs:148-160`).
    ArchiveConfirm,
    /// `RemoveProject`: `y` starts the removal.
    RemoveProjectConfirm,
    /// `RemoveProject`: Space toggles the delete-worktrees checkbox.
    ToggleRemoveWorktrees,
    /// `TmuxChoice`: an explicit pick, which is the only thing that persists.
    ChooseTmux(bool),
    /// `AgentPicker`: Space toggles "make this the default agent".
    ToggleDefaultAgent,
    ThemePickerSubmit,
    ThemePickerSwitchTab,
    ThemeManagerDeleteConfirm,
    ThemeManagerDeleteCancel,
    ThemeManagerRenameSubmit,
    ThemeManagerRenameCancel,
    OnboardSkip,
    OnboardAdvance,
    /// `Onboarding`: Tab alternates path/name focus on the Project step.
    OnboardToggleFocus,
}

// ── the slot ─────────────────────────────────────────────────────────────

/// The result of [`ModalSlot::cancel`] — cancel is not a synonym for close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The slot is now empty.
    Closed,
    /// Teardown only: skip the still-running script and proceed to removal.
    SkippedTeardownScript,
    /// Refused: an in-flight removal cannot be interrupted.
    Refused,
    /// The slot was repointed at the modal this one returns to.
    ReturnedTo(ModalKind),
}

/// The single modal slot. One deep, replace-don't-stack, no back-stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModalSlot {
    modal: Option<Modal>,
}

impl ModalSlot {
    pub fn new() -> Self {
        Self { modal: None }
    }

    pub fn is_open(&self) -> bool {
        self.modal.is_some()
    }

    pub fn kind(&self) -> Option<ModalKind> {
        self.modal.as_ref().map(Modal::kind)
    }

    pub fn get(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut Modal> {
        self.modal.as_mut()
    }

    /// Open `modal`, **replacing** whatever was there and dropping its state.
    /// There is no stack and no restore (carried decision 4).
    pub fn open(&mut self, modal: Modal) {
        self.modal = Some(modal);
    }

    /// The window's close request. Known, deliberately preserved gap: the quit
    /// confirm clobbers any open modal and cancelling does not restore it
    /// (`modals.rs:350-354`).
    pub fn open_quit_confirm(&mut self, native_running: usize) {
        self.open(Modal::quit_confirm(native_running));
    }

    /// Force the slot empty, ignoring every per-modal cancel rule. Only for
    /// paths that already decided (a confirm resolving, a wizard finishing).
    pub fn close(&mut self) {
        self.modal = None;
    }

    /// Cancel the current modal. Port of `cancel_modal` (`modals.rs:677-702`)
    /// plus the two documented return trips.
    pub fn cancel(&mut self) -> CancelOutcome {
        match self.modal.as_mut() {
            None => CancelOutcome::Closed,
            // Teardown repurposes cancel: skip a still-running script (proceed
            // to removal), dismiss once removal has finished, and do nothing
            // mid-removal — an in-flight `git worktree remove` cannot be
            // safely interrupted, and there is no button for that stage
            // either.
            Some(Modal::Teardown { stage, .. }) => match *stage {
                TeardownStage::Done { .. } => {
                    self.modal = None;
                    CancelOutcome::Closed
                }
                TeardownStage::RunningScript => CancelOutcome::SkippedTeardownScript,
                TeardownStage::Removing => CancelOutcome::Refused,
            },
            // `handle_remove_project_key` returns early while busy, so cancel
            // is refused for the same reason.
            Some(Modal::RemoveProject { in_progress, .. }) if *in_progress => {
                CancelOutcome::Refused
            }
            Some(Modal::Changelog {
                return_to_settings: true,
            }) => {
                self.modal = Some(Modal::Settings);
                CancelOutcome::ReturnedTo(ModalKind::Settings)
            }
            Some(Modal::ThemePicker { return_to, .. }) => {
                match std::mem::replace(return_to, ThemePickerReturn::Close) {
                    ThemePickerReturn::Close => {
                        self.modal = None;
                        CancelOutcome::Closed
                    }
                    ThemePickerReturn::Settings => {
                        self.modal = Some(Modal::Settings);
                        CancelOutcome::ReturnedTo(ModalKind::Settings)
                    }
                    // The documented `open_child` exception (`modals.rs:660-668`):
                    // the editor's buffers ride through the picker and come back.
                    ThemePickerReturn::ScriptsEditor(state) => {
                        self.modal = Some(Modal::ScriptsEditor(state));
                        CancelOutcome::ReturnedTo(ModalKind::ScriptsEditor)
                    }
                }
            }
            Some(_) => {
                self.modal = None;
                CancelOutcome::Closed
            }
        }
    }
}

/// The per-modal keyboard verdict — a table, one row per arm of
/// `handle_modal_key` (`modals.rs:94-336`) and `handle_remove_project_key`
/// (`modals.rs:69-93`).
pub fn key_verdict(modal: &Modal, key: ModalKey, mods: ModalMods, ctx: KeyCtx) -> ModalKeyVerdict {
    use ModalAction as A;
    use ModalKey as K;
    use ModalKeyVerdict as V;

    /// `Key::Character` matching in iced is case-insensitive across the `y`/`n`
    /// style arms (`"y" | "Y"`); fold once here so every arm below can use the
    /// lowercase form.
    fn ch(key: ModalKey) -> Option<char> {
        match key {
            ModalKey::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        }
    }
    /// Ctrl+C cancels the `Input` prompt and either add-project step
    /// (`modals.rs:100-104,120-131`).
    fn ctrl_c(key: ModalKey, mods: ModalMods) -> bool {
        mods.ctrl && ch(key) == Some('c')
    }

    match modal {
        // Text entry, caret movement, selection and paste belong to the
        // focused `Input`; only the lifecycle keys are claimed here.
        Modal::Input { .. } => match key {
            K::Escape => V::Close,
            K::Enter => V::Submit,
            _ if ctrl_c(key, mods) => V::Close,
            _ => V::FallThrough,
        },

        // Esc from the pick-source step and Ctrl+C from either step cancel the
        // whole wizard; everything else is the wizard delegate's
        // (`modals.rs:117-136`).
        Modal::AddProject(st) => {
            let cancels =
                ctrl_c(key, mods) || (key == K::Escape && st.step == AddProjectStep::PickSource);
            if cancels {
                V::Close
            } else {
                V::FallThrough
            }
        }

        Modal::Confirm { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Custom(A::Confirm(false)),
            (K::Enter, _) => V::Custom(A::Confirm(true)),
            (_, Some('y')) => V::Custom(A::Confirm(true)),
            (_, Some('n')) => V::Custom(A::Confirm(false)),
            _ => V::Ignore,
        },

        // y/n mirrors the destructive-confirm convention. `y` routes through
        // the gate's own re-check so it cannot bypass a disabled button.
        Modal::ArchiveProject { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Close,
            (_, Some('y')) => V::Custom(A::ArchiveConfirm),
            (_, Some('n')) => V::Close,
            _ => V::Ignore,
        },

        // Esc/n cancel, y confirms (Enter deliberately does not), Space
        // toggles the checkbox — all ignored while removal is in flight
        // (`handle_remove_project_key`, `modals.rs:69-93`).
        Modal::RemoveProject { in_progress, .. } => {
            if *in_progress {
                return V::Ignore;
            }
            match (key, ch(key)) {
                (K::Escape, _) => V::Close,
                (K::Space, _) => V::Custom(A::ToggleRemoveWorktrees),
                (_, Some('y')) => V::Custom(A::RemoveProjectConfirm),
                (_, Some('n')) => V::Close,
                _ => V::Ignore,
            }
        }

        Modal::Message(_) => match (key, ch(key)) {
            (K::Escape | K::Enter, _) | (_, Some('q')) => V::Close,
            _ => V::Ignore,
        },

        Modal::TmuxChoice => match (key, ch(key)) {
            (K::Enter, _) | (_, Some('t' | 'y')) => V::Custom(A::ChooseTmux(true)),
            (_, Some('n')) => V::Custom(A::ChooseTmux(false)),
            // Esc dismisses without persisting, so the choice is re-asked on
            // the next launch. Only explicit picks record a backend.
            (K::Escape, _) => V::Close,
            _ => V::Ignore,
        },

        Modal::AgentPicker { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Close,
            (K::Enter, _) => V::Submit,
            (K::Space, _) => V::Custom(A::ToggleDefaultAgent),
            (K::Down, _) | (_, Some('j')) => V::Move(1),
            (K::Up, _) | (_, Some('k')) => V::Move(-1),
            _ => V::Ignore,
        },

        Modal::ThemePicker { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Close,
            (K::Enter, _) => V::Custom(A::ThemePickerSubmit),
            (K::Down, _) | (_, Some('j')) => V::Move(1),
            (K::Up, _) | (_, Some('k')) => V::Move(-1),
            (K::Tab, _) | (_, Some('h' | 'l')) => V::Custom(A::ThemePickerSwitchTab),
            _ => V::Ignore,
        },

        // Three nested sub-states, in this precedence order: the editor
        // sub-view delegates wholesale, then the delete confirm, then the
        // inline rename, then the plain list (`modals.rs:186-228`).
        Modal::ThemeManager {
            rename,
            pending_delete,
            editor,
            ..
        } => {
            if editor.is_some() {
                V::FallThrough
            } else if pending_delete.is_some() {
                // Enter works here too (unlike `confirm_modal`), since this
                // dialog has no other use for it.
                match (key, ch(key)) {
                    (K::Enter, _) | (_, Some('y')) => V::Custom(A::ThemeManagerDeleteConfirm),
                    (K::Escape, _) | (_, Some('n')) => V::Custom(A::ThemeManagerDeleteCancel),
                    _ => V::Ignore,
                }
            } else if rename.is_some() {
                match key {
                    K::Enter => V::Custom(A::ThemeManagerRenameSubmit),
                    K::Escape => V::Custom(A::ThemeManagerRenameCancel),
                    // The rename buffer is a focused field; text is its own.
                    _ => V::FallThrough,
                }
            } else {
                match key {
                    K::Escape => V::Close,
                    K::Down => V::Move(1),
                    K::Up => V::Move(-1),
                    _ => V::Ignore,
                }
            }
        }

        // The palette owns its whole keyboard (`handle_session_launcher_key`).
        Modal::SessionLauncher(_) => V::FallThrough,

        Modal::Settings | Modal::ArchivedProjects | Modal::ScriptsEditor(_) => match key {
            K::Escape => V::Close,
            _ => V::Ignore,
        },

        // Cancel is gated by stage inside `ModalSlot::cancel`; there is no
        // separate refusal here.
        Modal::Teardown { .. } => match key {
            K::Escape => V::Close,
            _ => V::Ignore,
        },

        Modal::Updating => match key {
            K::Escape if !ctx.update_in_flight => V::Close,
            _ => V::Ignore,
        },

        Modal::Changelog { .. } => match key {
            K::Escape => V::Close,
            _ => V::Ignore,
        },

        // Escape **or** the overlay's own registry chord closes it
        // (`modals.rs:301-308`).
        Modal::ShortcutOverlay => match key {
            K::Escape => V::Close,
            _ if ctx.is_shortcut_overlay_chord => V::Close,
            _ => V::Ignore,
        },

        Modal::Onboarding { step, .. } => {
            let project_step = *step == OnboardStep::Project;
            match key {
                K::Escape => V::Custom(A::OnboardSkip),
                K::Enter => V::Custom(A::OnboardAdvance),
                K::Down if project_step => V::Move(1),
                K::Up if project_step => V::Move(-1),
                K::Tab if project_step => V::Custom(A::OnboardToggleFocus),
                _ => V::Ignore,
            }
        }
    }
}

/// Whether Escape has something to dismiss when **no** modal is open. `false`
/// means Escape must reach the PTY — many TUI programs need it, and
/// swallowing it unconditionally would regress that. Port of
/// `escape_should_dismiss` (`pty_input.rs:364-378`).
pub fn escape_should_dismiss(
    pending_kill: bool,
    pending_kill_terminal: bool,
    agent_menu_open: bool,
    attention_open: bool,
) -> bool {
    pending_kill || pending_kill_terminal || agent_menu_open || attention_open
}

/// A chord bound as a gpui **action** in a modal's own key context (carried
/// decision 3, second row of the module-doc table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModalChord {
    pub key: ModalKey,
    pub mods: ModalMods,
}

/// The chords each modal's key context binds as actions. The drift guard test
/// asserts every one of them is claimed by [`key_verdict`]; a context that
/// binds a chord the table ignores would silently swallow it.
pub fn bound_chords(kind: ModalKind) -> &'static [ModalChord] {
    match kind {
        // The overlay closes on Escape **or** its own registry chord
        // (`modals.rs:301-308`). The chord is bound in this context so it
        // arrives as an action rather than as text.
        ModalKind::ShortcutOverlay => &[ModalChord {
            key: ModalKey::Char('/'),
            mods: ModalMods {
                ctrl: false,
                alt: false,
                shift: false,
                platform: true,
            },
        }],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts(setup: &str) -> ScriptsEditorState {
        ScriptsEditorState {
            project_path: "/p".into(),
            setup: setup.into(),
            run: String::new(),
            teardown: String::new(),
        }
    }

    fn theme_picker(return_to: ThemePickerReturn) -> Modal {
        Modal::ThemePicker {
            sel_dark: 0,
            sel_light: 0,
            dark_tab: true,
            original: "tokyonight-storm".into(),
            follow_system: false,
            scope: ThemePickerScope::App,
            project_use_default: false,
            return_to,
        }
    }

    fn teardown(stage: TeardownStage) -> Modal {
        Modal::Teardown {
            wt_path: "/w".into(),
            project_path: "/p".into(),
            stage,
            message: String::new(),
            removal_started: false,
        }
    }

    fn remove_project(in_progress: bool) -> Modal {
        Modal::RemoveProject {
            idx: 0,
            name: "p".into(),
            project_path: "/p".into(),
            worktrees: vec![],
            also_remove_worktrees: false,
            in_progress,
            done: 0,
            current: String::new(),
            errors: vec![],
        }
    }

    fn theme_manager(
        rename: Option<(String, String)>,
        pending_delete: Option<String>,
        editor: Option<String>,
    ) -> Modal {
        Modal::ThemeManager {
            selected: 0,
            rename,
            rename_error: None,
            pending_delete,
            editor,
        }
    }

    // ── Step 1: the slot's invariants ────────────────────────────────────

    #[test]
    fn opening_a_modal_replaces_the_open_one_and_drops_its_state() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::ScriptsEditor(Box::new(scripts("echo hi"))));
        slot.open(Modal::Settings);
        assert_eq!(slot.kind(), Some(ModalKind::Settings));
        // There is no stack: cancelling the replacement leaves nothing behind.
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn quit_confirm_clobbers_the_open_modal_and_cancelling_leaves_none() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::Settings);
        slot.open_quit_confirm(2);
        let Some(Modal::Confirm { kind, prompt, .. }) = slot.get() else {
            unreachable!()
        };
        assert_eq!(*kind, ConfirmKind::Quit);
        assert!(prompt.contains("2 running sessions"));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        // NOT the clobbered Settings modal — the preserved gap.
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn quit_confirm_singular_noun() {
        let Modal::Confirm { prompt, .. } = Modal::quit_confirm(1) else {
            unreachable!()
        };
        assert!(prompt.contains("1 running session will end"), "{prompt}");
    }

    #[test]
    fn cancel_on_teardown_skips_the_script_then_closes_and_refuses_mid_removal() {
        let mut slot = ModalSlot::new();
        slot.open(teardown(TeardownStage::RunningScript));
        assert_eq!(slot.cancel(), CancelOutcome::SkippedTeardownScript);
        assert_eq!(slot.kind(), Some(ModalKind::Teardown));

        slot.open(teardown(TeardownStage::Removing));
        assert_eq!(slot.cancel(), CancelOutcome::Refused);
        assert_eq!(slot.kind(), Some(ModalKind::Teardown));

        slot.open(teardown(TeardownStage::Done { failed: false }));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn cancel_on_remove_project_is_refused_while_in_progress() {
        let mut slot = ModalSlot::new();
        slot.open(remove_project(true));
        assert_eq!(slot.cancel(), CancelOutcome::Refused);
        assert_eq!(slot.kind(), Some(ModalKind::RemoveProject));

        slot.open(remove_project(false));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn changelog_overlays_settings_and_dismissing_returns_to_settings() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::Settings);
        slot.open(Modal::Changelog {
            return_to_settings: true,
        });
        assert_eq!(
            slot.cancel(),
            CancelOutcome::ReturnedTo(ModalKind::Settings)
        );
        assert_eq!(slot.kind(), Some(ModalKind::Settings));
    }

    #[test]
    fn theme_picker_return_to_settings_round_trip() {
        let mut slot = ModalSlot::new();
        slot.open(theme_picker(ThemePickerReturn::Settings));
        assert_eq!(
            slot.cancel(),
            CancelOutcome::ReturnedTo(ModalKind::Settings)
        );
        assert_eq!(slot.kind(), Some(ModalKind::Settings));

        slot.open(theme_picker(ThemePickerReturn::Close));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn scripts_editor_to_theme_picker_and_back_preserves_the_buffers() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::ScriptsEditor(Box::new(scripts("cargo build"))));
        // The round trip carries the editor state through the picker.
        let Some(Modal::ScriptsEditor(state)) = slot.get() else {
            unreachable!()
        };
        let carried = state.clone();
        slot.open(theme_picker(ThemePickerReturn::ScriptsEditor(carried)));
        assert_eq!(
            slot.cancel(),
            CancelOutcome::ReturnedTo(ModalKind::ScriptsEditor)
        );
        let Some(Modal::ScriptsEditor(state)) = slot.get() else {
            unreachable!()
        };
        assert_eq!(state.setup, "cargo build");
    }

    // ── Step 2: the per-modal keyboard verdicts ──────────────────────────

    fn v(modal: &Modal, key: ModalKey) -> ModalKeyVerdict {
        key_verdict(modal, key, ModalMods::NONE, KeyCtx::default())
    }

    fn input() -> Modal {
        Modal::Input {
            title: "t".into(),
            buffer: String::new(),
            note: None,
        }
    }

    fn confirm() -> Modal {
        Modal::Confirm {
            title: "t".into(),
            prompt: "p".into(),
            destructive: true,
            kind: ConfirmKind::RemoveWorktree("/w".into()),
        }
    }

    #[test]
    fn input_escape_enter_and_ctrl_c() {
        let m = input();
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Submit);
        assert_eq!(
            key_verdict(&m, ModalKey::Char('c'), ModalMods::CTRL, KeyCtx::default()),
            ModalKeyVerdict::Close
        );
        // A bare letter is text, not a command.
        assert_eq!(v(&m, ModalKey::Char('c')), ModalKeyVerdict::FallThrough);
    }

    #[test]
    fn confirm_escape_is_no_enter_is_yes_and_y_n() {
        let m = confirm();
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::Confirm(false))
        );
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::Confirm(true))
        );
        assert_eq!(
            v(&m, ModalKey::Char('Y')),
            ModalKeyVerdict::Custom(ModalAction::Confirm(true))
        );
        assert_eq!(
            v(&m, ModalKey::Char('n')),
            ModalKeyVerdict::Custom(ModalAction::Confirm(false))
        );
    }

    #[test]
    fn archive_project_y_routes_through_the_gate_recheck() {
        let m = Modal::ArchiveProject {
            idx: 0,
            name: "p".into(),
            sessions: vec![],
        };
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(v(&m, ModalKey::Char('n')), ModalKeyVerdict::Close);
        assert_eq!(
            v(&m, ModalKey::Char('y')),
            ModalKeyVerdict::Custom(ModalAction::ArchiveConfirm)
        );
        // Enter deliberately does NOT confirm a destructive gate.
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Ignore);
    }

    #[test]
    fn message_dismisses_on_escape_enter_or_q() {
        let m = Modal::Message("boom".into());
        for k in [ModalKey::Escape, ModalKey::Enter, ModalKey::Char('q')] {
            assert_eq!(v(&m, k), ModalKeyVerdict::Close, "{k:?}");
        }
        assert_eq!(v(&m, ModalKey::Char('z')), ModalKeyVerdict::Ignore);
    }

    #[test]
    fn tmux_choice_escape_persists_nothing() {
        let m = Modal::TmuxChoice;
        for k in [ModalKey::Enter, ModalKey::Char('t'), ModalKey::Char('y')] {
            assert_eq!(
                v(&m, k),
                ModalKeyVerdict::Custom(ModalAction::ChooseTmux(true)),
                "{k:?}"
            );
        }
        assert_eq!(
            v(&m, ModalKey::Char('n')),
            ModalKeyVerdict::Custom(ModalAction::ChooseTmux(false))
        );
        // Escape records NO backend, so the choice is re-asked next launch.
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
    }

    #[test]
    fn theme_picker_arrows_jk_and_tab_hl() {
        let m = theme_picker(ThemePickerReturn::Close);
        assert_eq!(v(&m, ModalKey::Down), ModalKeyVerdict::Move(1));
        assert_eq!(v(&m, ModalKey::Char('j')), ModalKeyVerdict::Move(1));
        assert_eq!(v(&m, ModalKey::Up), ModalKeyVerdict::Move(-1));
        assert_eq!(v(&m, ModalKey::Char('k')), ModalKeyVerdict::Move(-1));
        for k in [ModalKey::Tab, ModalKey::Char('h'), ModalKey::Char('l')] {
            assert_eq!(
                v(&m, k),
                ModalKeyVerdict::Custom(ModalAction::ThemePickerSwitchTab),
                "{k:?}"
            );
        }
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::ThemePickerSubmit)
        );
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
    }

    #[test]
    fn theme_manager_has_three_nested_sub_states() {
        // Editor open wins over everything and delegates.
        let m = theme_manager(None, None, Some(String::new()));
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::FallThrough);

        // pending_delete outranks rename.
        let m = theme_manager(Some(("a".into(), "b".into())), Some("a".into()), None);
        assert_eq!(
            v(&m, ModalKey::Char('y')),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerDeleteConfirm)
        );
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerDeleteConfirm)
        );
        assert_eq!(
            v(&m, ModalKey::Char('n')),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerDeleteCancel)
        );
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerDeleteCancel)
        );

        // Rename: Enter/Escape only.
        let m = theme_manager(Some(("a".into(), "b".into())), None, None);
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerRenameSubmit)
        );
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerRenameCancel)
        );
        // The rename buffer keeps its own text keys.
        assert_eq!(v(&m, ModalKey::Char('x')), ModalKeyVerdict::FallThrough);

        // The plain list.
        let m = theme_manager(None, None, None);
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(v(&m, ModalKey::Down), ModalKeyVerdict::Move(1));
        assert_eq!(v(&m, ModalKey::Up), ModalKeyVerdict::Move(-1));
    }

    #[test]
    fn agent_picker_space_toggles_default() {
        let m = Modal::AgentPicker {
            project: "p".into(),
            wt_path: "/w".into(),
            sel: 0,
        };
        assert_eq!(
            v(&m, ModalKey::Space),
            ModalKeyVerdict::Custom(ModalAction::ToggleDefaultAgent)
        );
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Submit);
        assert_eq!(v(&m, ModalKey::Char('j')), ModalKeyVerdict::Move(1));
        assert_eq!(v(&m, ModalKey::Char('k')), ModalKeyVerdict::Move(-1));
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
    }

    #[test]
    fn updating_escape_only_closes_when_not_mid_update() {
        let m = Modal::Updating;
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(
            key_verdict(
                &m,
                ModalKey::Escape,
                ModalMods::NONE,
                KeyCtx {
                    update_in_flight: true,
                    ..KeyCtx::default()
                }
            ),
            ModalKeyVerdict::Ignore
        );
    }

    #[test]
    fn shortcut_overlay_closes_on_escape_or_its_own_chord() {
        let m = Modal::ShortcutOverlay;
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(
            key_verdict(
                &m,
                ModalKey::Char('/'),
                ModalMods {
                    platform: true,
                    ..ModalMods::NONE
                },
                KeyCtx {
                    is_shortcut_overlay_chord: true,
                    ..KeyCtx::default()
                }
            ),
            ModalKeyVerdict::Close
        );
        assert_eq!(v(&m, ModalKey::Char('/')), ModalKeyVerdict::Ignore);
    }

    #[test]
    fn onboarding_escape_skips_and_tab_alternates_focus_on_the_project_step() {
        let onboard = |step| Modal::Onboarding {
            step,
            path: String::new(),
            dir_sel: 0,
            name: None,
            note: None,
            added_proj: None,
            agent_sel: 0,
            perms_skip: false,
            name_focused: false,
        };
        let m = onboard(OnboardStep::Project);
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::OnboardSkip)
        );
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::OnboardAdvance)
        );
        assert_eq!(
            v(&m, ModalKey::Tab),
            ModalKeyVerdict::Custom(ModalAction::OnboardToggleFocus)
        );
        assert_eq!(v(&m, ModalKey::Down), ModalKeyVerdict::Move(1));

        // Every other step ignores Tab and the arrows.
        let m = onboard(OnboardStep::Welcome);
        assert_eq!(v(&m, ModalKey::Tab), ModalKeyVerdict::Ignore);
        assert_eq!(v(&m, ModalKey::Down), ModalKeyVerdict::Ignore);
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::OnboardAdvance)
        );
    }

    #[test]
    fn escape_only_modals() {
        let scripts_editor = Modal::ScriptsEditor(Box::new(scripts("")));
        for m in [
            &Modal::Settings,
            &Modal::ArchivedProjects,
            &scripts_editor,
            &teardown(TeardownStage::RunningScript),
        ] {
            assert_eq!(v(m, ModalKey::Escape), ModalKeyVerdict::Close, "{m:?}");
            assert_eq!(v(m, ModalKey::Char('q')), ModalKeyVerdict::Ignore, "{m:?}");
        }
    }

    #[test]
    fn remove_project_keys_and_the_busy_refusal() {
        let m = remove_project(false);
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(v(&m, ModalKey::Char('n')), ModalKeyVerdict::Close);
        assert_eq!(
            v(&m, ModalKey::Char('y')),
            ModalKeyVerdict::Custom(ModalAction::RemoveProjectConfirm)
        );
        assert_eq!(
            v(&m, ModalKey::Space),
            ModalKeyVerdict::Custom(ModalAction::ToggleRemoveWorktrees)
        );
        // Enter deliberately does not confirm (`modals.rs:66-68`).
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Ignore);

        // Busy: every key is ignored, Escape included.
        let m = remove_project(true);
        for k in [
            ModalKey::Escape,
            ModalKey::Enter,
            ModalKey::Space,
            ModalKey::Char('y'),
        ] {
            assert_eq!(v(&m, k), ModalKeyVerdict::Ignore, "{k:?}");
        }
    }

    #[test]
    fn add_project_escape_only_cancels_from_pick_source_but_ctrl_c_cancels_anywhere() {
        let mut st = AddProjectState::default();
        let m = Modal::AddProject(Box::new(st.clone()));
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::Close);
        assert_eq!(
            key_verdict(&m, ModalKey::Char('C'), ModalMods::CTRL, KeyCtx::default()),
            ModalKeyVerdict::Close
        );

        st.step = AddProjectStep::Details;
        let m = Modal::AddProject(Box::new(st));
        // Escape on the details step belongs to the wizard delegate.
        assert_eq!(v(&m, ModalKey::Escape), ModalKeyVerdict::FallThrough);
        assert_eq!(
            key_verdict(&m, ModalKey::Char('c'), ModalMods::CTRL, KeyCtx::default()),
            ModalKeyVerdict::Close
        );
    }

    #[test]
    fn the_palette_delegates_every_key() {
        let m = Modal::SessionLauncher(Box::default());
        for k in [
            ModalKey::Escape,
            ModalKey::Enter,
            ModalKey::Left,
            ModalKey::Char('a'),
        ] {
            assert_eq!(v(&m, k), ModalKeyVerdict::FallThrough, "{k:?}");
        }
    }

    // ── Step 5: the drift guard ──────────────────────────────────────────

    #[test]
    fn no_modal_context_binds_a_chord_its_verdict_table_ignores() {
        for kind in ModalKind::ALL {
            for chord in bound_chords(kind) {
                let modal = sample_modal(kind);
                let ctx = KeyCtx {
                    update_in_flight: false,
                    // The overlay's own chord is the only registry chord bound
                    // in a modal context today.
                    is_shortcut_overlay_chord: kind == ModalKind::ShortcutOverlay,
                };
                let verdict = key_verdict(&modal, chord.key, chord.mods, ctx);
                assert!(
                    !matches!(
                        verdict,
                        ModalKeyVerdict::Ignore | ModalKeyVerdict::FallThrough
                    ),
                    "{kind:?} binds {chord:?} but key_verdict returns {verdict:?}"
                );
            }
        }
    }

    #[test]
    fn every_kind_has_a_unique_key_context() {
        let mut seen = std::collections::HashSet::new();
        for kind in ModalKind::ALL {
            assert!(
                seen.insert(kind.key_context()),
                "duplicate key context for {kind:?}"
            );
        }
    }

    #[test]
    fn kind_round_trips_through_every_variant() {
        for kind in ModalKind::ALL {
            assert_eq!(sample_modal(kind).kind(), kind);
        }
    }

    /// One representative `Modal` per kind, for the table-driven tests.
    fn sample_modal(kind: ModalKind) -> Modal {
        match kind {
            ModalKind::Input => input(),
            ModalKind::Confirm => confirm(),
            ModalKind::AddProject => Modal::AddProject(Box::default()),
            ModalKind::RemoveProject => remove_project(false),
            ModalKind::ArchiveProject => Modal::ArchiveProject {
                idx: 0,
                name: "p".into(),
                sessions: vec![],
            },
            ModalKind::ArchivedProjects => Modal::ArchivedProjects,
            ModalKind::Message => Modal::Message("m".into()),
            ModalKind::TmuxChoice => Modal::TmuxChoice,
            ModalKind::AgentPicker => Modal::AgentPicker {
                project: "p".into(),
                wt_path: "/w".into(),
                sel: 0,
            },
            ModalKind::SessionLauncher => Modal::SessionLauncher(Box::default()),
            ModalKind::ThemePicker => theme_picker(ThemePickerReturn::Close),
            ModalKind::ThemeManager => theme_manager(None, None, None),
            ModalKind::Settings => Modal::Settings,
            ModalKind::ShortcutOverlay => Modal::ShortcutOverlay,
            ModalKind::Teardown => teardown(TeardownStage::RunningScript),
            ModalKind::ScriptsEditor => Modal::ScriptsEditor(Box::new(scripts(""))),
            ModalKind::Updating => Modal::Updating,
            ModalKind::Changelog => Modal::Changelog {
                return_to_settings: true,
            },
            ModalKind::Onboarding => Modal::Onboarding {
                step: OnboardStep::Welcome,
                path: String::new(),
                dir_sel: 0,
                name: None,
                note: None,
                added_proj: None,
                agent_sel: 0,
                perms_skip: false,
                name_focused: false,
            },
        }
    }

    #[test]
    fn escape_dismiss_is_the_or_of_its_four_inputs() {
        assert!(!escape_should_dismiss(false, false, false, false));
        for i in 0..4 {
            let f = |n: usize| n == i;
            assert!(escape_should_dismiss(f(0), f(1), f(2), f(3)), "input {i}");
        }
    }
}
