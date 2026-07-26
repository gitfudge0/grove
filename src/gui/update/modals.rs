use super::shortcuts::{match_global_shortcut, GlobalShortcut};
use super::{focus, move_cursor_to_end, remove_worktree_task};
use crate::app::{ConfirmKind, Modal};
use crate::gui::add_project;
use crate::gui::add_project::AddProjectState;
use crate::gui::session_launcher::LauncherState;
use crate::gui::state::{Grove, Msg, ThemeManagerMsg, UpgradeState};
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::Task;
use std::sync::Arc;

impl Grove {
    /// The add-project wizard's live state — but only while
    /// `Modal::AddProject` is the modal actually showing.
    ///
    /// `Modal::AddProject` is a marker variant and `Grove::add_project` is a
    /// plain `Option`; the "`Some` exactly when active" invariant between them
    /// is not expressible in either type alone (the state can't move into the
    /// variant's payload: `Modal` lives in the `app` layer, below `gui`, and
    /// folding it in would alias `self.app` with the borrows that today read
    /// `self.add_project` and `self.app` disjointly). Checking both halves
    /// here, in one place, is what enforces it — every re-check that pairs a
    /// modal test with a field access goes through this instead of testing the
    /// two independently and hoping they agree.
    pub(in crate::gui) fn add_project_modal(&self) -> Option<&AddProjectState> {
        match self.app.modal {
            Modal::AddProject => self.add_project.as_ref(),
            _ => None,
        }
    }

    /// The command palette's live state — but only while
    /// `Modal::SessionLauncher` is the modal actually showing. Same
    /// marker-variant invariant as [`Grove::add_project_modal`]; see there.
    pub(in crate::gui) fn launcher_modal(&self) -> Option<&LauncherState> {
        match self.app.modal {
            Modal::SessionLauncher => self.launcher.as_ref(),
            _ => None,
        }
    }

    /// Mutable counterpart to [`Grove::launcher_modal`]; same invariant.
    pub(in crate::gui) fn launcher_modal_mut(&mut self) -> Option<&mut LauncherState> {
        match self.app.modal {
            Modal::SessionLauncher => self.launcher.as_mut(),
            _ => None,
        }
    }

    /// Route a chosen folder (native picker / file drop / typed path) into the
    /// add-project wizard, gated on the same invariant the accessors above
    /// enforce. `add_project::choose` needs the whole `&mut Option<..>` slot
    /// (it clears and repoints it), so it can't take an `&mut` from
    /// `add_project_modal_mut` — the guard is spelled out here instead.
    pub(in crate::gui) fn choose_add_project_folder(
        &mut self,
        path: std::path::PathBuf,
    ) -> Task<Msg> {
        if self.add_project_modal().is_none() {
            return Task::none();
        }
        add_project::choose(&mut self.add_project, path);
        self.focus_add_project_field()
    }

    /// Keyboard handling for the remove-project modal: Esc/n cancel, y
    /// confirms (Enter deliberately does not), Space toggles the
    /// delete-worktrees checkbox. Ignored while removal is in flight.
    pub(super) fn handle_remove_project_key(&mut self, key: Key, busy: bool) -> Task<Msg> {
        if busy {
            return Task::none();
        }
        match key {
            Key::Named(Named::Escape) => self.cancel_modal(),
            Key::Named(Named::Space) => {
                if let Modal::RemoveProject {
                    also_remove_worktrees,
                    ..
                } = &mut self.app.modal
                {
                    *also_remove_worktrees = !*also_remove_worktrees;
                }
            }
            Key::Character(s) => match s.as_str() {
                "y" | "Y" => return self.kick_off_remove_project(),
                "n" | "N" => self.cancel_modal(),
                _ => {}
            },
            _ => {}
        }
        Task::none()
    }

    pub(super) fn handle_modal_key(&mut self, key: Key, mods: Modifiers) -> Task<Msg> {
        match &self.app.modal {
            // Text entry, caret movement, selection, and paste are owned by the
            // `text_input` widgets. The subscription only drives the directory
            // match list and modal lifecycle.
            Modal::Input { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_modal_input(),
                Key::Character(s) if mods.control() && matches!(s.as_str(), "c" | "C") => {
                    self.cancel_modal();
                }
                _ => {}
            },
            // Esc from the pick-source step and Ctrl+C from either step both
            // cancel the whole modal via the shared `cancel_modal` (which
            // also clears `self.add_project`) — checked here, ahead of the
            // delegate, since `add_project::handle_key` only owns `&mut App`
            // and can't reach a `Grove`-only method. Everything else
            // (arrows/Tab/Enter on pick-source; Esc/Enter on details) is
            // `add_project::handle_key`, extracted verbatim.
            Modal::AddProject => {
                let is_pick_source = matches!(
                    self.add_project_modal().map(|st| st.step),
                    Some(add_project::AddProjectStep::PickSource)
                );
                match key {
                    Key::Named(Named::Escape) if is_pick_source => {
                        self.cancel_modal();
                        return Task::none();
                    }
                    Key::Character(ref s) if mods.control() && matches!(s.as_str(), "c" | "C") => {
                        self.cancel_modal();
                        return Task::none();
                    }
                    _ => {}
                }
                let (task, rebuild) =
                    add_project::handle_key(&mut self.app, &mut self.add_project, key);
                if rebuild == add_project::WtCacheRebuild::Rebuild {
                    self.rebuild_wt_cache();
                }
                return task.map(Msg::AddProject);
            }
            Modal::Confirm { .. } => match key {
                Key::Named(Named::Escape) => return self.confirm_modal_response(false),
                Key::Named(Named::Enter) => return self.confirm_modal_response(true),
                Key::Character(s) => match s.as_str() {
                    "y" | "Y" => return self.confirm_modal_response(true),
                    "n" | "N" => return self.confirm_modal_response(false),
                    _ => {}
                },
                _ => {}
            },
            Modal::Message(_) => match key {
                Key::Named(Named::Escape | Named::Enter) => self.cancel_modal(),
                Key::Character(s) if matches!(s.as_str(), "q" | "Q") => self.cancel_modal(),
                _ => {}
            },
            Modal::ThemePicker { .. } => match key {
                Key::Named(Named::Escape) => self.theme_picker_cancel(),
                Key::Named(Named::Enter) => self.theme_picker_submit(),
                Key::Named(Named::ArrowDown) => self.theme_picker_move(1),
                Key::Named(Named::ArrowUp) => self.theme_picker_move(-1),
                Key::Named(Named::Tab) => self.theme_picker_switch_tab(),
                Key::Character(s) => match s.as_str() {
                    "j" | "J" => self.theme_picker_move(1),
                    "k" | "K" => self.theme_picker_move(-1),
                    "h" | "H" | "l" | "L" => self.theme_picker_switch_tab(),
                    _ => {}
                },
                _ => {}
            },
            Modal::ThemeManager {
                rename,
                pending_delete,
                ..
            } => {
                if self.theme_manager_editor.is_some() {
                    let (task, invalidate) = crate::gui::theme_manager_editor::handle_key(
                        &mut self.theme_manager_editor,
                        &mut self.app,
                        key,
                        mods,
                    );
                    if invalidate
                        == crate::gui::theme_manager_editor::PtyCacheInvalidate::Invalidate
                    {
                        self.invalidate_pty_render_cache();
                    }
                    return task.map(|v| Msg::ThemeManager(ThemeManagerMsg::Editor(v)));
                } else if pending_delete.is_some() {
                    // y/n mirrors `confirm_modal`'s destructive-confirm
                    // convention (Enter deliberately does not there); Enter
                    // still works here too since this dialog has no other
                    // use for it.
                    match key {
                        Key::Named(Named::Enter) => self.theme_manager_delete_confirm(),
                        Key::Named(Named::Escape) => self.theme_manager_delete_cancel(),
                        Key::Character(s) => match s.as_str() {
                            "y" | "Y" => self.theme_manager_delete_confirm(),
                            "n" | "N" => self.theme_manager_delete_cancel(),
                            _ => {}
                        },
                        _ => {}
                    }
                } else if rename.is_some() {
                    match key {
                        Key::Named(Named::Enter) => self.theme_manager_rename_submit(),
                        Key::Named(Named::Escape) => self.theme_manager_rename_cancel(),
                        _ => {}
                    }
                } else {
                    match key {
                        Key::Named(Named::Escape) => self.theme_manager_close(),
                        Key::Named(Named::ArrowDown) => self.theme_manager_move(1),
                        Key::Named(Named::ArrowUp) => self.theme_manager_move(-1),
                        _ => {}
                    }
                }
            }
            Modal::AgentPicker { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_agent_picker(),
                Key::Named(Named::ArrowDown) => self.app.picker_move(1),
                Key::Named(Named::ArrowUp) => self.app.picker_move(-1),
                Key::Named(Named::Space) => self.agent_picker_toggle_default(),
                Key::Character(s) => match s.as_str() {
                    "j" | "J" => self.app.picker_move(1),
                    "k" | "K" => self.app.picker_move(-1),
                    _ => {}
                },
                _ => {}
            },
            Modal::SessionLauncher => return self.handle_session_launcher_key(key, mods),
            Modal::Settings => {
                if matches!(key, Key::Named(Named::Escape)) {
                    self.set_modal(Modal::None);
                }
            }
            Modal::Updating => {
                if matches!(key, Key::Named(Named::Escape))
                    && !matches!(self.upgrade, UpgradeState::Updating(_))
                {
                    self.set_modal(Modal::None);
                }
            }
            Modal::TmuxChoice => match key {
                Key::Named(Named::Enter) => self.choose_tmux(true),
                // Esc dismisses without persisting, so the choice is re-asked
                // on the next launch. Only explicit picks record a backend.
                Key::Named(Named::Escape) => self.set_modal(Modal::None),
                Key::Character(s) => match s.as_str() {
                    "t" | "T" | "y" | "Y" => self.choose_tmux(true),
                    "n" | "N" => self.choose_tmux(false),
                    _ => {}
                },
                _ => {}
            },
            Modal::Onboarding { step, .. } => {
                let step = *step;
                match key {
                    Key::Named(Named::Escape) => self.onboard_skip(),
                    Key::Named(Named::Enter) => return self.onboard_advance(),
                    Key::Named(Named::ArrowDown) => {
                        if step == crate::app::OnboardStep::Project {
                            self.app.onboard_dir_move(1);
                        }
                    }
                    Key::Named(Named::ArrowUp) => {
                        if step == crate::app::OnboardStep::Project {
                            self.app.onboard_dir_move(-1);
                        }
                    }
                    Key::Named(Named::Tab) => {
                        if step == crate::app::OnboardStep::Project {
                            if self.app.onboard_toggle_project_focus() {
                                return focus(crate::gui::view::modal_name_id());
                            }
                            self.app.onboard_dir_pick();
                            return Task::batch([
                                focus(crate::gui::view::modal_input_id()),
                                move_cursor_to_end(crate::gui::view::modal_input_id()),
                            ]);
                        }
                    }
                    _ => {}
                }
            }
            Modal::ShortcutOverlay => {
                if matches!(key, Key::Named(Named::Escape))
                    || match_global_shortcut(&key, mods, self.current_screen())
                        == Some(GlobalShortcut::ShortcutOverlay)
                {
                    self.set_modal(Modal::None);
                }
            }
            // No handler arm here meant every key, including Escape, was
            // swallowed by the "any modal open" guard with no way to dismiss
            // from the keyboard (Bug 10).
            Modal::ScriptsEditor => {
                if matches!(key, Key::Named(Named::Escape)) {
                    // Same path as the Cancel button (`Msg::ScriptsEditorCancel`),
                    // so unsaved edits are discarded and `scripts_editor` is reset.
                    self.cancel_modal();
                }
            }
            Modal::Teardown => {
                if matches!(key, Key::Named(Named::Escape)) {
                    // `cancel_modal` already gates this by teardown stage: it
                    // skips a still-running script (proceed to removal) or
                    // dismisses once removal has finished (mirroring "close"),
                    // and is a no-op mid-removal — there's no button for that
                    // stage either, since an in-flight `git worktree remove`
                    // can't be safely interrupted.
                    self.cancel_modal();
                }
            }
            _ => {}
        }
        Task::none()
    }

    /// Window close request: quit immediately unless native sessions would
    /// die with the window, in which case confirm first.
    pub(super) fn on_close_requested(&mut self, id: iced::window::Id) -> Task<Msg> {
        // tmux-backed sessions survive grove; only running native
        // sessions die with the window.
        let native_running = self.app.native_sessions_running();
        if native_running == 0 {
            self.flush_ui_zoom_save();
            return iced::window::close(id);
        }
        let noun = if native_running == 1 {
            "session"
        } else {
            "sessions"
        };
        // Known gap: grove is one-modal-deep, so the quit confirm
        // replaces any open modal and cancelling does not restore it.
        // Acceptable for now; a modal stack would be needed to do
        // better.
        self.set_modal(Modal::Confirm {
            title: "Quit Grove?".into(),
            prompt: format!("{native_running} running {noun} will end. quit anyway?"),
            destructive: true,
            kind: ConfirmKind::Quit,
        });
        Task::none()
    }

    /// Top-level key routing: the remove-project modal owns its own keys, and
    /// a key handled while the theme picker stays open re-scrolls it to the
    /// (possibly moved) selection.
    pub(super) fn on_key_press(
        &mut self,
        key: Key,
        modified_key: Key,
        mods: Modifiers,
    ) -> Task<Msg> {
        if let Modal::RemoveProject { in_progress, .. } = &self.app.modal {
            let busy = *in_progress;
            return self.handle_remove_project_key(key, busy);
        }
        let was_theme_picker = matches!(self.app.modal, Modal::ThemePicker { .. });
        let task = self.handle_key(key, modified_key, mods);
        if was_theme_picker && matches!(self.app.modal, Modal::ThemePicker { .. }) {
            return self.scroll_theme_picker_to_selection();
        }
        task
    }

    pub(in crate::gui) fn on_add_project(&mut self, msg: add_project::Msg) -> Task<Msg> {
        match msg {
            add_project::Msg::Open => {
                self.open_agent_menu = None;
                self.app.focus_pane(crate::app::Pane::Projects);
                // `add_project::open` only owns `&mut App` and sets
                // `app.modal` directly — it can't call `set_modal`, so
                // this routes through `open_child`, which clears every
                // other child-state field before it runs. Covers both
                // reachable callers (the sidebar "+ Add project" button
                // and the palette's `AddProject` row) uniformly, rather
                // than leaving it to each caller to have pre-cleared the
                // palette.
                self.open_child(|g| add_project::open(&mut g.app, &mut g.add_project));
                focus(crate::gui::view::modal_input_id())
            }
            other => {
                let (task, rebuild) =
                    add_project::update(&mut self.app, &mut self.add_project, other);
                if rebuild == add_project::WtCacheRebuild::Rebuild {
                    self.rebuild_wt_cache();
                }
                task.map(Msg::AddProject)
            }
        }
    }

    pub(super) fn on_scripts(&mut self, msg: crate::gui::scripts_editor::Msg) -> Task<Msg> {
        match msg {
            // EXCEPTION to `open_child`/`set_modal`: `ThemePicker` is a
            // `Modal` variant with no backing `Grove`-owned `Option`
            // field, so opening it never routes through `open_child` —
            // and this deliberately does NOT clear `self.scripts_editor`,
            // so the editor's live text buffers survive the round trip
            // back from the theme picker instead of being silently
            // discarded.
            crate::gui::scripts_editor::Msg::OpenProjectThemePicker { proj } => {
                self.app.open_project_theme_picker(proj);
                self.scroll_theme_picker_to_selection()
            }
            crate::gui::scripts_editor::Msg::Cancel => {
                self.cancel_modal();
                Task::none()
            }
            msg @ crate::gui::scripts_editor::Msg::Open { .. } => {
                self.open_agent_menu = None;
                // `scripts_editor`'s private `open` (reached via
                // `update` below) only owns `&mut App` plus its own
                // `&mut Option<ScriptsEditorState>` — like
                // `add_project::open`, it can't reach the other sibling
                // child-state fields, so this routes through
                // `open_child` to clear them first (same shape as the
                // `Msg::AddProject(Open)` handler above).
                self.open_child(|g| {
                    crate::gui::scripts_editor::update(&mut g.scripts_editor, &mut g.app, msg)
                })
                .map(Msg::Scripts)
            }
            other => {
                crate::gui::scripts_editor::update(&mut self.scripts_editor, &mut self.app, other)
                    .map(Msg::Scripts)
            }
        }
    }

    pub(super) fn on_toggle_remove_worktrees(&mut self, v: bool) {
        if let Modal::RemoveProject {
            also_remove_worktrees,
            in_progress,
            ..
        } = &mut self.app.modal
        {
            if !*in_progress {
                *also_remove_worktrees = v;
            }
        }
    }

    pub(super) fn on_modal_confirm(&mut self, yes: bool) -> Task<Msg> {
        if matches!(self.app.modal, Modal::Teardown) {
            // The teardown modal's only confirm action is dismissal,
            // and only once removal has finished.
            if matches!(
                self.app.teardown.as_ref().map(|t| t.stage),
                Some(crate::app::TeardownStage::Done { .. })
            ) {
                self.app.close_teardown();
            }
            Task::none()
        } else {
            self.confirm_modal_response(yes)
        }
    }

    pub(super) fn on_add_project_browse(&mut self) -> Task<Msg> {
        // One dialog at a time — a second click while the picker is up
        // must not spawn another.
        if self.picker_open {
            return Task::none();
        }
        if matches!(self.app.modal, Modal::AddProject | Modal::Onboarding { .. }) {
            self.picker_open = true;
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose a project folder")
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Msg::AddProjectPicked,
            );
        }
        Task::none()
    }

    pub(super) fn on_add_project_picked(
        &mut self,
        picked: Option<std::path::PathBuf>,
    ) -> Task<Msg> {
        self.picker_open = false;
        // A late result after the modal closed (or changed) must not
        // mutate an unrelated modal; `None` = user cancelled.
        if let Some(path) = picked {
            match &self.app.modal {
                Modal::AddProject => return self.choose_add_project_folder(path),
                Modal::Onboarding {
                    step: crate::app::OnboardStep::Project,
                    ..
                } => {
                    self.app.onboard_set_path(format!("{}/", path.display()));
                    return move_cursor_to_end(crate::gui::view::modal_input_id());
                }
                _ => {}
            }
        }
        Task::none()
    }

    pub(super) fn choose_tmux(&mut self, enabled: bool) {
        if let Err(e) = self.app.choose_tmux_enabled(enabled) {
            self.set_modal(Modal::Message(format!("Tmux setup failed: {e}")));
        }
    }

    pub(super) fn submit_modal_input(&mut self) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_input() {
            self.set_modal(Modal::Message(format!("Input failed: {e}")));
        }
        self.resize_new_sessions(&before);
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        self.rebuild_wt_cache();
    }

    /// Resolve a Confirm modal. `ConfirmKind::Quit` is handled here (it needs
    /// an iced Task to exit); everything else delegates to the app layer.
    pub(super) fn confirm_modal_response(&mut self, yes: bool) -> Task<Msg> {
        if matches!(
            self.app.modal,
            Modal::Confirm {
                kind: ConfirmKind::Quit,
                ..
            }
        ) {
            self.set_modal(Modal::None);
            if yes {
                self.flush_ui_zoom_save();
                return iced::exit();
            }
            return Task::none();
        }
        self.submit_modal_confirm(yes);
        Task::none()
    }

    pub(super) fn submit_modal_confirm(&mut self, yes: bool) {
        let before = self.session_keys();
        if let Err(e) = self.app.submit_confirm(yes) {
            self.set_modal(Modal::Message(format!("Action failed: {e}")));
        }
        self.resize_new_sessions(&before);
        // If the grid is open, append the new session index so it appears.
        if self.grid_view && self.app.sessions.len() > before.len() {
            self.tile_order.push(self.app.sessions.len() - 1);
            self.persist_grid_order();
            self.refresh_pty_viewport();
        }
        // The teardown PTY lives outside `app.sessions`, so resize it directly.
        if let Some(s) = self.app.teardown.as_mut().and_then(|t| t.session.as_mut()) {
            s.resize(self.pty_layout.rows, self.pty_layout.sess_cols);
        }
        self.rebuild_wt_cache();
    }

    /// Resize any sessions spawned during this update to the current PTY
    /// viewport. Sessions created indirectly (e.g. auto-spawned when a new
    /// worktree is added) otherwise stay at the 80x24 PTY default and don't
    /// fill the workspace width.
    pub(in crate::gui) fn resize_new_sessions(&mut self, before: &[usize]) {
        for s in &mut self.app.sessions {
            let key = Arc::as_ptr(&s.dirty) as usize;
            if !before.contains(&key) {
                s.resize(self.pty_layout.rows, self.pty_layout.sess_cols);
            }
        }
    }

    pub(in crate::gui) fn session_keys(&self) -> Vec<usize> {
        self.app
            .sessions
            .iter()
            .map(|s| Arc::as_ptr(&s.dirty) as usize)
            .collect()
    }

    /// After a choose-funnel attempt, focus whichever add-project field is now
    /// primary: the name field once the details step is showing, else the
    /// step-1 path input (the funnel rejected the folder).
    pub(super) fn focus_add_project_field(&self) -> Task<Msg> {
        add_project::focus_field(&self.add_project).map(Msg::AddProject)
    }

    /// Single choke point for changing `self.app.modal`: sets the new value
    /// and unconditionally clears every `Grove`-owned child-state field that
    /// backs some modal variant (`add_project`, `scripts_editor`, `launcher`,
    /// `theme_manager_editor`) — clear-by-default, so a caller that forgets
    /// to populate the field belonging to the incoming variant gets an
    /// immediately-visible empty view instead of a silently stale one that
    /// only shows up later. Callers that need the incoming variant's own
    /// state populate the relevant field with a direct assignment right
    /// after calling this (see `open_session_launcher`).
    ///
    /// Child `open()` free functions (`add_project::open`,
    /// `theme_manager_editor::open`, and `scripts_editor`'s private `open`
    /// reached via its `update`) can't call this directly — they only own
    /// `&mut App` plus their own state slot, not `&mut Grove`. They route
    /// through `open_child` instead, which clears the same fields this does
    /// before handing control to the child. See `open_child`'s doc comment.
    pub(in crate::gui) fn set_modal(&mut self, modal: Modal) {
        self.add_project = None;
        self.scripts_editor = None;
        self.launcher = None;
        self.theme_manager_editor = None;
        self.app.modal = modal;
    }

    /// Grove-side wrapper for child `open()` paths that only own `&mut App`
    /// (plus their own `&mut Option<..>` state slot) and so structurally
    /// cannot call `set_modal` themselves. Clears every `Grove`-owned
    /// child-state field — the same set `set_modal` clears — and then runs
    /// `f`, which is free to repoint `app.modal` at its own variant and
    /// populate its own field. Because the clearing lives here instead of at
    /// each call site, a call site can no longer forget it, and a future
    /// fifth child field only needs to be added to this one list rather than
    /// to every call site.
    ///
    /// Not used for the ScriptsEditor → ThemePicker round trip: `ThemePicker`
    /// is a `Modal` variant with no backing `Grove`-owned `Option` field, so
    /// opening it never opens a "child" in this sense and never routes
    /// through here — see the comment at that call site in the
    /// `Msg::Scripts` arm, which is what keeps `self.scripts_editor` alive
    /// across the round trip.
    pub(super) fn open_child<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.add_project = None;
        self.scripts_editor = None;
        self.launcher = None;
        self.theme_manager_editor = None;
        f(self)
    }

    pub(in crate::gui) fn cancel_modal(&mut self) {
        // The teardown modal repurposes cancel: skip a still-running script
        // (proceed to removal) or dismiss once removal has finished.
        if matches!(self.app.modal, Modal::Teardown) {
            match self.app.teardown.as_ref().map(|t| t.stage) {
                Some(crate::app::TeardownStage::Done { .. }) => self.app.close_teardown(),
                _ => self.app.skip_teardown_script(),
            }
            return;
        }
        self.set_modal(Modal::None);
    }

    /// Begin executing a confirmed remove-project action. If the user opted
    /// to delete worktrees on disk, kick off the recursive teardown task;
    /// otherwise finalize inline and close the modal.
    pub(super) fn kick_off_remove_project(&mut self) -> Task<Msg> {
        let (idx, also, project_path, mut queue) = match &self.app.modal {
            Modal::RemoveProject {
                idx,
                also_remove_worktrees,
                project_path,
                worktrees,
                in_progress,
                ..
            } if !*in_progress => (
                *idx,
                *also_remove_worktrees,
                project_path.clone(),
                worktrees.clone(),
            ),
            _ => return Task::none(),
        };

        if !also || queue.is_empty() {
            match self.app.finalize_remove_project(idx) {
                Ok(msg) if !msg.is_empty() => self.app.set_toast(msg),
                Err(e) => self.app.set_error_toast(format!("err: {e}")),
                _ => {}
            }
            self.set_modal(Modal::None);
            self.rebuild_wt_cache();
            return Task::none();
        }

        // Kill any sessions tied to these worktrees up front so the
        // PTY handles are released before `git worktree remove --force`
        // touches the filesystem.
        for wt in &queue {
            self.app.kill_sessions_for_wt(wt);
        }

        if let Modal::RemoveProject {
            in_progress,
            done,
            current,
            errors,
            ..
        } = &mut self.app.modal
        {
            *in_progress = true;
            *done = 0;
            *errors = Vec::new();
            *current = queue.first().cloned().unwrap_or_default();
        }

        let first = queue.remove(0);
        remove_worktree_task(project_path, first, queue)
    }

    /// Process the result of one worktree removal and either dispatch the
    /// next one or finalize the project removal when the queue is empty.
    pub(super) fn advance_remove_project(
        &mut self,
        path: String,
        error: Option<String>,
        remaining: Vec<String>,
    ) -> Task<Msg> {
        let (idx, project_path) = match &mut self.app.modal {
            Modal::RemoveProject {
                idx,
                project_path,
                done,
                current,
                errors,
                ..
            } => {
                *done += 1;
                if let Some(e) = error {
                    errors.push(format!("{path}: {e}"));
                }
                *current = remaining.first().cloned().unwrap_or_default();
                (*idx, project_path.clone())
            }
            _ => return Task::none(),
        };

        if let Some(next) = remaining.first().cloned() {
            let rest: Vec<String> = remaining.into_iter().skip(1).collect();
            return remove_worktree_task(project_path, next, rest);
        }

        // Done — finalize.
        let errors = match &self.app.modal {
            Modal::RemoveProject { errors, .. } => errors.clone(),
            _ => Vec::new(),
        };
        match self.app.finalize_remove_project(idx) {
            Ok(msg) if !msg.is_empty() && !errors.is_empty() => {
                self.app
                    .set_error_toast(format!("{} ({} worktree errors)", msg, errors.len()));
            }
            Ok(msg) if !msg.is_empty() => self.app.set_toast(msg),
            Err(e) => self.app.set_error_toast(format!("err: {e}")),
            _ => {}
        }
        self.set_modal(Modal::None);
        self.rebuild_wt_cache();
        Task::none()
    }
}
