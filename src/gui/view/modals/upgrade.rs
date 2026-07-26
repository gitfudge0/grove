//! Update-in-progress modal and the changelog viewer — the two modals tied
//! to the self-upgrade flow.

use crate::gui::metrics::UI_BOLD;
use crate::gui::palette as c;
use crate::gui::state::UpgradeMsg;
use crate::gui::state::{Grove, Msg, UpgradeState};
use crate::gui::widgets::{
    divider_h, ghost_scrollable, icon_btn, modal_action, modal_footer_hints, modal_header,
    modal_header_row, modal_panel,
};
use iced::widget::{column, container, row, text, Column, Space};
use iced::{Background, Element, Length, Padding};

impl Grove {
    pub(super) fn updating_modal(&self) -> Element<'_, Msg> {
        use iced::Alignment::Center;

        let header = modal_header("Updating Grove", c::MAGENTA());

        // Keys are blocked while the update is in flight (see
        // `Modal::Updating` in update.rs), so no footer hint appears then;
        // once it lands on Updated/Failed, Esc is wired to dismiss.
        let footer = match &self.upgrade {
            UpgradeState::Updating(_) => None,
            UpgradeState::Updated => Some(modal_footer_hints(&[("esc", "later")])),
            UpgradeState::UpdateFailed(_) => Some(modal_footer_hints(&[("esc", "close")])),
            _ => None,
        };

        let body: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Updating(stage) => {
                let label = match stage {
                    grove_core::upgrade::Stage::Downloading => "Downloading…",
                    grove_core::upgrade::Stage::Building => "Building…",
                    grove_core::upgrade::Stage::Installing => "Installing…",
                    grove_core::upgrade::Stage::Done => "Finishing…",
                };
                row![
                    crate::gui::icons::spinner(16.0, c::FG_DIM(), self.anim.blink_tick),
                    Space::new().width(10),
                    text(label).size(12).color(c::FG()),
                ]
                .align_y(Center)
                .into()
            }
            UpgradeState::Updated => column![
                text("Update installed. Restart Grove to apply")
                    .size(12)
                    .color(c::FG()),
                Space::new().height(10),
                row![
                    modal_action(
                        "Restart",
                        crate::gui::widgets::ModalBtn::Primary,
                        Msg::Upgrade(UpgradeMsg::RestartApp)
                    ),
                    Space::new().width(8),
                    modal_action(
                        "Later",
                        crate::gui::widgets::ModalBtn::Plain,
                        Msg::ModalCancel
                    ),
                ]
                .align_y(Center),
            ]
            .into(),
            UpgradeState::UpdateFailed(e) => column![
                text("Update failed").size(12).color(c::FG()),
                Space::new().height(6),
                text(e.clone()).size(11).color(c::FG_MUTE()),
                Space::new().height(10),
                modal_action(
                    "Close",
                    crate::gui::widgets::ModalBtn::Plain,
                    Msg::ModalCancel
                ),
            ]
            .into(),
            _ => text("Updating…").size(12).color(c::FG_DIM()).into(),
        };

        let mut content = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if let Some(footer) = footer {
            content = content.push(divider_h(c::BORDER_SOFT())).push(footer);
        }
        modal_panel(content.into(), 420.0)
    }

    /// Consumed from `super::super` (`view/mod.rs`), not just this
    /// directory: the settings modal's "View changelog" action and the
    /// appbar both reach `Grove::changelog_modal` directly, so this needs to
    /// stay visible one level above `pub(super)`.
    pub(in crate::gui::view) fn changelog_modal(&self) -> Element<'_, Msg> {
        use crate::gui::state::ChangelogState;
        use iced::Alignment::Center;

        let header = modal_header_row(
            row![
                text("Changelog").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                icon_btn("close", Msg::Upgrade(UpgradeMsg::CloseChangelog)),
            ]
            .align_y(Center)
            .into(),
        );

        let inner: Element<'_, Msg> = match &self.changelog {
            ChangelogState::Idle | ChangelogState::Loading => row![
                crate::gui::icons::spinner(16.0, c::FG_DIM(), self.anim.blink_tick),
                Space::new().width(10),
                text("Loading\u{2026}").size(12).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            ChangelogState::Error(e) => text(format!("Couldn't load changelog: {e}"))
                .size(12)
                .color(c::FG_MUTE())
                .into(),
            ChangelogState::Loaded(notes) if notes.is_empty() => {
                text("No releases yet.").size(12).color(c::FG_MUTE()).into()
            }
            ChangelogState::Loaded(notes) => {
                let mut list = Column::new().spacing(18);
                for n in notes {
                    let mut head = row![text(n.tag.clone()).size(13).font(UI_BOLD).color(c::FG()),]
                        .spacing(8)
                        .align_y(Center);
                    if !n.name.is_empty() && n.name != n.tag {
                        head = head.push(text(n.name.clone()).size(13).color(c::FG_DIM()));
                    }
                    if !n.date.is_empty() {
                        head = head.push(Space::new().width(Length::Fill));
                        head = head.push(text(n.date.clone()).size(11).color(c::FG_MUTE()));
                    }
                    let body_text = grove_core::upgrade::clean_markdown(&n.body);
                    let entry = column![
                        head,
                        Space::new().height(4),
                        text(body_text).size(12).color(c::FG_MUTE()),
                    ]
                    .spacing(0);
                    list = list.push(entry);
                }
                ghost_scrollable(container(list))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        };

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(inner)
                .width(Length::Fill)
                .height(Length::Fixed(420.0))
                .padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ]
        .spacing(0);

        let panel = modal_panel(body.into(), 600.0);

        // Centered overlay on a dim backdrop, matching the settings modal.
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
