use crate::app::Modal;
use crate::gui::state::{Grove, Msg, ThemeManagerMsg};
use iced::Task;

impl Grove {
    /// Theme-manager modal family dispatch (`Msg::ThemeManager`).
    pub(super) fn on_theme_manager(&mut self, msg: ThemeManagerMsg) -> Task<Msg> {
        match msg {
            ThemeManagerMsg::Open => return self.open_theme_manager(),
            ThemeManagerMsg::Select(i) => self.theme_manager_select(i),
            ThemeManagerMsg::RenameStart(i) => self.theme_manager_rename_start(i),
            ThemeManagerMsg::RenameChanged(s) => self.theme_manager_rename_changed(s),
            ThemeManagerMsg::RenameSubmit => self.theme_manager_rename_submit(),
            ThemeManagerMsg::RenameCancel => self.theme_manager_rename_cancel(),
            ThemeManagerMsg::Duplicate(i) => self.theme_manager_duplicate(i),
            ThemeManagerMsg::DeleteStart(i) => self.theme_manager_delete_start(i),
            ThemeManagerMsg::DeleteConfirm => self.theme_manager_delete_confirm(),
            ThemeManagerMsg::DeleteCancel => self.theme_manager_delete_cancel(),
            ThemeManagerMsg::New => return self.theme_manager_new(),
            ThemeManagerMsg::Close => self.theme_manager_close(),
            ThemeManagerMsg::Editor(msg) => return self.on_theme_manager_editor(msg),
        }
        Task::none()
    }

    /// Opens the dedicated theme-management modal, replacing whatever's
    /// currently showing (including the palette, if it was open) — this is a
    /// top-level `Modal`, not a palette drill-in pane.
    pub(in crate::gui) fn open_theme_manager(&mut self) -> Task<Msg> {
        self.set_modal(Modal::ThemeManager {
            selected: 0,
            rename: None,
            rename_error: None,
            pending_delete: None,
        });
        Task::none()
    }

    pub(super) fn on_theme_manager_editor(
        &mut self,
        msg: crate::gui::theme_manager_editor::Msg,
    ) -> Task<Msg> {
        match msg {
            crate::gui::theme_manager_editor::Msg::Edit(idx) => self.theme_manager_edit_start(idx),
            msg => {
                let (task, invalidate) = crate::gui::theme_manager_editor::update(
                    &mut self.app,
                    &mut self.theme_manager_editor,
                    self.live_mods,
                    msg,
                );
                if invalidate == crate::gui::theme_manager_editor::PtyCacheInvalidate::Invalidate {
                    self.invalidate_pty_render_cache();
                }
                task.map(|v| Msg::ThemeManager(ThemeManagerMsg::Editor(v)))
            }
        }
    }

    pub(super) fn theme_manager_close(&mut self) {
        self.set_modal(Modal::None);
    }

    pub(super) fn theme_manager_select(&mut self, idx: usize) {
        let len = grove_core::theme::all_custom_themes().len();
        if idx >= len {
            return;
        }
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = idx;
        }
    }

    pub(super) fn theme_manager_move(&mut self, delta: i32) {
        let len = grove_core::theme::all_custom_themes().len();
        if len == 0 {
            return;
        }
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = crate::gui::launcher::clamp(*selected, delta, len);
        }
    }

    /// "New theme": creates a fresh custom theme seeded from the *current
    /// active theme's* mode default (`DEFAULT_DARK_THEME`/`DEFAULT_LIGHT_
    /// THEME`), auto-named via `theme::duplicate_name("untitled")`, and opens
    /// the EDITOR sub-view on it directly.
    pub(super) fn theme_manager_new(&mut self) -> Task<Msg> {
        let kind = grove_core::theme::current().kind;
        let seed_name = match kind {
            grove_core::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
            grove_core::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
        };
        let Some(seed) = grove_core::theme::by_name(seed_name) else {
            return Task::none(); // defensive: the mode defaults are builtins
        };
        let new_name = grove_core::theme::duplicate_name("untitled");
        let mut new_theme = seed;
        new_theme.name = std::borrow::Cow::Owned(new_name);
        new_theme.kind = kind;
        if let Err(e) = grove_core::theme::add_custom(new_theme.clone()) {
            self.app.set_error_toast(e);
            return Task::none();
        }
        // `theme_manager_editor::open` only owns `&mut App` and sets
        // `app.modal` directly — it can't call `set_modal`, so this routes
        // through `open_child`. Reachable only from `Modal::ThemeManager`
        // (opened via `set_modal` in `open_theme_manager`), where the other
        // three child-state fields are already `None`, so this is a no-op
        // clear today — but it means this call site can't silently start
        // leaking stale state if that ever changes.
        let task = self.open_child(|g| {
            crate::gui::theme_manager_editor::open(
                &mut g.app,
                &mut g.theme_manager_editor,
                new_theme,
            )
        });
        self.invalidate_pty_render_cache();
        if let Some(ed) = &mut self.theme_manager_editor {
            ed.created_this_session = true;
        }
        task.map(|v| Msg::ThemeManager(ThemeManagerMsg::Editor(v)))
    }

    /// Duplicates the theme at row `idx` into a new custom theme (auto-named
    /// via `theme::duplicate_name`) and selects the copy.
    pub(super) fn theme_manager_duplicate(&mut self, idx: usize) {
        let Some(base) = grove_core::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        let new_name = grove_core::theme::duplicate_name(&base.name);
        let mut copy = base;
        copy.name = std::borrow::Cow::Owned(new_name.clone());
        if let Err(e) = grove_core::theme::add_custom(copy) {
            self.app.set_error_toast(e);
            return;
        }
        let idx = grove_core::theme::all_custom_themes()
            .iter()
            .position(|t| t.name == new_name)
            .unwrap_or(idx);
        if let Modal::ThemeManager { selected, .. } = &mut self.app.modal {
            *selected = idx;
        }
    }

    pub(super) fn theme_manager_rename_start(&mut self, idx: usize) {
        let Some(theme) = grove_core::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        if let Modal::ThemeManager {
            selected,
            rename,
            rename_error,
            ..
        } = &mut self.app.modal
        {
            *selected = idx;
            let name = theme.name.to_string();
            *rename = Some((name.clone(), name));
            *rename_error = None;
        }
    }

    pub(super) fn theme_manager_rename_changed(&mut self, s: String) {
        if let Modal::ThemeManager {
            rename: Some((_, buf)),
            rename_error,
            ..
        } = &mut self.app.modal
        {
            *buf = s;
            *rename_error = None;
        }
    }

    pub(super) fn theme_manager_rename_submit(&mut self) {
        let Modal::ThemeManager {
            rename: Some((original, buffer)),
            ..
        } = &self.app.modal
        else {
            return;
        };
        let (original, buffer) = (original.clone(), buffer.clone());
        let new_name = buffer.trim().to_string();
        if new_name.is_empty() {
            if let Modal::ThemeManager { rename_error, .. } = &mut self.app.modal {
                *rename_error = Some("name can't be empty".to_string());
            }
            return;
        }
        if new_name == original {
            self.theme_manager_rename_cancel();
            return;
        }
        match grove_core::theme::rename_custom(&original, &new_name) {
            Ok(()) => {
                self.persist_theme_rename(&original, &new_name);
                self.invalidate_pty_render_cache();
                let idx = grove_core::theme::all_custom_themes()
                    .iter()
                    .position(|t| t.name == new_name)
                    .unwrap_or(0);
                if let Modal::ThemeManager {
                    selected,
                    rename,
                    rename_error,
                    ..
                } = &mut self.app.modal
                {
                    *selected = idx;
                    *rename = None;
                    *rename_error = None;
                }
            }
            Err(e) => {
                if let Modal::ThemeManager { rename_error, .. } = &mut self.app.modal {
                    *rename_error = Some(e);
                }
            }
        }
    }

    pub(super) fn theme_manager_rename_cancel(&mut self) {
        if let Modal::ThemeManager {
            rename,
            rename_error,
            ..
        } = &mut self.app.modal
        {
            *rename = None;
            *rename_error = None;
        }
    }

    /// Updates any persisted reference to a renamed/saved-under-a-new-name
    /// custom theme — `store.theme`/`theme_dark`/`theme_light` and any
    /// `project.theme` pin — from `old` to `new`, then saves the store if
    /// anything changed. `theme::rename_custom`/`update_custom` only touch
    /// the in-memory `CUSTOM` registry (and `ACTIVE`, for `rename_custom`);
    /// without this, a rename would leave stale pins in `store.json` that
    /// silently fall back to a default theme (or a since-reused name) the
    /// next time they're read. Mirrors `theme_manager_delete_confirm`'s
    /// store-update pattern, extended to project pins.
    pub(super) fn persist_theme_rename(&mut self, old: &str, new: &str) {
        let mut changed = false;
        if self.app.store.theme.as_deref() == Some(old) {
            self.app.store.theme = Some(new.to_string());
            changed = true;
        }
        if self.app.store.theme_dark.as_deref() == Some(old) {
            self.app.store.theme_dark = Some(new.to_string());
            changed = true;
        }
        if self.app.store.theme_light.as_deref() == Some(old) {
            self.app.store.theme_light = Some(new.to_string());
            changed = true;
        }
        for proj in &mut self.app.store.projects {
            if proj.theme.as_deref() == Some(old) {
                proj.theme = Some(new.to_string());
                changed = true;
            }
        }
        if changed {
            grove_core::storage::persist(&self.app.store);
        }
    }

    pub(super) fn theme_manager_delete_start(&mut self, idx: usize) {
        let Some(theme) = grove_core::theme::all_custom_themes().get(idx).cloned() else {
            return;
        };
        if let Modal::ThemeManager {
            selected,
            pending_delete,
            ..
        } = &mut self.app.modal
        {
            *selected = idx;
            *pending_delete = Some(theme.name.to_string());
        }
    }

    /// Deletes the pending custom theme. Falls back to the mode default
    /// (`DEFAULT_DARK_THEME`/`DEFAULT_LIGHT_THEME`) if it was the active
    /// theme — same fallback the palette's Theme pane used.
    pub(super) fn theme_manager_delete_confirm(&mut self) {
        let Modal::ThemeManager {
            pending_delete: Some(name),
            ..
        } = &self.app.modal
        else {
            return;
        };
        let name = name.clone();
        let Some(kind) = grove_core::theme::all_custom_themes()
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.kind)
        else {
            // The pending theme is already gone (e.g. deleted from another
            // path, or `themes.json` reloaded out from under the modal) —
            // clear the stale confirmation instead of leaving the dialog
            // permanently stuck on a theme that no longer exists.
            if let Modal::ThemeManager { pending_delete, .. } = &mut self.app.modal {
                *pending_delete = None;
            }
            return;
        };
        let was_active = grove_core::theme::current().name == name;
        grove_core::theme::delete_custom(&name);
        if was_active {
            let fallback = match kind {
                grove_core::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
                grove_core::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
            };
            grove_core::theme::set_by_name(fallback);
            self.app.store.theme = Some(fallback.to_string());
            match kind {
                grove_core::theme::ThemeKind::Dark => {
                    self.app.store.theme_dark = Some(fallback.to_string());
                }
                grove_core::theme::ThemeKind::Light => {
                    self.app.store.theme_light = Some(fallback.to_string());
                }
            }
            grove_core::storage::persist(&self.app.store);
        }
        self.invalidate_pty_render_cache();
        let total = grove_core::theme::all_custom_themes().len();
        if let Modal::ThemeManager {
            selected,
            pending_delete,
            ..
        } = &mut self.app.modal
        {
            *pending_delete = None;
            *selected = (*selected).min(total.saturating_sub(1));
        }
    }

    pub(super) fn theme_manager_delete_cancel(&mut self) {
        if let Modal::ThemeManager { pending_delete, .. } = &mut self.app.modal {
            *pending_delete = None;
        }
    }

    /// List row's "Edit" button: opens the EDITOR sub-view on the custom
    /// theme at row `idx`.
    pub(super) fn theme_manager_edit_start(&mut self, idx: usize) -> Task<Msg> {
        let Some(theme) = grove_core::theme::all_custom_themes().get(idx).cloned() else {
            return Task::none();
        };
        // Same shape as `theme_manager_new` above: routes through
        // `open_child` since `theme_manager_editor::open` can't call
        // `set_modal` itself.
        let task = self.open_child(|g| {
            crate::gui::theme_manager_editor::open(&mut g.app, &mut g.theme_manager_editor, theme)
        });
        self.invalidate_pty_render_cache();
        task.map(|v| Msg::ThemeManager(ThemeManagerMsg::Editor(v)))
    }
}
