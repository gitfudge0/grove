//! `ModalLayer` — one entity, one slot, the scrim, focus-on-mount and Escape routing.
//! Port of `src/gui/view/modals/mod.rs:30-151` + `src/gui/update/modals.rs`; state machine is [`crate::modal`].
//! Onboarding replaces the screen full-viewport (`view/modals/mod.rs:107-110`); SessionLauncher top-drops (`view/modals/mod.rs:114-121`).
//! Escape reaches here from a focused field because `InputState::escape()` calls `cx.propagate()` (`input/state.rs:1685`).

pub mod add_project;
pub mod confirm;
pub mod diff_viewer;
pub mod input;
pub mod launcher;
pub mod project;
pub mod settings;
pub mod theme_picker;

use crate::views::rpx;
use crate::views::tokens::SPACE_LG;
use gpui::{
    div, prelude::*, AnimationExt as _, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Window,
};

use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::diff_viewer::DiffViewerState;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::SessionRegistry;
use crate::entities::toast::ToastState;
use crate::entities::upgrade::Upgrade;
use crate::entities::workspace_state::WorkspaceState;
use crate::modal::{
    key_verdict, CancelOutcome, KeyCtx, LauncherView, Modal, ModalAction, ModalKey,
    ModalKeyVerdict, ModalKind, ModalMods, ModalSlot,
};
use crate::settings::SettingsState;

use self::input::ModalInput;

pub use crate::views::dispatch::{ModalClick, ModalDispatch, SettingToggle};

/// What the layer cannot do for itself and hands back to `Workspace`.
#[derive(Clone, Debug)]
pub enum ModalEvent {
    Quit,
    Closed,
    TmuxEnabled,
    RestartApp,
    /// Toast producer covers a failure exactly once (recorded ambiguity 7).
    SpawnAgent {
        project: String,
        wt_path: String,
        agent: grove_core::agent::Agent,
    },
    WorktreeAdded {
        path: String,
    },
    TreeInvalidated,
    NewHomeTerminal,
    SelectSession(crate::entities::session_registry::SessionId),
    SelectTerminal(usize),
    RunScript {
        wt_path: String,
        script: String,
    },
}

/// The single modal slot, its focus, and whatever field the open modal owns.
pub struct ModalLayer {
    slot: ModalSlot,
    focus: FocusHandle,
    /// Index conventions per modal: see `build_fields` for the field-order mapping.
    pub(super) fields: Vec<ModalInput>,
    /// Kept alive to sync buffers on backspace/delete, which `InputState` consumes before this element's key handler sees them.
    field_subs: Vec<gpui::Subscription>,
    /// Guards against a second OS dialog while one is open (`modals.rs:490-534`).
    pub(super) picker_open: bool,
    /// Set until the first `render` with a `&mut Window` performs the focus.
    needs_focus: bool,
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
    tree: Entity<ProjectTree>,
    toast: Entity<ToastState>,
    activity: Entity<ActivityStore>,
    clock: Entity<AnimationClock>,
    pub(super) upgrade: Entity<Upgrade>,
    pub(super) tools: Vec<settings::ToolStatus>,
    tools_task: Option<gpui::Task<()>>,
    /// Modal-owned, never in the registry — a teardown PTY must not appear in the rail (`src/app/modal.rs:163-175`).
    pub(super) teardown_view: Option<Entity<crate::views::terminal_view::TerminalView>>,
    pub(super) teardown_session: Option<Entity<crate::entities::terminal_session::TerminalSession>>,
    teardown_poll: Option<gpui::Task<()>>,
    pub(super) diff_viewer: Option<Entity<crate::entities::diff_viewer::DiffViewerState>>,
    /// Session-scoped hand-dragged width override; `None` means auto-fit. Lives here (not on `DiffViewerState`) because that entity is recreated on every open/close.
    pub(crate) file_list_w_override: Option<f32>,
    diff_divider_drag: Option<crate::views::components::DividerDrag>,
    last_diff_divider_press: Option<std::time::Instant>,
    pub(super) palette_scroll: gpui::ScrollHandle,
    /// Only a changed selection may move the scroll, or the wheel would snap back every frame.
    palette_scrolled_to: std::cell::Cell<Option<(crate::modal::LauncherView, usize)>>,
    /// Shared by the theme picker and the wizard's directory matches; only one renders at a time.
    pub(super) list_scroll: gpui::ScrollHandle,
    list_scrolled_to: std::cell::Cell<Option<(usize, usize)>>,
}

impl EventEmitter<ModalEvent> for ModalLayer {}

impl Focusable for ModalLayer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ModalLayer {
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
            field_subs: Vec::new(),
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
            diff_viewer: None,
            file_list_w_override: None,
            diff_divider_drag: None,
            last_diff_divider_press: None,
            palette_scroll: gpui::ScrollHandle::new(),
            palette_scrolled_to: std::cell::Cell::new(None),
            list_scroll: gpui::ScrollHandle::new(),
            list_scrolled_to: std::cell::Cell::new(None),
        }
    }

    /// Mirrors `Workspace::dispatcher`.
    pub fn dispatcher(cx: &mut Context<Self>) -> ModalDispatch {
        let weak = cx.entity().downgrade();
        std::rc::Rc::new(move |click, window, cx: &mut App| {
            let _ = weak.update(cx, |this: &mut Self, cx| this.on_click(click, window, cx));
        })
    }

    pub fn is_open(&self) -> bool {
        self.slot.is_open()
    }

    /// [`diff_viewer::effective_mode`]'s input; must stay in agreement with [`diff_viewer::render`]'s own `content_w`.
    pub(crate) fn diff_content_w(&self, window: &Window, cx: &App) -> f32 {
        let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
        let win_w = f32::from(window.viewport_size().width) / zoom;
        let file_list_w = self
            .diff_viewer
            .as_ref()
            .map_or(crate::views::tokens::DIFF_FILE_LIST_W, |dv| {
                diff_viewer::file_list_w(dv.read(cx), window, self.file_list_w_override)
            });
        win_w - crate::views::tokens::DIFF_PANEL_INSET * 2.0 - file_list_w
    }

    fn logical_window_width(window: &Window, cx: &App) -> f32 {
        let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
        f32::from(window.viewport_size().width) / zoom
    }

    /// Mirrors `Sidebar::on_divider_press`: double-click resets to auto-fit, else starts a drag.
    fn on_diff_divider_press(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(dv) = self.diff_viewer.clone() else {
            return;
        };
        let now = std::time::Instant::now();
        let double = self
            .last_diff_divider_press
            .is_some_and(|t| now.duration_since(t) < crate::views::components::DOUBLE_CLICK);
        if double {
            self.diff_divider_drag = None;
            self.last_diff_divider_press = None;
            self.file_list_w_override = None;
            cx.notify();
        } else {
            self.last_diff_divider_press = Some(now);
            let start_width =
                diff_viewer::file_list_w(dv.read(cx), window, self.file_list_w_override);
            self.diff_divider_drag = Some(crate::views::components::DividerDrag {
                grab_offset: None,
                start_width,
            });
        }
    }

    /// Mirrors `Sidebar::on_root_mouse_move`; attached to the root div, the only element that keeps receiving moves off the divider's hit zone.
    pub(crate) fn on_diff_divider_mouse_move(
        &mut self,
        cursor_x: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.diff_divider_drag else {
            return;
        };
        let offset = match drag.grab_offset {
            Some(o) => o,
            None => {
                let o = drag.start_width - cursor_x;
                self.diff_divider_drag = Some(crate::views::components::DividerDrag {
                    grab_offset: Some(o),
                    ..drag
                });
                o
            }
        };
        let win_w = Self::logical_window_width(window, cx);
        let clamped = diff_viewer::clamp_file_list_w(cursor_x + offset, win_w);
        // Epsilon guard lives here (not on mouse-up) because the override is written on every move, not just release.
        if (clamped - drag.start_width).abs() < crate::views::components::DRAG_EPSILON {
            return;
        }
        self.file_list_w_override = Some(clamped);
        cx.notify();
    }

    /// Mirrors `Sidebar::on_root_mouse_up`; no persistence, the override is session-only.
    pub(crate) fn on_diff_divider_mouse_up(&mut self, _cx: &mut Context<Self>) {
        self.diff_divider_drag = None;
    }

    #[allow(dead_code)]
    pub fn kind(&self) -> Option<ModalKind> {
        self.slot.kind()
    }

    pub fn slot(&self) -> &ModalSlot {
        &self.slot
    }

    /// `child_ix` is the container's direct-child index, not `sel` — `card` interleaves dividers so the two differ.
    pub(super) fn scroll_list_to(&self, tag: usize, sel: usize, child_ix: usize) {
        if self.list_scrolled_to.get() == Some((tag, sel)) {
            return;
        }
        self.list_scrolled_to.set(Some((tag, sel)));
        self.list_scroll.scroll_to_item(child_ix);
    }

    pub(super) fn reset_list_scroll(&self) {
        self.list_scroll.set_offset(gpui::Point::default());
        self.list_scrolled_to.set(None);
    }

    pub fn open(&mut self, modal: Modal, cx: &mut Context<Self>) {
        if matches!(modal.kind(), ModalKind::Settings) {
            self.detect_tools(cx);
        }
        if matches!(modal.kind(), ModalKind::SessionLauncher) {
            let active_proj = self.state.read(cx).proj_idx();
            let targets = crate::entities::project_tree::cache_sweep_targets(
                &cx.global::<crate::settings::SettingsState>().store,
                active_proj,
            );
            self.tree.update(cx, |tree, cx| {
                tree.sweep_wt_cache(targets, cx);
            });
        }
        self.diff_viewer = if let Modal::DiffViewer { wt_path } = &modal {
            let mode = cx
                .global::<crate::settings::SettingsState>()
                .store
                .diff_mode;
            Some(cx.new(|cx| {
                crate::entities::diff_viewer::DiffViewerState::new(wt_path.clone(), mode, cx)
            }))
        } else {
            None
        };
        let is_input = matches!(modal.kind(), ModalKind::Input);
        self.slot.open(modal);
        self.reset_list_scroll();
        self.fields.clear();
        self.field_subs.clear();
        self.needs_focus = true;
        if is_input {
            self.load_base_branches(cx);
        }
        cx.notify();
    }

    /// Not a synonym for close: Teardown skips, RemoveProject refuses, ThemePicker/changelog return to their parent.
    pub fn cancel(&mut self, cx: &mut Context<Self>) -> CancelOutcome {
        // Must run before `slot.cancel()` may swap the picker out; self-guarding and a no-op once `original` is consumed by commit.
        self.restore_theme_before_leaving(cx);
        let outcome = self.slot.cancel();
        match outcome {
            CancelOutcome::Refused => {}
            CancelOutcome::SkippedTeardownScript => {
                self.skip_teardown_script(cx);
            }
            CancelOutcome::Closed => {
                self.fields.clear();
                self.field_subs.clear();
                self.diff_viewer = None;
                cx.emit(ModalEvent::Closed);
            }
            CancelOutcome::ReturnedTo(_) => {
                self.fields.clear();
                self.field_subs.clear();
                self.diff_viewer = None;
                self.needs_focus = true;
            }
        }
        cx.notify();
        outcome
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.slot.close();
        self.fields.clear();
        self.field_subs.clear();
        self.diff_viewer = None;
        cx.emit(ModalEvent::Closed);
        cx.notify();
    }

    /// `None` means the key belongs to whatever is focused.
    fn translate(ev: &KeyDownEvent) -> Option<(ModalKey, ModalMods)> {
        let ks = &ev.keystroke;
        let key = match ks.key.as_str() {
            "escape" => ModalKey::Escape,
            "enter" => ModalKey::Enter,
            "tab" => ModalKey::Tab,
            "space" => ModalKey::Space,
            "backspace" => ModalKey::Backspace,
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
            // Cmd on macOS, Ctrl+Shift elsewhere (`keymap::platform_mod_prefix`).
            platform: if cfg!(target_os = "macos") {
                m.platform
            } else {
                m.control && m.shift
            },
        };
        Some((key, mods))
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, mods)) = Self::translate(ev) else {
            return;
        };
        self.handle_key(key, mods, window, cx);
    }

    /// Reclaimed as actions (`keymap::modal_input_bindings`) so there is one verdict table, not two.
    fn on_modal_key(
        &mut self,
        key: ModalKey,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mods = ModalMods {
            shift,
            ..ModalMods::default()
        };
        self.handle_key(key, mods, window, cx);
    }

    fn handle_key(
        &mut self,
        key: ModalKey,
        mods: ModalMods,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.slot.get() else {
            return;
        };
        let ctx = KeyCtx {
            update_in_flight: !crate::entities::upgrade_state::escape_closes(
                self.upgrade.read(cx).state(),
            ),
            is_shortcut_overlay_chord: false,
        };
        let verdict = key_verdict(modal, key, mods, ctx);
        match verdict {
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
        // `Ignore` must also stop here, or the key falls through to the workspace behind the scrim.
        cx.stop_propagation();
    }

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
            Modal::Input { base, .. } if base.open => {
                base.move_highlight(delta);
                cx.notify();
            }
            Modal::ThemePicker { .. } => self.theme_picker_move(delta, cx),
            Modal::ThemeManager { .. } => self.theme_manager_move(delta, cx),
            Modal::Onboarding { .. } | Modal::AddProject(_) => {
                self.wizard_dir_move(delta, cx);
            }
            Modal::DiffViewer { .. } => {
                if let Some(dv) = self.diff_viewer.clone() {
                    let body_focused = dv.read(cx).body_focused;
                    dv.update(cx, |dv, cx| {
                        if body_focused {
                            dv.scroll_body(delta, cx);
                        } else {
                            dv.move_selection(delta, cx);
                        }
                    });
                }
                cx.notify();
            }
            _ => cx.notify(),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.slot.kind() {
            Some(ModalKind::Input) => self.submit_input(window, cx),
            Some(ModalKind::AgentPicker) => self.submit_agent_picker(cx),
            Some(ModalKind::AddProject) => self.wizard_next(window, cx),
            Some(ModalKind::ScriptsEditor) => self.save_scripts(cx),
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
            ModalAction::BaseDropdownClose => self.close_base_dropdown(window, cx),
            ModalAction::BaseDropdownPick => self.pick_base_branch(None, window, cx),
            ModalAction::BaseFilterPush(c) => self.edit_base_filter(Some(c), cx),
            ModalAction::BaseFilterPop => self.edit_base_filter(None, cx),
            ModalAction::ChooseTmux(enabled) => self.choose_tmux(enabled, cx),
            ModalAction::ThemePickerSubmit => self.theme_picker_submit(cx),
            ModalAction::ThemePickerSwitchTab => self.theme_picker_switch_tab(cx),
            ModalAction::ThemeManagerDeleteConfirm => self.theme_manager_delete_confirm(cx),
            ModalAction::ThemeManagerDeleteCancel => self.theme_manager_delete_cancel(cx),
            ModalAction::ThemeManagerRenameSubmit => self.theme_manager_rename_submit(cx),
            ModalAction::ThemeManagerRenameCancel => self.theme_manager_rename_cancel(cx),
            ModalAction::ScriptsRenameStart => self.scripts_rename_start(window, cx),
            ModalAction::ScriptsRenameCommit => self.scripts_rename_commit(window, cx),
            ModalAction::ScriptsRenameCancel => self.scripts_rename_cancel(window, cx),
            ModalAction::OnboardSkip => self.onboard_skip(cx),
            ModalAction::OnboardAdvance => self.onboard_advance(window, cx),
            ModalAction::OnboardToggleFocus => self.onboard_toggle_focus(window, cx),
            ModalAction::DiffFocusBody => {
                if let Some(dv) = self.diff_viewer.clone() {
                    dv.update(cx, DiffViewerState::focus_body);
                }
            }
            ModalAction::DiffToggleMode => {
                let Some(dv) = self.diff_viewer.clone() else {
                    return;
                };
                let content_w = self.diff_content_w(window, cx);
                let stored = cx
                    .global::<crate::settings::SettingsState>()
                    .store
                    .diff_mode;
                let (_, split_enabled) = diff_viewer::effective_mode(content_w, stored);
                if !split_enabled {
                    return;
                }
                let next = match stored {
                    grove_core::storage::DiffMode::Unified => grove_core::storage::DiffMode::Split,
                    grove_core::storage::DiffMode::Split => grove_core::storage::DiffMode::Unified,
                };
                SettingsState::update(cx, move |s| s.diff_mode = next);
                SettingsState::flush_now(cx);
                dv.update(cx, |dv, cx| dv.set_mode(next, cx));
            }
        }
    }

    /// Every arm is the same effect the keyboard path performs, so a click and its keystroke can't diverge.
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
            ModalClick::BaseDropdownToggle => self.toggle_base_dropdown(window, cx),
            ModalClick::BaseSelect(i) => self.pick_base_branch(Some(i), window, cx),
            ModalClick::SelectRow(i) => {
                enum SelectRowFollowUp {
                    Launcher,
                    Theme,
                }
                let follow_up = match self.slot.get_mut() {
                    Some(Modal::AgentPicker { sel, .. }) => {
                        *sel = i;
                        None
                    }
                    Some(Modal::SessionLauncher(st)) => {
                        st.sel = i;
                        if st.view != LauncherView::RowActions {
                            // In RowActions the anchor is the strip's session, not a row, and must survive the click.
                            st.anchor = None;
                        }
                        Some(SelectRowFollowUp::Launcher)
                    }
                    Some(Modal::ThemePicker { .. }) => Some(SelectRowFollowUp::Theme),
                    _ => None,
                };
                match follow_up {
                    Some(SelectRowFollowUp::Launcher) => self.activate_palette_row(window, cx),
                    Some(SelectRowFollowUp::Theme) => self.theme_picker_click(i, cx),
                    None => cx.notify(),
                }
            }
            ModalClick::Submit => self.submit(window, cx),
            ModalClick::ThemePickerApply => self.theme_picker_submit(cx),
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
            ModalClick::SelectDiffFile { path } => {
                if let Some(dv) = self.diff_viewer.clone() {
                    dv.update(cx, |dv, cx| dv.select(path, cx));
                }
            }
            ModalClick::SetDiffMode(mode) => {
                SettingsState::update(cx, move |s| s.diff_mode = mode);
                SettingsState::flush_now(cx);
                if let Some(dv) = self.diff_viewer.clone() {
                    dv.update(cx, |dv, cx| dv.set_mode(mode, cx));
                }
            }
            ModalClick::ToggleDiffListStyle => {
                if let Some(dv) = self.diff_viewer.clone() {
                    dv.update(cx, DiffViewerState::toggle_list_style);
                }
            }
            ModalClick::ToggleDiffTreeDir { path } => {
                if let Some(dv) = self.diff_viewer.clone() {
                    dv.update(cx, |dv, cx| dv.toggle_dir(path, cx));
                }
            }
            ModalClick::DiffFileListDividerPress => self.on_diff_divider_press(window, cx),
            other => self.on_click_late(other, window, cx),
        }
    }

    /// Split out purely to keep [`Self::on_click`] readable.
    fn on_click_late(&mut self, click: ModalClick, window: &mut Window, cx: &mut Context<Self>) {
        self.on_wizard_click(click.clone(), window, cx);
    }

    /// Called on the first render after the slot is repointed — the first point a `&mut Window` exists.
    fn ensure_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.needs_focus {
            return;
        }
        self.needs_focus = false;
        self.build_fields(window, cx);
    }

    pub(super) fn rebuild_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.needs_focus = false;
        self.build_fields(window, cx);
        cx.notify();
    }

    fn build_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use input::InputPolicy;
        self.fields.clear();
        self.field_subs.clear();
        let Some(modal) = self.slot.get() else {
            return;
        };
        let kind = modal.kind();
        let policy = InputPolicy::for_modal(kind);
        match modal {
            Modal::Input { buffer, .. } => {
                self.fields.push(ModalInput::single_line(
                    policy,
                    crate::modal::WORKTREE_NAME_PLACEHOLDER,
                    buffer,
                    window,
                    cx,
                ));
            }
            Modal::SessionLauncher(st) => {
                let placeholder = match st.scope {
                    crate::launcher::PaletteScope::All => "Search projects & sessions…",
                    crate::launcher::PaletteScope::WorktreesOnly => "Search worktrees…",
                };
                self.fields.push(ModalInput::single_line(
                    policy,
                    placeholder,
                    &st.query,
                    window,
                    cx,
                ));
            }
            Modal::AddProject(st) => {
                let (placeholder, initial): (String, &String) = match st.step {
                    crate::modal::AddProjectStep::PickSource => {
                        ("~/code/my-repo".to_string(), &st.path)
                    }
                    crate::modal::AddProjectStep::Details => {
                        (crate::add_project::path_basename(&st.path), &st.name)
                    }
                };
                self.fields.push(ModalInput::single_line(
                    policy,
                    &placeholder,
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
                // Example values, matching the AddProject wizard: the name field
                // shows the folder that an empty submit would actually use.
                let name_hint = crate::add_project::path_basename(path);
                let name_hint = if name_hint.is_empty() {
                    "my-repo".to_string()
                } else {
                    name_hint
                };
                self.fields.push(ModalInput::single_line(
                    policy,
                    "~/code/my-repo",
                    path,
                    window,
                    cx,
                ));
                self.fields.push(ModalInput::single_line(
                    policy,
                    &name_hint,
                    name.as_deref().unwrap_or(""),
                    window,
                    cx,
                ));
            }
            Modal::ScriptsEditor(st) => {
                // Name field kept at index 0 for `sync_wizard_buffers`/rename even though display mode doesn't render it.
                self.fields.push(ModalInput::single_line(
                    policy,
                    "project name",
                    &st.name,
                    window,
                    cx,
                ));
                for (placeholder, initial) in [
                    ("npm install", &st.setup),
                    ("npm run dev", &st.run),
                    ("docker compose down", &st.teardown),
                ] {
                    self.fields.push(ModalInput::single_line(
                        policy,
                        placeholder,
                        initial,
                        window,
                        cx,
                    ));
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
        // `InputState` bindings consume backspace/delete before this element's key handler, so the slot must sync on every change event.
        let states: Vec<_> = self.fields.iter().map(|f| f.state().clone()).collect();
        for state in states {
            let sub = cx.subscribe(
                &state,
                |this, _, ev: &gpui_component::input::InputEvent, cx| {
                    if matches!(ev, gpui_component::input::InputEvent::Change) {
                        this.sync_wizard_buffers(cx);
                        this.reset_palette_scroll();
                        this.reset_list_scroll();
                        cx.notify();
                    }
                },
            );
            self.field_subs.push(sub);
        }
        // `ScriptsEditor` field 0 (name) isn't rendered in display mode, so focus field 1 (Setup) instead.
        let default_focus = match modal {
            Modal::ScriptsEditor(_) => self.fields.get(1).or_else(|| self.fields.first()),
            _ => self.fields.first(),
        };
        match default_focus {
            Some(f) => f.focus_at_end(window, cx),
            None => window.focus(&self.focus, cx),
        }
    }

    /// gpui-component owns the text; the slot owns the truth.
    pub(super) fn sync_wizard_buffers(&mut self, cx: &mut Context<Self>) {
        let values: Vec<String> = self.fields.iter().map(|f| f.value(cx)).collect();
        match self.slot.get_mut() {
            // Keeps the slot's own copy of the typed name truthful; the Base row's
            // existing-branch check re-reads the live field on every keystroke.
            Some(Modal::Input { buffer, .. }) => {
                if let Some(v) = values.first() {
                    buffer.clone_from(v);
                }
            }
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
                    st.name.clone_from(v);
                }
                if let Some(v) = values.get(1) {
                    st.setup.clone_from(v);
                }
                if let Some(v) = values.get(2) {
                    st.run.clone_from(v);
                }
                if let Some(v) = values.get(3) {
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
            | ModalKind::AgentPicker => confirm::render(self, &dispatch, window, cx),
            ModalKind::RemoveProject
            | ModalKind::ArchiveProject
            | ModalKind::ArchivedProjects
            | ModalKind::Teardown => project::render(self, &dispatch, cx),
            ModalKind::AddProject | ModalKind::Onboarding => {
                add_project::render(self, &dispatch, window, cx)
            }
            ModalKind::SessionLauncher => launcher::render(self, &dispatch, cx),
            ModalKind::ThemePicker | ModalKind::ThemeManager => {
                theme_picker::render(self, &dispatch, window, cx)
            }
            ModalKind::Settings
            | ModalKind::ShortcutOverlay
            | ModalKind::ScriptsEditor
            | ModalKind::Updating
            | ModalKind::Changelog => settings::render(self, &dispatch, window, cx),
            ModalKind::DiffViewer => diff_viewer::render(self, &dispatch, window, cx),
        };

        let framed = if kind.is_screen_replacement() {
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
                        // Matches iced original's 200ms fade + 8px settle (`onboarding.rs:47-56`).
                        .with_animation(
                            "onboarding-enter",
                            gpui::Animation::new(std::time::Duration::from_millis(200))
                                .with_easing(gpui::ease_out_quint()),
                            |el, delta| el.opacity(delta).pt(rpx(SPACE_LG * (1.0 - delta))),
                        ),
                )
        } else if kind.top_drops() {
            crate::views::components::scrim_top_drop(panel)
        } else {
            crate::views::components::scrim(panel)
        };

        div()
            .id("modal-layer")
            .key_context(kind.key_context())
            .track_focus(&self.focus)
            // Without `occlude`, a press reaches whatever is behind the scrim and `TerminalView::on_mouse_down` steals focus back to the PTY.
            .occlude()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &crate::keymap::ModalUp, window, cx| {
                this.on_modal_key(ModalKey::Up, false, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalDown, window, cx| {
                    this.on_modal_key(ModalKey::Down, false, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalLeft, window, cx| {
                    this.on_modal_key(ModalKey::Left, false, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalRight, window, cx| {
                    this.on_modal_key(ModalKey::Right, false, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalTab, window, cx| {
                    this.on_modal_key(ModalKey::Tab, false, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalShiftTab, window, cx| {
                    this.on_modal_key(ModalKey::Tab, true, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::keymap::ModalEnter, window, cx| {
                    this.on_modal_key(ModalKey::Enter, false, window, cx);
                }),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // Wired at the root, not inside `diff_viewer::render`'s element, which isn't guaranteed to still cover the cursor mid-drag.
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, window, cx| {
                let zoom = cx.global::<crate::zoom::ZoomState>().zoom.max(0.1);
                let x = f32::from(e.position.x) / zoom;
                this.on_diff_divider_mouse_move(x, window, cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _window, cx| {
                    this.on_diff_divider_mouse_up(cx);
                }),
            )
            .child(framed)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use grove_core::storage::Store;

    struct KeyRecorder {
        focus: FocusHandle,
        keys: Rc<RefCell<Vec<String>>>,
    }

    impl Focusable for KeyRecorder {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for KeyRecorder {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("terminal-stand-in")
                .track_focus(&self.focus)
                .key_context("Terminal")
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                    this.keys.borrow_mut().push(ev.keystroke.key.clone());
                }))
                // `TerminalView::on_mouse_down` takes focus on every press.
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        window.focus(&this.focus, cx);
                    }),
                )
                .size_full()
        }
    }

    struct TestRoot {
        focus: FocusHandle,
        terminal: Entity<KeyRecorder>,
        modals: Entity<ModalLayer>,
        focused_once: bool,
        _closed_sub: gpui::Subscription,
    }

    impl Render for TestRoot {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if !self.focused_once {
                self.focused_once = true;
                let handle = self.terminal.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            let modal_open = self.modals.read(cx).is_open();
            let modals = self.modals.clone();
            div()
                .track_focus(&self.focus)
                .when(!modal_open, |d| d.key_context("Workspace"))
                .on_action(cx.listener(|this, _: &crate::keymap::Settings, _, cx| {
                    this.modals
                        .clone()
                        .update(cx, |l, cx| l.open(Modal::Settings, cx));
                }))
                .size_full()
                .child(self.terminal.clone())
                .when(modal_open, |d| d.child(modals))
        }
    }

    fn boot_globals(cx: &mut App) {
        cx.set_global(SettingsState::new(Store::default()));
        cx.set_global(crate::theme::ThemeState::new(
            false,
            crate::theme::DEFAULT_DARK_THEME.to_string(),
            crate::theme::DEFAULT_LIGHT_THEME.to_string(),
        ));
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        gpui_component::init(cx);
        crate::theme::sync_component_theme(cx);
        cx.bind_keys(crate::keymap::bindings());
    }

    fn new_modal_layer(cx: &mut Context<TestRoot>) -> Entity<ModalLayer> {
        let state = cx.new(|_| WorkspaceState::new(&Store::default(), 1280.0));
        let registry = cx.new(|_| SessionRegistry::new());
        let tree = cx.new(|_| ProjectTree::new());
        let toast = cx.new(|_| ToastState::new());
        let activity = cx.new(|_| ActivityStore::new());
        let clock = cx.new(AnimationClock::new);
        let upgrade = cx.new(Upgrade::new);
        cx.new(|cx| ModalLayer::new(state, registry, tree, toast, activity, clock, upgrade, cx))
    }

    fn build_root(cx: &mut Context<TestRoot>, keys: Rc<RefCell<Vec<String>>>) -> TestRoot {
        let modals = new_modal_layer(cx);
        let terminal = cx.new(|cx| KeyRecorder {
            focus: cx.focus_handle(),
            keys,
        });
        // Mirrors `Workspace::on_modal_event`: a closed modal hands the keyboard back to the body.
        let sub = cx.subscribe(&modals, |this: &mut TestRoot, _, ev: &ModalEvent, cx| {
            if matches!(ev, ModalEvent::Closed) {
                this.focused_once = false;
                cx.notify();
            }
        });
        TestRoot {
            focus: cx.focus_handle(),
            terminal,
            modals,
            focused_once: false,
            _closed_sub: sub,
        }
    }

    /// `Input` paints its placeholder in gpui-component's `muted_foreground`, not
    /// Grove's palette, so an unsynced build would show a fixed third-party grey.
    /// This pins the actual colour the placeholder renders in.
    #[gpui::test]
    fn the_placeholder_renders_in_groves_own_muted_tone(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        cx.update(|cx| {
            assert_eq!(
                gpui_component::Theme::global(cx).muted_foreground,
                crate::theme::FG_MUTE(),
                "placeholder tone drifted from Grove's FG_MUTE"
            );
            // And a real value, which inherits `panel_surface`'s text colour, must
            // never be the same tone as a placeholder.
            assert_ne!(crate::theme::FG_MUTE(), crate::theme::FG());
        });
    }

    /// The field the placeholder belongs to still submits nothing when untouched.
    #[gpui::test]
    fn an_untouched_field_holds_no_value_despite_its_placeholder(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let (root, vcx) =
            cx.add_window_view(|_, cx| build_root(cx, Rc::new(RefCell::new(Vec::new()))));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());
        modals.update(vcx, |l, cx| {
            l.open(
                Modal::Input {
                    title: "New worktree".into(),
                    buffer: String::new(),
                    note: None,
                    base: crate::modal::BaseBranchState::default(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        modals.update(vcx, |l, cx| {
            let Some(field) = l.fields.first() else {
                panic!("the name field was never built");
            };
            assert_eq!(
                field.value(cx),
                "",
                "the placeholder leaked into the field's value"
            );
        });
    }

    /// Escape must close the open modal and never reach the terminal behind the scrim.
    #[gpui::test]
    fn escape_closes_the_modal_and_never_reaches_the_terminal(cx: &mut TestAppContext) {
        for modal in [
            Modal::Settings,
            Modal::Message("m".into()),
            Modal::Input {
                title: "t".into(),
                buffer: String::new(),
                note: None,
                base: crate::modal::BaseBranchState::default(),
            },
            Modal::SessionLauncher(Box::default()),
        ] {
            let label = format!("{:?}", modal.kind());
            cx.update(boot_globals);
            let keys = Rc::new(RefCell::new(Vec::new()));
            let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
            vcx.run_until_parked();
            let modals = root.read_with(vcx, |r, _| r.modals.clone());
            keys.borrow_mut().clear();

            modals.update(vcx, |l, cx| l.open(modal, cx));
            vcx.run_until_parked();
            assert!(modals.read_with(vcx, |l, _| l.is_open()), "{label} opened");

            vcx.simulate_keystrokes("escape");
            vcx.run_until_parked();
            assert_eq!(
                keys.borrow().as_slice(),
                &[] as &[String],
                "{label}: the terminal behind the scrim must not see the keystroke"
            );
            assert!(
                !modals.read_with(vcx, |l, _| l.is_open()),
                "{label}: escape must close the modal"
            );
        }
    }

    /// A click through the scrim used to focus the terminal (`TerminalView::on_mouse_down`), stealing keystrokes from the modal.
    #[gpui::test]
    fn a_click_through_the_scrim_cannot_steal_focus_from_the_modal(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.update(vcx, |l, cx| l.open(Modal::Settings, cx));
        vcx.run_until_parked();
        keys.borrow_mut().clear();

        vcx.simulate_click(
            gpui::point(gpui::px(20.0), gpui::px(20.0)),
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert_eq!(
            keys.borrow().as_slice(),
            &[] as &[String],
            "the terminal must not receive the keystroke after a click on the scrim"
        );
        assert!(
            !modals.read_with(vcx, |l, _| l.is_open()),
            "escape must still close the modal after a click on the scrim"
        );
    }

    /// Prevents `SettingsState::flush_now` from writing the developer's real `projects.json`; mirrors `settings.rs`'s helper.
    fn isolate_config_dir() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let dir =
                std::env::temp_dir().join(format!("grove-gpui-modals-{}", std::process::id()));
            let _ = fs_err::create_dir_all(&dir);
            std::env::set_var("GROVE_CONFIG_DIR", &dir);
        });
    }

    /// Both cancel and submit run through [`ModalLayer::cancel`]; only submit consumes `original`, so cancel's restore doesn't clobber a committed pick.
    #[gpui::test]
    fn cancelling_the_theme_picker_restores_the_theme_and_submitting_does_not(
        cx: &mut TestAppContext,
    ) {
        use crate::modal::{ThemePickerReturn, ThemePickerScope};
        use theme_picker::ThemePreview;

        isolate_config_dir();
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        let original = crate::theme::DEFAULT_DARK_THEME.to_string();
        vcx.update(|_, cx| crate::theme::ThemeState::set_by_name(cx, &original));

        modals.update(vcx, |l, cx| {
            l.open_theme_picker(ThemePickerScope::App, ThemePickerReturn::Close, cx);
            l.theme_picker_move(1, cx);
        });
        vcx.run_until_parked();
        let previewed = grove_core::theme::current().name.to_string();
        assert_ne!(
            previewed, original,
            "the picker must be previewing something other than the original"
        );

        modals.update(vcx, |l, cx| {
            l.cancel(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            grove_core::theme::current().name.as_ref(),
            original,
            "cancel must put the original theme back"
        );
        assert!(
            modals.read_with(vcx, |_, cx| {
                cx.try_global::<ThemePreview>()
                    .is_none_or(|p| p.project.is_none() && p.app.is_none())
            }),
            "cancel must drop the live preview global"
        );

        modals.update(vcx, |l, cx| {
            l.open_theme_picker(ThemePickerScope::App, ThemePickerReturn::Close, cx);
            l.theme_picker_move(1, cx);
        });
        vcx.run_until_parked();
        let picked = grove_core::theme::current().name.to_string();
        assert_ne!(picked, original);

        modals.update(vcx, super::ModalLayer::theme_picker_submit);
        vcx.run_until_parked();
        assert_eq!(
            grove_core::theme::current().name.as_ref(),
            picked,
            "submit must not be undone by cancel's restore"
        );
        assert!(
            !modals.read_with(vcx, |l, _| l.is_open()),
            "submit closes the picker"
        );

        // The active theme is process-global; restore it.
        vcx.update(|_, cx| crate::theme::ThemeState::set_by_name(cx, &original));
    }

    #[gpui::test]
    fn escape_closes_a_modal_opened_by_its_keybinding(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());
        keys.borrow_mut().clear();

        // Cycles 2 and 3 cover the reopen path, after field/focus teardown.
        for cycle in 0..3 {
            vcx.simulate_keystrokes(&format!("{},", crate::keymap::platform_mod_prefix()));
            vcx.run_until_parked();
            assert!(
                modals.read_with(vcx, |l, _| l.is_open()),
                "cycle {cycle}: the Settings keybinding opened the modal"
            );
            keys.borrow_mut().clear();

            vcx.simulate_keystrokes("escape");
            vcx.run_until_parked();
            assert_eq!(
                keys.borrow().as_slice(),
                &[] as &[String],
                "cycle {cycle}: the terminal behind the scrim must not see the keystroke"
            );
            assert!(
                !modals.read_with(vcx, |l, _| l.is_open()),
                "cycle {cycle}: escape must close the modal"
            );
        }
    }

    // No `debug_selector` on these buttons, so this checks the id/ModalClick source-text pairing plus driving the click through `on_click` directly, rather than a coordinate-based simulate_click.
    #[gpui::test]
    fn relocated_left_slot_actions_still_dispatch_their_click(cx: &mut TestAppContext) {
        isolate_config_dir();
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        let confirm_src = include_str!("confirm.rs");
        let theme_picker_src = include_str!("theme_picker.rs");
        let settings_src = include_str!("settings.rs");
        assert!(
            confirm_src.contains("\"ap-default\"")
                && confirm_src.contains("ModalClick::ToggleDefaultAgent"),
            "confirm.rs must still wire ap-default to ToggleDefaultAgent"
        );
        assert!(
            theme_picker_src.contains("\"tm-new\"")
                && theme_picker_src.contains("ModalClick::ThemeNew"),
            "theme_picker.rs must still wire tm-new to ThemeNew"
        );
        assert!(
            settings_src.contains("\"se-archive\"")
                && settings_src.contains("ModalClick::OpenArchiveGate"),
            "settings.rs must still wire se-archive to OpenArchiveGate"
        );
        assert!(
            settings_src.contains("\"set-updates-refresh\"")
                && settings_src.contains("ModalClick::CheckUpdates"),
            "settings.rs must still wire set-updates-refresh to CheckUpdates"
        );

        modals.update(vcx, |l, cx| {
            l.open(
                Modal::AgentPicker {
                    project: "p".into(),
                    wt_path: "/w".into(),
                    sel: 0,
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let before = vcx.update(|_, cx| cx.global::<SettingsState>().store.default_agent);
        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_click(ModalClick::ToggleDefaultAgent, window, cx);
            });
        });
        vcx.run_until_parked();
        let after = vcx.update(|_, cx| cx.global::<SettingsState>().store.default_agent);
        assert_ne!(
            before, after,
            "ap-default's ModalClick must still toggle the default agent"
        );
        modals.update(vcx, |l, cx| {
            l.cancel(cx);
        });
        vcx.run_until_parked();

        modals.update(vcx, |l, cx| {
            l.open(
                Modal::ThemeManager {
                    selected: 0,
                    rename: None,
                    rename_error: None,
                    pending_delete: None,
                    editor: None,
                },
                cx,
            );
        });
        vcx.run_until_parked();
        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_click(ModalClick::ThemeNew, window, cx);
            });
        });
        vcx.run_until_parked();
        assert!(
            matches!(
                modals.read_with(vcx, |l, _| l.slot().get().cloned()),
                Some(Modal::ThemeManager {
                    editor: Some(_),
                    ..
                })
            ),
            "tm-new's ModalClick must still open the theme editor"
        );
        modals.update(vcx, |l, cx| {
            l.cancel(cx);
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            SettingsState::update(cx, |store| {
                store.projects.push(grove_core::storage::Project {
                    name: "p".into(),
                    path: "/p".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    theme: None,
                    archived: false,
                    worktree_dir: None,
                });
            });
        });
        modals.update(vcx, |l, cx| {
            l.open(
                Modal::ScriptsEditor(Box::new(crate::modal::ScriptsEditorState {
                    project_path: "/p".into(),
                    name: "p".into(),
                    setup: String::new(),
                    run: String::new(),
                    teardown: String::new(),
                    renaming: false,
                })),
                cx,
            );
        });
        vcx.run_until_parked();
        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_click(ModalClick::OpenArchiveGate, window, cx);
            });
        });
        vcx.run_until_parked();
        assert!(
            matches!(
                modals.read_with(vcx, |l, _| l.slot().kind()),
                Some(ModalKind::ArchiveProject)
            ),
            "se-archive's ModalClick must still open the archive gate"
        );
        modals.update(vcx, |l, cx| {
            l.cancel(cx);
        });
        vcx.run_until_parked();

        modals.update(vcx, |l, cx| {
            l.open(Modal::Settings, cx);
        });
        vcx.run_until_parked();
        // Asserted before `run_until_parked` drains the background fetch, which may race past `Checking`.
        let checking = root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_click(ModalClick::CheckUpdates, window, cx);
                matches!(
                    l.upgrade.read(cx).state(),
                    crate::entities::upgrade_state::UpgradeState::Checking
                )
            })
        });
        assert!(
            checking,
            "set-updates-refresh's ModalClick must still dispatch CheckUpdates"
        );
        vcx.run_until_parked();
    }

    /// Escape must be refused mid-operation (plan.md §9.1.1); `Updating` isn't covered here since it needs a real network-fetched release.
    #[gpui::test]
    fn blocking_progress_states_refuse_escape(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.update(vcx, |l, cx| {
            l.open(
                Modal::RemoveProject {
                    idx: 0,
                    name: "p".into(),
                    project_path: "/p".into(),
                    worktrees: vec![],
                    also_remove_worktrees: false,
                    in_progress: true,
                    done: 0,
                    current: String::new(),
                    errors: vec![],
                },
                cx,
            );
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(
            modals.read_with(vcx, |l, _| l.is_open()),
            "escape must be refused while RemoveProject is in progress"
        );

        // Cancel is refused by `ModalSlot::cancel`'s `Removing` arm.
        modals.update(vcx, |l, cx| {
            l.open(
                Modal::Teardown {
                    wt_path: "/w".into(),
                    project_path: "/p".into(),
                    stage: crate::modal::TeardownStage::Removing,
                    message: "Removing worktree…".into(),
                    removal_started: true,
                },
                cx,
            );
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(
            modals.read_with(vcx, |l, _| l.is_open()),
            "escape must be refused while Teardown is removing"
        );
    }

    // G6 (Updating(Updated) → Restart) skipped: reaching `UpgradeState::Updated` needs a live network check, no test hook exists.

    /// A sub-epsilon move (hand jitter) must leave `file_list_w_override` unset; only past `DRAG_EPSILON` does it get written.
    #[gpui::test]
    fn a_sub_epsilon_move_leaves_the_override_unset_and_a_real_drag_sets_it(
        cx: &mut TestAppContext,
    ) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.update(vcx, |l, cx| {
            l.open(
                Modal::DiffViewer {
                    wt_path: "/nonexistent-wt".into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();

        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_diff_divider_press(window, cx);
            });
        });
        assert!(
            modals.read_with(vcx, |l, _| l.diff_divider_drag.is_some()),
            "press must start a drag"
        );
        let start_width = modals.read_with(vcx, |l, _| {
            let Some(drag) = l.diff_divider_drag else {
                panic!("press must have started a drag");
            };
            drag.start_width
        });

        // `grab_offset` is captured on this move (offset 0), so the clamped width equals `start_width`.
        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_diff_divider_mouse_move(start_width, window, cx);
            });
        });
        assert_eq!(
            modals.read_with(vcx, |l, _| l.file_list_w_override),
            None,
            "a sub-epsilon move must leave the override unset"
        );

        let jump = start_width + crate::views::components::DRAG_EPSILON + 5.0;
        let win_w = root.update_in(vcx, |_, window, cx| {
            ModalLayer::logical_window_width(window, cx)
        });
        root.update_in(vcx, |_, window, cx| {
            modals.update(cx, |l, cx| {
                l.on_diff_divider_mouse_move(jump, window, cx);
            });
        });
        assert_eq!(
            modals.read_with(vcx, |l, _| l.file_list_w_override),
            Some(diff_viewer::clamp_file_list_w(jump, win_w)),
            "a move past the epsilon must set a clamped override"
        );
    }
}
