//! `Modal::ArchivedProjects` — the archived-projects list (Settings →
//! Archived projects). Structurally a copy of [`super::theme_manager`]'s list
//! idiom: header + close, divider, a `max_height`-capped ghost scrollable of
//! `[6, 10]`/radius-6 rows, and a fixed-width right-hand action slot so the
//! icon column never shifts.

use crate::gui::icons::icon;
use crate::gui::palette as c;
use crate::gui::state::{ArchiveMsg, Grove, Msg};
use crate::gui::widgets::{
    divider_h, ghost_scrollable, icon_btn, modal_footer_hints, modal_header_row, modal_panel,
    truncate_middle,
};
use iced::border::Radius;
use iced::widget::{button, container, row, text, Column, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

/// A 22×22 row action. `glyph`/`hover_bg` are what distinguishes restore
/// (cyan on the neutral hover fill) from delete (red on the red wash). The
/// per-mini hover comes from `button::Status` matching — the same mechanism
/// the theme manager's own minis use — since these, unlike the row itself, do
/// have a press action.
fn mini<'a>(name: &'static str, glyph: Color, hover_bg: Color, msg: Msg) -> Element<'a, Msg> {
    button(container(icon(name, 12.0, glyph)).center_x(22).center_y(22))
        .on_press(msg)
        .padding(0)
        .style(move |_, status| button::Style {
            background: match status {
                button::Status::Hovered | button::Status::Pressed => {
                    Some(Background::Color(hover_bg))
                }
                _ => None,
            },
            text_color: glyph,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
        })
        .into()
}

impl Grove {
    pub(super) fn archived_projects_modal(&self) -> Element<'_, Msg> {
        let empty = self.app.store.archived_count() == 0;

        let body_content: Element<'_, Msg> = if empty {
            // Same in-list empty idiom as the theme manager / launcher setting
            // panes: centered muted line, full width, generous vertical pad.
            container(text("No archived projects.").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(4);
            // TRUE indices straight from the store: never re-`enumerate()` a
            // filtered sequence, or restore/delete hit the wrong project.
            for (idx, p) in self.app.store.archived_projects() {
                let name_zone = container(text(p.name.clone()).size(13).color(c::FG()))
                    .width(Length::Fill)
                    .clip(true);
                let path_zone = container(
                    text(truncate_middle(&p.path, 44))
                        .size(11)
                        .color(c::FG_MUTE())
                        .font(iced::Font::MONOSPACE),
                )
                .width(Length::Fill)
                .clip(true);
                // Fixed-width slot: both actions are always present, so the
                // path column never reflows between rows or on hover.
                let slot = container(
                    row![
                        mini(
                            "restore",
                            c::CYAN(),
                            c::BG_HOVER(),
                            Msg::Archive(ArchiveMsg::Restore(idx)),
                        ),
                        mini(
                            "trash",
                            c::RED(),
                            c::RED_WASH(),
                            Msg::Archive(ArchiveMsg::Delete(idx)),
                        ),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center),
                )
                .width(Length::Fixed(48.0));

                // The row has no press action of its own — the only actions are
                // the two explicit minis, so a stray click on the row must not
                // restore or delete anything. An `on_press`-less Iced button
                // reports `Status::Disabled`, never `Hovered`, so the row fill
                // comes from the same descendant-hover mechanism the sidebar's
                // worktree rows use (`mouse_area` → `hovered_archived`) rather
                // than from button-status matching.
                let hovered = self.hovered_archived == Some(idx);
                list = list.push(
                    iced::widget::mouse_area(
                        container(
                            row![name_zone, path_zone, slot]
                                .spacing(10)
                                .align_y(iced::Alignment::Center),
                        )
                        .width(Length::Fill)
                        .padding(Padding::from([6, 10]))
                        .style(move |_| container::Style {
                            background: hovered.then(|| Background::Color(c::BG_HL())),
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: Radius::from(6.0),
                            },
                            ..Default::default()
                        }),
                    )
                    .on_enter(Msg::Archive(ArchiveMsg::Hover(Some(idx))))
                    .on_exit(Msg::Archive(ArchiveMsg::Hover(None))),
                );
            }
            container(ghost_scrollable(list).height(Length::Shrink))
                .max_height(360.0)
                .width(Length::Fill)
                .into()
        };

        let header = modal_header_row(
            row![
                text("Archived projects").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                icon_btn("close", Msg::Archive(ArchiveMsg::CloseList)),
            ]
            .align_y(iced::Alignment::Center)
            .into(),
        );

        // Esc is the only key this modal binds in either state: restore and
        // delete are per-row mouse actions, so there is no keyboard selection
        // to advertise. Claiming "↑↓ select" here would be a hint for a
        // binding that does nothing.
        let footer = modal_footer_hints(&[("esc", "close")]);

        let body = iced::widget::column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body_content).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            footer,
        ];

        modal_panel(body.into(), 560.0)
    }
}
