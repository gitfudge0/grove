//! Keyboard handling for `Modal::SessionLauncher`, covering all four
//! palette sub-states (switch / settings drill-in with its sub-panes and
//! resize mode / row-actions strip / root-typing).

use super::helpers::*;
use super::state::*;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::update::{global_mods, SettingRow};
use grove_core::agent::Agent;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::Task;

impl Grove {
    /// Key handling for `Modal::SessionLauncher`, covering all four palette
    /// sub-states (switch / settings drill-in with its sub-panes and resize
    /// mode / row-actions strip / root-typing). Moved verbatim out
    /// of `handle_modal_key`'s `Modal::SessionLauncher` arm; dispatched from
    /// there via a single delegating match arm, the same shape
    /// `theme_manager_editor::handle_key` delegates from `Modal::ThemeManager`
    /// -- except this one stays `impl Grove` rather than taking `&mut App`
    /// (see this module's doc comment for why: the palette's key handling
    /// calls `Grove`'s own message handlers directly -- zoom,
    /// `open_theme_manager` via mod+M/mod+E, row actions, settings commits --
    /// and several Grove-only fields throughout, not just at the one flagged
    /// call site).
    pub(in crate::gui) fn handle_session_launcher_key(
        &mut self,
        key: Key,
        mods: Modifiers,
    ) -> Task<GMsg> {
        let Some(LauncherState {
            input,
            selected,
            browse_all,
            switch,
            row_actions,
            settings,
            ..
        }) = self.launcher_modal()
        else {
            return Task::none();
        };
        let (input, selected, browse_all) = (input.clone(), *selected, *browse_all);
        if let Some(sel) = *switch {
            // "Switch to session" drill-in: ↑↓ move the session
            // selection (clamped, no wrap); Enter switches focus and
            // closes the palette; Esc backs out to the root list.
            let len = self.switch_rows(&input).len();
            let list_delta: Option<i32> = match &key {
                Key::Named(Named::ArrowDown) => Some(1),
                Key::Named(Named::ArrowUp) => Some(-1),
                _ => None,
            };
            if let Some(delta) = list_delta {
                let new_sel = crate::gui::launcher::clamp(sel, delta, len);
                self.set_switch_selected(new_sel);
            } else {
                match key {
                    Key::Named(Named::Escape) => {
                        if let Some(LauncherState { switch, .. }) = self.launcher_modal_mut() {
                            *switch = None;
                        }
                        // The root list was recomputed against the cleared
                        // input when the drill-in opened; the old cursor no
                        // longer points at the row it was on.
                        self.set_palette_selected(0);
                    }
                    Key::Named(Named::Enter) => {
                        if let Some(row) = self.resolve_switch_selected(&input) {
                            return self.launcher_switch_to_row(row);
                        }
                    }
                    _ => {}
                }
            }
        } else if let Some(s) = settings.clone() {
            // Settings drill-in. `resizing` (Root pane only, D4) takes
            // priority: it's a modal-within-the-modal for the App-size
            // row where arrows/±/0 adjust zoom instead of moving the
            // list cursor, and Enter/Esc merely *leave* the mode
            // rather than popping the drill-in. Otherwise, behavior
            // branches on `s.pane`: Root keeps the phase-1 filtered-
            // list nav (Enter now opens a sub-pane for the five enum
            // rows via `activate_setting`, not a no-op); each sub-pane
            // has its own short, unfiltered row count and its own
            // commit/cancel (see the `Grove::*_pane_*` methods).
            let dir_delta: Option<i32> = match &key {
                Key::Named(Named::ArrowDown) => Some(1),
                Key::Named(Named::ArrowUp) => Some(-1),
                _ => None,
            };
            if s.resizing {
                if let Some(delta) = dir_delta {
                    // ↑↓ exit resizing, then move the Root cursor
                    // exactly as it would outside resize mode.
                    let rows_len = self.settings_rows_filtered(&input).len();
                    let new_sel = crate::gui::launcher::clamp(s.selected, delta, rows_len);
                    if let Some(LauncherState {
                        settings: Some(ss), ..
                    }) = self.launcher_modal_mut()
                    {
                        ss.resizing = false;
                        ss.selected = new_sel;
                    }
                    return self.scroll_launcher_settings_to_selection();
                }
                match key {
                    Key::Named(Named::ArrowLeft) => {
                        self.on_zoom_out();
                        return Task::none();
                    }
                    Key::Named(Named::ArrowRight) => {
                        self.on_zoom_in();
                        return Task::none();
                    }
                    Key::Named(Named::Enter | Named::Escape) => {
                        if let Some(LauncherState {
                            settings: Some(ss), ..
                        }) = self.launcher_modal_mut()
                        {
                            ss.resizing = false;
                        }
                    }
                    Key::Character(ch) => match ch.as_str() {
                        "-" => {
                            self.on_zoom_out();
                            return Task::none();
                        }
                        "+" => {
                            self.on_zoom_in();
                            return Task::none();
                        }
                        "0" => {
                            self.on_zoom_reset();
                            return Task::none();
                        }
                        _ => {}
                    },
                    _ => {}
                }
            } else if let Some(strip_sel) = s.update_actions {
                // Update-actions strip (E3): ←→/Tab move across the
                // strip's actions, ⏎ runs one, Esc collapses just
                // the strip. ↑↓ collapse it and move the Root cursor
                // as normal, same shape as resizing above.
                let len = update_available_actions(matches!(
                    self.upgrade_method,
                    grove_core::upgrade::InstallMethod::Unknown
                ))
                .len();
                if let Some(delta) = dir_delta {
                    let rows_len = self.settings_rows_filtered(&input).len();
                    let new_sel = crate::gui::launcher::clamp(s.selected, delta, rows_len);
                    if let Some(LauncherState {
                        settings: Some(ss), ..
                    }) = self.launcher_modal_mut()
                    {
                        ss.update_actions = None;
                        ss.selected = new_sel;
                    }
                    return self.scroll_launcher_settings_to_selection();
                }
                let strip_delta: Option<i32> = match &key {
                    Key::Named(Named::ArrowLeft) => Some(-1),
                    Key::Named(Named::ArrowRight | Named::Tab) => Some(1),
                    _ => None,
                };
                if let Some(delta) = strip_delta {
                    let new_sel = crate::gui::launcher::clamp(strip_sel, delta, len);
                    if let Some(LauncherState {
                        settings: Some(ss), ..
                    }) = self.launcher_modal_mut()
                    {
                        ss.update_actions = Some(new_sel);
                    }
                } else {
                    match key {
                        Key::Named(Named::Escape) => self.close_update_actions_strip(),
                        Key::Named(Named::Enter) => return self.update_actions_commit(strip_sel),
                        _ => {}
                    }
                }
            } else {
                match s.pane {
                    SettingsPane::Root => {
                        let sel = s.selected;
                        let rows = self.settings_rows_filtered(&input);
                        if let Some(delta) = dir_delta {
                            let new_sel = crate::gui::launcher::clamp(sel, delta, rows.len());
                            if let Some(LauncherState {
                                settings: Some(ss), ..
                            }) = self.launcher_modal_mut()
                            {
                                ss.selected = new_sel;
                            }
                            return self.scroll_launcher_settings_to_selection();
                        }
                        match key {
                            Key::Named(Named::Escape) => {
                                if let Some(LauncherState {
                                    settings, input, ..
                                }) = self.launcher_modal_mut()
                                {
                                    *settings = None;
                                    input.clear();
                                }
                                self.set_palette_selected(0);
                            }
                            Key::Named(Named::Enter) => {
                                if let Some(&sr) = rows.get(sel) {
                                    return self.activate_setting(sr);
                                }
                            }
                            _ => {}
                        }
                    }
                    SettingsPane::Theme { .. } => {
                        if let Some(delta) = dir_delta {
                            return self.theme_pane_move(delta);
                        }
                        match key {
                            Key::Named(Named::Escape) => return self.theme_pane_cancel(),
                            Key::Named(Named::Enter) => return self.theme_pane_commit(),
                            // Tab cycles the mode row; bare
                            // letters always stay with the
                            // search input (fuzzy-filtering the
                            // list) — edit/manage are ⌘-chorded
                            // specifically so they never collide
                            // with typing a theme name.
                            Key::Named(Named::Tab) => return self.theme_pane_cycle_mode(),
                            Key::Character(s) if global_mods(mods) => match s.as_str() {
                                "e" | "E" => return self.theme_pane_open_editor(),
                                "m" | "M" => return self.open_theme_manager(),
                                _ => {}
                            },
                            _ => {}
                        }
                    }
                    SettingsPane::Backend => {
                        if let Some(delta) = dir_delta {
                            let new_sel = crate::gui::launcher::clamp(s.selected, delta, 2);
                            if let Some(LauncherState {
                                settings: Some(ss), ..
                            }) = self.launcher_modal_mut()
                            {
                                ss.selected = new_sel;
                            }
                        } else {
                            match key {
                                Key::Named(Named::Escape) => {
                                    return self.return_to_settings_root(SettingRow::Backend)
                                }
                                Key::Named(Named::Enter) => {
                                    return self.backend_pane_commit(s.selected)
                                }
                                _ => {}
                            }
                        }
                    }
                    SettingsPane::Permissions => {
                        if let Some(delta) = dir_delta {
                            let new_sel = crate::gui::launcher::clamp(s.selected, delta, 2);
                            if let Some(LauncherState {
                                settings: Some(ss), ..
                            }) = self.launcher_modal_mut()
                            {
                                ss.selected = new_sel;
                            }
                        } else {
                            match key {
                                Key::Named(Named::Escape) => {
                                    return self.return_to_settings_root(SettingRow::Permissions)
                                }
                                Key::Named(Named::Enter) => {
                                    return self.permissions_pane_commit(s.selected)
                                }
                                _ => {}
                            }
                        }
                    }
                    SettingsPane::ProjectTheme { .. } => {
                        if let Some(delta) = dir_delta {
                            return self.theme_pane_move(delta);
                        }
                        match key {
                            Key::Named(Named::Escape) => return self.theme_pane_cancel(),
                            Key::Named(Named::Enter) => return self.theme_pane_commit(),
                            // Tab cycles Dark/Light only — no
                            // System mode for a project override
                            // (`theme_pane_cycle_mode` branches on
                            // scope internally).
                            Key::Named(Named::Tab) => return self.theme_pane_cycle_mode(),
                            _ => {}
                        }
                    }
                    SettingsPane::DefaultAgent => {
                        let len = Agent::ALL.len();
                        if let Some(delta) = dir_delta {
                            let new_sel = crate::gui::launcher::clamp(s.selected, delta, len);
                            if let Some(LauncherState {
                                settings: Some(ss), ..
                            }) = self.launcher_modal_mut()
                            {
                                ss.selected = new_sel;
                            }
                        } else {
                            match key {
                                Key::Named(Named::Escape) => {
                                    return self.return_to_settings_root(SettingRow::DefaultAgent)
                                }
                                Key::Named(Named::Enter) => {
                                    return self.default_agent_pane_commit(s.selected)
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        } else if let Some(ra) = row_actions.clone() {
            // Inline row-actions strip: ↑↓ move between the actions
            // (clamped, no wrap); ←→ walk the agent bar hosted on the
            // "Launch session…" row (action 0 only — a no-op on the
            // others); Enter runs the selected action; Esc collapses the
            // strip back to the plain list.
            let list_delta: Option<i32> = match &key {
                Key::Named(Named::ArrowDown) => Some(1),
                Key::Named(Named::ArrowUp) => Some(-1),
                _ => None,
            };
            let agent_delta: Option<i32> = match &key {
                Key::Named(Named::ArrowLeft) => Some(-1),
                Key::Named(Named::ArrowRight) => Some(1),
                _ => None,
            };
            if let Some(delta) = agent_delta {
                if ra.action == 0 {
                    let next = cycle_agent(ra.agent_sel, delta, self.app.available_agents.len());
                    if let Some(LauncherState {
                        row_actions: Some(rr),
                        ..
                    }) = self.launcher_modal_mut()
                    {
                        rr.agent_sel = next;
                    }
                }
                // The search field keeps keyboard focus throughout, so
                // it already moved its own caret on this very arrow (the
                // subscription forwards it regardless — see
                // `should_forward`'s ←→ carve-out). Pin the caret back to
                // the end rather than leaving it drifting mid-query.
                return crate::gui::update::move_cursor_to_end(crate::gui::view::modal_input_id());
            }
            if let Some(delta) = list_delta {
                let base = if self.app.project_themes_enabled() {
                    3
                } else {
                    2
                };
                let action_count = base + self.row_action_scripts(ra.proj).len();
                let new_action = crate::gui::launcher::clamp(ra.action, delta, action_count);
                if let Some(LauncherState {
                    row_actions: Some(rr),
                    ..
                }) = self.launcher_modal_mut()
                {
                    rr.action = new_action;
                }
            } else {
                match key {
                    Key::Named(Named::Escape) => {
                        if let Some(LauncherState { row_actions, .. }) = self.launcher_modal_mut() {
                            *row_actions = None;
                        }
                    }
                    Key::Named(Named::Enter) => {
                        return self.launcher_run_row_action(
                            ra.proj,
                            ra.wt_path,
                            ra.agent_sel,
                            ra.action,
                        )
                    }
                    _ => {}
                }
            }
        } else {
            // Root or typing/browse-all: ↑↓ move the list selection;
            // Tab reveals contextual actions (Recent/Combo rows) or
            // opens the switch-to-session drill-in (arrows can't:
            // ←→ move the caret in the focused search input). Plain
            // letters belong to the input too, never to nav.
            let rows = self.palette_rows(&input, browse_all);
            let list_delta: Option<i32> = match &key {
                Key::Named(Named::ArrowDown) => Some(1),
                Key::Named(Named::ArrowUp) => Some(-1),
                _ => None,
            };
            if let Some(delta) = list_delta {
                let new_selected = crate::gui::launcher::clamp(selected, delta, rows.len());
                self.set_palette_selected(new_selected);
                return self.scroll_launcher_palette_to_selection();
            }
            let enter_actions = matches!(&key, Key::Named(Named::Tab));
            if enter_actions {
                return match self.resolve_selected(&rows) {
                    Some(idx) => self.launcher_enter_row_actions(idx, &input, browse_all),
                    None => Task::none(),
                };
            }
            match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => {
                    return match self.resolve_selected(&rows) {
                        Some(idx) => self.launcher_activate(idx),
                        None => Task::none(),
                    }
                }
                Key::Character(s) if global_mods(mods) => {
                    if let Some(n) = s.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                        // mod+digit addresses sessions only:
                        // in typed mode Setting rows sort
                        // above Combo rows (B2), so a raw
                        // list index would hand ⌘1 to a
                        // setting instead of the first
                        // session. No-op when fewer than
                        // `n` session rows match.
                        if let Some(idx) = nth_session_row(&rows, n) {
                            return self.launcher_activate(idx);
                        }
                    }
                }
                _ => {}
            }
        }

        Task::none()
    }
}
