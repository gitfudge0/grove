//! Small primitives: dots, dividers, text helpers, overlays, empty states.

use super::modal::keycap;
use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::Msg;
use grove_core::agent::Agent;
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
/// Shorten `s` to at most `max` chars by collapsing the middle with `…`.
/// Returns the original string unchanged if it already fits.
pub(in crate::gui) fn truncate_middle(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max || max < 2 {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let prefix: String = s.chars().take(head).collect();
    let suffix: String = s.chars().skip(len - tail).collect();
    format!("{prefix}…{suffix}")
}

pub(in crate::gui) fn dot<'a>(color: Color) -> Element<'a, Msg> {
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

pub(in crate::gui) fn vline<'a>() -> Element<'a, Msg> {
    container(Space::new().width(1))
        .width(1)
        .height(18)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BORDER())),
            ..Default::default()
        })
        .into()
}

pub(in crate::gui) fn divider_h<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(1))
        .width(Length::Fill)
        .height(1)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

pub(in crate::gui) fn divider_v<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().width(1))
        .width(1)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}
pub(in crate::gui) fn sidebar_agent_menu_overlay<'a>(
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
/// Title/subtitle for the sidebar project-tree empty state, or `None` when the
/// tree has rows to render.
///
/// Split out as pure logic so the branch choice is testable without a widget
/// harness. The two states must never share copy: each has a different fix, and
/// one message would send the user to the wrong place.
pub(in crate::gui) fn sidebar_empty_copy(
    total_projects: usize,
    active_projects: usize,
) -> Option<(&'static str, &'static str)> {
    match (total_projects, active_projects) {
        (_, a) if a > 0 => None,
        (0, _) => Some(("No projects yet", "Add one with + above.")),
        _ => Some((
            "All projects archived",
            "Restore one from Settings → Archived projects.",
        )),
    }
}

/// In-panel empty state for the sidebar project tree (mock frames C1 / C2).
/// The docked TERMINALS section still renders below this — only the tree is
/// empty, not the rail.
pub(in crate::gui) fn sidebar_empty<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Msg> {
    container(
        column![
            text(title).size(14).color(c::FG_DIM()),
            text(subtitle).size(12).color(c::FG_MUTE()),
        ]
        .spacing(6)
        .align_x(iced::Alignment::Center),
    )
    .padding(Padding::from([30, 16]))
    .width(Length::Fill)
    .align_x(iced::alignment::Horizontal::Center)
    .into()
}

pub(in crate::gui) fn empty_workspace<'a>() -> Element<'a, Msg> {
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

/// Same chrome as `empty_workspace()`, for the zero-active-projects case only
/// (mock frame C3). Deliberately a sibling rather than an edit to
/// `empty_workspace()`: with at least one active project, "click a worktree's
/// start button" is the correct instruction and must stay. With none, there is
/// no visible worktree to click, so that copy would be actively misleading.
pub(in crate::gui) fn empty_no_projects_workspace<'a>() -> Element<'a, Msg> {
    container(
        column![
            text("No active projects").size(14).color(c::FG_DIM()),
            text("Add or restore a project to get started.")
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

/// Same chrome as `empty_workspace()`, shown in the terminal tab when every
/// home terminal has been closed. The subtitle's mod+key hint renders the
/// actual ⌘ glyph on macOS (mirroring `mod_key_chip` in view.rs) and
/// `platform_mod_label()+t` elsewhere.
pub(in crate::gui) fn empty_terminals_workspace<'a>() -> Element<'a, Msg> {
    let keycap_content: Element<'a, Msg> = if cfg!(target_os = "macos") {
        row![
            icon("command", 10.0, c::FG_DIM()),
            text("t").font(MONO_FONT).size(11).color(c::FG_DIM()),
        ]
        .spacing(1)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        text(format!("{}+t", crate::gui::update::platform_mod_label()))
            .font(MONO_FONT)
            .size(11)
            .color(c::FG_DIM())
            .into()
    };

    let hint: Element<'a, Msg> = row![
        keycap(keycap_content),
        text("open a terminal")
            .font(MONO_FONT)
            .size(10)
            .color(c::FG_MUTE()),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .into();

    container(
        column![text("no terminals open").size(14).color(c::FG_DIM()), hint,]
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
/// Fake letter-spacing by joining every character with a U+2009 thin space
/// (confirmed present in the bundled BlexMono Nerd Font's `cmap`) — Iced has
/// no letter-spacing property. Uppercases the input.
pub(in crate::gui) fn tracked(label: &str) -> String {
    label
        .to_uppercase()
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join("\u{2009}")
}
/// Vertical scrollable with an invisible scrollbar: zero-width rail and a
/// transparent scroller, so wheel/trackpad scrolling works but nothing is
/// drawn over the content. Shared by the sidebar tree, palette lists, theme
/// pickers, settings body, and changelog.
pub(in crate::gui) fn ghost_scrollable<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
) -> iced::widget::Scrollable<'a, M> {
    use iced::widget::scrollable::{self, Direction, Rail, Scrollbar, Scroller};
    let invisible_rail = Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
        },
    };
    iced::widget::scrollable(content)
        .direction(Direction::Vertical(
            Scrollbar::new().width(0).scroller_width(0),
        ))
        .style(move |theme, status| scrollable::Style {
            container: container::Style::default(),
            vertical_rail: invisible_rail,
            horizontal_rail: invisible_rail,
            gap: None,
            ..scrollable::default(theme, status)
        })
}

/// A small filled pill marking the current default among a list of choices —
/// same visual idiom as `settings_modal`'s Tools-section "Default" badge
/// (view.rs), extracted here so the palette's DefaultAgent sub-pane can reuse
/// it without touching that modal's own (deliberately untouched) code.
pub(in crate::gui) fn slot_badge<'a>(label: &'static str) -> Element<'a, Msg> {
    container(
        text(label)
            .size(11)
            .color(c::FG())
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .padding(Padding::from([4, 12]))
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG_HL())),
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: Radius::from(4.0),
        },
        ..Default::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::sidebar_empty_copy;

    /// Any active project means the tree renders rows — no empty state at all.
    #[test]
    fn active_projects_suppress_the_empty_state() {
        assert!(sidebar_empty_copy(1, 1).is_none());
        assert!(sidebar_empty_copy(5, 2).is_none());
    }

    /// C2 vs C1: the two causes must not share copy, because the fix differs
    /// (add a project vs restore one from Settings).
    #[test]
    fn empty_and_all_archived_pick_distinct_copy() {
        let none = sidebar_empty_copy(0, 0).expect("no projects at all is an empty state");
        assert_eq!(none, ("No projects yet", "Add one with + above."));

        let archived = sidebar_empty_copy(3, 0).expect("all-archived is an empty state");
        assert_eq!(
            archived,
            (
                "All projects archived",
                "Restore one from Settings → Archived projects."
            )
        );
        assert_ne!(none, archived);
    }
}
