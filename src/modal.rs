//! The pure modal state machine: the single slot, the per-modal Escape verdicts, the key-context strings, and the quit-confirm clobber rule.
//! Ported from `src/app/modal.rs:5-186` and `src/gui/update/modals.rs:69-336,645-702`.
//!
//! gpui's structural dispatch replaces the iced `should_forward`/`MODAL_OPEN`/`PALETTE_OPEN` statics (carried decision 3); per-modal chords bind via [`ModalKind::key_context`].
//! ←/→ capture uses a descendant binding gated by `wants_arrows` since capture-phase interception doesn't work at this gpui rev; [`bound_chords`]'s test guards every bound chord is claimed by [`key_verdict`].

use grove_core::git::BranchRef;

/// What a `Confirm` modal is actually confirming (`src/app/modal.rs:177-186`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    // TODO(unwired): handled by views::modals::confirm but nothing raises it; live flow uses Modal::RemoveProject directly.
    #[allow(dead_code)]
    RemoveProject(usize),
    RemoveWorktree(String),
    InitAndAddWorktree {
        name: String,
        /// The base branch picked in the prompt, carried across the init round trip so the answer survives.
        base: Option<String>,
    },
    Quit,
}

/// The Base row's unset state. Prose, not a branch name — the renderer sets it in
/// sans for exactly that reason, while real branch names stay mono.
pub const BASE_UNSET_LABEL: &str = "Current HEAD";

/// The new-worktree field's placeholder: an example name, not an instruction.
///
/// A field paints nothing (§14), so an empty one is only visible because of this.
/// No `/` — `valid_worktree_name` takes alphanumerics, `-`, `_` and `.` only, and
/// `the_worktree_placeholder_is_a_name_the_validator_accepts` holds it to that.
pub const WORKTREE_NAME_PLACEHOLDER: &str = "fix-billing-retry";

/// At or above this many branches the dropdown grows a filter line; below it the list is short enough to read.
pub const BRANCH_FILTER_MIN: usize = 12;

/// The "Base" branch picker living inside the new-worktree [`Modal::Input`].
///
/// `filter` is a plain `String` rather than a second `InputState`: it is a
/// same-modal, throwaway match string fed from `key_verdict`, and a second
/// focus-managed field would buy nothing but focus bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct BaseBranchState {
    /// The repo the branches were listed from; empty until the modal knows its project.
    pub repo: String,
    /// Empty until the background listing lands, which is the paintable initial state.
    pub branches: Vec<BranchRef>,
    /// `None` means "whatever HEAD is", which is also what submit passes through.
    pub chosen: Option<String>,
    pub open: bool,
    pub filter: String,
    /// Index into the *filtered* list.
    pub highlight: usize,
    pub loaded: bool,
}

// `BranchRef` is Stage-1 API and deliberately left untouched, so it carries no
// `PartialEq`; `Modal`'s derive needs one here, and branch identity is the name.
impl PartialEq for BaseBranchState {
    fn eq(&self, other: &Self) -> bool {
        self.repo == other.repo
            && self.chosen == other.chosen
            && self.open == other.open
            && self.filter == other.filter
            && self.highlight == other.highlight
            && self.loaded == other.loaded
            && self.branches.len() == other.branches.len()
            && self
                .branches
                .iter()
                .zip(&other.branches)
                .all(|(a, b)| a.name == b.name)
    }
}

/// True when `name` names an existing **local** branch.
///
/// Only locals count: `BranchRef::name` is `%(refname:short)`, so a remote reads
/// as `origin/foo` and can never equal a typed worktree name, and `add_worktree`
/// keys its "check the branch out as-is" path strictly on `refs/heads/<name>`.
pub fn branch_exists(name: &str, branches: &[BranchRef]) -> bool {
    let name = name.trim();
    !name.is_empty() && branches.iter().any(|b| !b.is_remote && b.name == name)
}

/// Case-insensitive substring match over branch names.
pub fn filter_branches<'a>(branches: &'a [BranchRef], filter: &str) -> Vec<&'a BranchRef> {
    let needle = filter.trim().to_ascii_lowercase();
    branches
        .iter()
        .filter(|b| needle.is_empty() || b.name.to_ascii_lowercase().contains(&needle))
        .collect()
}

/// A paint-only view of the visible branch list. Group rows deliberately carry
/// no selection index: keyboard and pointer selection always address
/// [`BaseBranchState::visible`], not this expanded list.
#[derive(Clone, Debug)]
pub enum BaseBranchDropdownItem<'a> {
    Group {
        /// The complete slash-delimited path represented by this heading.
        path: &'a str,
        depth: usize,
    },
    Branch {
        /// Index into the filtered, selectable branch list.
        index: usize,
        branch: &'a BranchRef,
        /// The final path component; its ancestor is already named by a group.
        label: &'a str,
        depth: usize,
    },
}

/// Expands slash-delimited visible branch names into quiet group headings and
/// indented selectable rows without changing the branches' order or indices.
pub fn branch_dropdown_items<'a>(visible: &[&'a BranchRef]) -> Vec<BaseBranchDropdownItem<'a>> {
    let mut items = Vec::with_capacity(visible.len());
    let mut previous_groups: Vec<&str> = Vec::new();

    for (index, branch) in visible.iter().enumerate() {
        let mut groups = Vec::new();
        for (offset, _) in branch.name.match_indices('/') {
            groups.push(&branch.name[..offset]);
        }
        let label = branch
            .name
            .rsplit_once('/')
            .map_or(branch.name.as_str(), |(_, leaf)| leaf);
        let shared = groups
            .iter()
            .zip(&previous_groups)
            .take_while(|(next, prior)| next == prior)
            .count();

        for (depth, path) in groups.iter().enumerate().skip(shared) {
            items.push(BaseBranchDropdownItem::Group { path, depth });
        }
        items.push(BaseBranchDropdownItem::Branch {
            index,
            branch,
            label,
            depth: groups.len(),
        });
        previous_groups = groups;
    }

    items
}

impl BaseBranchState {
    /// Folds one background listing in; a resolvable default seeds the choice, `None` leaves it unset.
    pub fn apply_loaded(&mut self, branches: Vec<BranchRef>, default: Option<String>) {
        self.branches = branches;
        self.chosen = default;
        self.loaded = true;
    }

    pub fn visible(&self) -> Vec<&BranchRef> {
        filter_branches(&self.branches, &self.filter)
    }

    pub fn wants_filter(&self) -> bool {
        self.branches.len() >= BRANCH_FILTER_MIN
    }

    /// A typed name that already exists is checked out as-is, so the base is not the user's to pick.
    pub fn locked(&self, typed: &str) -> bool {
        branch_exists(typed, &self.branches)
    }

    /// The base actually handed to `git::add_worktree`: none at all when the name is checked out as-is.
    pub fn base_for_submit(&self, typed: &str) -> Option<String> {
        if self.locked(typed) {
            None
        } else {
            self.chosen.clone()
        }
    }

    pub fn open_dropdown(&mut self) {
        self.open = true;
        self.filter.clear();
        self.highlight = self
            .chosen
            .as_ref()
            .and_then(|c| self.branches.iter().position(|b| &b.name == c))
            .unwrap_or(0);
    }

    pub fn close_dropdown(&mut self) {
        self.open = false;
        self.filter.clear();
        self.highlight = 0;
    }

    pub fn move_highlight(&mut self, delta: i32) {
        let len = self.visible().len();
        if len == 0 {
            self.highlight = 0;
            return;
        }
        let next = (self.highlight.min(len - 1) as i32 + delta).rem_euclid(len as i32);
        self.highlight = next as usize;
    }

    /// Commits the highlighted (or `i`-th visible) branch and closes the dropdown.
    pub fn pick(&mut self, i: usize) {
        if let Some(b) = self.visible().get(i) {
            let name = b.name.clone();
            self.chosen = Some(name);
        }
        self.close_dropdown();
    }

    pub fn pick_highlighted(&mut self) {
        self.pick(self.highlight);
    }

    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.highlight = 0;
    }

    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.highlight = 0;
    }
}

/// Stage of an in-progress worktree teardown (`src/app/modal.rs:152-161`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownStage {
    RunningScript,
    Removing,
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

    pub fn flow() -> &'static [OnboardStep] {
        &Self::ALL
    }

    pub fn index_in(self) -> usize {
        Self::flow().iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn prev(self) -> Option<OnboardStep> {
        self.index_in().checked_sub(1).map(|i| Self::flow()[i])
    }

    pub fn label(self) -> &'static str {
        match self {
            OnboardStep::Welcome => "welcome",
            OnboardStep::Environment => "environment",
            OnboardStep::Project => "project",
            OnboardStep::Session => "session",
        }
    }
}

/// Keyed by project **name** (`src/app/theme_picker.rs:17-23`) — indices shift under add/remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemePickerScope {
    App,
    Project(String),
}

/// Pure `String`s so they survive the ScriptsEditor → ThemePicker → ScriptsEditor round trip (`modals.rs:660-668`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptsEditorState {
    pub project_path: String,
    pub name: String,
    pub setup: String,
    pub run: String,
    pub teardown: String,
    pub renaming: bool,
}

/// Where a `ThemePicker` goes when it closes (`src/app/modal.rs:79-82` plus the ScriptsEditor round trip).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemePickerReturn {
    Close,
    Settings,
    ScriptsEditor(Box<ScriptsEditorState>),
}

/// Unlike iced's `Modal`, child state lives inline here so replacing the slot drops old state automatically (carried decision 4).
#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Input {
        title: String,
        buffer: String,
        note: Option<String>,
        /// The new-worktree prompt's Base picker; the only `Input` in the app.
        base: BaseBranchState,
    },
    Confirm {
        title: String,
        prompt: String,
        destructive: bool,
        kind: ConfirmKind,
    },
    AddProject(Box<AddProjectState>),
    RemoveProject {
        idx: usize,
        name: String,
        project_path: String,
        worktrees: Vec<String>,
        also_remove_worktrees: bool,
        in_progress: bool,
        done: usize,
        current: String,
        errors: Vec<String>,
    },
    /// `sessions` is one row per SESSION, not per worktree, and not filtered to running ones so the count can't disagree with `kill_sessions_for_project` (`modals.rs:703-745`).
    ArchiveProject {
        idx: usize,
        name: String,
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
    SessionLauncher(Box<LauncherSlotState>),
    ThemePicker {
        sel_dark: usize,
        sel_light: usize,
        dark_tab: bool,
        original: String,
        follow_system: bool,
        scope: ThemePickerScope,
        project_use_default: bool,
        return_to: ThemePickerReturn,
    },
    ThemeManager {
        selected: usize,
        rename: Option<(String, String)>,
        rename_error: Option<String>,
        pending_delete: Option<String>,
        editor: Option<String>,
    },
    Settings,
    ShortcutOverlay,
    Teardown {
        wt_path: String,
        project_path: String,
        stage: TeardownStage,
        message: String,
        /// Set once `git worktree remove` has been kicked off, so a `Removing` frame paints before it runs.
        removal_started: bool,
    },
    ScriptsEditor(Box<ScriptsEditorState>),
    Updating,
    Changelog {
        return_to_settings: bool,
    },
    /// File list, patch cache and loading flag live on the gpui side; this only names which worktree is open.
    DiffViewer {
        wt_path: String,
    },
    /// Rendered full-viewport as a screen replacement, never through the scrim.
    Onboarding {
        step: OnboardStep,
        path: String,
        dir_sel: usize,
        name: Option<String>,
        note: Option<String>,
        added_proj: Option<usize>,
        agent_sel: usize,
        perms_skip: bool,
        name_focused: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddProjectStep {
    #[default]
    PickSource,
    Details,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddProjectState {
    pub step: AddProjectStep,
    /// Step 1: the typed path buffer. Step 2: the canonicalized folder.
    pub path: String,
    pub dir_sel: usize,
    pub name: String,
    pub note: Option<String>,
    pub init_git: bool,
    /// `Some(branch)` = a repo on that branch, `None` = not a repo; re-probed on every folder choice, never persisted.
    pub git_branch: Option<String>,
}

/// Which list state the palette is showing (`session_launcher/state.rs:13-58`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherView {
    #[default]
    Root,
    BrowseAll,
    Switch,
    RowActions,
    Settings,
}

/// Row model and ranking live in [`crate::launcher`], which is pure and gpui-free.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherSlotState {
    pub query: String,
    pub sel: usize,
    pub view: LauncherView,
    /// By identity, not index — a query edit or re-sort must not activate a different row.
    pub anchor: Option<crate::launcher::RowIdentity>,
    pub agent_sel: usize,
    pub scope: crate::launcher::PaletteScope,
    /// Temporary path-keyed selection used only by the worktrees-only flow.
    pub(crate) selected_worktrees: crate::launcher::WorktreeSelection,
}

impl LauncherSlotState {
    /// Reuses the open palette for a fresh multi-project selection flow.
    pub(crate) fn enter_worktrees_only(&mut self) {
        self.query.clear();
        self.sel = 0;
        self.view = LauncherView::Root;
        self.anchor = None;
        self.agent_sel = 0;
        self.scope = crate::launcher::PaletteScope::WorktreesOnly;
        self.selected_worktrees.clear();
    }
}

/// The slot's discriminant — what the key table, key contexts and drift guard index by.
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
    DiffViewer,
    Onboarding,
}

impl ModalKind {
    pub const ALL: [ModalKind; 20] = [
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
        ModalKind::DiffViewer,
        ModalKind::Onboarding,
    ];

    /// The gpui key-context string this modal's root element declares.
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
            ModalKind::DiffViewer => "ModalDiffViewer",
            ModalKind::Onboarding => "ModalOnboarding",
        }
    }

    pub fn is_screen_replacement(self) -> bool {
        matches!(self, ModalKind::Onboarding)
    }

    pub fn top_drops(self) -> bool {
        matches!(self, ModalKind::SessionLauncher)
    }

    /// Carried decision 2 (`PALETTE_OPEN` carve-out); everything else lets the caret have ←/→.
    pub fn wants_arrows(self) -> bool {
        matches!(self, ModalKind::SessionLauncher | ModalKind::AddProject)
    }

    /// Multiline buffers leave Tab clear so it indents (carried decision 2).
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
            Modal::DiffViewer { .. } => ModalKind::DiffViewer,
            Modal::Onboarding { .. } => ModalKind::Onboarding,
        }
    }

    /// Built from the running **native** session count; tmux-backed sessions survive grove and aren't counted.
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

    /// A constructor since the variant has nine fields and exactly one legitimate initial value.
    pub fn onboarding() -> Modal {
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
}

/// The subset of keystrokes the modal table reasons about; anything unmapped is `FallThrough`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKey {
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Char(char),
}

/// `platform` is the global-shortcut modifier (Cmd on macOS, Ctrl+Shift elsewhere), pre-resolved by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModalMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

impl ModalMods {
    // Test-only chord spellings for verdict-table assertions; the live path builds ModalMods from gpui::Modifiers.
    #[allow(dead_code)]
    pub const NONE: ModalMods = ModalMods {
        ctrl: false,
        alt: false,
        shift: false,
        platform: false,
    };
    #[allow(dead_code)]
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
    pub update_in_flight: bool,
    /// This keystroke *is* the shortcut overlay's own registry chord.
    pub is_shortcut_overlay_chord: bool,
}

/// What the layer must do with a keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalKeyVerdict {
    /// Routes through [`ModalSlot::cancel`], which is *not* always "close" (Teardown, RemoveProject).
    Close,
    Submit,
    Move(i32),
    Custom(ModalAction),
    Ignore,
    /// Not claimed: the modal's own sub-widget or the focused `Input` gets it.
    FallThrough,
}

/// The modal-specific half of [`ModalKeyVerdict::Custom`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalAction {
    Confirm(bool),
    /// New-worktree Base picker, all four only reachable while its dropdown is open.
    BaseDropdownClose,
    BaseDropdownPick,
    BaseFilterPush(char),
    BaseFilterPop,
    /// `y` routes through the gate re-check so it cannot bypass a disabled Archive button.
    ArchiveConfirm,
    RemoveProjectConfirm,
    ToggleRemoveWorktrees,
    /// An explicit pick, which is the only thing that persists.
    ChooseTmux(bool),
    ToggleDefaultAgent,
    ThemePickerSubmit,
    ThemePickerSwitchTab,
    ThemeManagerDeleteConfirm,
    ThemeManagerDeleteCancel,
    ThemeManagerRenameSubmit,
    ThemeManagerRenameCancel,
    // TODO(unwired): keyboard half of ScriptsRenameStart — ModalClick fires it but key_verdict never produces this action.
    #[allow(dead_code)]
    ScriptsRenameStart,
    ScriptsRenameCommit,
    ScriptsRenameCancel,
    OnboardSkip,
    OnboardAdvance,
    OnboardToggleFocus,
    /// Moves keyboard focus from the file list to the scrolling body.
    DiffFocusBody,
    /// A no-op when the narrow-window fallback has Split disabled.
    DiffToggleMode,
}

/// The result of [`ModalSlot::cancel`] — cancel is not a synonym for close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Closed,
    SkippedTeardownScript,
    Refused,
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

    /// **Replaces** whatever was there and drops its state — no stack, no restore (carried decision 4).
    pub fn open(&mut self, modal: Modal) {
        self.modal = Some(modal);
    }

    /// Force the slot empty, ignoring every per-modal cancel rule.
    pub fn close(&mut self) {
        self.modal = None;
    }

    /// Port of `cancel_modal` (`modals.rs:677-702`) plus the two documented return trips.
    pub fn cancel(&mut self) -> CancelOutcome {
        match self.modal.as_mut() {
            None => CancelOutcome::Closed,
            // Teardown repurposes cancel: skip a running script, dismiss once done, refuse mid-removal.
            Some(Modal::Teardown { stage, .. }) => match *stage {
                TeardownStage::Done { .. } => {
                    self.modal = None;
                    CancelOutcome::Closed
                }
                TeardownStage::RunningScript => CancelOutcome::SkippedTeardownScript,
                TeardownStage::Removing => CancelOutcome::Refused,
            },
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

/// One row per arm of `handle_modal_key` (`modals.rs:94-336`) and `handle_remove_project_key` (`modals.rs:69-93`).
pub fn key_verdict(modal: &Modal, key: ModalKey, mods: ModalMods, ctx: KeyCtx) -> ModalKeyVerdict {
    use ModalAction as A;
    use ModalKey as K;
    use ModalKeyVerdict as V;

    /// Folds `y`/`Y`-style case-insensitive arms to lowercase once.
    fn ch(key: ModalKey) -> Option<char> {
        match key {
            ModalKey::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        }
    }
    fn ctrl_c(key: ModalKey, mods: ModalMods) -> bool {
        mods.ctrl && ch(key) == Some('c')
    }

    match modal {
        // An open Base dropdown owns the keyboard: Escape closes only the dropdown and
        // Enter only commits a branch, so the closed-dropdown arm below keeps its
        // prefill-then-single-Enter behaviour untouched. Deliberately keyed on the
        // modal's own runtime state rather than the global `wants_arrows` policy.
        Modal::Input { base, .. } if base.open => match key {
            _ if ctrl_c(key, mods) => V::Close,
            K::Escape => V::Custom(A::BaseDropdownClose),
            K::Enter => V::Custom(A::BaseDropdownPick),
            K::Down => V::Move(1),
            K::Up => V::Move(-1),
            K::Backspace => V::Custom(A::BaseFilterPop),
            K::Char(c) if !mods.ctrl && !mods.alt && !mods.platform => {
                V::Custom(A::BaseFilterPush(c))
            }
            _ => V::Ignore,
        },

        // Only the lifecycle keys are claimed here; the rest belongs to the focused Input.
        Modal::Input { .. } => match key {
            K::Escape => V::Close,
            K::Enter => V::Submit,
            _ if ctrl_c(key, mods) => V::Close,
            _ => V::FallThrough,
        },

        // Esc from pick-source and Ctrl+C from either step cancel the wizard; everything else is the delegate's.
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

        // `y` routes through the gate's own re-check so it cannot bypass a disabled button.
        Modal::ArchiveProject { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Close,
            (_, Some('y')) => V::Custom(A::ArchiveConfirm),
            (_, Some('n')) => V::Close,
            _ => V::Ignore,
        },

        // All ignored while removal is in flight (handle_remove_project_key, modals.rs:69-93).
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
            // Esc dismisses without persisting; only explicit picks record a backend.
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

        // Precedence: editor sub-view, then delete confirm, then inline rename, then the plain list.
        Modal::ThemeManager {
            rename,
            pending_delete,
            editor,
            ..
        } => {
            if editor.is_some() {
                V::FallThrough
            } else if pending_delete.is_some() {
                // Enter works here too (unlike confirm_modal); this dialog has no other use for it.
                match (key, ch(key)) {
                    (K::Enter, _) | (_, Some('y')) => V::Custom(A::ThemeManagerDeleteConfirm),
                    (K::Escape, _) | (_, Some('n')) => V::Custom(A::ThemeManagerDeleteCancel),
                    _ => V::Ignore,
                }
            } else if rename.is_some() {
                match key {
                    K::Enter => V::Custom(A::ThemeManagerRenameSubmit),
                    K::Escape => V::Custom(A::ThemeManagerRenameCancel),
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

        Modal::SessionLauncher(_) => V::FallThrough,

        Modal::Settings | Modal::ArchivedProjects => match key {
            K::Escape => V::Close,
            _ => V::Ignore,
        },

        // V::FallThrough lets the focused field see the key first, or typing is dead in both sub-states.
        Modal::ScriptsEditor(st) => {
            if st.renaming {
                match key {
                    K::Enter => V::Custom(A::ScriptsRenameCommit),
                    K::Escape => V::Custom(A::ScriptsRenameCancel),
                    _ => V::FallThrough,
                }
            } else {
                match key {
                    K::Escape => V::Close,
                    K::Enter => V::Submit,
                    _ => V::FallThrough,
                }
            }
        }

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

        Modal::ShortcutOverlay => match key {
            K::Escape => V::Close,
            _ if ctx.is_shortcut_overlay_chord => V::Close,
            _ => V::Ignore,
        },

        // Everything unclaimed is swallowed rather than reaching a PTY behind the modal.
        Modal::DiffViewer { .. } => match (key, ch(key)) {
            (K::Escape, _) => V::Close,
            (K::Enter, _) => V::Custom(A::DiffFocusBody),
            (K::Down, _) | (_, Some('j')) => V::Move(1),
            (K::Up, _) | (_, Some('k')) => V::Move(-1),
            (K::Tab, _) => V::Custom(A::DiffToggleMode),
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

/// `false` means Escape must reach the PTY, since many TUI programs need it. Port of `escape_should_dismiss` (`pty_input.rs:364-378`).
pub fn escape_should_dismiss(
    pending_kill: bool,
    pending_kill_terminal: bool,
    agent_menu_open: bool,
    attention_open: bool,
) -> bool {
    pending_kill || pending_kill_terminal || agent_menu_open || attention_open
}

/// A chord bound as a gpui action in a modal's own key context (carried decision 3).
// Consumed only by #[cfg(test)] drift-guard code asserting every bound chord is claimed by key_verdict.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModalChord {
    pub key: ModalKey,
    pub mods: ModalMods,
}

/// A context binding a chord the table ignores would silently swallow it.
#[allow(dead_code)]
pub fn bound_chords(kind: ModalKind) -> &'static [ModalChord] {
    match kind {
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
            name: "p".into(),
            setup: setup.into(),
            run: String::new(),
            teardown: String::new(),
            renaming: false,
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

    #[test]
    fn opening_a_modal_replaces_the_open_one_and_drops_its_state() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::ScriptsEditor(Box::new(scripts("echo hi"))));
        slot.open(Modal::Settings);
        assert_eq!(slot.kind(), Some(ModalKind::Settings));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
        assert_eq!(slot.kind(), None);
    }

    #[test]
    fn quit_confirm_clobbers_the_open_modal_and_cancelling_leaves_none() {
        let mut slot = ModalSlot::new();
        slot.open(Modal::Settings);
        // Cancelling does not restore the clobbered modal — a deliberately preserved gap (modals.rs:350-354).
        slot.open(Modal::quit_confirm(2));
        let Some(Modal::Confirm { kind, prompt, .. }) = slot.get() else {
            unreachable!()
        };
        assert_eq!(*kind, ConfirmKind::Quit);
        assert!(prompt.contains("2 running sessions"));
        assert_eq!(slot.cancel(), CancelOutcome::Closed);
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

    fn v(modal: &Modal, key: ModalKey) -> ModalKeyVerdict {
        key_verdict(modal, key, ModalMods::NONE, KeyCtx::default())
    }

    fn input() -> Modal {
        Modal::Input {
            title: "t".into(),
            buffer: String::new(),
            note: None,
            base: BaseBranchState::default(),
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

        let m = theme_manager(Some(("a".into(), "b".into())), None, None);
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerRenameSubmit)
        );
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::ThemeManagerRenameCancel)
        );
        assert_eq!(v(&m, ModalKey::Char('x')), ModalKeyVerdict::FallThrough);

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
        for m in [
            &Modal::Settings,
            &Modal::ArchivedProjects,
            &teardown(TeardownStage::RunningScript),
        ] {
            assert_eq!(v(m, ModalKey::Escape), ModalKeyVerdict::Close, "{m:?}");
            assert_eq!(v(m, ModalKey::Char('q')), ModalKeyVerdict::Ignore, "{m:?}");
        }
    }

    #[test]
    fn scripts_editor_enter_submits() {
        let m = Modal::ScriptsEditor(Box::new(scripts("")));
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Submit);
    }

    #[test]
    fn scripts_editor_typing_falls_through_to_the_field() {
        let m = Modal::ScriptsEditor(Box::new(scripts("")));
        assert_eq!(v(&m, ModalKey::Char('a')), ModalKeyVerdict::FallThrough);
        assert_eq!(v(&m, ModalKey::Tab), ModalKeyVerdict::FallThrough);
    }

    #[test]
    fn scripts_editor_rename_mode_keys() {
        let mut st = scripts("");
        st.renaming = true;
        let m = Modal::ScriptsEditor(Box::new(st));
        assert_eq!(
            v(&m, ModalKey::Enter),
            ModalKeyVerdict::Custom(ModalAction::ScriptsRenameCommit)
        );
        assert_eq!(
            v(&m, ModalKey::Escape),
            ModalKeyVerdict::Custom(ModalAction::ScriptsRenameCancel)
        );
        assert_eq!(v(&m, ModalKey::Char('x')), ModalKeyVerdict::FallThrough);
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
        assert_eq!(v(&m, ModalKey::Enter), ModalKeyVerdict::Ignore);

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

    #[test]
    fn entering_worktrees_only_resets_the_launcher_to_a_fresh_selection() {
        let mut st = LauncherSlotState {
            query: "multi repo".into(),
            sel: 4,
            view: LauncherView::RowActions,
            anchor: Some(crate::launcher::RowIdentity::Settings),
            agent_sel: 2,
            scope: crate::launcher::PaletteScope::All,
            selected_worktrees: crate::launcher::WorktreeSelection::default(),
        };
        st.selected_worktrees.toggle("/worktrees/one");
        st.enter_worktrees_only();

        assert_eq!(st.query, "");
        assert_eq!(st.sel, 0);
        assert_eq!(st.view, LauncherView::Root);
        assert_eq!(st.anchor, None);
        assert_eq!(st.agent_sel, 0);
        assert_eq!(st.scope, crate::launcher::PaletteScope::WorktreesOnly);
        assert_eq!(st.selected_worktrees.count(), 0);
    }

    #[test]
    fn no_modal_context_binds_a_chord_its_verdict_table_ignores() {
        for kind in ModalKind::ALL {
            for chord in bound_chords(kind) {
                let modal = sample_modal(kind);
                let ctx = KeyCtx {
                    update_in_flight: false,
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
            ModalKind::DiffViewer => Modal::DiffViewer {
                wt_path: "/w".into(),
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

/// The new-worktree Base picker's pure half.
#[cfg(test)]
mod base_branch_tests {
    use super::*;

    fn v(modal: &Modal, key: ModalKey) -> ModalKeyVerdict {
        key_verdict(modal, key, ModalMods::NONE, KeyCtx::default())
    }

    fn br(name: &str, is_remote: bool) -> BranchRef {
        BranchRef {
            name: name.into(),
            is_remote,
            is_head: false,
            ahead: 0,
            behind: 0,
        }
    }

    fn some_branches() -> Vec<BranchRef> {
        vec![
            br("main", false),
            br("feature/login", false),
            br("origin/main", true),
            br("origin/Release", true),
        ]
    }

    #[test]
    fn a_resolvable_default_seeds_the_choice() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), Some("main".into()));
        assert_eq!(st.chosen.as_deref(), Some("main"));
        assert!(st.loaded);
    }

    #[test]
    fn no_resolvable_default_stays_unset_but_legible() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), None);
        assert_eq!(st.chosen, None);
        assert!(
            !BASE_UNSET_LABEL.is_empty(),
            "the unset state stays legible"
        );
        // Unset still submits as today's implicit HEAD.
        assert_eq!(st.base_for_submit("wip"), None);
    }

    #[test]
    fn only_local_branch_names_count_as_existing() {
        let bs = some_branches();
        assert!(branch_exists("main", &bs));
        assert!(branch_exists("feature/login", &bs));
        // A remote reads as `origin/…`, so a typed name can never collide with one.
        assert!(!branch_exists("Release", &bs));
        assert!(!branch_exists("origin/main", &bs));
        assert!(!branch_exists("brand-new", &bs));
        assert!(!branch_exists("", &bs));
        assert!(!branch_exists("   ", &bs));
    }

    #[test]
    fn an_existing_name_locks_the_row_and_drops_the_base() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), Some("main".into()));
        assert!(st.locked("main"));
        assert_eq!(st.base_for_submit("main"), None);
        assert!(!st.locked("brand-new"));
        assert_eq!(st.base_for_submit("brand-new").as_deref(), Some("main"));
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_match() {
        let bs = some_branches();
        let names = |f: &str| {
            filter_branches(&bs, f)
                .into_iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("").len(), 4);
        assert_eq!(names("  ").len(), 4);
        assert_eq!(names("MAIN"), vec!["main", "origin/main"]);
        assert_eq!(names("release"), vec!["origin/Release"]);
        assert_eq!(names("login"), vec!["feature/login"]);
        assert!(names("nope").is_empty());
    }

    #[test]
    fn slash_delimited_branches_gain_quiet_hierarchy_without_reordering_rows() {
        let branches = vec![
            br("main", false),
            br("feature/auth/login", false),
            br("feature/auth/logout", false),
            br("feature/search", false),
            br("origin/main", true),
        ];
        let visible = filter_branches(&branches, "");
        let items = branch_dropdown_items(&visible);
        let presentation = items
            .iter()
            .map(|item| match item {
                BaseBranchDropdownItem::Group { path, depth } => {
                    format!("group:{depth}:{path}")
                }
                BaseBranchDropdownItem::Branch {
                    index,
                    label,
                    depth,
                    ..
                } => format!("branch:{index}:{depth}:{label}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            presentation,
            [
                "branch:0:0:main",
                "group:0:feature",
                "group:1:feature/auth",
                "branch:1:2:login",
                "branch:2:2:logout",
                "branch:3:1:search",
                "group:0:origin",
                "branch:4:1:main",
            ]
        );
    }

    #[test]
    fn filtered_hierarchy_keeps_click_and_keyboard_indices_in_the_visible_list() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(
            vec![
                br("main", false),
                br("feature/auth/login", false),
                br("feature/auth/logout", false),
                br("feature/search", false),
            ],
            None,
        );
        st.open_dropdown();
        for c in "auth".chars() {
            st.push_filter(c);
        }

        let visible = st.visible();
        let selectable = branch_dropdown_items(&visible)
            .into_iter()
            .filter_map(|item| match item {
                BaseBranchDropdownItem::Branch { index, branch, .. } => {
                    Some((index, branch.name.clone()))
                }
                BaseBranchDropdownItem::Group { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            selectable,
            vec![
                (0, "feature/auth/login".to_string()),
                (1, "feature/auth/logout".to_string())
            ]
        );

        st.move_highlight(1);
        st.pick_highlighted();
        assert_eq!(st.chosen.as_deref(), Some("feature/auth/logout"));
    }

    #[test]
    fn the_filter_only_appears_once_the_list_is_long() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), None);
        assert!(!st.wants_filter());
        let many: Vec<BranchRef> = (0..BRANCH_FILTER_MIN)
            .map(|i| br(&format!("b{i}"), false))
            .collect();
        st.apply_loaded(many, None);
        assert!(st.wants_filter());
    }

    #[test]
    fn a_single_branch_repo_still_opens_a_one_entry_dropdown() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(vec![br("main", false)], Some("main".into()));
        st.open_dropdown();
        assert!(st.open);
        assert_eq!(st.visible().len(), 1);
        st.move_highlight(1);
        assert_eq!(st.highlight, 0);
    }

    #[test]
    fn opening_highlights_the_current_choice_and_picking_commits_it() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), Some("origin/main".into()));
        st.open_dropdown();
        assert_eq!(st.highlight, 2);
        st.move_highlight(1);
        assert_eq!(st.highlight, 3);
        st.pick_highlighted();
        assert_eq!(st.chosen.as_deref(), Some("origin/Release"));
        assert!(!st.open, "picking closes the dropdown");
    }

    #[test]
    fn closing_the_dropdown_keeps_the_choice_and_clears_the_filter() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), Some("main".into()));
        st.open_dropdown();
        st.push_filter('o');
        st.push_filter('r');
        assert_eq!(st.filter, "or");
        assert_eq!(st.visible().len(), 2);
        st.pop_filter();
        assert_eq!(st.filter, "o");
        st.close_dropdown();
        assert_eq!(st.chosen.as_deref(), Some("main"));
        assert!(st.filter.is_empty());
    }

    #[test]
    fn picking_indexes_the_filtered_list_not_the_full_one() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), None);
        st.open_dropdown();
        st.push_filter('o');
        st.push_filter('r');
        st.pick(1);
        assert_eq!(st.chosen.as_deref(), Some("origin/Release"));
    }

    #[test]
    fn an_empty_or_unloaded_list_is_paintable_and_inert() {
        let mut st = BaseBranchState::default();
        assert!(!st.loaded);
        assert!(
            !BASE_UNSET_LABEL.is_empty(),
            "the unset state stays legible"
        );
        st.open_dropdown();
        assert!(st.visible().is_empty());
        st.move_highlight(-1);
        st.pick_highlighted();
        assert_eq!(st.chosen, None);
    }

    #[test]
    fn an_open_dropdown_claims_the_keys_a_closed_one_leaves_alone() {
        use ModalKey as K;
        use ModalKeyVerdict as V;
        let closed = Modal::Input {
            title: "t".into(),
            buffer: String::new(),
            note: None,
            base: BaseBranchState::default(),
        };
        assert_eq!(
            v(&closed, K::Enter),
            V::Submit,
            "prefill + one Enter submits"
        );
        assert_eq!(v(&closed, K::Escape), V::Close);
        assert_eq!(v(&closed, K::Down), V::FallThrough);
        assert_eq!(v(&closed, K::Char('a')), V::FallThrough);

        let mut base = BaseBranchState::default();
        base.apply_loaded(some_branches(), Some("main".into()));
        base.open_dropdown();
        let open = Modal::Input {
            title: "t".into(),
            buffer: String::new(),
            note: None,
            base,
        };
        assert_eq!(
            v(&open, K::Escape),
            V::Custom(ModalAction::BaseDropdownClose),
            "Escape closes the dropdown, not the modal"
        );
        assert_eq!(
            v(&open, K::Enter),
            V::Custom(ModalAction::BaseDropdownPick),
            "Enter selects, it does not submit"
        );
        assert_eq!(v(&open, K::Down), V::Move(1));
        assert_eq!(v(&open, K::Up), V::Move(-1));
        assert_eq!(
            v(&open, K::Char('m')),
            V::Custom(ModalAction::BaseFilterPush('m'))
        );
        assert_eq!(
            v(&open, K::Backspace),
            V::Custom(ModalAction::BaseFilterPop)
        );
        assert_eq!(
            key_verdict(&open, K::Char('c'), ModalMods::CTRL, KeyCtx::default()),
            V::Close,
            "Ctrl+C still closes the whole modal"
        );
    }

    #[test]
    fn the_init_round_trip_carries_the_chosen_base() {
        let mut st = BaseBranchState::default();
        st.apply_loaded(some_branches(), Some("main".into()));
        let kind = ConfirmKind::InitAndAddWorktree {
            name: "wip".into(),
            base: st.base_for_submit("wip"),
        };
        let ConfirmKind::InitAndAddWorktree { name, base } = kind else {
            panic!("wrong kind");
        };
        assert_eq!(name, "wip");
        assert_eq!(base.as_deref(), Some("main"));
    }
}

#[cfg(test)]
mod placeholder_tests {
    use super::*;

    /// A placeholder demonstrating a value the validator rejects would teach the
    /// wrong format, so the example is held to the real rule.
    #[test]
    fn the_worktree_placeholder_is_a_name_the_validator_accepts() {
        assert!(
            grove_core::git::valid_worktree_name(WORKTREE_NAME_PLACEHOLDER),
            "{WORKTREE_NAME_PLACEHOLDER:?} would be rejected on submit"
        );
        // Specifically: slashes are not accepted, whatever branch-name habit suggests.
        assert!(!grove_core::git::valid_worktree_name("fix/billing-retry"));
        assert!(!WORKTREE_NAME_PLACEHOLDER.contains('/'));
    }

    /// The placeholder is never the buffer, so an untouched field submits nothing.
    #[test]
    fn a_fresh_input_modal_carries_an_empty_buffer_not_the_placeholder() {
        let modal = Modal::Input {
            title: "New worktree".into(),
            buffer: String::new(),
            note: None,
            base: BaseBranchState::default(),
        };
        let Modal::Input { buffer, .. } = modal else {
            unreachable!()
        };
        assert!(buffer.is_empty());
        assert_ne!(buffer, WORKTREE_NAME_PLACEHOLDER);
    }
}
