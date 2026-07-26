//! The modal layer: dispatch (`modal_layer`) plus every individual
//! `*_modal` renderer (input/confirm/remove-project/settings/theme
//! picker/theme manager/shortcut overlay/teardown/changelog/...), split by
//! family:
//! - [`confirm`]       — input/confirm/remove-project/message/teardown
//! - [`settings`]      — project settings, tmux choice, agent picker, the
//!   settings modal itself, and the shortcut overlay
//! - [`upgrade`]        — update-in-progress and changelog
//! - [`theme_picker`]  — the app/project theme picker
//! - [`theme_manager`] — the custom-theme list/manage modal

mod confirm;
mod settings;
mod theme_manager;
mod theme_picker;
mod upgrade;

use crate::app::Modal;
use crate::gui::palette as c;
use crate::gui::session_launcher::LauncherState;
use crate::gui::state::{Grove, Msg};
use iced::widget::{container, Space};
use iced::{Background, Element, Length};

impl Grove {
    // ── modal layer ───────────────────────────────────────────────────────
    pub(super) fn modal_layer(&self) -> Element<'_, Msg> {
        let panel: Element<'_, Msg> = match &self.app.modal {
            Modal::Input {
                title,
                buffer,
                note,
            } => self.input_modal(title, buffer, note.as_deref()),
            Modal::Confirm {
                title,
                prompt,
                destructive,
                kind,
            } => self.confirm_modal(title, prompt, *destructive, kind),
            Modal::AddProject => crate::gui::add_project::view(self),
            Modal::RemoveProject {
                name,
                worktrees,
                also_remove_worktrees,
                in_progress,
                done,
                current,
                errors,
                ..
            } => self.remove_project_modal(
                name,
                worktrees,
                *also_remove_worktrees,
                *in_progress,
                *done,
                current,
                errors,
            ),
            Modal::Message(message) => self.message_modal(message),
            Modal::TmuxChoice => self.tmux_choice_modal(),
            Modal::AgentPicker {
                project,
                wt_path,
                sel,
            } => self.agent_picker_modal(project, wt_path, *sel),
            Modal::ThemePicker {
                sel_dark,
                sel_light,
                tab,
                follow_system,
                scope,
                project_use_default,
                ..
            } => self.theme_picker_modal(
                *sel_dark,
                *sel_light,
                *tab,
                *follow_system,
                scope.clone(),
                *project_use_default,
            ),
            Modal::Settings => self.settings_modal(),
            Modal::ThemeManager {
                selected,
                rename,
                rename_error,
                pending_delete,
            } => match &self.theme_manager_editor {
                Some(ed) => crate::gui::theme_manager_editor::view(ed),
                None => self.theme_manager_modal(
                    *selected,
                    rename.as_ref(),
                    rename_error.as_deref(),
                    pending_delete.as_deref(),
                ),
            },
            Modal::ShortcutOverlay => self.shortcut_overlay_modal(),
            Modal::Updating => self.updating_modal(),
            Modal::Teardown => self.teardown_modal(),
            Modal::ScriptsEditor => self.project_settings_modal(),
            // Onboarding never reaches the modal layer: `view()` returns
            // `onboarding_view(...)` directly while it's active (see above),
            // full-viewport with no sidebar/statusbar/scrim behind it.
            Modal::Onboarding { .. } => unreachable!("onboarding short-circuits in view()"),
            // The palette already returns a `Length::Fill` x `Length::Fill`
            // element that top-aligns itself internally (see
            // `session_launcher_modal`), so wrapping it in the shared
            // center_x/center_y container below is a no-op — it stays
            // top-dropped instead of vertically centered like every other
            // modal.
            Modal::SessionLauncher => match self.launcher_modal() {
                Some(LauncherState {
                    input,
                    selected,
                    browse_all,
                    options,
                    switch,
                    row_actions,
                    settings,
                    ..
                }) => self.session_launcher_modal(
                    input,
                    *selected,
                    *browse_all,
                    options.as_ref(),
                    *switch,
                    row_actions.as_ref(),
                    settings.as_ref(),
                ),
                None => Space::new().width(0).into(),
            },
            Modal::None => Space::new().width(0).into(),
        };

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::SCRIM())),
                ..Default::default()
            })
            .into()
    }
}
