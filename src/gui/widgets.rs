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
    container(Space::new().width(7))
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
    container(Space::new().width(1))
        .width(1)
        .height(18)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BORDER())),
            ..Default::default()
        })
        .into()
}

pub fn divider_h<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(1))
        .width(Length::Fill)
        .height(1)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

pub fn divider_v<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().width(1))
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
    Right,
}

pub fn seg_button<'a>(label: &str, active: bool, side: SegSide, msg: Msg) -> Element<'a, Msg> {
    seg_button_inner(label, active, side, msg, false)
}

/// Danger-flavored variant of [`seg_button`]: when `active`, fills with
/// [`c::RED_WASH()`] and colors text [`c::RED()`] instead of the neutral
/// `BG_HL()` / `FG()` treatment. Used for the "skip permissions" side of the
/// permissions segmented control, so it reads as a safety signal rather than
/// an ordinary selection.
pub fn seg_button_danger<'a>(
    label: &str,
    active: bool,
    side: SegSide,
    msg: Msg,
) -> Element<'a, Msg> {
    seg_button_inner(label, active, side, msg, true)
}

fn seg_button_inner<'a>(
    label: &str,
    active: bool,
    side: SegSide,
    msg: Msg,
    danger: bool,
) -> Element<'a, Msg> {
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
pub fn skip_perms_seg<'a>(skip_on: bool, on_skip: Msg, on_safe: Msg) -> Element<'a, Msg> {
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
            snap: false,
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
                snap: false,
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
            snap: false,
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
    let backdrop = button(Space::new().width(Length::Fill).height(Length::Fill))
        .on_press(Msg::CloseAgentMenu)
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

    let positioned = column![
        Space::new().height(top),
        row![
            Space::new().width(Length::Fill),
            agent_menu(proj, wt, is_main, available)
        ]
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 8.0,
        }),
        Space::new().height(Length::Fill),
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
                snap: false,
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
            snap: false,
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
            snap: false,
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
            snap: false,
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
                snap: false,
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
    modal_action_sized(label, kind, 12, msg)
}

/// [`modal_action`] with an explicit text size, for spots (e.g. a demoted
/// footer strip) where the default 12px button reads too loud.
pub fn modal_action_sized<'a>(
    label: &'static str,
    kind: ModalBtn,
    size: u16,
    msg: Msg,
) -> Element<'a, Msg> {
    button(text(label).size(iced::Pixels::from(size as f32)))
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
                snap: false,
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
    checkbox(checked)
        .label(label)
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
/// treatment: active rows get `bg_highlight`, hovered rows `bg_hover`. Fixed
/// at the shared 28px `ROW_H` with square corners — see
/// [`modal_list_row_sized`] for the command palette's taller, rounded rows.
pub fn modal_list_row<'a>(
    label: impl Into<Element<'a, Msg>>,
    active: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    modal_list_row_sized(label, active, msg, ROW_H, 0.0, 10.0)
}

/// Generalized form of [`modal_list_row`]: same active/hover fill logic
/// (`bg_highlight` / `bg_hover`), but with caller-chosen height, corner
/// radius, and horizontal padding. The command palette uses this directly
/// for its 44px main rows and 36px single-line action rows, both with a 6px
/// radius (`modal_list_row` itself keeps 0 radius so every other list's
/// square-cornered rows are unaffected).
pub fn modal_list_row_sized<'a>(
    label: impl Into<Element<'a, Msg>>,
    active: bool,
    msg: Msg,
    height: f32,
    radius: f32,
    pad_x: f32,
) -> Element<'a, Msg> {
    button(
        container(label)
            .width(Length::Fill)
            .height(height)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0.0, pad_x]))
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
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(radius),
            },
            shadow: Shadow::default(),
            snap: false,
        }
    })
    .into()
}

/// Command palette row height (recents/combo rows) — taller than the shared
/// 28px `ROW_H` used elsewhere, per the palette redesign. Kept local to this
/// module rather than added to `metrics.rs`, which stays a global constant
/// other lists depend on.
pub const PALETTE_ROW_H: f32 = 44.0;
const PALETTE_ROW_RADIUS: f32 = 6.0;
const PALETTE_ROW_PAD_X: f32 = 12.0;

/// A selectable row inside the session launcher's palette.
///
/// - `active && focused` (the selected row, keyboard-driven) gets the
///   prominent treatment — a cyan-tinted gradient fill, a cyan ring, a left
///   accent bar, and bright text.
/// - `active && !focused` keeps a quiet `bg_highlight` fill with dimmed text.
/// - hovered / idle rows match the shared modal treatment.
///
/// `height` lets callers pick their row size: recents/combo rows use
/// [`PALETTE_ROW_H`] (44px), the options-state agent list uses 36px. Corners
/// are always [`PALETTE_ROW_RADIUS`] (6px).
pub fn launcher_row<'a>(
    label: impl Into<Element<'a, Msg>>,
    active: bool,
    focused: bool,
    msg: Msg,
    height: f32,
) -> Element<'a, Msg> {
    use iced::gradient::{self, Gradient};
    use iced::Radians;

    // Resting/idle rows match the shared modal treatment, just taller and
    // rounded to match the palette's row idiom.
    if !(active && focused) {
        return modal_list_row_sized(
            label,
            active,
            msg,
            height,
            PALETTE_ROW_RADIUS,
            PALETTE_ROW_PAD_X,
        );
    }

    // The selected, focused row: cyan gradient fill + ring.
    let btn = button(
        container(label)
            .width(Length::Fill)
            .height(height)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0.0, PALETTE_ROW_PAD_X]))
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
                radius: Radius::from(PALETTE_ROW_RADIUS),
            },
            shadow: Shadow::default(),
            snap: false,
        }
    });

    // Left accent bar, overlaid so it doesn't shift row content.
    let bar = container(
        container(Space::new().width(3))
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
    .padding(Padding::from([6, 0]));

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
            snap: false,
        }
    })
    .into()
}
