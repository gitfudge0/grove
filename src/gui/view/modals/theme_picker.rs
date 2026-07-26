//! The app/project theme picker modal (dark/light tabs + selectable list).

use super::super::theme_picker_scrollable_id;
use crate::gui::metrics::ROW_H;
use crate::gui::palette as c;
use crate::gui::state::ThemePickerMsg;
use crate::gui::state::{Grove, Msg};
use crate::gui::widgets::{
    ghost_scrollable, modal_action, modal_checkbox, modal_header, modal_list_row, modal_panel,
    seg_button, ModalBtn, SegSide,
};
use iced::border::Radius;
use iced::widget::{column, container, row, text, Column, Space};
use iced::{Background, Border, Element, Length, Padding};

impl Grove {
    pub(super) fn theme_picker_modal(
        &self,
        sel_dark: usize,
        sel_light: usize,
        tab: grove_core::theme::ThemeKind,
        follow_system: bool,
        scope: crate::app::ThemePickerScope,
        project_use_default: bool,
    ) -> Element<'_, Msg> {
        use crate::app::ThemePickerScope;
        let is_project = matches!(scope, ThemePickerScope::Project(_));
        let themes = grove_core::theme::selectable_themes_of(tab);
        let sel = match tab {
            grove_core::theme::ThemeKind::Dark => sel_dark,
            grove_core::theme::ThemeKind::Light => sel_light,
        };

        // Same segmented control as the appbar backend switch and the sidebar
        // view switch — one vocabulary for "choose one of N".
        let tabs = container(
            row![
                seg_button(
                    "Dark",
                    matches!(tab, grove_core::theme::ThemeKind::Dark),
                    SegSide::Left,
                    Msg::ThemePicker(ThemePickerMsg::SwitchTab),
                ),
                seg_button(
                    "Light",
                    matches!(tab, grove_core::theme::ThemeKind::Light),
                    SegSide::Right,
                    Msg::ThemePicker(ThemePickerMsg::SwitchTab),
                ),
            ]
            .spacing(0),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let mut list = Column::new().spacing(0);
        if is_project {
            list = list.push(modal_list_row(
                text("Default (follow app)")
                    .size(12)
                    .color(if project_use_default {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }),
                project_use_default,
                Msg::ThemePicker(ThemePickerMsg::SelectDefault),
            ));
        }
        for (i, th) in themes.iter().enumerate() {
            let active = i == sel && !(is_project && project_use_default);
            let name = th.name.to_string();
            list = list.push(modal_list_row(
                text(name)
                    .size(12)
                    .color(if active { c::FG() } else { c::FG_DIM() }),
                active,
                Msg::ThemePicker(ThemePickerMsg::Select(i)),
            ));
        }

        let list_h = ((themes.len() + usize::from(is_project)).min(12) as f32) * ROW_H;
        let scroller = container(ghost_scrollable(list).id(theme_picker_scrollable_id()))
            .width(Length::Fill)
            .height(Length::Fixed(list_h))
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            });

        let title = match &scope {
            ThemePickerScope::App => "Theme".to_string(),
            ThemePickerScope::Project(name) => {
                // Resolve by name (projects are keyed by unique name, not a
                // stable index) — fall back gracefully if it was removed
                // while the picker was open.
                let still_exists = self.app.store.projects.iter().any(|p| &p.name == name);
                if still_exists {
                    format!("Project theme — {name}")
                } else {
                    "Project theme".to_string()
                }
            }
        };

        let mut body = column![].spacing(12);
        if !is_project {
            body = body.push(modal_checkbox(
                "Follow system appearance".into(),
                follow_system,
                c::MAGENTA(),
                Some(|v| Msg::ThemePicker(ThemePickerMsg::ToggleSystem(v))),
            ));
        }
        body = body
            .push(tabs)
            .push(scroller)
            .push(Space::new().height(8))
            .push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action(
                        "Cancel",
                        ModalBtn::Plain,
                        Msg::ThemePicker(ThemePickerMsg::Cancel)
                    ),
                    modal_action(
                        "Apply",
                        ModalBtn::Primary,
                        Msg::ThemePicker(ThemePickerMsg::Submit)
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );

        let panel_body = column![
            modal_header(&title, c::MAGENTA()),
            container(body).padding(Padding::from([16, 20])),
        ];

        modal_panel(panel_body.into(), 460.0)
    }
}
