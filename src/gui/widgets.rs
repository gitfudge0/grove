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

pub fn split_start_button<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    has_run: bool,
    available: &[Agent],
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, is_main, 12.0, false, true, has_run, available)
}

/// Flat variant: no per-chip background or border on hover (just an icon
/// color shift) and a tight padding-only hit box so the row doesn't grow
/// vertically when the chips appear.
pub fn split_start_button_flat<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    icon_size: f32,
    available: &[Agent],
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, is_main, icon_size, true, true, false, available)
}

/// Flat spawn chips for an activity-stream session row: same agent / terminal
/// launchers as `split_start_button_flat`, but the destructive delete-worktree
/// chip is never shown. Deleting a worktree from a row that still has a live
/// session is the wrong gesture; that action belongs to the worktree row.
pub fn session_spawn_chips_flat<'a>(
    proj: usize,
    wt: usize,
    icon_size: f32,
    available: &[Agent],
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, false, icon_size, true, false, false, available)
}

#[allow(clippy::too_many_arguments)]
fn split_start_button_inner<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    icon_size: f32,
    flat: bool,
    allow_delete: bool,
    show_run: bool,
    available: &[Agent],
) -> Element<'a, Msg> {
    let make = |name, msg| {
        if flat {
            flat_action_btn(name, icon_size, c::FG_MUTE(), msg)
        } else {
            mini_action_btn(name, icon_size, c::FG_MUTE(), msg)
        }
    };

    let mut r = row![]
        .spacing(if flat { 6 } else { 2 })
        .align_y(iced::Alignment::Center);
    // Only surface agents whose binary is actually on `$PATH` (see
    // `Agent::available` / `App::available_agents`). Terminal is always
    // available, so its chip is added unconditionally below.
    for agent in [Agent::Claude, Agent::Codex, Agent::OpenCode] {
        if available.contains(&agent) {
            r = r.push(make(
                agent.icon_name(),
                Msg::StartSession { proj, wt, agent },
            ));
        }
    }
    r = r.push(make("term", Msg::StartTerminal { proj, wt }));
    // The run-script play button only appears when the project actually has a
    // `run` script configured.
    if show_run {
        r = r.push(make("play", Msg::RunScript { proj, wt }));
    }
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
    available: &[Agent],
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
            agent_menu(proj, wt, is_main, available)
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

fn agent_menu<'a>(proj: usize, wt: usize, is_main: bool, available: &[Agent]) -> Element<'a, Msg> {
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

    let mut items = column![].spacing(0);
    // Same availability gate as the inline spawn chips: hide menu entries for
    // agents whose binary isn't on `$PATH`.
    for agent in [Agent::Codex, Agent::OpenCode] {
        if available.contains(&agent) {
            items = items.push(agent_item(agent));
        }
    }
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

/// A flat appbar button whose content is a centered SVG icon in a fixed-width
/// box. Used for the zoom `-` / `+` so both glyphs share an identical footprint
/// and sit perfectly centered, flanking the percentage label as one unit.
pub fn control_icon_btn<'a>(
    name: &'static str,
    msg: Msg,
    box_w: f32,
    icon_size: f32,
) -> Element<'a, Msg> {
    button(
        container(icon(name, icon_size, c::FG_DIM()))
            .center_x(Length::Fixed(box_w))
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

/// A flat appbar text button with explicit text size and horizontal padding,
/// used for the zoom percentage label between the `-` / `+` icon buttons.
pub fn control_btn_sized<'a>(
    label: String,
    msg: Msg,
    text_size: u16,
    h_padding: u16,
) -> Element<'a, Msg> {
    button(
        container(
            text(label)
                .font(UI_FONT)
                .size(text_size)
                .line_height(1.0)
                .height(18)
                .align_y(iced::alignment::Vertical::Center)
                .color(c::FG_DIM()),
        )
        .padding(Padding::from([0, h_padding]))
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

/// Full-width outlined footer button used at the bottom of the sidebar
/// (`+ add project`, `+ new terminal`). Shared so both footers get the same
/// rest / hover treatment.
pub fn footer_btn<'a>(label: &'static str, msg: Msg) -> Element<'a, Msg> {
    container(
        button(
            container(text(label).size(12))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .on_press(msg)
        .width(Length::Fill)
        .height(28.0)
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
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
            }
        }),
    )
    .padding(Padding::from([12, 12]))
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

pub fn modal_panel<'a>(content: Element<'a, Msg>, width: f32, accent: Color) -> Element<'a, Msg> {
    container(content)
        .width(width)
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

/// Visual weight of a modal footer button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModalBtn {
    /// Dismiss / secondary action.
    Plain,
    /// Default affirmative action.
    Primary,
    /// Default affirmative action with destructive consequences.
    Danger,
}

pub fn modal_action<'a>(label: &'static str, kind: ModalBtn, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(12))
        .on_press(msg)
        .padding(Padding::from([6, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let filled = !matches!(kind, ModalBtn::Plain);
            let bg = if hovered {
                c::BG_HOVER()
            } else if filled {
                c::BG_HL()
            } else {
                c::BG()
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: match kind {
                    ModalBtn::Plain => c::FG_DIM(),
                    ModalBtn::Primary => c::FG(),
                    ModalBtn::Danger => c::RED(),
                },
                border: Border {
                    color: if matches!(kind, ModalBtn::Danger) {
                        c::RED()
                    } else {
                        c::BORDER()
                    },
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

/// Modal checkbox in the shared themed style; `accent` colors the tick and
/// the checked border. `on_toggle: None` renders it disabled.
pub fn modal_checkbox<'a>(
    label: String,
    checked: bool,
    accent: Color,
    on_toggle: Option<fn(bool) -> Msg>,
) -> Element<'a, Msg> {
    use iced::widget::checkbox;
    use iced::widget::checkbox::{Status as CheckboxStatus, Style as CheckboxStyle};
    checkbox(label, checked)
        .on_toggle_maybe(on_toggle)
        .size(14)
        .spacing(8)
        .text_size(12)
        .font(UI_FONT)
        .style(move |_, status| {
            let (checked, disabled, hovered) = match status {
                CheckboxStatus::Active { is_checked } => (is_checked, false, false),
                CheckboxStatus::Hovered { is_checked } => (is_checked, false, true),
                CheckboxStatus::Disabled { is_checked } => (is_checked, true, false),
            };
            let border_color = if checked {
                accent
            } else if hovered {
                c::FG_DIM()
            } else {
                c::BORDER()
            };
            CheckboxStyle {
                background: Background::Color(if checked {
                    c::BG_HL()
                } else if hovered {
                    c::BG_HOVER()
                } else {
                    c::BG()
                }),
                icon_color: if disabled { c::FG_MUTE() } else { accent },
                border: Border {
                    color: border_color,
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                text_color: Some(if disabled { c::FG_MUTE() } else { c::FG_DIM() }),
            }
        })
        .into()
}

/// One selectable row inside a modal list (agent picker, theme picker,
/// directory matches). Shared so every list uses the same active/hover
/// treatment: active rows get `bg_highlight`, hovered rows `bg_hover`.
pub fn modal_list_row<'a>(
    label: impl Into<Element<'a, Msg>>,
    active: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    button(
        container(label)
            .width(Length::Fill)
            .height(ROW_H)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0, 10]))
            .clip(true),
    )
    .on_press(msg)
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
            text_color: if active { c::FG() } else { c::FG_DIM() },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}

/// A selectable row inside the session launcher's Miller columns.
///
/// Unlike [`modal_list_row`], selection here is column-aware so focus is
/// unambiguous across the three columns:
/// - `active && focused` (the selected row in the column you're driving) gets
///   the prominent treatment — a cyan-tinted gradient fill, a cyan ring, a
///   left accent bar, and bright text.
/// - `active && !focused` (a remembered selection in a resting column) keeps a
///   quiet `bg_highlight` fill with dimmed text.
/// - hovered / idle rows match the shared modal treatment.
pub fn launcher_row<'a>(
    label: impl Into<Element<'a, Msg>>,
    active: bool,
    focused: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    use iced::gradient::{self, Gradient};
    use iced::Radians;

    // Resting columns (and non-selected rows) match the shared modal treatment.
    if !(active && focused) {
        return modal_list_row(label, active, msg);
    }

    // The selected row in the focused column: cyan gradient fill + ring.
    let btn = button(
        container(label)
            .width(Length::Fill)
            .height(ROW_H)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0, 10]))
            .clip(true),
    )
    .on_press(msg)
    .width(Length::Fill)
    .padding(0)
    .style(|_, _| {
        // Horizontal cyan tint, brighter at the accent-bar edge.
        let g = gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
            .add_stop(0.0, c::SEL_TINT_STRONG())
            .add_stop(1.0, c::SEL_TINT_SOFT());
        button::Style {
            background: Some(Background::Gradient(Gradient::Linear(g))),
            text_color: c::FG(),
            border: Border {
                color: c::SEL_RING(),
                width: 1.0,
                radius: Radius::from(5.0),
            },
            shadow: Shadow::default(),
        }
    });

    // Left accent bar, overlaid so it doesn't shift row content.
    let bar = container(
        container(Space::with_width(3))
            .width(3)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::CYAN())),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(2.0),
                },
                ..Default::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::Alignment::Start)
    .align_y(iced::Alignment::Center)
    .padding(Padding::from([2, 0]));

    iced::widget::stack![btn, bar].into()
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
