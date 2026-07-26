//! Custom-theme list/manage modal (`Modal::ThemeManager`'s LIST stage) plus
//! its per-row swatch-strip preview helper.

use super::super::{input_field_style, theme_manager_scrollable_id};
use crate::gui::palette as c;
use crate::gui::state::ThemeManagerMsg;
use crate::gui::state::{Grove, Msg};
use crate::gui::widgets::{
    action_mini, action_mini_danger, divider_h, ghost_scrollable, icon_btn, modal_action,
    modal_action_sized, modal_footer_hints, modal_header, modal_header_row, modal_panel, ModalBtn,
};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, text_input, Column, Row, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

impl Grove {
    /// A small 11-swatch strip previewing a theme's whole palette, in
    /// `theme::FIELD_NAMES` order — used by `Modal::ThemeManager`'s rows.
    fn theme_swatch_strip<'a>(theme: &grove_core::theme::Theme) -> Element<'a, Msg> {
        let mut strip = Row::new().spacing(2);
        for i in 0..grove_core::theme::FIELD_NAMES.len() {
            let color = c::ic(theme.field(i));
            strip = strip.push(
                container(Space::new().width(10).height(10)).style(move |_| container::Style {
                    background: Some(Background::Color(color)),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(2.0),
                    },
                    ..Default::default()
                }),
            );
        }
        strip.into()
    }

    /// `Modal::ThemeManager`'s LIST view (Stage A): every custom theme as a
    /// row (name, kind badge, 11-color swatch strip) with per-row Rename/
    /// Duplicate/Delete actions, plus a global "New theme" action. Mirrors
    /// `theme_picker_modal`'s structure/styling. The paste-first editor view
    /// is a later stage — for now nothing here opens it.
    pub(super) fn theme_manager_modal<'a>(
        &'a self,
        selected: usize,
        rename: Option<&'a (String, String)>,
        rename_error: Option<&'a str>,
        pending_delete: Option<&'a str>,
    ) -> Element<'a, Msg> {
        // A pending delete swaps the whole panel for a `confirm_modal`-shaped
        // dialog (header/body/footer) rather than an inline row — matching
        // how every other destructive confirmation in the app looks
        // (`confirm_modal`, `remove_project_modal`), not a bespoke inline
        // treatment.
        if let Some(name) = pending_delete {
            let body_zone = column![
                text(format!("Delete theme \"{name}\"?"))
                    .size(13)
                    .color(c::FG_DIM())
                    .wrapping(iced::widget::text::Wrapping::Word),
                text("This cannot be undone.").size(11).color(c::FG_MUTE()),
                Space::new().height(4),
                row![
                    Space::new().width(Length::Fill),
                    modal_action(
                        "Cancel",
                        ModalBtn::Plain,
                        Msg::ThemeManager(ThemeManagerMsg::DeleteCancel)
                    ),
                    modal_action(
                        "Delete",
                        ModalBtn::Danger,
                        Msg::ThemeManager(ThemeManagerMsg::DeleteConfirm)
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(8);
            let body = column![
                modal_header("Delete theme", c::RED()),
                divider_h(c::BORDER_SOFT()),
                container(body_zone).padding(Padding::from([14, 16])),
                divider_h(c::BORDER_SOFT()),
                modal_footer_hints(&[("y", "delete"), ("esc", "cancel")]),
            ];
            return modal_panel(body.into(), 420.0);
        }

        let themes = grove_core::theme::all_custom_themes();

        let body_content: Element<'a, Msg> = if themes.is_empty() {
            container(
                text("No custom themes yet — create one or paste a palette.")
                    .size(12)
                    .color(c::FG_MUTE()),
            )
            .padding(Padding::from([30, 16]))
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
        } else {
            let mut list = Column::new().spacing(4);
            for (i, t) in themes.iter().enumerate() {
                let active = i == selected;
                let renaming = rename.map(|(orig, _)| orig.as_str()) == Some(t.name.as_ref());
                let row_el: Element<'a, Msg> = if renaming {
                    let buf = rename.map_or("", |(_, b)| b.as_str());
                    let mut col = column![row![
                        text_input("theme name", buf)
                            .on_input(|v| Msg::ThemeManager(ThemeManagerMsg::RenameChanged(v)))
                            .on_submit(Msg::ThemeManager(ThemeManagerMsg::RenameSubmit))
                            .style(input_field_style)
                            .size(13)
                            .width(Length::Fill),
                        modal_action_sized(
                            "Save",
                            ModalBtn::Primary,
                            11,
                            Msg::ThemeManager(ThemeManagerMsg::RenameSubmit)
                        ),
                        modal_action_sized(
                            "Cancel",
                            ModalBtn::Plain,
                            11,
                            Msg::ThemeManager(ThemeManagerMsg::RenameCancel)
                        ),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)]
                    .spacing(4);
                    if let Some(err) = rename_error {
                        col = col.push(text(err).size(11).color(c::RED()));
                    }
                    container(col)
                        .width(Length::Fill)
                        .padding(Padding::from([6, 10]))
                        .style(|_| container::Style {
                            background: Some(Background::Color(c::SEL_TINT_STRONG())),
                            border: Border {
                                color: c::SEL_RING(),
                                width: 1.0,
                                radius: Radius::from(6.0),
                            },
                            ..Default::default()
                        })
                        .into()
                } else {
                    let badge = container(
                        text(match t.kind {
                            grove_core::theme::ThemeKind::Dark => "DARK",
                            grove_core::theme::ThemeKind::Light => "LIGHT",
                        })
                        .size(9)
                        .color(c::FG_MUTE()),
                    )
                    .padding(Padding::from([1, 5]))
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::BG_HL())),
                        border: Border {
                            color: c::BORDER(),
                            width: 1.0,
                            radius: Radius::from(4.0),
                        },
                        ..Default::default()
                    });
                    // Name + badge live in a `Fill`, clipped zone so a long
                    // theme name truncates instead of pushing the icon
                    // cluster past the modal's edge (the overflow the text
                    // buttons used to cause); the swatch strip gets its own
                    // capped/clipped width for the same reason, then the
                    // icon cluster keeps its natural (`Shrink`) size so it
                    // never clips.
                    let name_zone = container(
                        row![
                            text(t.name.to_string()).size(13).color(if active {
                                c::FG()
                            } else {
                                c::FG_DIM()
                            }),
                            badge,
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .clip(true);
                    let swatch_zone = container(Self::theme_swatch_strip(t))
                        .width(Length::Fixed(90.0))
                        .clip(true);
                    let icons = row![
                        Self::hint(
                            action_mini(
                                "edit",
                                Msg::ThemeManager(ThemeManagerMsg::Editor(
                                    crate::gui::theme_manager_editor::Msg::Edit(i),
                                )),
                            ),
                            "edit",
                        ),
                        Self::hint(
                            action_mini(
                                "rename",
                                Msg::ThemeManager(ThemeManagerMsg::RenameStart(i))
                            ),
                            "rename"
                        ),
                        Self::hint(
                            action_mini(
                                "duplicate",
                                Msg::ThemeManager(ThemeManagerMsg::Duplicate(i))
                            ),
                            "duplicate"
                        ),
                        Self::hint(
                            action_mini_danger(
                                "trash",
                                Msg::ThemeManager(ThemeManagerMsg::DeleteStart(i))
                            ),
                            "delete"
                        ),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center);
                    container(
                        row![name_zone, swatch_zone, icons]
                            .spacing(10)
                            .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding(Padding::from([6, 10]))
                    .style(move |_| container::Style {
                        background: Some(Background::Color(if active {
                            c::BG_HL()
                        } else {
                            Color::TRANSPARENT
                        })),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    })
                    .into()
                };
                list = list.push(
                    button(row_el)
                        .on_press(Msg::ThemeManager(ThemeManagerMsg::Select(i)))
                        .padding(0)
                        .style(|_, _| button::Style {
                            background: None,
                            text_color: c::FG(),
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: Radius::from(6.0),
                            },
                            ..Default::default()
                        }),
                );
            }
            container(
                ghost_scrollable(list)
                    .id(theme_manager_scrollable_id())
                    .height(Length::Shrink),
            )
            .max_height(360.0)
            .width(Length::Fill)
            .into()
        };

        // Header: bare title + a close icon button — same shape as the
        // Settings modal's header (`icon_btn("close", …)`), since like
        // Settings this list has no unsaved state of its own (every row
        // action persists immediately) rather than the Cancel/Save modals'
        // header-with-step-counter shape.
        let header = modal_header_row(
            row![
                text("Manage themes").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                icon_btn("close", Msg::ThemeManager(ThemeManagerMsg::Close)),
            ]
            .align_y(iced::Alignment::Center)
            .into(),
        );

        // "+ New theme" is a body-level action row (mirrors the scripts
        // editor's/confirm dialogs' own trailing action rows), not a header
        // control.
        let body_zone = column![
            row![
                Space::new().width(Length::Fill),
                modal_action(
                    "+ New theme",
                    ModalBtn::Primary,
                    Msg::ThemeManager(ThemeManagerMsg::New)
                ),
            ]
            .align_y(iced::Alignment::Center),
            body_content,
        ]
        .spacing(10);

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("↑↓", "select"), ("esc", "close")]),
        ];

        modal_panel(body.into(), 560.0)
    }
}
