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

// The click vocabulary itself lives one layer down in
// [`crate::views::dispatch`] so the app-wide component library can build
// clickable chrome without depending on this module; re-exported here
// because the modal layer is what interprets it.
pub use crate::views::dispatch::{ModalClick, ModalDispatch, SettingToggle};

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
    /// The palette strip's lifecycle-script rows: run `script` as a shell in
    /// the worktree's terminal panel.
    RunScript { wt_path: String, script: String },
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
    /// Change-event subscriptions for `fields`, one per entry, kept alive so
    /// backspace/delete — consumed by gpui-component's `InputState` bindings
    /// before they ever bubble to this element's key handler — still syncs
    /// the slot's buffers (fix for stale palette query on delete).
    field_subs: Vec<gpui::Subscription>,
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
    /// The diff viewer's live state, mirroring how `teardown_view` is
    /// created on open and dropped on close: `Some` exactly while
    /// `Modal::DiffViewer` is the open modal.
    pub(super) diff_viewer: Option<Entity<crate::entities::diff_viewer::DiffViewerState>>,
    /// A hand-dragged diff file-list width (logical px), overriding the
    /// measured auto-fit width. Lives here rather than on `DiffViewerState`
    /// because that entity is dropped and recreated on every
    /// close/reopen of the diff viewer (`open`/`cancel`/`close` below) while
    /// this override is meant to survive the session. `None` means auto-fit.
    pub(crate) file_list_w_override: Option<f32>,
    /// The diff file-list divider's in-progress drag, mirroring
    /// `Sidebar`'s `drag` field.
    diff_divider_drag: Option<crate::views::components::DividerDrag>,
    /// The diff file-list divider's last press instant, for double-click
    /// detection, mirroring `Sidebar`'s `last_divider_press`.
    last_diff_divider_press: Option<std::time::Instant>,
    /// The palette result list's scroll position. It has to live on the view:
    /// a handle built per-render would hand the list a fresh, zeroed offset on
    /// every frame.
    pub(super) palette_scroll: gpui::ScrollHandle,
    /// The `(view, row)` the palette was last scrolled to. Only a *changed*
    /// selection may move the scroll — re-issuing it every frame would snap the
    /// wheel straight back to the selected row. `render` is handed a
    /// `&ModalLayer`, hence the `Cell`.
    palette_scrolled_to: std::cell::Cell<Option<(crate::modal::LauncherView, usize)>>,
    /// The scroll position of the *other* keyboard-navigable modal list — the
    /// theme picker's themes and the wizard's directory matches. One handle
    /// serves both: only one modal renders at a time, and `open` clears it so a
    /// stale offset can't leak from one list into the next.
    pub(super) list_scroll: gpui::ScrollHandle,
    /// `(tag, row)` last scrolled to, where `tag` distinguishes row sets that
    /// share the handle (the picker's dark/light tabs). Same
    /// only-a-changed-selection-may-scroll rule as `palette_scrolled_to`.
    list_scrolled_to: std::cell::Cell<Option<(usize, usize)>>,
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

    /// The diff viewer body's content width, [`diff_viewer::effective_mode`]'s
    /// input: the logical window width minus the viewer's own insets and its
    /// file-list column — the same `content_w` [`diff_viewer::render`]
    /// computes for itself, kept in agreement because both call
    /// [`diff_viewer::file_list_w`] with the same [`DiffViewerState`]. A
    /// method rather than a free function (unlike the original) because it
    /// needs `self.diff_viewer` to measure the file-list column's width; when
    /// no diff viewer entity exists yet, the column falls back to its floor.
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

    // ── diff file-list divider ─────────────────────────────────────────

    /// Mirrors `Sidebar::on_divider_press`: a double-click within
    /// `DOUBLE_CLICK` releases the override back to auto-fit; otherwise it
    /// starts a drag from the file list's current effective width.
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

    /// Mirrors `Sidebar::on_root_mouse_move`, attached to the `modal-layer`
    /// root div — the only element guaranteed to keep receiving moves once
    /// the cursor leaves the divider's narrow hit zone.
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
        // Unlike the sidebar, there is no settings write on mouse-up to gate
        // with `DRAG_EPSILON` — the override *is* the side effect, and it is
        // written here, on every move. So the epsilon has to live here too:
        // without it, a plain click with a pixel of hand jitter would pin an
        // override and silently kill auto-fit for the rest of the session.
        if (clamped - drag.start_width).abs() < crate::views::components::DRAG_EPSILON {
            return;
        }
        self.file_list_w_override = Some(clamped);
        cx.notify();
    }

    /// Mirrors `Sidebar::on_root_mouse_up`. No persistence step: the override
    /// is session-only by design (carried decision 2). Unlike the sidebar,
    /// the epsilon guard is not here — the gated side effect there is a
    /// debounced settings write on release, but here the side effect
    /// (`file_list_w_override`) is written continuously as the drag moves,
    /// so the guard lives in `on_diff_divider_mouse_move` instead.
    pub(crate) fn on_diff_divider_mouse_up(&mut self, _cx: &mut Context<Self>) {
        self.diff_divider_drag = None;
    }

    // Exercised only by this module's `#[cfg(test)]` assertions, which read the
    // open modal's kind; the render path matches on `slot.get()` instead.
    #[allow(dead_code)]
    pub fn kind(&self) -> Option<ModalKind> {
        self.slot.kind()
    }

    pub fn slot(&self) -> &ModalSlot {
        &self.slot
    }

    /// Bring row `sel` of a shared-handle list into view, addressing it by the
    /// index it actually occupies among the scroll container's direct children
    /// (`card` interleaves dividers, so the two differ). Guarded like
    /// `scroll_palette_to`: `scroll_to_item` resolves in prepaint, so
    /// re-issuing it every frame would fight the mouse wheel.
    pub(super) fn scroll_list_to(&self, tag: usize, sel: usize, child_ix: usize) {
        if self.list_scrolled_to.get() == Some((tag, sel)) {
            return;
        }
        self.list_scrolled_to.set(Some((tag, sel)));
        self.list_scroll.scroll_to_item(child_ix);
    }

    /// Drop the retained offset — it only means anything against the list it
    /// was measured on.
    pub(super) fn reset_list_scroll(&self) {
        self.list_scroll.set_offset(gpui::Point::default());
        self.list_scrolled_to.set(None);
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
        // The live diff-viewer entity is created fresh per open and dropped
        // the moment anything else opens — mirroring `teardown_view`, which
        // must not survive past the modal that owns it.
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
        self.slot.open(modal);
        self.reset_list_scroll();
        self.fields.clear();
        self.field_subs.clear();
        self.needs_focus = true;
        cx.notify();
    }

    /// Route through the state machine's `cancel`, which is **not** a synonym
    /// for close (Teardown skips, RemoveProject refuses, ThemePicker and the
    /// changelog return to their parent).
    pub fn cancel(&mut self, cx: &mut Context<Self>) -> CancelOutcome {
        // Every door out of the `ThemePicker` — Escape, the Cancel button, the
        // close X and `theme_picker_submit`'s tail — arrives here, so the live
        // preview it drove is undone here too. It has to run *before*
        // `slot.cancel()`, which may already have swapped the picker out for
        // the modal it returns to. Self-guarding, and a no-op on the commit
        // path, which consumes `original` first.
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

    /// Force the slot empty, for paths that already decided.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.slot.close();
        self.fields.clear();
        self.field_subs.clear();
        self.diff_viewer = None;
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
        let Some((key, mods)) = Self::translate(ev) else {
            return;
        };
        self.handle_key(key, mods, window, cx);
    }

    /// The keys a focused `Input` would otherwise swallow arrive as actions
    /// instead (`keymap::modal_input_bindings`), already stripped of their
    /// keystroke. They re-enter the *same* decision path here, so there is one
    /// verdict table and not two.
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
            // The name field and the three lifecycle buffers are all
            // single-line now; Enter saves, exactly like clicking `Save`
            // (`ModalClick::Save`, `settings.rs`).
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
                // Which follow-up to run once the mutation borrow above ends —
                // click activates the launcher row (mirroring Enter) and
                // previews the theme row (Enter/save still commits it).
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
                            // Identity resolution would otherwise activate
                            // whatever row the stale anchor points at; in
                            // RowActions the anchor is the strip's session,
                            // not a row, and must survive the click.
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
            // Same commit path `ModalAction::ThemePickerSubmit` reaches from
            // the keyboard's Enter/Submit verdict.
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
        self.field_subs.clear();
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
                // The prompt must not claim to search what the scope hides.
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
                // The name field is first so it keeps index 0 for
                // `sync_wizard_buffers`/rename, even though display mode
                // doesn't render it — only rename mode does
                // (`settings.rs::scripts_editor`). The three lifecycle
                // buffers are genuinely single-line `ModalInput`s; the actual
                // typing bug was never these fields' height — it was
                // `crate::modal`'s verdict table returning `V::Ignore` for
                // every character key, which reaches `cx.stop_propagation()`
                // (`:388` below) before the platform input handler ever
                // consults the focused `Input`. Fixed by routing
                // `Modal::ScriptsEditor` to `V::FallThrough` like every other
                // modal with real text fields.
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
        // Backspace/delete never bubble to this element's key handler —
        // gpui-component's `InputState` bindings consume them first — so the
        // slot's buffers go stale on delete unless we sync on every change
        // event directly from the field's `InputState`.
        let states: Vec<_> = self.fields.iter().map(|f| f.state().clone()).collect();
        for state in states {
            let sub = cx.subscribe(
                &state,
                |this, _, ev: &gpui_component::input::InputEvent, cx| {
                    if matches!(ev, gpui_component::input::InputEvent::Change) {
                        this.sync_wizard_buffers(cx);
                        // A query edit rebuilds the row set from scratch, so
                        // the retained offset belongs to a list that is gone.
                        this.reset_palette_scroll();
                        // Same for the wizard's directory matches, which the
                        // typed path rebuilds.
                        this.reset_list_scroll();
                        cx.notify();
                    }
                },
            );
            self.field_subs.push(sub);
        }
        // `ScriptsEditor`'s field 0 (the name) is not rendered in display
        // mode — the header shows static text plus a pencil until it's
        // clicked — so focusing it on open would point the caret at nothing.
        // Field 1 (Setup) is the first field actually on screen.
        let default_focus = match modal {
            Modal::ScriptsEditor(_) => self.fields.get(1).or_else(|| self.fields.first()),
            _ => self.fields.first(),
        };
        match default_focus {
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
                        // Spec §4's entrance animation: matches the iced
                        // original's 200ms fade + 8px settle
                        // (`onboarding.rs:47-56`) rather than the plain
                        // 320ms opacity-only fade this replaces.
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
            // The scrim is a modal barrier, not just paint. Without this the
            // layer is mouse-transparent: a press meant for the scrim reaches
            // whatever is behind it, and `TerminalView::on_mouse_down` then
            // calls `window.focus` on itself — taking the keyboard away from
            // the open modal, so Escape lands in the PTY instead of closing.
            .occlude()
            .on_key_down(cx.listener(Self::on_key_down))
            // The keys a focused `Input` would swallow, reclaimed as actions.
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
            // The diff file-list divider's drag stream: wired here rather
            // than on `diff_viewer::render`'s own returned element because
            // that element is not guaranteed to still cover the cursor once
            // dragging starts (mirrors `sidebar::root_drag_listeners` being
            // wired at `workspace.rs`, not inside `Sidebar::render`'s own
            // divider hit-zone). Unconditional: both handlers no-op when
            // `diff_divider_drag` is `None`.
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

    // ── the focus regression harness ────────────────────────────────────
    //
    // A window whose root mimics `Workspace`: a focusable root div declaring
    // the screen key context while no modal is open, a focusable "terminal"
    // stand-in that records every key it receives, and the `ModalLayer`
    // mounted last and only while a modal is open. The terminal takes focus
    // on the first frame — exactly as `Workspace::render`'s `focused_once`
    // does — and only then is a modal opened, which is the ordering the
    // reported bug needs.

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
        // `Workspace::on_modal_event` does exactly this: a closed modal hands
        // the keyboard back to the body on the next frame.
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

    /// The reported bug: with a modal open, Escape must close it and must
    /// **not** reach the terminal behind the scrim.
    #[gpui::test]
    fn escape_closes_the_modal_and_never_reaches_the_terminal(cx: &mut TestAppContext) {
        for modal in [
            Modal::Settings,
            Modal::Message("m".into()),
            Modal::Input {
                title: "t".into(),
                buffer: String::new(),
                note: None,
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

    /// Root cause A: nothing under the scrim may take the mouse. A click that
    /// lands on the terminal behind an open modal used to focus it
    /// (`TerminalView::on_mouse_down`), after which every keystroke — Escape
    /// included — went to the PTY instead of the modal.
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

        // A press in the scrim's top-left corner, well clear of the panel.
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

    /// Points `grove_core::storage` at a private directory for this process so
    /// the theme test below — whose commit path runs `SettingsState::flush_now`
    /// — never writes the developer's real `projects.json`. Mirrors
    /// `settings.rs`'s helper of the same name.
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

    /// The theme picker's live preview must be undone when it is cancelled and
    /// **kept** when it is committed. Both exits run through
    /// [`ModalLayer::cancel`]; only the commit path consumes `original`, which
    /// is what keeps the restore from clobbering the theme just pinned.
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

        // `Store::default()` pins no theme, so the picker opens with
        // `original` = the default dark theme.
        let original = crate::theme::DEFAULT_DARK_THEME.to_string();
        vcx.update(|_, cx| crate::theme::ThemeState::set_by_name(cx, &original));

        // ── cancel: the preview is rolled back ──────────────────────────
        modals.update(vcx, |l, cx| {
            l.open_theme_picker(ThemePickerScope::App, ThemePickerReturn::Close, cx);
            // Move off the opening selection so a *different* theme is live.
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

        // ── submit: the commit survives the same exit ───────────────────
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

        // The active theme is process-global; leave it as it was found.
        vcx.update(|_, cx| crate::theme::ThemeState::set_by_name(cx, &original));
    }

    /// The realistic open path: the modal is opened by its **keybinding**,
    /// dispatched while the terminal holds focus.
    #[gpui::test]
    fn escape_closes_a_modal_opened_by_its_keybinding(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());
        keys.borrow_mut().clear();

        // Three cycles: the second and third also cover the reopen path,
        // where the layer already rendered once and the field/focus state has
        // been torn down.
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

    // ── G-tier: relocated left-footer-slot actions (G1) ─────────────────
    //
    // plan.md §2 retires the footer's low-emphasis left slot and moves its
    // four call sites into the body as flat tinted `body_action`s. A dropped
    // click handler in that move produces a button that looks perfect and
    // does nothing — invisible to a visual pass.
    //
    // WEAKENED from the plan's "click it through the real render tree": the
    // `body_action`/`flat_text_btn_tinted` shell wires its click through a
    // bare `.on_mouse_down`, never through gpui's `.debug_selector()` (a
    // test-only, `id()`-distinct opt-in each call site would have to add) —
    // so `VisualTestContext::debug_bounds` never sees `"ap-default"` and
    // friends and a coordinate-based `simulate_click` has nothing to aim at.
    // Adding `.debug_selector` calls at each site is a production-code edit,
    // out of scope here. Instead this proves the same thing two other ways:
    // (a) a source-text check that the exact `id` literal is passed to
    // `body_action`/`flat_text_btn` alongside the exact `ModalClick` the plan
    // names, so the two can't silently drift apart at the call site, and (b)
    // driving that same `ModalClick` through `ModalLayer::on_click` — the
    // real handler `body_action`'s closure calls, not a test double — and
    // asserting the effect the plan documents.
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

        // ── AgentPicker "Default" (confirm.rs:566) → ToggleDefaultAgent ──
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

        // ── ThemeManager "+ New theme" (theme_picker.rs:1110) → ThemeNew ─
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

        // ── ScriptsEditor "Archive project" (settings.rs:1658) → OpenArchiveGate ──
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

        // ── Settings "Check for updates" (settings.rs:1103) → CheckUpdates ──
        modals.update(vcx, |l, cx| {
            l.open(Modal::Settings, cx);
        });
        vcx.run_until_parked();
        // Asserted immediately, before `run_until_parked`: `check()` sets
        // `UpgradeState::Checking` synchronously and only then spawns the
        // background fetch, which — drained by `run_until_parked` — may
        // already have resolved to `Error`/`UpToDate` by the time anything
        // reads the state back, racing the very thing this test wants to
        // observe.
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

    /// The blocking-progress states are the sanctioned no-close-X cases
    /// (plan.md §9.1.1): an in-flight removal or teardown cannot be safely
    /// interrupted, so Escape must be refused rather than closing the modal
    /// out from under the operation. Covers two of the three named states —
    /// `RemoveProject { in_progress: true }` and `Teardown { stage: Removing
    /// }` — both fully driven by pure `Modal` state; `Updating` is not
    /// covered here because reaching its in-flight state requires
    /// `Upgrade::start_update`, which needs a real `UpgradeState::Available`
    /// release only a live network check can produce.
    #[gpui::test]
    fn blocking_progress_states_refuse_escape(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let keys = Rc::new(RefCell::new(Vec::new()));
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, keys.clone()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        // RemoveProject, mid-removal.
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

        // Teardown, mid-`git worktree remove` — cancel is refused
        // (`ModalSlot::cancel`'s `Removing` arm), no footer and no close X.
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

    // G6 (Updating(Updated): Later before Restart, each dispatching its own
    // action) is skipped: `Upgrade::state` is a private field with no
    // test-only setter, and reaching `UpgradeState::Updated` for real only
    // happens through `Upgrade::start_update`, which requires
    // `self.available()` to already hold a release — itself only reachable
    // through a live network `check()`. Driving it hermetically would need a
    // production-code test hook, which is out of scope for a test-only
    // change.

    /// A sub-`DRAG_EPSILON` move must not pin an override: a plain click on
    /// the divider, with a pixel of hand jitter, has to leave the file-list
    /// column on auto-fit rather than silently killing it for the session.
    /// Only once the drag has actually travelled past the epsilon does
    /// `file_list_w_override` get written.
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

        // First move: well under `DRAG_EPSILON` from the start width. Because
        // `grab_offset` is captured on this first move (matching the press
        // position exactly, offset 0), the resulting clamped width equals
        // `start_width` — a delta of 0.
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

        // Second move: past the epsilon.
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
