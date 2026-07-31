//! `ModalLayer` — one entity, one slot, the scrim, focus-on-mount and the
//! Escape routing.
//!
//! Port of `src/gui/view/modals/mod.rs:30-151` (`modal_layer`) plus the
//! lifecycle half of `src/gui/update/modals.rs`. The state machine itself is
//! [`crate::modal`]; nothing here re-decides a verdict.
//!
//! Two documented exceptions to "centered on a scrim":
//! - **Onboarding** replaces the screen entirely — full-viewport, no sidebar,
//!   no statusbar, no scrim (`view/modals/mod.rs:107-110`).
//! - **SessionLauncher** top-drops instead of centering
//!   (`view/modals/mod.rs:114-121`).
//!
//! # Why Escape reaches here from inside a focused field
//!
//! `InputState::escape()` calls `cx.propagate()` (vendored
//! `input/state.rs:1685`), so the keystroke survives binding dispatch and
//! gpui's `finish_dispatch_key_event` then runs this element's bubble-phase
//! `on_key_down`. That is the structural replacement for iced's
//! `should_forward` Escape carve-out — see [`crate::modal`]'s module doc.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

pub mod add_project;
pub mod confirm;
pub mod input;
pub mod launcher;
pub mod project;
pub mod settings;
pub mod shell;
pub mod theme_picker;

use gpui::{
    div, prelude::*, AnimationExt as _, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Window,
};

use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::SessionRegistry;
use crate::entities::toast::ToastState;
use crate::entities::upgrade::Upgrade;
use crate::entities::workspace_state::WorkspaceState;
use crate::modal::{
    key_verdict, CancelOutcome, KeyCtx, Modal, ModalAction, ModalKey, ModalKeyVerdict, ModalKind,
    ModalMods, ModalSlot,
};
use crate::settings::SettingsState;

use self::input::ModalInput;

/// Every mouse-driven intent a modal's chrome can raise. Buttons and
/// checkboxes only own a `&mut Window, &mut App`, so they cannot touch the
/// layer entity directly; they raise one of these through [`ModalDispatch`],
/// a weak-entity closure built by [`ModalLayer::dispatcher`]. This is the
/// mouse half of the keyboard verdict table — iced's modals are fully
/// clickable and so are these.
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
    /// AddProject / Onboarding wizard.
    WizardBrowse,
    WizardPickDir(usize),
    WizardNext,
    WizardBack,
    WizardToggleInitGit,
    OnboardSkip,
    OnboardAdvance,
    OnboardPickAgent(usize),
    OnboardPerms(bool),
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

/// What the layer cannot do for itself and hands back to `Workspace`.
#[derive(Clone, Debug)]
pub enum ModalEvent {
    /// The quit confirm was accepted: flush and close the window.
    Quit,
    /// The slot became empty; focus goes back to the body.
    Closed,
    /// tmux was switched on; the workspace re-scans for sidecar sessions.
    TmuxEnabled,
    /// The post-update restart: relaunch, flush, exit.
    RestartApp,
    /// Spawn an agent session through `Sidebar::spawn_session`, so the toast
    /// producer covers a failure exactly once (recorded ambiguity 7).
    SpawnAgent {
        project: String,
        wt_path: String,
        agent: grove_core::agent::Agent,
    },
    /// Add a worktree to the selected project and (re)build the tree.
    WorktreeAdded,
    /// A project's worktrees changed on disk; the tree cache is stale.
    TreeInvalidated,
    /// The palette's terminal rows: spawn and focus a home terminal.
    NewHomeTerminal,
    /// The switch drill-in picked a session.
    SelectSession(crate::entities::session_registry::SessionId),
    /// The switch drill-in picked a home terminal, by index.
    SelectTerminal(usize),
}

/// The single modal slot, its focus, and whatever field the open modal owns.
pub struct ModalLayer {
    slot: ModalSlot,
    focus: FocusHandle,
    /// The open modal's text fields, rebuilt whenever the slot is repointed.
    /// Dropping them with the slot is what makes "replace drops the old state"
    /// a type property rather than a discipline (carried decision 4).
    ///
    /// Index conventions, per modal: `Input`/`SessionLauncher` = `[0]` the
    /// single field; `AddProject` = `[0]` the path (step 1) or the name
    /// (step 2); `Onboarding` = `[0]` path, `[1]` name; `ScriptsEditor` =
    /// `[0]` setup, `[1]` run, `[2]` teardown; `ThemeManager` = `[0]` the
    /// editor buffer when the editor sub-view is open.
    pub(super) fields: Vec<ModalInput>,
    /// One OS dialog at a time — a second click while the picker is up must
    /// not spawn another (`modals.rs:490-534`).
    pub(super) picker_open: bool,
    /// Set while the modal that just opened has not been focused yet; the
    /// first `render` with a `&mut Window` performs the focus
    /// (carried decision 5).
    needs_focus: bool,
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
    tree: Entity<ProjectTree>,
    toast: Entity<ToastState>,
    activity: Entity<ActivityStore>,
    clock: Entity<AnimationClock>,
    /// The upgrade flow the Updates/Changelog views render and act on.
    pub(super) upgrade: Entity<Upgrade>,
    /// The Settings → Tools rows, detected off-thread whenever Settings opens
    /// or the refresh button is clicked (`src/gui/update/upgrade.rs:158-191`).
    pub(super) tools: Vec<settings::ToolStatus>,
    tools_task: Option<gpui::Task<()>>,
    /// The teardown script's live PTY view. Modal-owned, never in the
    /// registry — a teardown PTY must not appear in the rail, exactly as
    /// iced keeps it out of `app.sessions` (`src/app/modal.rs:163-175`).
    pub(super) teardown_view: Option<Entity<crate::views::terminal_view::TerminalView>>,
    pub(super) teardown_session: Option<Entity<crate::entities::terminal_session::TerminalSession>>,
    /// Polls the teardown script for exit; dropped when the stage advances.
    teardown_poll: Option<gpui::Task<()>>,
}

impl EventEmitter<ModalEvent> for ModalLayer {}

impl Focusable for ModalLayer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ModalLayer {
    // One owner, one constructor: the layer genuinely needs every entity it is
    // handed, and a `Deps` struct here would only move the same eight names one
    // indirection away.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Entity<WorkspaceState>,
        registry: Entity<SessionRegistry>,
        tree: Entity<ProjectTree>,
        toast: Entity<ToastState>,
        activity: Entity<ActivityStore>,
        clock: Entity<AnimationClock>,
        upgrade: Entity<Upgrade>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            slot: ModalSlot::new(),
            focus: cx.focus_handle(),
            fields: Vec::new(),
            picker_open: false,
            needs_focus: false,
            state,
            registry,
            tree,
            toast,
            activity,
            clock,
            upgrade,
            tools: Vec::new(),
            tools_task: None,
            teardown_view: None,
            teardown_session: None,
            teardown_poll: None,
        }
    }

    /// A weak-entity click dispatcher for the pure view functions. Mirrors
    /// `Workspace::dispatcher`.
    pub fn dispatcher(cx: &mut Context<Self>) -> ModalDispatch {
        let weak = cx.entity().downgrade();
        std::rc::Rc::new(move |click, window, cx: &mut App| {
            let _ = weak.update(cx, |this: &mut Self, cx| this.on_click(click, window, cx));
        })
    }

    pub fn is_open(&self) -> bool {
        self.slot.is_open()
    }

    pub fn kind(&self) -> Option<ModalKind> {
        self.slot.kind()
    }

    pub fn slot(&self) -> &ModalSlot {
        &self.slot
    }

    /// Open `modal`, replacing whatever was there. The old modal's field is
    /// dropped with it; the new one's is built on the next render, which is
    /// the first point a `&mut Window` exists.
    pub fn open(&mut self, modal: Modal, cx: &mut Context<Self>) {
        // `on_open_settings` dispatches the tool scan alongside opening the
        // modal (`src/gui/update/mod.rs:551-555`).
        if matches!(modal.kind(), ModalKind::Settings) {
            self.detect_tools(cx);
        }
        self.slot.open(modal);
        self.fields.clear();
        self.needs_focus = true;
        cx.notify();
    }

    /// The window's close request. The quit confirm clobbers whatever is open
    /// and cancelling does not restore it — a known, deliberately preserved
    /// gap (`modals.rs:350-354`).
    pub fn open_quit_confirm(&mut self, native_running: usize, cx: &mut Context<Self>) {
        self.slot.open_quit_confirm(native_running);
        self.fields.clear();
        self.needs_focus = true;
        cx.notify();
    }

    /// Route through the state machine's `cancel`, which is **not** a synonym
    /// for close (Teardown skips, RemoveProject refuses, ThemePicker and the
    /// changelog return to their parent).
    pub fn cancel(&mut self, cx: &mut Context<Self>) -> CancelOutcome {
        let outcome = self.slot.cancel();
        match outcome {
            CancelOutcome::Refused => {}
            CancelOutcome::SkippedTeardownScript => {
                self.skip_teardown_script(cx);
            }
            CancelOutcome::Closed => {
                self.fields.clear();
                cx.emit(ModalEvent::Closed);
            }
            CancelOutcome::ReturnedTo(_) => {
                self.fields.clear();
                self.needs_focus = true;
            }
        }
        cx.notify();
        outcome
    }

    /// Force the slot empty, for paths that already decided.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.slot.close();
        self.fields.clear();
        cx.emit(ModalEvent::Closed);
        cx.notify();
    }

    // ── keyboard ────────────────────────────────────────────────────────

    /// Translate a gpui keystroke into the pure alphabet. `None` means the
    /// table has nothing to say and the key belongs to whatever is focused.
    fn translate(ev: &KeyDownEvent) -> Option<(ModalKey, ModalMods)> {
        let ks = &ev.keystroke;
        let key = match ks.key.as_str() {
            "escape" => ModalKey::Escape,
            "enter" => ModalKey::Enter,
            "tab" => ModalKey::Tab,
            "space" => ModalKey::Space,
            "up" => ModalKey::Up,
            "down" => ModalKey::Down,
            "left" => ModalKey::Left,
            "right" => ModalKey::Right,
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => ModalKey::Char(c),
                    _ => return None,
                }
            }
        };
        let m = ks.modifiers;
        let mods = ModalMods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            // The global-shortcut modifier: Cmd on macOS, Ctrl+Shift elsewhere
            // (`keymap::platform_mod_prefix`).
            platform: if cfg!(target_os = "macos") {
                m.platform
            } else {
                m.control && m.shift
            },
        };
        Some((key, mods))
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(modal) = self.slot.get() else {
            return;
        };
        let Some((key, mods)) = Self::translate(ev) else {
            return;
        };
        let ctx = KeyCtx {
            // Genuinely in flight now: `escape_closes` is the real answer, and
            // an apply refuses Escape until it lands.
            update_in_flight: !crate::entities::upgrade_state::escape_closes(
                self.upgrade.read(cx).state(),
            ),
            is_shortcut_overlay_chord: false,
        };
        let verdict = key_verdict(modal, key, mods, ctx);
        match verdict {
            // Not ours by the shared table: the palette owns its whole
            // keyboard, the wizard delegate owns the rest, and anything
            // neither claims belongs to the focused field.
            ModalKeyVerdict::FallThrough => {
                let claimed = match self.slot.kind() {
                    Some(ModalKind::SessionLauncher) => self.palette_key(key, mods, window, cx),
                    Some(ModalKind::AddProject | ModalKind::Onboarding) => {
                        self.wizard_key(key, mods, window, cx)
                    }
                    _ => false,
                };
                if claimed {
                    cx.stop_propagation();
                }
                return;
            }
            ModalKeyVerdict::Ignore => {}
            ModalKeyVerdict::Close => {
                self.cancel(cx);
            }
            ModalKeyVerdict::Submit => self.submit(window, cx),
            ModalKeyVerdict::Move(delta) => self.move_selection(delta, cx),
            ModalKeyVerdict::Custom(action) => self.perform(action, window, cx),
        }
        // Everything the table claimed — `Ignore` included — stops here. An
        // ignored key must not fall through to the workspace behind the scrim.
        cx.stop_propagation();
    }

    // ── the effects the verdicts name ───────────────────────────────────

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(modal) = self.slot.get_mut() else {
            return;
        };
        let step = |sel: &mut usize, len: usize| {
            if len == 0 {
                return;
            }
            let next = (*sel as i32 + delta).rem_euclid(len as i32);
            *sel = next as usize;
        };
        match modal {
            Modal::AgentPicker { sel, .. } => {
                step(sel, confirm::AVAILABLE_AGENTS.len());
                cx.notify();
            }
            Modal::ThemePicker { .. } => self.theme_picker_move(delta, cx),
            Modal::ThemeManager { .. } => self.theme_manager_move(delta, cx),
            Modal::Onboarding { .. } | Modal::AddProject(_) => {
                self.wizard_dir_move(delta, cx);
            }
            _ => cx.notify(),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.slot.kind() {
            Some(ModalKind::Input) => self.submit_input(window, cx),
            Some(ModalKind::AgentPicker) => self.submit_agent_picker(cx),
            Some(ModalKind::AddProject) => self.wizard_next(window, cx),
            _ => {}
        }
    }

    fn perform(&mut self, action: ModalAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ModalAction::Confirm(yes) => self.resolve_confirm(yes, window, cx),
            ModalAction::ArchiveConfirm => self.archive_confirm(cx),
            ModalAction::RemoveProjectConfirm => self.kick_off_remove_project(cx),
            ModalAction::ToggleRemoveWorktrees => {
                if let Some(Modal::RemoveProject {
                    also_remove_worktrees,
                    ..
                }) = self.slot.get_mut()
                {
                    *also_remove_worktrees = !*also_remove_worktrees;
                }
                cx.notify();
            }
            ModalAction::ToggleDefaultAgent => self.toggle_default_agent(cx),
            ModalAction::ChooseTmux(enabled) => self.choose_tmux(enabled, cx),
            ModalAction::ThemePickerSubmit => self.theme_picker_submit(cx),
            ModalAction::ThemePickerSwitchTab => self.theme_picker_switch_tab(cx),
            ModalAction::ThemeManagerDeleteConfirm => self.theme_manager_delete_confirm(cx),
            ModalAction::ThemeManagerDeleteCancel => self.theme_manager_delete_cancel(cx),
            ModalAction::ThemeManagerRenameSubmit => self.theme_manager_rename_submit(cx),
            ModalAction::ThemeManagerRenameCancel => self.theme_manager_rename_cancel(cx),
            ModalAction::OnboardSkip => self.onboard_skip(cx),
            ModalAction::OnboardAdvance => self.onboard_advance(window, cx),
            ModalAction::OnboardToggleFocus => self.onboard_toggle_focus(window, cx),
        }
    }

    /// The mouse half of the verdict table. Every arm is the same effect the
    /// keyboard path performs, so a click and its keystroke can never diverge.
    fn on_click(&mut self, click: ModalClick, window: &mut Window, cx: &mut Context<Self>) {
        match click {
            ModalClick::Cancel => {
                self.cancel(cx);
            }
            ModalClick::Confirm(yes) => self.resolve_confirm(yes, window, cx),
            ModalClick::ChooseTmux(on) => self.choose_tmux(on, cx),
            ModalClick::ToggleRemoveWorktrees => {
                self.perform(ModalAction::ToggleRemoveWorktrees, window, cx);
            }
            ModalClick::RemoveProjectConfirm => self.kick_off_remove_project(cx),
            ModalClick::ArchiveConfirm => self.archive_confirm(cx),
            ModalClick::ArchiveKillSessions => self.archive_kill_sessions(cx),
            ModalClick::RestoreArchived(idx) => self.restore_archived(idx, cx),
            ModalClick::DeleteArchived(idx) => self.delete_archived(idx, cx),
            ModalClick::SelectRow(i) => {
                if let Some(Modal::AgentPicker { sel, .. }) = self.slot.get_mut() {
                    *sel = i;
                }
                cx.notify();
            }
            ModalClick::Submit => self.submit(window, cx),
            ModalClick::ToggleDefaultAgent => self.toggle_default_agent(cx),
            ModalClick::CheckUpdates => {
                self.upgrade.update(cx, |u, cx| u.check(true, cx));
            }
            ModalClick::StartUpdate => {
                self.upgrade.update(cx, Upgrade::start_update);
                self.open(Modal::Updating, cx);
            }
            ModalClick::SkipVersion => {
                self.upgrade.update(cx, Upgrade::skip);
                cx.notify();
            }
            ModalClick::CopyReleaseUrl => self.copy_release_url(cx),
            ModalClick::RestartApp => cx.emit(ModalEvent::RestartApp),
            ModalClick::RefreshTools => self.detect_tools(cx),
            ModalClick::SetDefaultAgent(agent) => {
                SettingsState::update(cx, move |store| store.default_agent = Some(agent));
                SettingsState::flush_now(cx);
                cx.notify();
            }
            other => self.on_click_late(other, window, cx),
        }
    }

    /// The clicks owned by Tasks 4-6's modals. Split out purely to keep
    /// [`Self::on_click`] readable.
    fn on_click_late(&mut self, click: ModalClick, window: &mut Window, cx: &mut Context<Self>) {
        self.on_wizard_click(click.clone(), window, cx);
    }

    // ── render ──────────────────────────────────────────────────────────

    /// Build the open modal's fields, if it has any. Called on the first
    /// render after the slot is repointed, which is the first point a
    /// `&mut Window` exists (carried decision 5).
    fn ensure_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.needs_focus {
            return;
        }
        self.needs_focus = false;
        self.build_fields(window, cx);
    }

    /// Rebuild the fields immediately (a wizard step changed, so the field set
    /// changed with it) and re-focus.
    pub(super) fn rebuild_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.needs_focus = false;
        self.build_fields(window, cx);
        cx.notify();
    }

    fn build_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use input::InputPolicy;
        self.fields.clear();
        let Some(modal) = self.slot.get() else {
            return;
        };
        let kind = modal.kind();
        let policy = InputPolicy::for_modal(kind);
        match modal {
            Modal::Input { buffer, .. } => {
                self.fields
                    .push(ModalInput::single_line(policy, "", buffer, window, cx));
            }
            Modal::SessionLauncher(st) => {
                self.fields.push(ModalInput::single_line(
                    policy,
                    "Search projects & sessions…",
                    &st.query,
                    window,
                    cx,
                ));
            }
            Modal::AddProject(st) => {
                let (placeholder, initial) = match st.step {
                    crate::modal::AddProjectStep::PickSource => ("~/path/to/project", &st.path),
                    crate::modal::AddProjectStep::Details => {
                        ("project name (defaults to the folder)", &st.name)
                    }
                };
                self.fields.push(ModalInput::single_line(
                    policy,
                    placeholder,
                    initial,
                    window,
                    cx,
                ));
            }
            Modal::Onboarding {
                step: crate::modal::OnboardStep::Project,
                path,
                name,
                ..
            } => {
                self.fields.push(ModalInput::single_line(
                    policy,
                    "~/path/to/project",
                    path,
                    window,
                    cx,
                ));
                self.fields.push(ModalInput::single_line(
                    policy,
                    "project name (optional)",
                    name.as_deref().unwrap_or(""),
                    window,
                    cx,
                ));
            }
            Modal::ScriptsEditor(st) => {
                for (placeholder, initial) in [
                    ("setup script", &st.setup),
                    ("run script", &st.run),
                    ("teardown script", &st.teardown),
                ] {
                    self.fields
                        .push(ModalInput::multi_line(placeholder, initial, 6, window, cx));
                }
            }
            Modal::ThemeManager {
                editor: Some(buffer),
                ..
            } => {
                self.fields.push(ModalInput::multi_line(
                    "paste a theme JSON object",
                    buffer,
                    14,
                    window,
                    cx,
                ));
            }
            _ => {}
        }
        match self.fields.first() {
            // A field that is never focused silently eats nothing and looks
            // broken.
            Some(f) => f.focus_at_end(window, cx),
            // Every modal without a field focuses its own root, so Escape and
            // its letter keys have somewhere to land.
            None => window.focus(&self.focus, cx),
        }
    }

    /// Pull the live field buffers back into the slot before any decision that
    /// reads them. gpui-component owns the text; the slot owns the truth.
    pub(super) fn sync_wizard_buffers(&mut self, cx: &mut Context<Self>) {
        let values: Vec<String> = self.fields.iter().map(|f| f.value(cx)).collect();
        match self.slot.get_mut() {
            Some(Modal::AddProject(st)) => {
                if let Some(v) = values.first() {
                    match st.step {
                        crate::modal::AddProjectStep::PickSource => st.path.clone_from(v),
                        crate::modal::AddProjectStep::Details => st.name.clone_from(v),
                    }
                }
            }
            Some(Modal::Onboarding { path, name, .. }) => {
                if let Some(v) = values.first() {
                    path.clone_from(v);
                }
                if let Some(v) = values.get(1) {
                    *name = (!v.trim().is_empty()).then(|| v.clone());
                }
            }
            Some(Modal::ScriptsEditor(st)) => {
                if let Some(v) = values.first() {
                    st.setup.clone_from(v);
                }
                if let Some(v) = values.get(1) {
                    st.run.clone_from(v);
                }
                if let Some(v) = values.get(2) {
                    st.teardown.clone_from(v);
                }
            }
            Some(Modal::SessionLauncher(st)) => {
                if let Some(v) = values.first() {
                    st.query.clone_from(v);
                }
            }
            Some(Modal::ThemeManager { editor, .. }) => {
                if let (Some(buf), Some(v)) = (editor.as_mut(), values.first()) {
                    buf.clone_from(v);
                }
            }
            _ => {}
        }
    }
}

impl Render for ModalLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_fields(window, cx);
        let Some(kind) = self.slot.kind() else {
            return div().into_any_element();
        };

        let dispatch = Self::dispatcher(cx);
        let panel = match kind {
            ModalKind::Input
            | ModalKind::Confirm
            | ModalKind::Message
            | ModalKind::TmuxChoice
            | ModalKind::AgentPicker => confirm::render(self, &dispatch, cx),
            ModalKind::RemoveProject
            | ModalKind::ArchiveProject
            | ModalKind::ArchivedProjects
            | ModalKind::Teardown => project::render(self, &dispatch, cx),
            ModalKind::AddProject | ModalKind::Onboarding => {
                add_project::render(self, &dispatch, cx)
            }
            ModalKind::SessionLauncher => launcher::render(self, &dispatch, cx),
            ModalKind::ThemePicker | ModalKind::ThemeManager => {
                theme_picker::render(self, &dispatch, cx)
            }
            ModalKind::Settings
            | ModalKind::ShortcutOverlay
            | ModalKind::ScriptsEditor
            | ModalKind::Updating
            | ModalKind::Changelog => settings::render(self, &dispatch, cx),
        };

        let framed = if kind.is_screen_replacement() {
            // Onboarding is not a modal-layer modal: it replaces the screen —
            // full viewport, no sidebar, no statusbar, no scrim. The entrance
            // animation is spec §4's `with_animation`.
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(crate::theme::BG())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .child(panel)
                        // Spec §4's entrance animation.
                        .with_animation(
                            "onboarding-enter",
                            gpui::Animation::new(std::time::Duration::from_millis(320))
                                .with_easing(gpui::ease_out_quint()),
                            gpui::Styled::opacity,
                        ),
                )
        } else if kind.top_drops() {
            shell::scrim_top_drop(panel)
        } else {
            shell::scrim(panel)
        };

        div()
            .id("modal-layer")
            .key_context(kind.key_context())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(framed)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind either replaces the screen, top-drops or centers — and the
    /// two exceptions are exactly the two the oracle documents.
    #[test]
    fn only_onboarding_replaces_the_screen_and_only_the_palette_top_drops() {
        for kind in ModalKind::ALL {
            assert_eq!(
                kind.is_screen_replacement(),
                kind == ModalKind::Onboarding,
                "{kind:?}"
            );
            assert_eq!(
                kind.top_drops(),
                kind == ModalKind::SessionLauncher,
                "{kind:?}"
            );
        }
    }
}
