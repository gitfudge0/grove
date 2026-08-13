//! The click-intent vocabulary the view layer's leaf components speak.
//!
//! These types sit *below* both [`crate::views::components`] and
//! [`crate::views::modals`] so the app-wide component library can wire a button
//! to an intent without depending on the modal layer. The modal layer is what
//! interprets them — see [`crate::views::modals::ModalLayer::dispatcher`] — but
//! nothing here knows that.

use gpui::{App, Window};

/// Every mouse-driven intent a modal's chrome can raise. Buttons and
/// checkboxes only own a `&mut Window, &mut App`, so they cannot touch the
/// layer entity directly; they raise one of these through [`ModalDispatch`],
/// a weak-entity closure built by [`crate::views::modals::ModalLayer::dispatcher`].
/// This is the mouse half of the keyboard verdict table — iced's modals are
/// fully clickable and so are these.
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
    Submit,
    ToggleDefaultAgent,
    /// ThemePicker.
    ThemePickerTab(bool),
    ThemePickerToggleFollowSystem,
    ThemePickerUseDefault,
    /// The picker's own body-level "Apply" — the same commit path
    /// `ModalAction::ThemePickerSubmit` reaches from the keyboard.
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
}

/// The boolean settings the Settings modal flips, each persisted immediately
/// (`src/gui/view/modals/settings.rs:130-625`; recorded ambiguity 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingToggle {
    Tmux,
    SkipPermissions,
    Chrome,
    ThemeFollowSystem,
}

/// A weak-entity click dispatcher handed to the pure view functions.
pub type ModalDispatch = std::rc::Rc<dyn Fn(ModalClick, &mut Window, &mut App)>;
