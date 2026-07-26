use super::scroll_to;
use crate::gui::state::{Grove, Msg, ThemePickerMsg};
use iced::Task;

impl Grove {
    /// Theme-picker modal family dispatch (`Msg::ThemePicker`).
    pub(super) fn on_theme_picker(&mut self, msg: ThemePickerMsg) -> Task<Msg> {
        match msg {
            ThemePickerMsg::Open => {
                // The only entry point now is the Settings Appearance section,
                // so the picker always returns to Settings when closed.
                self.app.open_theme_picker(true);
                return self.scroll_theme_picker_to_selection();
            }
            ThemePickerMsg::SwitchTab => {
                self.theme_picker_switch_tab();
                return self.scroll_theme_picker_to_selection();
            }
            ThemePickerMsg::Select(i) => {
                self.theme_picker_select(i);
                return self.scroll_theme_picker_to_selection();
            }
            ThemePickerMsg::SelectDefault => {
                self.app.theme_picker_select_default();
                // Preview: this project's visible PTYs must immediately switch
                // to showing the global theme.
                self.invalidate_pty_render_cache();
            }
            ThemePickerMsg::ToggleSystem(enabled) => self.theme_picker_toggle_system(enabled),
            ThemePickerMsg::Submit => self.theme_picker_submit(),
            ThemePickerMsg::Cancel => self.theme_picker_cancel(),
        }
        Task::none()
    }

    pub(super) fn scroll_theme_picker_to_selection(&self) -> Task<Msg> {
        use super::super::metrics::ROW_H;
        use crate::app::Modal;
        use iced::widget::scrollable::AbsoluteOffset;
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &self.app.modal
        else {
            return Task::none();
        };
        let sel = match tab {
            grove_core::theme::ThemeKind::Dark => *sel_dark,
            grove_core::theme::ThemeKind::Light => *sel_light,
        };
        let total = grove_core::theme::selectable_themes_of(*tab).len();
        let viewport_rows = total.min(12) as f32;
        let viewport_h = viewport_rows * ROW_H;
        let sel_y = sel as f32 * ROW_H;
        // Center the selection in the viewport, clamped to valid range.
        let max_y = (total as f32 * ROW_H - viewport_h).max(0.0);
        let y = (sel_y - (viewport_h - ROW_H) / 2.0).clamp(0.0, max_y);
        scroll_to(
            super::super::view::theme_picker_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    pub(super) fn on_system_theme_changed(&mut self, mode: iced::theme::Mode) {
        self.app.system_theme_mode = mode;
        // Re-resolve immediately if the persisted setting follows the
        // OS, or if the theme picker is open with the "follow
        // system" checkbox previewed-but-not-yet-submitted — otherwise
        // an OS appearance change mid-preview would silently freeze
        // the preview at the mode captured when the checkbox was
        // ticked.
        let previewing_system = matches!(
            self.app.modal,
            crate::app::Modal::ThemePicker {
                follow_system: true,
                ..
            }
        );
        if self.app.theme_follow_system {
            self.app.apply_system_theme();
            self.invalidate_pty_render_cache();
        } else if previewing_system {
            // Not yet submitted, so `apply_system_theme` (gated on
            // `theme_follow_system`) would no-op — resolve directly.
            let name = self
                .app
                .resolve_system_theme_name(self.app.system_theme_mode)
                .to_string();
            grove_core::theme::set_by_name(&name);
            self.invalidate_pty_render_cache();
        }
    }

    pub(super) fn theme_picker_select(&mut self, index: usize) {
        use crate::app::{Modal, ThemePickerScope};
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            follow_system,
            scope,
            project_use_default,
            ..
        } = &mut self.app.modal
        else {
            return;
        };
        let themes = grove_core::theme::selectable_themes_of(*tab);
        if index >= themes.len() {
            return;
        }
        match tab {
            grove_core::theme::ThemeKind::Dark => *sel_dark = index,
            grove_core::theme::ThemeKind::Light => *sel_light = index,
        }
        match scope {
            ThemePickerScope::App => {
                // Picking a concrete theme from the list opts back out of "system".
                *follow_system = false;
                grove_core::theme::set(themes[index].clone());
            }
            ThemePickerScope::Project(_) => {
                // Project scope never previews into the global active theme.
                *project_use_default = false;
            }
        }
        self.invalidate_pty_render_cache();
    }

    /// Toggle the theme picker's "follow system appearance" checkbox and
    /// preview the result immediately: checking it previews the resolved
    /// system theme; unchecking it restores the current tab's selection.
    pub(super) fn theme_picker_toggle_system(&mut self, enabled: bool) {
        use crate::app::Modal;
        let Modal::ThemePicker { follow_system, .. } = &mut self.app.modal else {
            return;
        };
        *follow_system = enabled;
        if enabled {
            let name = self
                .app
                .resolve_system_theme_name(self.app.system_theme_mode)
                .to_string();
            grove_core::theme::set_by_name(&name);
        } else if let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &self.app.modal
        {
            let themes = grove_core::theme::selectable_themes_of(*tab);
            let sel = match tab {
                grove_core::theme::ThemeKind::Dark => *sel_dark,
                grove_core::theme::ThemeKind::Light => *sel_light,
            };
            if let Some(t) = themes.get(sel) {
                grove_core::theme::set(t.clone());
            }
        }
        self.invalidate_pty_render_cache();
    }

    pub(super) fn theme_picker_move(&mut self, delta: i32) {
        self.app.theme_picker_move(delta);
        self.invalidate_pty_render_cache();
    }

    pub(super) fn theme_picker_switch_tab(&mut self) {
        self.app.theme_picker_switch_tab();
        self.invalidate_pty_render_cache();
    }

    pub(super) fn theme_picker_submit(&mut self) {
        if let Err(e) = self.app.theme_picker_submit() {
            self.set_modal(crate::app::Modal::Message(format!("Theme failed: {e}")));
        }
        self.invalidate_pty_render_cache();
    }

    pub(super) fn theme_picker_cancel(&mut self) {
        self.app.theme_picker_cancel();
        self.invalidate_pty_render_cache();
    }
}
