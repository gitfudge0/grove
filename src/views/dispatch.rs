//! The click-intent vocabulary the view layer's leaf components speak.
//! Sits below `components` and `modals` so a button can wire to an intent without depending on the modal layer, which interprets them.

use gpui::{App, Window};

/// Buttons/checkboxes only own `&mut Window, &mut App`, so they raise one of these through [`ModalDispatch`] rather than touching the layer entity directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalClick {
    /// Route through `ModalSlot::cancel` — the same path Escape takes.
    Cancel,
    /// Resolve a `Confirm` (`modal_action`'s two footer buttons).
    Confirm(bool),
    ChooseTmux(bool),
    ToggleRemoveWorktrees,
    RemoveProjectConfirm,
    ArchiveConfirm,
    ArchiveKillSessions,
    RestoreArchived(usize),
    DeleteArchived(usize),
    /// AgentPicker: click a row, then Enter or click again to start.
    SelectRow(usize),
    /// New-worktree prompt: open/close the Base branch picker.
    BaseDropdownToggle,
    /// New-worktree prompt: pick the n-th *filtered* branch row.
    BaseSelect(usize),
    Submit,
    ToggleDefaultAgent,
    /// ThemePicker.
    ThemePickerTab(bool),
    ThemePickerToggleFollowSystem,
    ThemePickerUseDefault,
    /// Same commit path `ModalAction::ThemePickerSubmit` reaches from the keyboard.
    ThemePickerApply,
    /// ScriptsEditor / ThemeManager / Settings buttons.
    Save,
    OpenProjectTheme,
    OpenArchiveGate,
    OpenArchivedProjects,
    OpenThemePicker,
    OpenThemeManager,
    OpenChangelog,
    /// Updates: a manual check, the apply, skip, copy-url and the restart.
    CheckUpdates,
    StartUpdate,
    SkipVersion,
    CopyReleaseUrl,
    RestartApp,
    /// Settings → Tools: re-run detection, or adopt a tool as the default.
    RefreshTools,
    SetDefaultAgent(grove_core::agent::Agent),
    /// Settings toggles, by the store key they flip.
    ToggleSetting(SettingToggle),
    /// ThemeManager row actions.
    ThemeSelect(usize),
    ThemeRenameStart(usize),
    ThemeRenameCommit,
    ThemeDuplicate(usize),
    ThemeDeleteRequest(usize),
    ThemeDeleteConfirm,
    ThemeDeleteCancel,
    ThemeNew,
    ThemeEditOpen(usize),
    ThemeEditSave,
    /// ScriptsEditor header: pencil / check / X.
    ScriptsRenameStart,
    ScriptsRenameCommit,
    ScriptsRenameCancel,
    /// AddProject / Onboarding wizard.
    WizardBrowse,
    WizardPickDir(usize),
    WizardNext,
    WizardBack,
    WizardToggleInitGit,
    OnboardSkip,
    OnboardAdvance,
    OnboardBack,
    OnboardPickAgent(usize),
    OnboardPerms(bool),
    /// Diff viewer: click a file-list row.
    SelectDiffFile {
        path: String,
    },
    /// Diff viewer: click the header's Unified/Split segment.
    SetDiffMode(grove_core::storage::DiffMode),
    /// Diff viewer: click the file list's Flat/Tree segment.
    ToggleDiffListStyle,
    /// Diff viewer: click a tree-mode directory row's disclosure chevron.
    ToggleDiffTreeDir {
        path: String,
    },
    /// Diff viewer: mouse-down on the file-list/body divider — starts (or, on double-click, releases) a width drag.
    DiffFileListDividerPress,
}

/// Each persisted immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingToggle {
    Tmux,
    SkipPermissions,
    Chrome,
    ThemeFollowSystem,
}

/// A weak-entity click dispatcher handed to the pure view functions.
pub type ModalDispatch = std::rc::Rc<dyn Fn(ModalClick, &mut Window, &mut App)>;
