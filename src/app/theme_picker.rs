use super::{cycle, App, Modal};
use anyhow::Result;
use grove_core::storage::{self, Store};
use grove_core::theme;

/// Fallback dark/light themes for "system" mode when the user hasn't picked
/// one explicitly yet (thematic pair: `tokyonight` and its day companion).
pub(crate) const DEFAULT_DARK_THEME: &str = "tokyonight-storm";
pub(crate) const DEFAULT_LIGHT_THEME: &str = "tokyonight-day";

/// Which theme the picker modal is editing: the global app theme, or a
/// single project's pinned "Project theme" (see `Store::project_themes_enabled`
/// / `storage::Project::theme`). Project scope never touches the global
/// active theme — hovering/selecting only updates local picker state, and
/// submitting writes `Project::theme` instead of `theme::set` + `Store::theme`.
#[derive(Clone, PartialEq)]
pub enum ThemePickerScope {
    App,
    /// Keyed by project name (unique, like the rest of this feature) rather
    /// than index — indices can shift under project add/remove while a
    /// picker is theoretically open, names don't.
    Project(String),
}

/// Rewrites any of `store.theme`/`theme_dark`/`theme_light` that names a
/// theme `theme::by_name` can no longer resolve (a builtin dropped from a
/// later curated set, or a custom theme deleted outside the app) to
/// `DEFAULT_DARK_THEME`. Deliberately no name-mapping of old builtins to
/// their closest modern equivalent — `tokyonight-storm` is the one fallback,
/// same as `theme_reload_fallback`'s at-runtime counterpart. Returns whether
/// anything changed, so the caller only re-persists the store when needed.
/// Pulled out of `App::new` as a free function so it's testable without a
/// full `App`/on-disk config.
pub(crate) fn migrate_stale_theme_names(store: &mut Store) -> bool {
    let mut changed = false;
    for slot in [
        &mut store.theme,
        &mut store.theme_dark,
        &mut store.theme_light,
    ] {
        if let Some(name) = slot.as_deref() {
            if theme::by_name(name).is_none() {
                *slot = Some(DEFAULT_DARK_THEME.to_string());
                changed = true;
            }
        }
    }
    changed
}

impl App {
    /// Whether the universal "Project themes" toggle is on (Settings →
    /// Appearance).
    pub(crate) fn project_themes_enabled(&self) -> bool {
        self.store.project_themes_enabled
    }

    pub(crate) fn set_project_themes_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.project_themes_enabled = enabled;
        // `?` (not a tail return) so `StoreError` converts into `anyhow`.
        storage::save(&self.store)?;
        Ok(())
    }

    /// Resolve the theme a PTY belonging to `project_name` should render its
    /// *content* in: `None` when Project themes is off, the project has no
    /// pinned theme, or its pinned name no longer matches a builtin — in
    /// every such case the caller falls back to the global active theme.
    ///
    /// `launcher_preview` is resolved by the caller (`Grove::pty`, in
    /// `view.rs`, via `session_launcher::project_theme_preview`) rather than
    /// here: it depends on `Grove::launcher`, GUI-model state this domain
    /// method can't reach on its own. `Some(inner)` wins outright over the
    /// persisted-pin lookup below — see `session_launcher::project_theme_preview`'s
    /// doc comment.
    pub(crate) fn project_theme_override(
        &self,
        project_name: &str,
        launcher_preview: Option<Option<theme::Theme>>,
    ) -> Option<theme::Theme> {
        // Live preview: while the project-scoped theme picker is open for
        // this project, its current highlight wins over the persisted pin —
        // this is the only path that lets a PTY preview a theme that hasn't
        // been saved yet. It never touches the global active theme (no
        // `theme::set` here), so other projects' tiles and app chrome are
        // unaffected. Cancel/submit both drop back to the persisted state
        // (cancel via the modal closing + an explicit cache invalidation,
        // submit because the pin is now saved).
        if let Modal::ThemePicker {
            scope: ThemePickerScope::Project(name),
            tab,
            sel_dark,
            sel_light,
            project_use_default,
            ..
        } = &self.modal
        {
            if name == project_name {
                if *project_use_default {
                    return None; // preview the global theme
                }
                let sel = match tab {
                    theme::ThemeKind::Dark => *sel_dark,
                    theme::ThemeKind::Light => *sel_light,
                };
                // Indexed rather than materialized: this runs per PTY per
                // frame while the picker is open.
                return theme::selectable_theme_at(*tab, sel);
            }
        }
        // Same live-preview idea for the in-palette project-theme pane
        // (Settings drill-in entered from a session row's actions strip):
        // `preview` is the pane's whole draft state, so it wins outright —
        // `None` means "preview the global theme", matching
        // `project_use_default` above.
        if let Some(preview) = launcher_preview {
            return preview;
        }
        if !self.store.project_themes_enabled {
            return None;
        }
        self.store
            .projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| p.theme.as_deref())
            .and_then(theme::by_name)
    }

    pub(crate) fn open_theme_picker(&mut self, return_to_settings: bool) {
        let original = theme::current();
        let tab = original.kind;
        let sel = theme::selectable_themes_of(tab)
            .iter()
            .position(|t| t.name == original.name)
            .unwrap_or(0);
        let (sel_dark, sel_light) = match tab {
            theme::ThemeKind::Dark => (sel, 0),
            theme::ThemeKind::Light => (0, sel),
        };
        self.modal = Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            original,
            return_to_settings,
            follow_system: self.theme_follow_system,
            scope: ThemePickerScope::App,
            project_use_default: false,
        };
    }

    /// Open the theme picker scoped to a single project's pinned "Project
    /// theme" (opened from the Project Settings modal's "Project theme"
    /// row). Unlike the app-scoped picker, hovering/selecting here never
    /// touches the global active theme — only `theme_picker_submit` writes
    /// anything, and it writes `Project::theme`, not `theme::set`.
    pub(crate) fn open_project_theme_picker(&mut self, proj: usize) {
        let Some(project) = self.store.projects.get(proj) else {
            return;
        };
        let name = project.name.clone();
        let original = theme::current();
        let pinned = project.theme.as_deref().and_then(theme::by_name);
        let project_use_default = pinned.is_none();
        let tab = pinned.as_ref().map_or(original.kind, |t| t.kind);
        let sel = pinned
            .as_ref()
            .and_then(|t| {
                theme::selectable_themes_of(tab)
                    .iter()
                    .position(|x| x.name == t.name)
            })
            .unwrap_or(0);
        let (sel_dark, sel_light) = match tab {
            theme::ThemeKind::Dark => (sel, 0),
            theme::ThemeKind::Light => (0, sel),
        };
        self.modal = Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            original,
            return_to_settings: false,
            follow_system: false,
            scope: ThemePickerScope::Project(name),
            project_use_default,
        };
    }

    /// The theme name to use for `mode` under "follow system" — the user's
    /// saved dark/light theme, falling back to the built-in defaults.
    pub(crate) fn resolve_system_theme_name(&self, mode: iced::theme::Mode) -> &str {
        match mode {
            iced::theme::Mode::Light => self
                .store
                .theme_light
                .as_deref()
                .unwrap_or(DEFAULT_LIGHT_THEME),
            iced::theme::Mode::Dark | iced::theme::Mode::None => self
                .store
                .theme_dark
                .as_deref()
                .unwrap_or(DEFAULT_DARK_THEME),
        }
    }

    /// Re-applies the active theme from `system_theme_mode` when following
    /// the OS setting. No-op otherwise.
    pub(crate) fn apply_system_theme(&mut self) {
        if self.theme_follow_system {
            let name = self
                .resolve_system_theme_name(self.system_theme_mode)
                .to_string();
            theme::set_by_name(&name);
        }
    }

    pub(crate) fn theme_picker_move(&mut self, delta: i32) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            project_use_default,
            ..
        } = &mut self.modal
        else {
            return;
        };
        let themes = theme::selectable_themes_of(*tab);
        if themes.is_empty() {
            return;
        }
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        *sel = cycle(*sel, delta, themes.len());
        match scope {
            ThemePickerScope::App => {
                *follow_system = false;
                theme::set(themes[*sel].clone());
            }
            ThemePickerScope::Project(_) => {
                // Project scope only edits local picker state — never the
                // global active theme.
                *project_use_default = false;
            }
        }
    }

    pub(crate) fn theme_picker_switch_tab(&mut self) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            ..
        } = &mut self.modal
        else {
            return;
        };
        *tab = match *tab {
            theme::ThemeKind::Dark => theme::ThemeKind::Light,
            theme::ThemeKind::Light => theme::ThemeKind::Dark,
        };
        if *scope != ThemePickerScope::App {
            return;
        }
        let themes = theme::selectable_themes_of(*tab);
        let sel = match tab {
            theme::ThemeKind::Dark => *sel_dark,
            theme::ThemeKind::Light => *sel_light,
        };
        // Switching tabs alone is just browsing, not a selection — leave
        // `follow_system` as the user set it via the checkbox. But if it's
        // checked, keep the preview showing the resolved system theme rather
        // than snapping to the tab's list selection (which would visually
        // contradict the still-checked checkbox).
        if *follow_system {
            let name = self
                .resolve_system_theme_name(self.system_theme_mode)
                .to_string();
            theme::set_by_name(&name);
        } else if let Some(t) = themes.get(sel) {
            theme::set(t.clone());
        }
    }

    /// Project scope only: select the "Default (follow app)" row, deselecting
    /// any concrete theme in the list. No-op in app scope.
    pub(crate) fn theme_picker_select_default(&mut self) {
        if let Modal::ThemePicker {
            scope: ThemePickerScope::Project(_),
            project_use_default,
            ..
        } = &mut self.modal
        {
            *project_use_default = true;
        }
    }

    pub(crate) fn theme_picker_submit(&mut self) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            return_to_settings,
            follow_system,
            scope,
            project_use_default,
            ..
        } = modal
        else {
            return Ok(());
        };
        if let ThemePickerScope::Project(name) = scope {
            // Project scope never touches the global active theme or
            // `Store::theme*` — it only pins/clears this project's override.
            let chosen = if project_use_default {
                None
            } else {
                let themes = theme::selectable_themes_of(tab);
                let sel = match tab {
                    theme::ThemeKind::Dark => sel_dark,
                    theme::ThemeKind::Light => sel_light,
                };
                themes.get(sel).map(|t| t.name.to_string())
            };
            // The project may have been removed while the picker was open
            // (e.g. via a remove-project flow elsewhere) — only save/toast
            // when the write actually lands somewhere.
            match self.store.projects.iter_mut().find(|p| p.name == name) {
                Some(p) => {
                    p.theme.clone_from(&chosen);
                    storage::save(&self.store)?;
                    let label = chosen.unwrap_or_else(|| "default".to_string());
                    self.set_toast(format!("project theme: {label}"));
                }
                None => {
                    self.set_error_toast(format!("project \"{name}\" no longer exists"));
                }
            }
            // Return to the Project Settings modal (`Modal::ScriptsEditor`);
            // its live editor state lives outside `Modal` in the GUI layer
            // and survives this round-trip untouched.
            self.modal = Modal::ScriptsEditor;
            return Ok(());
        }
        if return_to_settings {
            self.modal = Modal::Settings;
        }
        self.theme_follow_system = follow_system;
        self.store.theme_follow_system = follow_system;
        if follow_system {
            self.apply_system_theme();
            storage::save(&self.store)?;
            self.set_toast("theme: system".to_string());
            return Ok(());
        }
        let themes = theme::selectable_themes_of(tab);
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        let Some(chosen) = themes.get(sel).cloned() else {
            storage::save(&self.store)?;
            return Ok(());
        };
        self.store.theme = Some(chosen.name.to_string());
        match chosen.kind {
            theme::ThemeKind::Dark => self.store.theme_dark = Some(chosen.name.to_string()),
            theme::ThemeKind::Light => self.store.theme_light = Some(chosen.name.to_string()),
        }
        self.set_toast(format!("theme: {}", chosen.name));
        theme::set(chosen);
        storage::save(&self.store)?;
        Ok(())
    }

    pub(crate) fn theme_picker_cancel(&mut self) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        if let Modal::ThemePicker {
            original,
            return_to_settings,
            scope,
            ..
        } = modal
        {
            if let ThemePickerScope::Project(_) = scope {
                // Project scope never previewed into the global theme, so
                // there's nothing to restore — just return to Project Settings.
                self.modal = Modal::ScriptsEditor;
                return;
            }
            theme::set(original);
            if return_to_settings {
                self.modal = Modal::Settings;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{migrate_stale_theme_names, DEFAULT_DARK_THEME};
    use grove_core::storage::Store;

    #[test]
    fn migrate_stale_theme_names_rewrites_unresolvable_fields_only() {
        let _lock = grove_core::theme::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // A stale name in every slot, plus one that still resolves fine.
        let mut store = Store {
            theme: Some("this-theme-was-retired".to_string()),
            theme_dark: Some(DEFAULT_DARK_THEME.to_string()),
            theme_light: Some("also-retired".to_string()),
            ..Store::default()
        };
        assert!(migrate_stale_theme_names(&mut store));
        assert_eq!(store.theme.as_deref(), Some(DEFAULT_DARK_THEME));
        // Already resolvable — left untouched, not just rewritten to the
        // same value (matters if a future change makes the fallback name a
        // dynamic choice rather than a constant).
        assert_eq!(store.theme_dark.as_deref(), Some(DEFAULT_DARK_THEME));
        assert_eq!(store.theme_light.as_deref(), Some(DEFAULT_DARK_THEME));

        // Nothing stale: no-op, reports no change.
        let mut clean = Store {
            theme: Some(DEFAULT_DARK_THEME.to_string()),
            ..Store::default()
        };
        assert!(!migrate_stale_theme_names(&mut clean));

        // All `None`: no-op, reports no change (fresh install / follow-
        // system-only config with nothing pinned yet).
        let mut empty = Store::default();
        assert!(!migrate_stale_theme_names(&mut empty));
    }
}
