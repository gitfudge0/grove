//! `Grove::view` and the chrome it composes (appbar, sidebar, workspace,
//! statusbar, modal layer). Pure rendering — no state mutation.

mod appbar;
mod common;
mod modals;
mod sidebar;
mod statusbar;
mod terminal;

pub(in crate::gui) use common::{
    cap, digit_label, footer_mod_hint, git_on_path, highlighted_line, input_field_style,
    launcher_palette_scrollable_id, launcher_settings_scrollable_id, launcher_theme_scrollable_id,
    mod_key_chip, modal_input_id, modal_name_id, theme_manager_scrollable_id,
    theme_picker_scrollable_id,
};

use crate::app::Modal;
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg};
use iced::widget::{column, container, row, stack};
use iced::{Background, Element, Length};

impl Grove {
    pub fn view(&self) -> Element<'_, Msg> {
        // The first-run wizard owns the entire window while active: no
        // sidebar/statusbar/scrim behind it, just its own full-viewport chrome.
        // It still goes through the shared background wrapper below, but
        // skips `body`/the modal layer entirely.
        if let Modal::Onboarding {
            step,
            path,
            dir_sel,
            name,
            note,
            agent_sel,
            perms_skip,
            ..
        } = &self.app.modal
        {
            let content = self.onboarding_view(
                *step,
                path,
                *dir_sel,
                name.as_deref(),
                note.as_deref(),
                *agent_sel,
                *perms_skip,
            );
            return container(content)
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    text_color: Some(c::FG()),
                    ..Default::default()
                })
                .into();
        }
        // The attention queue walks every project × worktree × session to
        // rebuild the tree order; three call sites used to recompute it per
        // frame. Resolve it once here and hand references down.
        let waiting = self.waiting_sessions();
        // Per-frame memo of each project's resolved PTY theme (see
        // `pty_theme_for`) — the resolution does a linear theme-registry scan
        // behind an `RwLock`, and grid view repeats it per tile.
        terminal::reset_pty_theme_cache();
        let body = if self.app.chrome_visible {
            let workspace_row: Element<'_, Msg> = if self.grid_view {
                // Grid mode: sidebar is hidden, workspace fills the full width.
                self.workspace()
            } else {
                row![
                    self.sidebar(),
                    self.sidebar_resize_handle(),
                    self.workspace()
                ]
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
            };
            column![
                self.appbar(&waiting),
                container(workspace_row)
                    .height(Length::Fill)
                    .width(Length::Fill),
                self.statusbar(),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            let workspace: Element<'_, Msg> = if waiting.is_empty() {
                self.workspace()
            } else {
                stack![self.workspace(), self.zen_attention_pill(waiting.len())]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            };
            column![workspace].width(Length::Fill).height(Length::Fill)
        };

        let show_attention_dropdown = self.attention_open && self.app.chrome_visible;
        let content: Element<'_, Msg> = if matches!(self.app.modal, Modal::None)
            && !self.show_changelog
            && !show_attention_dropdown
        {
            body.into()
        } else {
            let mut layers = stack![body];
            if !matches!(self.app.modal, Modal::None) {
                layers = layers.push(self.modal_layer());
            }
            if self.show_changelog {
                layers = layers.push(self.changelog_modal());
            }
            if show_attention_dropdown {
                layers = layers.push(self.attention_dropdown(&waiting));
            }
            layers.width(Length::Fill).height(Length::Fill).into()
        };

        container(content)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                text_color: Some(c::FG()),
                ..Default::default()
            })
            .into()
    }
}
