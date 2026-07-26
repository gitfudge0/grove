//! Dispatch for `Msg::SessionLauncher` — the command palette's own message
//! set. The handlers themselves live in `crate::gui::session_launcher`; this
//! module only holds the arms that need `Grove`-owned state (or recursive
//! `Grove::update` dispatch) and so can't be free functions over there.

use super::global_mods;
use crate::gui::session_launcher::{self, LauncherState, SettingsPane};
use crate::gui::state::{Grove, Msg};
use iced::Task;

impl Grove {
    pub(super) fn on_session_launcher(&mut self, msg: session_launcher::Msg) -> Task<Msg> {
        match msg {
            session_launcher::Msg::Open => {
                crate::telemetry::track("launcher_opened", vec![]);
                self.open_session_launcher();
                super::focus(crate::gui::view::modal_input_id())
            }
            session_launcher::Msg::InputChanged(s) => {
                // A `global_mods` chord (⌘D, ⌘⌫, ...) is a command, never
                // text -- but `iced_widget::text_input` only special-cases
                // ⌘C/⌘X/⌘A explicitly (⌘V routes to `InputPasted`
                // via `on_paste` instead, see below); every other chord still
                // gets inserted/deleted unconditionally by its fallback
                // character/Backspace handling, publishing this very message.
                // Dropping it here (using `live_mods`, tracked independently
                // -- see `Msg::ModifiersChanged`'s doc comment for why
                // `KeyPress`'s own `mods` arrives too late to gate this)
                // leaves `input` untouched; the next render re-diffs the
                // field back to that unchanged value, undoing the widget's
                // own transient edit.
                if global_mods(self.live_mods) {
                    return Task::none();
                }
                self.launcher_input_changed(s)
            }
            // `text_input`'s dedicated ⌘V callback: unlike `InputChanged`
            // above, this only ever fires for a real paste (iced's text_input
            // calls `on_paste` instead of `on_input` when pasting -- see its
            // `text_input.rs` update handling), so it must NOT go through the
            // `global_mods` guard: the chord's modifier is still held when the
            // pasted content arrives, and that guard exists only to catch a
            // chord's spurious character insert, not to block an actual paste.
            session_launcher::Msg::InputPasted(s) => self.launcher_input_changed(s),
            session_launcher::Msg::Activate(i) => self.launcher_activate(i),
            session_launcher::Msg::OptionsPick(i) => {
                let len = self.app.available_agents.len();
                if let Some(LauncherState {
                    options: Some(r), ..
                }) = self.launcher_modal_mut()
                {
                    if i < len {
                        r.agent = i;
                    }
                }
                self.launcher_start();
                Task::none()
            }
            session_launcher::Msg::SwitchSessionPick(si) => self.launcher_switch_to(si),
            session_launcher::Msg::RowActionPick(action) => {
                let row_actions = match self.launcher_modal() {
                    Some(LauncherState {
                        row_actions: Some(r),
                        ..
                    }) => Some(r.clone()),
                    _ => None,
                };
                if let Some(r) = row_actions {
                    return self.launcher_run_row_action(r.proj, r.wt_path, r.agent, action);
                }
                Task::none()
            }
            session_launcher::Msg::SettingActivate(i) => {
                let input = match self.launcher_modal() {
                    Some(LauncherState {
                        input,
                        settings: Some(_),
                        ..
                    }) => input.clone(),
                    _ => return Task::none(),
                };
                let rows = self.settings_rows_filtered(&input);
                if let Some(&s) = rows.get(i) {
                    if let Some(LauncherState {
                        settings: Some(ls), ..
                    }) = self.launcher_modal_mut()
                    {
                        ls.selected = i;
                        // Clicking any row while the update-actions strip is
                        // expanded collapses it first -- activating the
                        // CheckUpdates row itself just re-opens it below.
                        ls.update_actions = None;
                    }
                    return self.activate_setting(s);
                }
                Task::none()
            }
            // `theme_pane_select` itself branches on which
            // `SettingsPane` variant is active (App vs Project) —
            // see its doc comment in `theme_panes.rs`.
            session_launcher::Msg::ThemePaneSelect(i) => self.theme_pane_select(i),
            session_launcher::Msg::ThemePaneDark => {
                self.theme_pane_set_kind(grove_core::theme::ThemeKind::Dark)
            }
            session_launcher::Msg::ThemePaneLight => {
                self.theme_pane_set_kind(grove_core::theme::ThemeKind::Light)
            }
            session_launcher::Msg::ThemePaneSystem => self.theme_pane_set_system(),
            session_launcher::Msg::SettingsPaneActivate(i) => {
                let pane = match self.launcher_modal() {
                    Some(LauncherState {
                        settings: Some(ls), ..
                    }) => ls.pane.clone(),
                    _ => return Task::none(),
                };
                match pane {
                    SettingsPane::Backend => self.backend_pane_commit(i),
                    SettingsPane::Permissions => self.permissions_pane_commit(i),
                    SettingsPane::DefaultAgent => self.default_agent_pane_commit(i),
                    SettingsPane::Root
                    | SettingsPane::Theme { .. }
                    | SettingsPane::ProjectTheme { .. } => Task::none(),
                }
            }
            // Not delegated to a free fn in `session_launcher` -- needs
            // `Grove`-only upgrade state and `Grove`'s own handlers
            // (`update_actions_commit` calls `on_start_update` /
            // `on_skip_version` / `on_copy_release_url`); see that variant's
            // doc comment in `session_launcher::Msg`.
            session_launcher::Msg::UpdateActionPick(i) => {
                if let Some(LauncherState {
                    settings: Some(ls), ..
                }) = self.launcher_modal_mut()
                {
                    if ls.update_actions.is_some() {
                        ls.update_actions = Some(i);
                        return self.update_actions_commit(i);
                    }
                }
                Task::none()
            }
        }
    }
}
