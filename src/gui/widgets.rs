//! Small, reusable widget primitives — dots, dividers, buttons, hint pills,
//! and the modal panel chrome. None of these hold view state; they take the
//! data they need and return an `Element<Msg>`.

use super::icons::icon;
use super::metrics::{MONO_FONT, ROW_H};
use super::palette as c;
use super::state::{Msg, SplitStartSegment};
use crate::agent::Agent;
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Theme};

pub fn dot<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_width(7))
        .width(7)
        .height(7)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color,
                width: 0.0,
                radius: Radius::from(3.5),
            },
            ..Default::default()
        })
        .into()
}

pub fn vline<'a>() -> Element<'a, Msg> {
    container(Space::with_width(1))
        .width(1)
        .height(18)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BORDER)),
            ..Default::default()
        })
        .into()
}

pub fn divider_h<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_height(1))
        .width(Length::Fill)
        .height(1)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

pub fn divider_v<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_width(1))
        .width(1)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

pub fn seg_button<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label.to_string()).size(11))
        .on_press(msg)
        .padding(Padding::from([4, 10]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if active {
                    Some(Background::Color(c::BG_HL))
                } else if hovered {
                    Some(Background::Color(c::BG_HOVER))
                } else {
                    None
                },
                text_color: if active { c::FG } else { c::FG_DIM },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
        .into()
}

pub fn icon_btn<'a>(name: &'static str, msg: Msg) -> Element<'a, Msg> {
    button(
        container(icon(name, 15.0, c::FG_DIM))
            .center_x(28)
            .center_y(28),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
            text_color: c::FG_DIM,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(5.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn split_start_button<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    let launch = button(
        container(icon("play", 9.0, c::GREEN))
            .center_x(28)
            .center_y(22),
    )
    .on_press(Msg::StartSession {
        proj,
        wt,
        agent: Agent::Claude,
    })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Left));

    let terminal = button(
        container(icon("term", 12.0, c::FG_MUTE))
            .center_x(28)
            .center_y(22),
    )
    .on_press(Msg::StartTerminal { proj, wt })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Middle));

    let menu = button(
        container(icon("more", 12.0, c::FG_MUTE))
            .center_x(22)
            .center_y(22),
    )
    .on_press(Msg::ToggleAgentMenu { proj, wt })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Right));

    row![launch, terminal, menu]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
}

pub fn delete_worktree_button<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    button(
        container(icon("trash", 11.0, c::RED))
            .center_x(24)
            .center_y(22),
    )
    .on_press(Msg::DeleteWorktree { proj, wt })
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if hovered {
                c::BG_HOVER
            } else {
                c::BG
            })),
            text_color: c::RED,
            border: Border {
                color: if hovered { c::RED } else { c::BORDER },
                width: 1.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn split_start_style(
    segment: SplitStartSegment,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let radius = match segment {
            SplitStartSegment::Left => Radius::default().left(4.0),
            SplitStartSegment::Middle => Radius::default(),
            SplitStartSegment::Right => Radius::default().right(4.0),
        };
        button::Style {
            background: Some(Background::Color(if hovered { c::BG_HOVER } else { c::BG })),
            text_color: if hovered { c::FG } else { c::FG_DIM },
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius,
            },
            shadow: Shadow::default(),
        }
    }
}

pub fn sidebar_agent_menu_overlay<'a>(proj: usize, wt: usize, top: f32) -> Element<'a, Msg> {
    container(
        column![
            Space::with_height(top),
            row![Space::with_width(Length::Fill), agent_menu(proj, wt)].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 8.0,
            }),
            Space::with_height(Length::Fill),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn agent_menu<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    let item = |agent: Agent| {
        button(
            container(
                text(agent.label())
                    .font(MONO_FONT)
                    .size(11)
                    .color(c::FG_DIM),
            )
            .width(Length::Fill)
            .center_y(24)
            .padding(Padding::from([0, 8])),
        )
        .on_press(Msg::StartSession { proj, wt, agent })
        .width(Length::Fill)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER))
                } else {
                    None
                },
                text_color: if hovered { c::FG } else { c::FG_DIM },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
    };

    container(column![item(Agent::Codex), item(Agent::OpenCode)].spacing(0))
        .width(96)
        .padding(Padding::from([3, 0]))
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG)),
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
        })
        .into()
}

pub fn action_mini<'a>(icon_name: &'static str, msg: Msg) -> Element<'a, Msg> {
    button(
        container(icon(icon_name, 12.0, c::FG_MUTE))
            .center_x(22)
            .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
            text_color: if hovered { c::FG } else { c::FG_MUTE },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn tool_btn<'a>(
    icon_name: &'static str,
    label: &str,
    danger: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    let label_owned = label.to_string();
    button(
        container(
            row![
                icon(icon_name, 12.0, c::FG_DIM),
                text(label_owned).size(11.5).color(c::FG_DIM),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([0, 8]))
        .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let color = if hovered {
            if danger {
                c::RED
            } else {
                c::FG
            }
        } else {
            c::FG_DIM
        };
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
            text_color: color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn empty_workspace<'a>() -> Element<'a, Msg> {
    container(
        column![
            text("no session selected").size(14).color(c::FG_DIM),
            text("click a worktree's start button to spawn an agent")
                .size(12)
                .color(c::FG_MUTE),
        ]
        .spacing(8)
        .align_x(iced::Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG)),
        ..Default::default()
    })
    .into()
}

pub fn modal_panel<'a>(
    content: Element<'a, Msg>,
    width: f32,
    height: f32,
    accent: Color,
) -> Element<'a, Msg> {
    container(content)
        .width(width)
        .height(height)
        .padding(Padding::from([16, 18]))
        .style(move |_| container::Style {
            background: Some(Background::Color(c::BG)),
            text_color: Some(c::FG),
            border: Border {
                color: accent,
                width: 1.0,
                radius: Radius::from(5.0),
            },
            ..Default::default()
        })
        .into()
}

pub fn modal_action<'a>(label: &'static str, primary: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(11.5))
        .on_press(msg)
        .padding(Padding::from([6, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let bg = if primary {
                if hovered {
                    c::BG_HOVER
                } else {
                    c::BG_HL
                }
            } else if hovered {
                c::BG_HOVER
            } else {
                c::BG
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: if primary { c::FG } else { c::FG_DIM },
                border: Border {
                    color: c::BORDER,
                    width: 1.0,
                    radius: Radius::from(5.0),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

pub fn modal_hints<'a>(pairs: &[(&'static str, &'static str)]) -> Element<'a, Msg> {
    let mut hints = iced::widget::Row::new()
        .spacing(8)
        .align_y(iced::Alignment::Center);
    for (key, label) in pairs {
        hints = hints.push(
            row![
                text(*key).font(MONO_FONT).size(11).color(c::YELLOW),
                text(*label).size(11).color(c::FG_MUTE),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        );
    }
    hints.into()
}

pub fn modal_dir_row<'a>(path: String, active: bool) -> Element<'a, Msg> {
    let msg_path = path.clone();
    button(
        container(
            text(path)
                .font(MONO_FONT)
                .size(12)
                .color(if active { c::FG } else { c::CYAN })
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .height(ROW_H)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 8]))
        .clip(true),
    )
    .on_press(Msg::ModalPickDir(msg_path))
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if active {
                Some(Background::Color(c::BG_HL))
            } else if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
            text_color: if active || hovered { c::FG } else { c::CYAN },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn clickable_row<'a>(
    content: impl Into<Element<'a, Msg>>,
    height: f32,
    active: bool,
    on_press: Msg,
) -> Element<'a, Msg> {
    let bg = if active {
        Some(Background::Color(c::BG_HL))
    } else {
        None
    };
    button(
        container(content.into())
            .height(height)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(on_press)
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered && !active {
                Some(Background::Color(c::BG_HOVER))
            } else {
                bg
            },
            text_color: if active || hovered { c::FG } else { c::FG_DIM },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}
