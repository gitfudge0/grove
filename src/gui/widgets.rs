//! Small, reusable widget primitives — dots, dividers, buttons, hint pills,
//! and the modal panel chrome. None of these hold view state; they take the
//! data they need and return an `Element<Msg>`.

use super::icons::icon;
use super::metrics::{ROW_H, UI_FONT};
use super::palette as c;
use super::state::Msg;
use crate::agent::Agent;
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

/// Shorten `s` to at most `max` chars by collapsing the middle with `…`.
/// Returns the original string unchanged if it already fits.
pub fn truncate_middle(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max || max < 2 {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let head = (keep + 1) / 2;
    let tail = keep - head;
    let prefix: String = s.chars().take(head).collect();
    let suffix: String = s.chars().skip(len - tail).collect();
    format!("{prefix}…{suffix}")
}

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
            background: Some(Background::Color(c::BORDER())),
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

pub enum SegSide {
    Left,
    Middle,
    Right,
}

pub fn seg_button<'a>(label: &str, active: bool, side: SegSide, msg: Msg) -> Element<'a, Msg> {
    button(text(label.to_string()).size(11))
        .on_press(msg)
        .padding(Padding::from([4, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let radius = match side {
                SegSide::Left => Radius {
                    top_left: 5.0,
                    top_right: 0.0,
                    bottom_right: 0.0,
                    bottom_left: 5.0,
                },
                SegSide::Middle => Radius::from(0.0),
                SegSide::Right => Radius {
                    top_left: 0.0,
                    top_right: 5.0,
                    bottom_right: 5.0,
                    bottom_left: 0.0,
                },
            };
            button::Style {
                background: if active {
                    Some(Background::Color(c::BG_HL()))
                } else if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if active { c::FG() } else { c::FG_DIM() },
                border: Border {
                    radius,
                    ..Border::default()
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

pub fn icon_btn<'a>(name: &'static str, msg: Msg) -> Element<'a, Msg> {
    button(
        container(icon(name, 15.0, c::FG_DIM()))
            .center_x(28)
            .center_y(28),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: c::FG_DIM(),
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

pub fn split_start_button<'a>(proj: usize, wt: usize, is_main: bool) -> Element<'a, Msg> {
    split_start_button_sized(proj, wt, is_main, 12.0)
}

pub fn split_start_button_sized<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    icon_size: f32,
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, is_main, icon_size, false, true)
}

/// Flat variant: no per-chip background or border on hover (just an icon
/// color shift) and a tight padding-only hit box so the row doesn't grow
/// vertically when the chips appear.
pub fn split_start_button_flat<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    icon_size: f32,
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, is_main, icon_size, true, true)
}

/// Flat spawn chips for an activity-stream session row: same agent / terminal
/// launchers as `split_start_button_flat`, but the destructive delete-worktree
/// chip is never shown. Deleting a worktree from a row that still has a live
/// session is the wrong gesture; that action belongs to the worktree row.
pub fn session_spawn_chips_flat<'a>(
    proj: usize,
    wt: usize,
    icon_size: f32,
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, false, icon_size, true, false)
}

fn split_start_button_inner<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    icon_size: f32,
    flat: bool,
    allow_delete: bool,
) -> Element<'a, Msg> {
    let make = |name, msg| {
        if flat {
            flat_action_btn(name, icon_size, c::FG_MUTE(), msg)
        } else {
            mini_action_btn(name, icon_size, c::FG_MUTE(), msg)
        }
    };
    let claude = make(
        "claude",
        Msg::StartSession {
            proj,
            wt,
            agent: Agent::Claude,
        },
    );
    let codex = make(
        "codex",
        Msg::StartSession {
            proj,
            wt,
            agent: Agent::Codex,
        },
    );
    let opencode = make(
        "opencode",
        Msg::StartSession {
            proj,
            wt,
            agent: Agent::OpenCode,
        },
    );
    let terminal = make("term", Msg::StartTerminal { proj, wt });

    let mut r = row![claude, codex, opencode, terminal]
        .spacing(if flat { 6 } else { 2 })
        .align_y(iced::Alignment::Center);
    // The main worktree is the repository checkout itself — deleting it via
    // `git worktree remove` would fail, so the trash icon is suppressed there.
    // Session-row spawn chips suppress it unconditionally via `allow_delete`.
    if allow_delete && !is_main {
        r = r.push(make("trash", Msg::DeleteWorktree { proj, wt }));
    }
    r.into()
}

/// Flat icon chip: transparent at rest and on hover (no box), only the
/// icon color brightens. Used inside dense subtitle rows where a hover
/// pill would push surrounding text around.
fn flat_action_btn<'a>(
    icon_name: &'static str,
    icon_size: f32,
    rest_color: Color,
    msg: Msg,
) -> Element<'a, Msg> {
    button(icon(icon_name, icon_size, rest_color))
        .on_press(msg)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 2.0,
            right: 2.0,
        })
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: None,
                text_color: if hovered { c::FG() } else { rest_color },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
        .into()
}

/// One of the three per-worktree action chips — transparent at rest, subtle
/// pill on hover. Matches `.mini` in the mockup.
fn mini_action_btn<'a>(
    icon_name: &'static str,
    icon_size: f32,
    rest_color: Color,
    msg: Msg,
) -> Element<'a, Msg> {
    button(
        container(icon(icon_name, icon_size, rest_color))
            .center_x(22)
            .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if hovered { c::FG() } else { rest_color },
            border: Border {
                color: if hovered {
                    c::BORDER_SOFT()
                } else {
                    Color::TRANSPARENT
                },
                width: 1.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn sidebar_agent_menu_overlay<'a>(
    proj: usize,
    wt: usize,
    top: f32,
    is_main: bool,
) -> Element<'a, Msg> {
    let backdrop = button(Space::new(Length::Fill, Length::Fill))
        .on_press(Msg::CloseAgentMenu)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .style(|_, _| button::Style {
            background: None,
            text_color: Color::TRANSPARENT,
            border: Border::default(),
            shadow: Shadow::default(),
        });

    let positioned = column![
        Space::with_height(top),
        row![
            Space::with_width(Length::Fill),
            agent_menu(proj, wt, is_main)
        ]
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 8.0,
        }),
        Space::with_height(Length::Fill),
    ]
    .height(Length::Fill);

    iced::widget::stack![backdrop, positioned]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn agent_menu<'a>(proj: usize, wt: usize, is_main: bool) -> Element<'a, Msg> {
    let item = |label: String, msg: Msg, danger: bool| {
        button(
            container(text(label).font(UI_FONT).size(11).color(if danger {
                c::RED()
            } else {
                c::FG_DIM()
            }))
            .width(Length::Fill)
            .center_y(24)
            .padding(Padding::from([0, 8])),
        )
        .on_press(msg)
        .width(Length::Fill)
        .padding(0)
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if danger {
                    c::RED()
                } else if hovered {
                    c::FG()
                } else {
                    c::FG_DIM()
                },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
    };

    let agent_item = |agent: Agent| {
        item(
            agent.label().to_string(),
            Msg::StartSession { proj, wt, agent },
            false,
        )
    };

    let mut items = column![agent_item(Agent::Codex), agent_item(Agent::OpenCode)].spacing(0);
    if !is_main {
        items = items
            .push(container(divider_h(c::BORDER())).padding(Padding::from([3, 0])))
            .push(item(
                "delete".to_string(),
                Msg::DeleteWorktree { proj, wt },
                true,
            ));
    }

    container(items)
        .width(120)
        .padding(Padding::from([3, 0]))
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG())),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
        })
        .into()
}

pub fn action_mini<'a>(icon_name: &'static str, msg: Msg) -> Element<'a, Msg> {
    action_mini_styled(icon_name, msg, false)
}

pub fn action_mini_danger<'a>(icon_name: &'static str, msg: Msg) -> Element<'a, Msg> {
    action_mini_styled(icon_name, msg, true)
}

fn action_mini_styled<'a>(icon_name: &'static str, msg: Msg, danger: bool) -> Element<'a, Msg> {
    let base_color = if danger { c::RED() } else { c::FG_MUTE() };
    button(
        container(icon(icon_name, 12.0, base_color))
            .center_x(22)
            .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if danger {
                c::RED()
            } else if hovered {
                c::FG()
            } else {
                c::FG_MUTE()
            },
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
    tool_btn_toggle(icon_name, label, danger, false, msg)
}

/// `tool_btn` with an extra `active` flag: when set, the button renders in the
/// cyan "on" state, matching how other toggle tools (sidebar tab, zen) signal
/// that they are currently engaged. Used by the header `term` button to show
/// the slide-over panel is open.
pub fn tool_btn_toggle<'a>(
    icon_name: &'static str,
    label: &str,
    danger: bool,
    active: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    let label_owned = label.to_string();
    let base = if active { c::CYAN() } else { c::FG_DIM() };
    button(
        container(
            row![
                container(icon(icon_name, 12.0, base))
                    .height(18)
                    .align_y(iced::Alignment::Center),
                text(label_owned)
                    .font(UI_FONT)
                    .size(12)
                    .line_height(1.0)
                    .height(18)
                    .align_y(iced::alignment::Vertical::Center)
                    .color(base),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([0, 8]))
        .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let color = if active {
            c::CYAN()
        } else if hovered {
            if danger {
                c::RED()
            } else {
                c::FG()
            }
        } else {
            c::FG_DIM()
        };
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
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

pub fn control_btn<'a>(label: String, msg: Msg) -> Element<'a, Msg> {
    button(
        container(
            text(label)
                .font(UI_FONT)
                .size(12)
                .line_height(1.0)
                .height(18)
                .align_y(iced::alignment::Vertical::Center)
                .color(c::FG_DIM()),
        )
        .padding(Padding::from([0, 8]))
        .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if hovered { c::FG() } else { c::FG_DIM() },
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
            text("no session selected").size(14).color(c::FG_DIM()),
            text("click a worktree's start button to spawn an agent")
                .size(12)
                .color(c::FG_MUTE()),
        ]
        .spacing(6)
        .align_x(iced::Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG())),
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
        .padding(Padding::from([16, 20]))
        .style(move |_| container::Style {
            background: Some(Background::Color(c::BG())),
            text_color: Some(c::FG()),
            border: Border {
                color: accent,
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        })
        .into()
}

pub fn modal_action<'a>(label: &'static str, primary: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(12))
        .on_press(msg)
        .padding(Padding::from([6, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let bg = if primary {
                if hovered {
                    c::BG_HOVER()
                } else {
                    c::BG_HL()
                }
            } else if hovered {
                c::BG_HOVER()
            } else {
                c::BG()
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: if primary { c::FG() } else { c::FG_DIM() },
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

pub fn modal_dir_row<'a>(path: String, active: bool) -> Element<'a, Msg> {
    let msg_path = path.clone();
    button(
        container(
            text(path)
                .font(UI_FONT)
                .size(12)
                .color(if active { c::FG() } else { c::CYAN() })
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
                Some(Background::Color(c::BG_HL()))
            } else if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if active || hovered {
                c::FG()
            } else {
                c::CYAN()
            },
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
        Some(Background::Color(c::BG_HL()))
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
                Some(Background::Color(c::BG_HOVER()))
            } else {
                bg
            },
            text_color: if active || hovered {
                c::FG()
            } else {
                c::FG_DIM()
            },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}
