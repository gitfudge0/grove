//! The Theme and Project-theme sub-panes of the Settings drill-in:
//! entering them, live-previewing a selection, committing/canceling, and
//! reloading custom themes. The app-scoped (`SettingsPane::Theme`) and
//! project-scoped (`SettingsPane::ProjectTheme`) halves share one
//! implementation per handler wherever their behavior is the same shape,
//! branching explicitly on which `SettingsPane` variant is active wherever
//! it genuinely isn't — that variant already *is* the scope tag (no
//! separate `enum ThemeScope` is threaded through: it would just have to be
//! kept in sync with which `SettingsPane` variant is live, which is a
//! second copy of the same fact). This mirrors `App`'s older
//! `ThemePickerScope`-driven `Modal::ThemePicker` handling (`src/app.rs`,
//! `theme_picker_move`/`_submit`/`_cancel`), which this module intentionally
//! doesn't reach into or refactor.
//!
//! Scope differences, all preserved and each visible at its own `match`:
//! - Mode control: App offers Dark/Light/System (`theme_pane_cycle_mode`,
//!   the System segment); Project offers Dark/Light only — no System mode
//!   for a per-project override.
//! - "Use app theme" row: Project's row list gets a leading `None` entry
//!   when the search query is empty (`project_theme_pane_rows`); App's
//!   never does.
//! - Live preview: App previews into the *global* active theme
//!   (`theme::set`), so Esc must restore `original`. Project previews only
//!   into the pane's local `preview` field — the global theme is never
//!   touched, so Project's Esc has nothing to roll back.
//! - Commit target: App writes `Store::theme`/`theme_dark`/`theme_light`/
//!   `theme_follow_system` and returns to the Settings root. Project writes
//!   one project's `Project::theme` override and closes the drill-in
//!   entirely (it was never entered from Settings root).
//! - `theme_pane_open_editor` (⌘E into the theme manager) exists only for
//!   App — there is no project-scoped equivalent.
//! - `enter_theme_pane`/`enter_project_theme_pane` are left unmerged: the
//!   App entry point routes through the shared `enter_settings_pane`/
//!   `open_settings_drill_in` scaffolding every other Settings sub-pane
//!   uses, which assumes entry from the Settings root. The Project entry
//!   point is reached from a session row's Tab actions strip instead, never
//!   from Settings root, and has to hand-set `row_actions`/`settings`
//!   itself — forcing it through the shared helper would mean growing that
//!   helper a branch for this one caller, which is worse than two small,
//!   honestly-different entry functions.

use super::helpers::*;
use super::state::*;
use crate::app::Modal;
use crate::gui::state::ThemeManagerMsg;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::update::{scroll_to, SettingRow};
use crate::gui::view::launcher_theme_scrollable_id;
use iced::Task;

impl Grove {
    /// Enter the Theme sub-pane (D1 in the palette redesign mock): previews
    /// live like `Modal::ThemePicker`, so `original` is captured up front for
    /// Esc to restore. Starts on the active theme's own kind list, cursor on
    /// the active theme (mirrors `App::open_theme_picker`).
    pub(super) fn enter_theme_pane(&mut self) -> Task<GMsg> {
        let original = grove_core::theme::current();
        let kind = original.kind;
        let selected = theme_pane_selected_index(kind, &original.name);
        let follow_system = self.app.theme_follow_system;
        self.enter_settings_pane(
            SettingsPane::Theme {
                original,
                kind,
                follow_system,
            },
            selected,
        );
        // The entry scroll is load-bearing: `themes_of` is alphabetical, so
        // without it the pre-selected current theme usually sits below the
        // pane's 280px fold and the pane appears to have selected nothing.
        self.scroll_launcher_theme_to_selection()
    }

    /// Scroll the Theme/ProjectTheme sub-pane's list so the selected row is
    /// centered — same idiom for both scopes, but pitched by different
    /// geometry: the Theme pane's list carries a CUSTOM section header, an
    /// empty-hint row, and a trailing "Manage themes…" row that the
    /// ProjectTheme pane's plain list doesn't (see `theme_pane_scroll_offset`
    /// vs `launcher_theme_scroll_offset`'s doc comments) — a real content
    /// difference, not just a missing control, so the two geometries stay
    /// separate even though this caller is unified.
    pub(super) fn scroll_launcher_theme_to_selection(&self) -> Task<GMsg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Some(LauncherState {
            input,
            settings: Some(LauncherSettings { pane, selected, .. }),
            ..
        }) = self.launcher_modal()
        else {
            return Task::none();
        };
        let y = match pane {
            SettingsPane::Theme { kind, .. } => {
                let n_builtin = theme_pane_rows(*kind, input).len();
                let n_custom = theme_pane_custom_rows(*kind, input).len();
                theme_pane_scroll_offset(n_builtin, n_custom, *selected)
            }
            SettingsPane::ProjectTheme { kind, .. } => {
                let total = project_theme_pane_rows(*kind, input).len();
                launcher_theme_scroll_offset(total, *selected)
            }
            _ => return Task::none(),
        };
        scroll_to(launcher_theme_scrollable_id(), AbsoluteOffset { x: 0.0, y })
    }

    /// Theme/ProjectTheme sub-pane ↑↓ / row click: move (or jump straight
    /// to, for a click) the selection within the currently shown
    /// fuzzy-filtered list. App previews live into the global active theme
    /// and opts back out of "follow system" (mirrors
    /// `ThemePickerScope::App`'s arm of `App::theme_picker_move`); Project
    /// only updates the local `preview` draft, never touching the global
    /// active theme (mirrors `ThemePickerScope::Project`'s arm).
    pub(in crate::gui) fn theme_pane_select(&mut self, idx: usize) -> Task<GMsg> {
        let input = match self.launcher_modal() {
            Some(LauncherState { input, .. }) => input.clone(),
            _ => return Task::none(),
        };
        let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        else {
            return Task::none();
        };
        match &mut ls.pane {
            SettingsPane::Theme {
                kind,
                follow_system,
                ..
            } => {
                let rows = theme_pane_combined_rows(*kind, &input);
                let Some(theme) = rows.get(idx).cloned() else {
                    return Task::none();
                };
                ls.selected = idx;
                *follow_system = false;
                grove_core::theme::set(theme);
            }
            SettingsPane::ProjectTheme { kind, preview, .. } => {
                let rows = project_theme_pane_rows(*kind, &input);
                let Some(row) = rows.get(idx).cloned() else {
                    return Task::none();
                };
                *preview = row;
                ls.selected = idx;
            }
            _ => return Task::none(),
        }
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme/ProjectTheme sub-pane ↑↓ (keyboard): delta-move
    /// `theme_pane_select` over the currently shown kind's fuzzy-filtered
    /// list length — App's combined (Built-in + Custom) list, or Project's
    /// same list fronted by its own "Use app theme" row.
    pub(super) fn theme_pane_move(&mut self, delta: i32) -> Task<GMsg> {
        let (selected, len) = match self.launcher_modal() {
            Some(LauncherState {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            }) => (*selected, theme_pane_combined_rows(*kind, input).len()),
            Some(LauncherState {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::ProjectTheme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            }) => (*selected, project_theme_pane_rows(*kind, input).len()),
            _ => return Task::none(),
        };
        let new_sel = crate::gui::launcher::clamp(selected, delta, len);
        self.theme_pane_select(new_sel)
    }

    /// Theme/ProjectTheme sub-pane Dark/Light segment (click or Tab-cycle):
    /// switches which kind's list is shown. App re-finds the *global*
    /// active theme's position in the new list (opting out of "follow
    /// system") and previews it; Project re-finds the *local preview*'s
    /// position instead (by name, `None` included), never touching the
    /// global active theme.
    pub(in crate::gui) fn theme_pane_set_kind(
        &mut self,
        kind: grove_core::theme::ThemeKind,
    ) -> Task<GMsg> {
        let input = match self.launcher_modal() {
            Some(LauncherState { input, .. }) => input.clone(),
            _ => return Task::none(),
        };
        let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        else {
            return Task::none();
        };
        match &mut ls.pane {
            SettingsPane::Theme {
                kind: k,
                follow_system,
                ..
            } => {
                let rows = theme_pane_combined_rows(kind, &input);
                let active = grove_core::theme::current();
                let idx = rows.iter().position(|t| t.name == active.name).unwrap_or(0);
                *k = kind;
                *follow_system = false;
                ls.selected = idx;
                if let Some(t) = rows.get(idx) {
                    grove_core::theme::set(t.clone());
                }
            }
            SettingsPane::ProjectTheme {
                kind: k, preview, ..
            } => {
                let rows = project_theme_pane_rows(kind, &input);
                let current_name = preview.as_ref().map(|t| t.name.to_string());
                let idx = rows
                    .iter()
                    .position(|row| row.as_ref().map(|t| t.name.to_string()) == current_name)
                    .unwrap_or(0);
                *k = kind;
                ls.selected = idx;
                *preview = rows.get(idx).cloned().flatten();
            }
            _ => return Task::none(),
        }
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme sub-pane System segment (click or Tab-cycle): previews the
    /// resolved system theme and marks "follow system" as a local draft
    /// (persisted on ⏎) — mirrors `Grove::theme_picker_toggle_system(true)`.
    /// The list always falls back to the dark set under "system", since
    /// system mode still needs a concrete dark choice. App scope only —
    /// there is no "follow system" concept for a project override.
    pub(in crate::gui) fn theme_pane_set_system(&mut self) -> Task<GMsg> {
        let name = self
            .app
            .resolve_system_theme_name(self.app.system_theme_mode)
            .to_string();
        let input = match self.launcher_modal() {
            Some(LauncherState { input, .. }) => input.clone(),
            _ => return Task::none(),
        };
        // The list snaps back to the (filtered) dark set: re-clamp the
        // cursor so a selection made deep in a longer list can't dangle
        // past the end.
        let dark_len = theme_pane_combined_rows(grove_core::theme::ThemeKind::Dark, &input).len();
        let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        else {
            return Task::none();
        };
        let SettingsPane::Theme {
            kind,
            follow_system,
            ..
        } = &mut ls.pane
        else {
            return Task::none();
        };
        ls.selected = crate::gui::launcher::clamp(ls.selected, 0, dark_len);
        *kind = grove_core::theme::ThemeKind::Dark;
        *follow_system = true;
        grove_core::theme::set_by_name(&name);
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }

    /// Theme/ProjectTheme sub-pane Tab: cycle the mode row. App cycles
    /// Dark → Light → System → Dark (`next_theme_mode`), routed through
    /// `theme_pane_set_kind`/`theme_pane_set_system` so preview/selection
    /// behave identically to clicking a segment. Project cycles Dark ↔
    /// Light only, through `theme_pane_set_kind` — no System mode for a
    /// project override.
    pub(super) fn theme_pane_cycle_mode(&mut self) -> Task<GMsg> {
        // Local, deliberately tiny enum: just enough to compute the next
        // mode from an immutable read of `self.launcher`, so the actual
        // `self.theme_pane_set_kind`/`theme_pane_set_system` calls (which
        // need `&mut self`) aren't made while still borrowing `self.launcher`.
        enum NextMode {
            Kind(grove_core::theme::ThemeKind),
            System,
        }
        let next = match self.launcher_modal() {
            Some(LauncherState {
                settings:
                    Some(LauncherSettings {
                        pane:
                            SettingsPane::Theme {
                                kind,
                                follow_system,
                                ..
                            },
                        ..
                    }),
                ..
            }) => Some(match next_theme_mode(*kind, *follow_system) {
                ThemeMode::Dark => NextMode::Kind(grove_core::theme::ThemeKind::Dark),
                ThemeMode::Light => NextMode::Kind(grove_core::theme::ThemeKind::Light),
                ThemeMode::System => NextMode::System,
            }),
            Some(LauncherState {
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::ProjectTheme { kind, .. },
                        ..
                    }),
                ..
            }) => Some(NextMode::Kind(project_theme_next_kind(*kind))),
            _ => None,
        };
        match next {
            Some(NextMode::Kind(kind)) => self.theme_pane_set_kind(kind),
            Some(NextMode::System) => self.theme_pane_set_system(),
            None => Task::none(),
        }
    }

    /// Theme/ProjectTheme sub-pane ⏎: persist the pane's current selection.
    /// App persists the previewed theme (or "follow system") through the
    /// same `Store` fields `App::theme_picker_submit` writes, then returns
    /// to Settings root landed on the App theme row. Project persists
    /// `preview` as this project's pinned override (or clears it) through
    /// the same write/toast `App::theme_picker_submit`'s project-scope arm
    /// uses, then closes the drill-in back to the plain session list (not
    /// Settings root — this pane was never entered from there).
    pub(super) fn theme_pane_commit(&mut self) -> Task<GMsg> {
        enum Commit {
            App {
                follow_system: bool,
            },
            Project {
                proj: usize,
                preview: Option<grove_core::theme::Theme>,
            },
        }
        let commit = match self.launcher_modal() {
            Some(LauncherState {
                settings: Some(ls), ..
            }) => match &ls.pane {
                SettingsPane::Theme { follow_system, .. } => Some(Commit::App {
                    follow_system: *follow_system,
                }),
                SettingsPane::ProjectTheme { proj, preview, .. } => Some(Commit::Project {
                    proj: *proj,
                    preview: preview.clone(),
                }),
                _ => None,
            },
            _ => None,
        };
        match commit {
            Some(Commit::App { follow_system }) => {
                self.app.theme_follow_system = follow_system;
                self.app.store.theme_follow_system = follow_system;
                if follow_system {
                    self.app.apply_system_theme();
                } else {
                    let chosen = grove_core::theme::current();
                    self.app.store.theme = Some(chosen.name.to_string());
                    match chosen.kind {
                        grove_core::theme::ThemeKind::Dark => {
                            self.app.store.theme_dark = Some(chosen.name.to_string());
                        }
                        grove_core::theme::ThemeKind::Light => {
                            self.app.store.theme_light = Some(chosen.name.to_string());
                        }
                    }
                    grove_core::theme::set(chosen);
                }
                grove_core::storage::persist(&self.app.store);
                self.invalidate_pty_render_cache();
                self.return_to_settings_root(SettingRow::Theme)
            }
            Some(Commit::Project { proj, preview }) => {
                match self.app.store.projects.get_mut(proj) {
                    Some(p) => {
                        p.theme = preview.as_ref().map(|t| t.name.to_string());
                        grove_core::storage::persist(&self.app.store);
                        let label = preview
                            .as_ref()
                            .map_or_else(|| "default".to_string(), |t| t.name.to_string());
                        self.app.set_toast(format!("project theme: {label}"));
                    }
                    None => {
                        self.app.set_error_toast("project no longer exists");
                    }
                }
                if let Some(LauncherState {
                    settings, input, ..
                }) = self.launcher_modal_mut()
                {
                    *settings = None;
                    input.clear();
                }
                self.invalidate_pty_render_cache();
                Task::none()
            }
            None => Task::none(),
        }
    }

    /// Theme/ProjectTheme sub-pane Esc. App restores the pre-entry theme
    /// (it's the only scope that ever live-previewed into the global active
    /// theme) and returns to Settings root. Project never touched the
    /// global theme, so there's nothing to roll back — it just drops the
    /// preview and closes the drill-in back to the plain session list
    /// (again, never entered from Settings root).
    pub(super) fn theme_pane_cancel(&mut self) -> Task<GMsg> {
        enum Cancel {
            App { original: grove_core::theme::Theme },
            Project,
        }
        let cancel = match self.launcher_modal() {
            Some(LauncherState {
                settings: Some(ls), ..
            }) => match &ls.pane {
                SettingsPane::Theme { original, .. } => Some(Cancel::App {
                    original: original.clone(),
                }),
                SettingsPane::ProjectTheme { .. } => Some(Cancel::Project),
                _ => None,
            },
            _ => None,
        };
        match cancel {
            Some(Cancel::App { original }) => {
                grove_core::theme::set(original);
                self.invalidate_pty_render_cache();
                self.return_to_settings_root(SettingRow::Theme)
            }
            Some(Cancel::Project) => {
                if let Some(LauncherState {
                    settings, input, ..
                }) = self.launcher_modal_mut()
                {
                    *settings = None;
                    input.clear();
                }
                self.invalidate_pty_render_cache();
                Task::none()
            }
            None => Task::none(),
        }
    }

    /// "Reload themes" command (palette root, keyword-only — see
    /// `palette_rows`): re-reads `themes.json` via `theme::load_custom`,
    /// replacing `App::theme_load_errors` with the fresh result and
    /// surfacing it as a toast (mock E1/E2). If the active theme's name no
    /// longer resolves anywhere afterward (edited out from under the app,
    /// deleted, synced from another machine, ...), falls back to the mode
    /// default *silently* — this is routine config drift, not an error, per
    /// the mock's E3 sticky note. If the Theme sub-pane happens to be open,
    /// its combined Built-in+Custom list just changed under it, so the
    /// cursor is reclamped and rescrolled instead of left dangling; there's
    /// nothing pane-specific to refresh otherwise, so the palette just closes
    /// (same "run and dismiss" idiom as `PaletteRow::AddProject`). This is
    /// App-only (as before the pane unification): reloading while the
    /// ProjectTheme pane is open still falls through to closing the palette.
    pub(super) fn reload_themes(&mut self) -> Task<GMsg> {
        self.app.theme_load_errors = grove_core::theme::load_custom();

        let active = grove_core::theme::current();
        if let Some(fallback) = theme_reload_fallback(&active.name, active.kind) {
            grove_core::theme::set_by_name(fallback);
            self.app.store.theme = Some(fallback.to_string());
            match active.kind {
                grove_core::theme::ThemeKind::Dark => {
                    self.app.store.theme_dark = Some(fallback.to_string());
                }
                grove_core::theme::ThemeKind::Light => {
                    self.app.store.theme_light = Some(fallback.to_string());
                }
            }
            grove_core::storage::persist(&self.app.store);
        }

        match grove_core::theme_file::summarize_errors(&self.app.theme_load_errors) {
            Some(summary) => self.app.set_error_toast(summary),
            None => self.app.set_toast("themes.json reloaded"),
        }

        let theme_pane_kind_input = match self.launcher_modal() {
            Some(LauncherState {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        ..
                    }),
                ..
            }) => Some((*kind, input.clone())),
            _ => None,
        };
        self.invalidate_pty_render_cache();
        match theme_pane_kind_input {
            Some((kind, input)) => {
                let total = theme_pane_combined_rows(kind, &input).len();
                if let Some(LauncherState {
                    settings: Some(ls), ..
                }) = self.launcher_modal_mut()
                {
                    ls.selected = ls.selected.min(total.saturating_sub(1));
                }
                self.scroll_launcher_theme_to_selection()
            }
            None => {
                self.set_modal(Modal::None);
                Task::none()
            }
        }
    }

    /// Theme sub-pane ⌘E on a custom row: opens `Modal::ThemeManager`
    /// directly in its EDITOR sub-view for that theme (closing the palette —
    /// `ThemeManager` is a top-level modal, not a palette drill-in pane).
    /// App scope only — the ProjectTheme pane has no swatch-editor entry
    /// point; a project override is always an existing theme by reference,
    /// never something edited in place from here.
    pub(super) fn theme_pane_open_editor(&mut self) -> Task<GMsg> {
        let (input, selected, kind) = match self.launcher_modal() {
            Some(LauncherState {
                input,
                settings:
                    Some(LauncherSettings {
                        pane: SettingsPane::Theme { kind, .. },
                        selected,
                        ..
                    }),
                ..
            }) => (input.clone(), *selected, *kind),
            _ => return Task::none(),
        };
        if !theme_pane_row_is_custom(kind, &input, selected) {
            return Task::none(); // builtins aren't editable
        }
        let Some(draft) = theme_pane_combined_rows(kind, &input)
            .get(selected)
            .cloned()
        else {
            return Task::none();
        };
        let task = crate::gui::theme_manager_editor::open(
            &mut self.app,
            &mut self.theme_manager_editor,
            draft,
        );
        // EXCEPTION to the `set_modal` choke point below: `theme_manager_
        // editor::open` only owns `&mut App` and just repointed `app.modal`
        // at `Modal::ThemeManager` directly, bypassing `set_modal` (it isn't
        // a `Grove` method, so it structurally can't call it) — it can't
        // reach `Grove::launcher` either, so this call clears the now-stale
        // palette state on its behalf (mirrors `add_project::open`'s call
        // sites, which follow the same shape).
        self.launcher = None;
        self.invalidate_pty_render_cache();
        task.map(|v| GMsg::ThemeManager(ThemeManagerMsg::Editor(v)))
    }

    /// Enter the ProjectTheme sub-pane from a session row's actions strip
    /// (action `2`) — unlike `enter_theme_pane`, this is never reached from
    /// the Settings root list, so it sets `Modal::SessionLauncher` fields
    /// directly rather than routing through `enter_settings_pane`/
    /// `open_settings_drill_in` (which assume that entry point and don't
    /// touch `row_actions`). Starts on the project's pinned theme if it has
    /// one, else the current global theme's kind with no preview ("Use app
    /// theme").
    pub(super) fn enter_project_theme_pane(&mut self, proj: usize) -> Task<GMsg> {
        let pinned = self
            .app
            .store
            .projects
            .get(proj)
            .and_then(|p| p.theme.as_deref())
            .and_then(grove_core::theme::by_name);
        let kind = pinned
            .as_ref()
            .map_or(grove_core::theme::current().kind, |t| t.kind);
        let rows = project_theme_pane_rows(kind, "");
        let selected = rows
            .iter()
            .position(|t| match (t, &pinned) {
                (Some(a), Some(b)) => a.name == b.name,
                (None, None) => true,
                _ => false,
            })
            .unwrap_or(0);
        if let Some(LauncherState {
            input,
            selected: outer_selected,
            row_actions,
            settings,
            ..
        }) = self.launcher_modal_mut()
        {
            input.clear();
            *outer_selected = 0;
            *row_actions = None;
            *settings = Some(LauncherSettings {
                pane: SettingsPane::ProjectTheme {
                    proj,
                    kind,
                    preview: pinned,
                },
                selected,
                resizing: false,
                update_actions: None,
            });
        }
        self.invalidate_pty_render_cache();
        self.scroll_launcher_theme_to_selection()
    }
}
