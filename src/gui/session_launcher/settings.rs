//! The Settings drill-in: its root row list, entering/leaving it and its
//! sub-panes, and the Backend/Permissions/Default-agent/App-size panes'
//! commit logic. Theme and Project-theme panes live in `theme_panes.rs`
//! instead — they're sizable enough, and different enough from these simple
//! single-select panes, to warrant their own file.

use super::helpers::*;
use super::state::*;
use crate::gui::state::{Grove, Msg as GMsg, UpgradeState};
use crate::gui::update::{scroll_to, SettingRow};
use crate::gui::view::launcher_settings_scrollable_id;
use grove_core::agent::Agent;
use iced::Task;

impl Grove {
    /// The typed-root-list arm of the toggle re-anchor (the drill-in arm is
    /// `reselect_after_toggle`): a toggle can drop its own row out of a
    /// value-matched query, leaving the cursor on a shifted row. Keep the
    /// cursor on the same setting row if it's still rendered, else clamp.
    /// Toggles only — every other Setting row just entered the drill-in,
    /// whose own cursor logic owns the selection now.
    pub(super) fn reselect_typed_setting(
        &mut self,
        s: SettingRow,
        input: &str,
        browse_all: bool,
        old: usize,
    ) {
        if !matches!(s, SettingRow::ProjectThemes | SettingRow::Telemetry) {
            return;
        }
        let rows = self.palette_rows(input, browse_all);
        let new_sel = rows
            .iter()
            .position(|r| matches!(r, PaletteRow::Setting(x) if *x == s))
            .unwrap_or_else(|| crate::gui::launcher::clamp(old, 0, rows.len()));
        self.set_palette_selected(new_sel);
    }

    /// Enter the Settings drill-in (root "Settings…" row, Enter or Tab):
    /// selects the first row and clears the search input, same rationale as
    /// `launcher_enter_switch` — a query like "settings" that found the row
    /// shouldn't still be filtering the drill-in's own, unrelated list.
    /// Returns the scroll-to-top task: the scrollable's offset persists by
    /// widget id, so a reopened drill-in would otherwise resume wherever a
    /// previous visit left it — with the cursor invisibly back at row 0.
    pub(super) fn open_settings_drill_in(&mut self) -> Task<GMsg> {
        if let Some(LauncherState {
            settings, input, ..
        }) = self.launcher_modal_mut()
        {
            *settings = Some(LauncherSettings {
                pane: SettingsPane::Root,
                selected: 0,
                resizing: false,
                update_actions: None,
            });
            input.clear();
        }
        self.set_palette_selected(0);
        self.scroll_launcher_settings_to_selection()
    }

    /// Apply the effect of activating setting `s` — from a root-mode direct
    /// `PaletteRow::Setting` match, or an Enter/click inside the Settings
    /// drill-in. The two toggles flip in place through the exact `Msg`
    /// handlers `settings_modal`'s checkboxes already use, so persistence
    /// stays on that single existing path; the value shown in the palette
    /// re-reads live state (`setting_value`) on the next frame, so no local
    /// mirror is needed here. `CheckUpdates` kicks off the same off-thread
    /// check `Msg::Upgrade(UpgradeMsg::CheckForUpdates)` does — its `Task` must be returned to
    /// the iced runtime (not discarded) or the check never fires — unless a
    /// release is already known to be available, in which case it expands
    /// the update-actions strip instead of pointlessly re-checking (E3; see
    /// `check_updates_opens_strip`). Enum rows (Theme/Backend/Permissions/
    /// DefaultAgent/AppSize) each open a dedicated sub-pane (`SettingsPane`)
    /// — see the `enter_*_pane` methods.
    pub(in crate::gui) fn activate_setting(&mut self, s: SettingRow) -> Task<GMsg> {
        match s {
            SettingRow::ProjectThemes => {
                self.on_project_themes_toggle(!self.app.project_themes_enabled());
                self.reselect_after_toggle(s)
            }
            SettingRow::Telemetry => {
                self.on_telemetry_toggle(!self.app.telemetry_enabled());
                self.reselect_after_toggle(s)
            }
            SettingRow::CheckUpdates => {
                if check_updates_opens_strip(&self.upgrade) {
                    self.open_update_actions_strip()
                } else {
                    self.on_check_for_updates(true)
                }
            }
            SettingRow::Theme => self.enter_theme_pane(),
            SettingRow::Backend => self.enter_backend_pane(),
            SettingRow::Permissions => self.enter_permissions_pane(),
            SettingRow::DefaultAgent => self.enter_default_agent_pane(),
            SettingRow::AppSize => self.enter_appsize_resize(),
        }
    }

    /// Re-anchor the drill-in Root cursor after a toggle: flipping On/Off
    /// rewrites the value string the active query may have been matching
    /// (e.g. "on"), so the row can drop out of — or shift within — the
    /// filtered list under the unmoved cursor. Keep the cursor on the
    /// toggled row when it survived the refilter (`reselect_setting`), else
    /// clamp, then scroll with it. No-op outside the drill-in — the
    /// root/typed list re-anchors in `launcher_activate`'s `Setting` arm.
    pub(super) fn reselect_after_toggle(&mut self, activated: SettingRow) -> Task<GMsg> {
        let input = match self.launcher_modal() {
            Some(LauncherState {
                input,
                settings: Some(_),
                ..
            }) => input.clone(),
            _ => return Task::none(),
        };
        let rows = self.settings_rows_filtered(&input);
        if let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        {
            ls.selected = reselect_setting(&rows, activated, ls.selected);
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Land `pane`/`selected` on the Settings drill-in and clear the query,
    /// same rationale as `open_settings_drill_in` — a query that found the
    /// enum row at root shouldn't keep filtering a sub-pane whose own list
    /// means something else. Reachable straight from a root/typing
    /// `PaletteRow::Setting` match (B2 in the mock), so the drill-in is
    /// opened first when absent — Esc from the pane then pops to the
    /// drill-in Root list, one level at a time, like any other pane exit.
    pub(super) fn enter_settings_pane(&mut self, pane: SettingsPane, selected: usize) {
        if !matches!(
            &self.launcher,
            Some(LauncherState {
                settings: Some(_),
                ..
            })
        ) {
            // The Root-list scroll task is deliberately not chained: this
            // immediately switches to a sub-pane, whose view doesn't render
            // that scrollable at all, and every path back to Root
            // (`return_to_settings_root`) re-scrolls on its own.
            let _ = self.open_settings_drill_in();
        }
        if let Some(LauncherState {
            settings: Some(ls),
            input,
            ..
        }) = self.launcher_modal_mut()
        {
            ls.pane = pane;
            ls.selected = selected;
            ls.update_actions = None;
            input.clear();
        }
    }

    /// Pop a sub-pane back to the Root settings list, landing the cursor on
    /// `from`'s row. Root's own list is recomputed unfiltered
    /// (`settings_rows_filtered("")`) since the query was cleared entering
    /// the sub-pane, mirroring `enter_settings_pane`. Returns the scroll
    /// task landing the viewport with the cursor — `from` can sit near the
    /// bottom of the list (Default agent, Check for updates).
    pub(super) fn return_to_settings_root(&mut self, from: SettingRow) -> Task<GMsg> {
        let selected = self
            .settings_rows_filtered("")
            .iter()
            .position(|s| *s == from)
            .unwrap_or(0);
        if let Some(LauncherState {
            settings: Some(ls),
            input,
            ..
        }) = self.launcher_modal_mut()
        {
            ls.pane = SettingsPane::Root;
            ls.selected = selected;
            ls.resizing = false;
            ls.update_actions = None;
            input.clear();
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Expand the update-available actions strip under the Check-for-updates
    /// row (E3). From the Settings drill-in the strip simply opens in place;
    /// from a root-mode `PaletteRow::Setting` match the drill-in is opened
    /// first, landed on that row, so the strip has a row to hang under.
    /// Returns the scroll task for that landing — CheckUpdates is the last
    /// row, guaranteed below the 380px fold of a fresh drill-in.
    pub(super) fn open_update_actions_strip(&mut self) -> Task<GMsg> {
        let in_drill_in = matches!(
            &self.launcher,
            Some(LauncherState {
                settings: Some(_),
                ..
            })
        );
        if !in_drill_in {
            // Entry scroll superseded by the one returned below, once the
            // cursor has landed on the CheckUpdates row.
            let _ = self.open_settings_drill_in();
            let idx = self
                .settings_rows_filtered("")
                .iter()
                .position(|s| *s == SettingRow::CheckUpdates)
                .unwrap_or(0);
            if let Some(LauncherState {
                settings: Some(ls), ..
            }) = self.launcher_modal_mut()
            {
                ls.selected = idx;
            }
        }
        if let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        {
            ls.update_actions = Some(0);
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Collapse the update-actions strip, staying in the drill-in Root list.
    pub(in crate::gui) fn close_update_actions_strip(&mut self) {
        if let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        {
            ls.update_actions = None;
        }
    }

    /// Enter the Backend sub-pane (D2): cursor starts on the active backend.
    pub(super) fn enter_backend_pane(&mut self) -> Task<GMsg> {
        let selected = backend_pane_selected_index(self.app.use_tmux());
        self.enter_settings_pane(SettingsPane::Backend, selected);
        Task::none()
    }

    /// Enter the Permissions sub-pane (E1): cursor starts on the active
    /// choice (Ask/Skip).
    pub(super) fn enter_permissions_pane(&mut self) -> Task<GMsg> {
        let selected = permissions_pane_selected_index(self.app.skip_permissions_enabled());
        self.enter_settings_pane(SettingsPane::Permissions, selected);
        Task::none()
    }

    /// Enter the DefaultAgent sub-pane (D3): cursor starts on the current
    /// default (or `Agent::ALL[0]` if none set). Kicks off the same tool
    /// detection `settings_modal` triggers on open when it hasn't run yet,
    /// so install status/version populate instead of showing "detecting…"
    /// forever.
    pub(super) fn enter_default_agent_pane(&mut self) -> Task<GMsg> {
        let selected = default_agent_pane_selected_index(self.app.store.default_agent);
        self.enter_settings_pane(SettingsPane::DefaultAgent, selected);
        if self.settings_tools.is_empty() {
            return self.detect_tools_task();
        }
        Task::none()
    }

    /// Enter App-size inline-edit mode (D4): stays on the Root pane —
    /// `resizing` swaps the selected row's value slot for the live stepper.
    /// From a root-mode `PaletteRow::Setting` match the drill-in is opened
    /// first, landed on the App-size row, so the stepper has a row to live on
    /// (same shape as `open_update_actions_strip`).
    pub(super) fn enter_appsize_resize(&mut self) -> Task<GMsg> {
        if !matches!(
            &self.launcher,
            Some(LauncherState {
                settings: Some(_),
                ..
            })
        ) {
            // Entry scroll superseded by the one returned below, once the
            // cursor has landed on the App-size row.
            let _ = self.open_settings_drill_in();
            let idx = self
                .settings_rows_filtered("")
                .iter()
                .position(|s| *s == SettingRow::AppSize)
                .unwrap_or(0);
            if let Some(LauncherState {
                settings: Some(ls), ..
            }) = self.launcher_modal_mut()
            {
                ls.selected = idx;
            }
        }
        if let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal_mut()
        {
            ls.resizing = true;
        }
        self.scroll_launcher_settings_to_selection()
    }

    /// Scroll the Settings drill-in's Root list so the selected row is
    /// centered — the Root-pane counterpart of
    /// `scroll_launcher_theme_to_selection`, chained from every path that
    /// moves the Root cursor or rebuilds the list (↑↓, drill-in entry,
    /// sub-pane exits landing near the bottom, query edits). No-op outside
    /// the Root pane.
    pub(super) fn scroll_launcher_settings_to_selection(&self) -> Task<GMsg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Some(LauncherState {
            input,
            settings:
                Some(LauncherSettings {
                    pane: SettingsPane::Root,
                    selected,
                    ..
                }),
            ..
        }) = self.launcher_modal()
        else {
            return Task::none();
        };
        let rows = self.settings_rows_filtered(input);
        let y = settings_root_scroll_offset(&rows, *selected);
        scroll_to(
            launcher_settings_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Backend/Permissions/DefaultAgent sub-pane ⏎/click: commit row
    /// `selected` and return to Root. Shared by the three since they're all
    /// "pick one of a short fixed list, apply immediately" — only which
    /// `Msg` fires (and, for DefaultAgent, the installed-agent guard) differs.
    pub(in crate::gui) fn backend_pane_commit(&mut self, selected: usize) -> Task<GMsg> {
        self.on_set_backend_tmux(selected != 0);
        self.return_to_settings_root(SettingRow::Backend)
    }

    pub(in crate::gui) fn permissions_pane_commit(&mut self, selected: usize) -> Task<GMsg> {
        self.on_set_skip_permissions(selected != 0);
        self.return_to_settings_root(SettingRow::Permissions)
    }

    /// Whether the DefaultAgent sub-pane row for `agent` is interactable.
    /// `Terminal` is always available; while tool detection is still
    /// empty/in-flight, every agent is treated as installed-unknown (no
    /// version text, but not inert) rather than inert.
    pub(super) fn default_agent_pane_row_installed(&self, agent: Agent) -> bool {
        if agent == Agent::Terminal || self.settings_tools.is_empty() {
            return true;
        }
        self.settings_tools
            .iter()
            .find(|t| t.agent == agent)
            .map(|t| t.installed)
            .unwrap_or(true)
    }

    pub(in crate::gui) fn default_agent_pane_commit(&mut self, selected: usize) -> Task<GMsg> {
        let Some(&agent) = Agent::ALL.get(selected) else {
            return Task::none();
        };
        if !self.default_agent_pane_row_installed(agent) {
            return Task::none();
        }
        self.on_set_default_agent(agent);
        self.return_to_settings_root(SettingRow::DefaultAgent)
    }

    /// Every `SettingRow` (in `SettingRow::ALL`'s section/definition order)
    /// fuzzy-filtered by `input`, for the Settings drill-in's live list and
    /// its keyboard nav. Shares the same 3-way (label/value/section) match
    /// `palette_rows` uses for root-mode `Setting` rows, via
    /// `launcher::matching_settings`, so the same query surfaces a setting
    /// whether you're still at root or already inside the drill-in.
    pub(in crate::gui) fn settings_rows_filtered(&self, input: &str) -> Vec<SettingRow> {
        let values: Vec<String> = SettingRow::ALL
            .iter()
            .map(|s| self.setting_value(*s))
            .collect();
        let candidates: Vec<(SettingRow, &str, &str, &str)> = SettingRow::ALL
            .iter()
            .zip(values.iter())
            .map(|(s, v)| (*s, s.label(), v.as_str(), s.section()))
            .collect();
        crate::gui::launcher::matching_settings(input, &candidates)
    }

    /// Live value string for `s`, as shown right-aligned on its palette row.
    /// Cross-checked against `settings_modal`'s own value sources (view.rs)
    /// so the palette and the browse-view Settings modal never disagree.
    pub(super) fn setting_value(&self, s: SettingRow) -> String {
        match s {
            SettingRow::Theme => grove_core::theme::current().name.to_string(),
            SettingRow::AppSize => format!("{:.0}%", self.pty_layout.zoom * 100.0),
            SettingRow::ProjectThemes => {
                if self.app.project_themes_enabled() {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingRow::Backend => {
                if self.app.use_tmux() {
                    "Tmux".to_string()
                } else {
                    "Native".to_string()
                }
            }
            SettingRow::Permissions => {
                if self.app.skip_permissions_enabled() {
                    "Skip".to_string()
                } else {
                    "Ask".to_string()
                }
            }
            SettingRow::Telemetry => {
                if self.app.telemetry_enabled() {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingRow::DefaultAgent => self
                .app
                .store
                .default_agent
                .map(|a| a.label().to_string())
                .unwrap_or_else(|| "auto".to_string()),
            SettingRow::CheckUpdates => {
                let ver = env!("CARGO_PKG_VERSION");
                match &self.upgrade {
                    UpgradeState::Idle => format!("v{ver}"),
                    UpgradeState::Checking => "Checking…".to_string(),
                    UpgradeState::UpToDate => format!("v{ver} · Up to date"),
                    UpgradeState::Available(r) => format!("Update available: {}", r.tag),
                    _ => "Updating…".to_string(),
                }
            }
        }
    }
}
