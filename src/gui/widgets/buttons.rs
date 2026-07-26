//! Buttons and clickable chips shared across the GUI.

use crate::gui::icons::icon;
use crate::gui::metrics::UI_FONT;
use crate::gui::palette as c;
use crate::gui::state::Msg;
use grove_core::agent::Agent;
use iced::border::Radius;
use iced::widget::{button, container, row, text};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
pub(in crate::gui) enum SegSide {
    Left,
    Right,
    /// A middle segment in a 3-way (or more) joined group — square on both
    /// sides; only the group's outer edges round. Used by the Theme
    /// sub-pane's Dark/Light/System mode row.
    Mid,
}

pub(in crate::gui) fn seg_button<'a, M: Clone + 'a>(
    label: &str,
    active: bool,
    side: SegSide,
    msg: M,
) -> Element<'a, M> {
    seg_button_inner(label, active, side, msg, false)
}

/// Danger-flavored variant of [`seg_button`]: when `active`, fills with
/// [`c::RED_WASH()`] and colors text [`c::RED()`] instead of the neutral
/// `BG_HL()` / `FG()` treatment. Used for the "skip permissions" side of the
/// permissions segmented control, so it reads as a safety signal rather than
/// an ordinary selection.
pub(in crate::gui) fn seg_button_danger<'a, M: Clone + 'a>(
    label: &str,
    active: bool,
    side: SegSide,
    msg: M,
) -> Element<'a, M> {
    seg_button_inner(label, active, side, msg, true)
}

fn seg_button_inner<'a, M: Clone + 'a>(
    label: &str,
    active: bool,
    side: SegSide,
    msg: M,
    danger: bool,
) -> Element<'a, M> {
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
                SegSide::Right => Radius {
                    top_left: 0.0,
                    top_right: 5.0,
                    bottom_right: 5.0,
                    bottom_left: 0.0,
                },
                SegSide::Mid => Radius::default(),
            };
            button::Style {
                background: if active {
                    Some(Background::Color(if danger {
                        c::RED_WASH()
                    } else {
                        c::BG_HL()
                    }))
                } else if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if active {
                    if danger {
                        c::RED()
                    } else {
                        c::FG()
                    }
                } else {
                    c::FG_DIM()
                },
                border: Border {
                    radius,
                    ..Border::default()
                },
                shadow: Shadow::default(),
                snap: false,
            }
        })
        .into()
}

/// The shared "Skip / Safe" permissions segmented control used by both
/// `settings_modal` and the onboarding permissions step. `skip_on` selects
/// which side is active; `on_skip` / `on_safe` are the messages each call
/// site emits (they differ per caller, so behavior stays identical to what
/// each site had before this was extracted).
pub(in crate::gui) fn skip_perms_seg<'a, M: Clone + 'a>(
    skip_on: bool,
    on_skip: M,
    on_safe: M,
) -> Element<'a, M> {
    container(
        row![
            seg_button_danger("Skip", skip_on, SegSide::Left, on_skip),
            seg_button("Safe", !skip_on, SegSide::Right, on_safe),
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
    })
    .into()
}

pub(in crate::gui) fn icon_btn<'a, M: Clone + 'a>(name: &'static str, msg: M) -> Element<'a, M> {
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
            snap: false,
        }
    })
    .into()
}

pub(in crate::gui) fn split_start_button<'a>(
    proj: usize,
    wt: usize,
    is_main: bool,
    has_run: bool,
    available: &[Agent],
) -> Element<'a, Msg> {
    split_start_button_inner(proj, wt, is_main, 12.0, false, true, has_run, available)
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
fn flat_action_btn<'a, M: Clone + 'a>(
    icon_name: &'static str,
    icon_size: f32,
    rest_color: Color,
    msg: M,
) -> Element<'a, M> {
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
                snap: false,
            }
        })
        .into()
}

/// One of the three per-worktree action chips — transparent at rest, subtle
/// pill on hover. Matches `.mini` in the mockup.
fn mini_action_btn<'a, M: Clone + 'a>(
    icon_name: &'static str,
    icon_size: f32,
    rest_color: Color,
    msg: M,
) -> Element<'a, M> {
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
            snap: false,
        }
    })
    .into()
}
pub(in crate::gui) fn action_mini<'a, M: Clone + 'a>(
    icon_name: &'static str,
    msg: M,
) -> Element<'a, M> {
    action_mini_styled(icon_name, msg, false)
}

pub(in crate::gui) fn action_mini_danger<'a, M: Clone + 'a>(
    icon_name: &'static str,
    msg: M,
) -> Element<'a, M> {
    action_mini_styled(icon_name, msg, true)
}

fn action_mini_styled<'a, M: Clone + 'a>(
    icon_name: &'static str,
    msg: M,
    danger: bool,
) -> Element<'a, M> {
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
            snap: false,
        }
    })
    .into()
}

pub(in crate::gui) fn tool_btn<'a, M: Clone + 'a>(
    icon_name: &'static str,
    label: &str,
    danger: bool,
    msg: M,
) -> Element<'a, M> {
    tool_btn_toggle(icon_name, label, danger, false, msg)
}

/// `tool_btn` with an extra `active` flag: when set, the button renders in the
/// cyan "on" state, matching how other toggle tools (sidebar tab, zen) signal
/// that they are currently engaged. Used by the header `term` button to show
/// the slide-over panel is open.
pub(in crate::gui) fn tool_btn_toggle<'a, M: Clone + 'a>(
    icon_name: &'static str,
    label: &str,
    danger: bool,
    active: bool,
    msg: M,
) -> Element<'a, M> {
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
            snap: false,
        }
    })
    .into()
}

/// A flat appbar button whose content is a centered SVG icon in a fixed-width
/// box. Used for the zoom `-` / `+` so both glyphs share an identical footprint
/// and sit perfectly centered, flanking the percentage label as one unit.
pub(in crate::gui) fn control_icon_btn<'a, M: Clone + 'a>(
    name: &'static str,
    msg: M,
    box_w: f32,
    icon_size: f32,
) -> Element<'a, M> {
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
            snap: false,
        }
    })
    .into()
}

/// A flat appbar text button with explicit text size and horizontal padding,
/// used for the zoom percentage label between the `-` / `+` icon buttons.
pub(in crate::gui) fn control_btn_sized<'a, M: Clone + 'a>(
    label: String,
    msg: M,
    text_size: u16,
    h_padding: u16,
) -> Element<'a, M> {
    button(
        container(
            text(label)
                .font(UI_FONT)
                .size(iced::Pixels::from(text_size as f32))
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
            snap: false,
        }
    })
    .into()
}
