//! Sidebar row builders — projects, worktrees, sessions, and the
//! "+ new worktree" affordance.

use super::icons::icon;
use super::metrics::{MONO_FONT, ROW_H, SUBTITLE_H};
use super::palette as c;
use super::state::Msg;
use super::widgets::{
    action_mini, clickable_row, delete_worktree_button, split_start_button,
};
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Element, Length, Padding, Shadow};

pub fn project_row<'a>(idx: usize, name: &str, count: usize, expanded: bool) -> Element<'a, Msg> {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let count_color = if count > 0 { c::GREEN } else { c::FG_MUTE };
    let row_content = row![
        container(icon(twist, 10.0, c::FG_MUTE))
            .width(14)
            .center_y(Length::Fill),
        text(name.to_string()).size(13).color(c::FG),
        text(format!("● {count}"))
            .font(MONO_FONT)
            .size(11)
            .color(count_color),
        Space::with_width(Length::Fill),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 12.0,
        right: 8.0,
    });

    clickable_row(row_content, ROW_H, false, Msg::ProjectClicked(idx))
}

pub fn worktree_row<'a>(
    proj: usize,
    wt: usize,
    name: &str,
    branch: &str,
    active: bool,
    is_main: bool,
) -> Element<'a, Msg> {
    // Split layout — action buttons are siblings of the left button, NOT
    // nested inside it. Nesting buttons inside a button causes both to fire
    // on a single click (iced 0.13 does not propagate captured-event status
    // through the outer button's on_event handler).
    let show_branch = branch != name;

    let label: Element<'a, Msg> = if show_branch {
        row![
            text(name.to_string())
                .size(13)
                .color(c::FG_DIM)
                .wrapping(iced::widget::text::Wrapping::None),
            text(format!(" · {branch}"))
                .font(MONO_FONT)
                .size(11)
                .color(c::FG_MUTE)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        text(name.to_string())
            .size(13)
            .color(c::FG_DIM)
            .wrapping(iced::widget::text::Wrapping::None)
            .into()
    };

    let left_content = row![
        container(icon("chev-right", 10.0, c::FG_MUTE))
            .width(14)
            .center_y(Length::Fill),
        container(label).width(Length::Fill).clip(true),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 28.0,
        right: 6.0,
    });

    let bg_opt = if active {
        Some(Background::Color(c::BG_HL))
    } else {
        None
    };
    let left_btn = button(
        container(left_content)
            .height(ROW_H)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Msg::WorktreeClicked { proj, wt })
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered && !active {
                Some(Background::Color(c::BG_HOVER))
            } else {
                bg_opt
            },
            text_color: if active || hovered { c::FG } else { c::FG_DIM },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    });

    let mut actions = row![split_start_button(proj, wt)]
        .spacing(6)
        .align_y(iced::Alignment::Center);
    if !is_main {
        actions = actions.push(delete_worktree_button(proj, wt));
    }
    let actions = actions.padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 0.0,
        right: 8.0,
    });

    container(row![left_btn, actions].align_y(iced::Alignment::Center))
        .height(ROW_H)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: if active {
                Some(Background::Color(c::BG_HL))
            } else {
                None
            },
            ..Default::default()
        })
        .into()
}

pub fn add_worktree_row<'a>(proj: usize) -> Element<'a, Msg> {
    let content = row![
        Space::with_width(28),
        text("+ new worktree").size(12).color(c::FG_MUTE),
        Space::with_width(Length::Fill),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 16.0,
        right: 8.0,
    });
    button(
        container(content)
            .height(ROW_H)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Msg::AddWorktree { proj })
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
            text_color: if hovered { c::FG_DIM } else { c::FG_MUTE },
            border: Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(0.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

pub fn session_row<'a>(idx: usize, s: &Session, active: bool) -> Element<'a, Msg> {
    let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
    let dot_color = if running { c::GREEN } else { c::FG_MUTE };
    let agent_color = if active { c::CYAN } else { c::FG };

    let subtitle = s
        .current_title()
        .filter(|t| !t.eq_ignore_ascii_case(&s.label) && !t.eq_ignore_ascii_case(s.agent.label()));

    let meta: Element<'a, Msg> = container(
        row![
            text(s.agent.label())
                .font(MONO_FONT)
                .size(12)
                .color(agent_color),
            text("·").size(11).color(c::FG_MUTE),
            text(s.label.clone())
                .font(MONO_FONT)
                .size(11)
                .color(c::FG_MUTE)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .clip(true)
    .into();

    let main_row: Element<'a, Msg> = row![
        Space::with_width(28),
        super::widgets::dot(dot_color),
        meta,
        action_mini("close", Msg::KillSession(idx)),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 16.0,
        right: 8.0,
    })
    .into();

    let row_h = if subtitle.is_some() {
        ROW_H + SUBTITLE_H
    } else {
        ROW_H
    };

    let row_content: Element<'a, Msg> = match subtitle {
        Some(t) => column![
            main_row,
            container(
                text(t)
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_MUTE)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 52.0,
                right: 8.0,
            })
            .width(Length::Fill)
            .clip(true),
        ]
        .into(),
        None => main_row,
    };

    clickable_row(row_content, row_h, active, Msg::SelectSession(idx))
}
