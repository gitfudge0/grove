//! The appbar: brand mark, agent-view toggle, attention-queue pill and its
//! dropdown, cog entry point into Settings.

use crate::gui::icons::icon;
use crate::gui::metrics::{APPBAR_H, MONO_FONT, UI_BOLD, UI_FONT};
use crate::gui::palette as c;
use crate::gui::rows::state_glyph;
use crate::gui::session_launcher;
use crate::gui::state::{Grove, Msg, UpgradeState};
use crate::gui::update::platform_mod_label;
use crate::gui::widgets::{divider_h, dot, icon_btn};
use iced::border::Radius;
use iced::widget::{button, column, container, row, stack, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

impl Grove {
    // ── appbar ────────────────────────────────────────────────────────────
    /// `waiting` is the attention queue, resolved once per `view()` (see
    /// `Grove::view`) — recomputing it here walked the whole tree again.
    pub(super) fn appbar<'a>(&'a self, waiting: &[usize]) -> Element<'a, Msg> {
        let brand = row![text("grove").font(UI_BOLD).size(14).color(c::MAGENTA()),]
            .spacing(8)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        // App size, theme, and terminal backend now live in the Settings modal;
        // the appbar's right cluster is just the cog entry point.
        let cog = icon_btn("cog", Msg::OpenSettings);
        let cog: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Available(_)) {
            stack![
                cog,
                container(dot(c::GREEN()))
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .into()
        } else {
            cog
        };
        // Agent-view toggle. In agent (grid) view it grows a "+" session-launcher
        // segment on its left, forming a single segmented combo; on every other
        // screen it is a lone muted button. The combo replaces the floating "+"
        // FAB that used to hover over the grid.
        let view_control: Element<'_, Msg> = if self.grid_view {
            let plus = button(
                container(icon("plus", 13.0, c::MAGENTA()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::SessionLauncher(session_launcher::Msg::Open))
            .padding(0)
            .style(|_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: c::MAGENTA(),
                // Round the left corners only — the right edge butts the grid seg.
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 4.0,
                        top_right: 0.0,
                        bottom_right: 0.0,
                        bottom_left: 4.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            let grid_seg = button(
                container(icon("grid", 13.0, c::CYAN()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(|_, status| button::Style {
                background: Some(Background::Color(
                    if matches!(status, button::Status::Hovered) {
                        c::BG_HOVER()
                    } else {
                        c::BG_HL()
                    },
                )),
                text_color: c::CYAN(),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 0.0,
                        top_right: 4.0,
                        bottom_right: 4.0,
                        bottom_left: 0.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            // A short, fixed-height hairline between the segments. Using a
            // Fill-height divider here would inherit the appbar's full height and
            // stretch the combo taller than the lone toggle button.
            let seg_divider = container(Space::new().width(1))
                .width(1)
                .height(Length::Fixed(14.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BORDER())),
                    ..Default::default()
                });
            container(row![plus, seg_divider, grid_seg].align_y(iced::Alignment::Center))
                .style(|_| container::Style {
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(5.0),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            let grid_color = c::FG_MUTE();
            button(
                container(icon("grid", 13.0, grid_color))
                    .center_x(22)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(move |_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: grid_color,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            })
            .into()
        };

        // Attention-queue pill: only rendered while at least one session is
        // waiting for input. Pulses in sync with the grid tile's amber accent
        // (see `attention_pulse`) and toggles the dropdown on click.
        let attention_pill: Option<Element<'_, Msg>> = if waiting.is_empty() {
            None
        } else {
            let dot_alpha = 1.0 - 0.4 * self.attention_pulse();
            let dot_color = Color {
                a: dot_alpha,
                ..c::AMBER()
            };
            let label = if waiting.len() == 1 {
                "1 needs you".to_string()
            } else {
                format!("{} need you", waiting.len())
            };
            let content = row![
                dot(dot_color),
                text(label).font(UI_FONT).size(11).color(c::AMBER()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            Some(
                button(
                    container(content)
                        .padding(Padding::from([4, 10]))
                        .style(|_| container::Style {
                            background: Some(Background::Color(Color {
                                a: 0.08,
                                ..c::AMBER()
                            })),
                            border: Border {
                                color: c::AMBER(),
                                width: 1.0,
                                radius: Radius::from(999.0),
                            },
                            ..Default::default()
                        }),
                )
                .on_press(Msg::ToggleAttentionQueue)
                .padding(0)
                .style(|_, status| button::Style {
                    background: if matches!(status, button::Status::Hovered) {
                        Some(Background::Color(Color {
                            a: 0.14,
                            ..c::AMBER()
                        }))
                    } else {
                        None
                    },
                    text_color: c::AMBER(),
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: false,
                })
                .into(),
            )
        };

        let mut right = row![view_control];
        if let Some(pill) = attention_pill {
            right = right.push(pill);
        }
        let right = right
            .push(cog)
            .spacing(4)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(self.sidebar_width),
            Space::new().width(Length::Fill),
            right,
        ]
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        let bar = container(inner)
            .height(APPBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar, divider_h(c::BORDER())].into()
    }

    /// Small floating badge shown top-right over the zen workspace while at
    /// least one session waits for input — chrome (and thus the appbar pill)
    /// is hidden in zen, so this is the only always-visible attention signal
    /// there. Clicking it jumps straight to the first waiting session; it is
    /// not a dropdown, so no backdrop/dismiss handling is needed.
    pub(super) fn zen_attention_pill(&self, count: usize) -> Element<'_, Msg> {
        let dot_alpha = 1.0 - 0.4 * self.attention_pulse();
        let dot_color = Color {
            a: dot_alpha,
            ..c::AMBER()
        };
        let content = row![
            dot(dot_color),
            text(count.to_string())
                .font(UI_FONT)
                .size(11)
                .color(c::AMBER()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let pill = button(
            container(content)
                .padding(Padding::from([2, 8]))
                .style(|_| container::Style {
                    background: Some(Background::Color(Color {
                        a: 0.08,
                        ..c::AMBER()
                    })),
                    border: Border {
                        color: c::AMBER(),
                        width: 1.0,
                        radius: Radius::from(999.0),
                    },
                    ..Default::default()
                }),
        )
        .on_press(Msg::JumpToWaitingSession)
        .padding(0)
        .style(|_, status| button::Style {
            background: if matches!(status, button::Status::Hovered) {
                Some(Background::Color(Color {
                    a: 0.14,
                    ..c::AMBER()
                }))
            } else {
                None
            },
            text_color: c::AMBER(),
            border: Border::default(),
            shadow: Shadow::default(),
            snap: false,
        });

        column![
            Space::new().height(12.0),
            row![Space::new().width(Length::Fill), pill].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 12.0,
            }),
            Space::new().height(Length::Fill),
        ]
        .height(Length::Fill)
        .into()
    }

    /// Anchored top-right dropdown listing every session currently waiting
    /// for input, opened via the appbar pill (`Msg::ToggleAttentionQueue`).
    /// Same backdrop-dismiss idiom as `sidebar_agent_menu_overlay`.
    pub(super) fn attention_dropdown<'a>(&'a self, waiting: &[usize]) -> Element<'a, Msg> {
        let backdrop = button(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(Msg::CloseAttentionQueue)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(0)
            .style(|_, _| button::Style {
                background: None,
                text_color: Color::TRANSPARENT,
                border: Border::default(),
                shadow: Shadow::default(),
                snap: false,
            });

        let mut rows_col = column![].spacing(0);
        for &si in waiting {
            let s = &self.app.sessions[si];
            let state = self.activity_state(s);
            let subtitle = format!("{} / {}", s.project, crate::app::path_basename(&s.wt_path));
            let content = row![
                state_glyph(state, self.anim.blink_tick, self.attention_pulse()),
                column![
                    text(s.agent.label()).font(UI_FONT).size(11).color(c::FG()),
                    text(subtitle).font(MONO_FONT).size(10).color(c::FG_MUTE()),
                ]
                .spacing(1),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .padding(Padding {
                top: 6.0,
                bottom: 6.0,
                left: 12.0,
                right: 10.0,
            });

            let row_btn = button(content)
                .on_press(Msg::SelectSession(si))
                .width(Length::Fill)
                .padding(0)
                .style(|_, status| button::Style {
                    background: if matches!(status, button::Status::Hovered) {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: c::FG(),
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: false,
                });

            // 3px amber left accent bar, same idiom as the waiting sidebar row.
            let bar: Element<'_, Msg> = container(
                container(Space::new().width(3.0))
                    .width(3.0)
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::AMBER())),
                        ..Default::default()
                    }),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Start)
            .into();

            rows_col = rows_col.push(stack![row_btn, bar]);
        }

        let footer_hint: Element<'_, Msg> = if cfg!(target_os = "macos") {
            row![
                icon("command", 10.0, c::FG_MUTE()),
                text("'").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(" jump to next")
                    .font(UI_FONT)
                    .size(10)
                    .color(c::FG_MUTE()),
            ]
            .spacing(1)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            text(format!("{}+' jump to next", platform_mod_label()))
                .font(UI_FONT)
                .size(10)
                .color(c::FG_MUTE())
                .into()
        };
        let footer = container(footer_hint).width(Length::Fill).padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 12.0,
            right: 10.0,
        });

        let panel = container(
            column![rows_col, divider_h(c::BORDER()), footer]
                .spacing(0)
                .width(Length::Fixed(280.0)),
        )
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let positioned = column![
            Space::new().height(APPBAR_H + 1.0),
            row![Space::new().width(Length::Fill), panel].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 16.0,
            }),
            Space::new().height(Length::Fill),
        ]
        .height(Length::Fill);

        stack![backdrop, positioned]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
