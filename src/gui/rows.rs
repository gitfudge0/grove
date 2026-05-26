//! Sidebar row builders — projects, worktrees, and sessions.

use super::icons::icon;
use super::metrics::{ROW_H, UI_BOLD, UI_FONT};
use super::palette as c;
use super::state::Msg;
use super::widgets::{action_mini, action_mini_danger, clickable_row, split_start_button};
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

/// 2px colored stripe used as a left rail (magenta on busy projects,
/// cyan on the active worktree). Pass `Color::TRANSPARENT` for "no rail".
fn left_rail<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_height(Length::Fill))
        .width(2.0)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// Branch "chip" — pill for feature branches, plain mono text for `main`.
fn branch_chip<'a>(branch: &str, subtle: bool) -> Element<'a, Msg> {
    let label = text(branch.to_string())
        .font(UI_FONT)
        .size(10)
        .color(if subtle { c::FG_MUTE() } else { c::FG_DIM() })
        .wrapping(iced::widget::text::Wrapping::None);
    if subtle {
        container(label).into()
    } else {
        container(label)
            .padding(Padding {
                top: 1.0,
                bottom: 1.0,
                left: 6.0,
                right: 6.0,
            })
            .style(|_| container::Style {
                background: Some(Background::Color(c::BORDER_SOFT())),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(3.0),
                },
                ..Default::default()
            })
            .into()
    }
}

pub fn project_row<'a>(idx: usize, name: &str, count: usize, expanded: bool) -> Element<'a, Msg> {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let has_sessions = count > 0;
    let count_color = if has_sessions {
        c::GREEN()
    } else {
        c::FG_MUTE()
    };
    let rail_color = Color::TRANSPARENT;

    let inline_count = text(format!("•{count}"))
        .font(UI_FONT)
        .size(11)
        .color(count_color);

    let project_label = row![
        container(icon(twist, 10.0, c::FG_MUTE()))
            .width(14)
            .center_y(Length::Fill),
        container(
            text(name.to_uppercase())
                .font(UI_BOLD)
                .size(12)
                .color(c::FG())
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .clip(true),
        inline_count,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 12.0,
        right: 4.0,
    });

    let project_btn = button(
        container(project_label)
            .height(ROW_H)
            .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .on_press(Msg::ProjectClicked(idx))
    .padding(0)
    .style(|_, _status| button::Style {
        background: None,
        text_color: c::FG_DIM(),
        border: Border::default(),
        shadow: Shadow::default(),
    });

    let add_btn = button(
        container(icon("plus", 12.0, c::FG_MUTE()))
            .center_x(22)
            .center_y(22),
    )
    .on_press(Msg::AddWorktree { proj: idx })
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if hovered { c::FG() } else { c::FG_MUTE() },
            border: Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    });

    let right = row![add_btn]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 8.0,
        });

    container(row![left_rail(rail_color), project_btn, right].align_y(iced::Alignment::Center))
        .height(ROW_H)
        .width(Length::Fill)
        .into()
}

pub fn worktree_row<'a>(
    proj: usize,
    wt: usize,
    name: &str,
    branch: &str,
    active: bool,
    is_main: bool,
    hovered: bool,
    expanded: bool,
) -> Element<'a, Msg> {
    // Split layout — action buttons are siblings of the left button, NOT
    // nested inside it. Nesting buttons inside a button causes both to fire
    // on a single click (iced 0.13 does not propagate captured-event status
    // through the outer button's on_event handler).
    // Only show a branch chip for non-default worktrees. The main worktree's
    // branch (typically `main` / `master`) is redundant with the project name.
    let show_branch = !is_main && branch != name && !branch.is_empty();

    let name_text = text(name.to_string())
        .size(13)
        .color(c::FG_DIM())
        .wrapping(iced::widget::text::Wrapping::None);

    let label: Element<'a, Msg> = if show_branch {
        column![
            container(name_text).clip(true),
            row![branch_chip(branch, false)].padding(Padding {
                top: 2.0,
                bottom: 0.0,
                left: 0.0,
                right: 0.0,
            }),
        ]
        .spacing(0)
        .into()
    } else {
        name_text.into()
    };

    let row_h = if show_branch { ROW_H + 14.0 } else { ROW_H };

    let twist = if expanded { "chev-down" } else { "chev-right" };
    let left_content = row![
        container(icon(twist, 10.0, c::FG_MUTE()))
            .width(14)
            .center_y(Length::Fill),
        container(label).width(Length::Fill).clip(true),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: if show_branch { 4.0 } else { 0.0 },
        bottom: if show_branch { 4.0 } else { 0.0 },
        left: 26.0,
        right: 6.0,
    });

    let bg_opt = if active {
        Some(Background::Color(c::BG_HL()))
    } else {
        None
    };
    let left_btn = button(
        container(left_content)
            .height(row_h)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Msg::WorktreeClicked { proj, wt })
    .width(Length::Fill)
    .padding(0)
    .style(move |_, _status| button::Style {
        background: bg_opt,
        text_color: if active { c::FG() } else { c::FG_DIM() },
        border: Border::default(),
        shadow: Shadow::default(),
    });

    let actions: Element<'a, Msg> = if hovered {
        row![split_start_button(proj, wt, is_main)]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 8.0,
            })
            .into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let rail_color = if active {
        c::CYAN()
    } else {
        Color::TRANSPARENT
    };

    container(
        row![
            left_rail(rail_color),
            row![left_btn, actions].align_y(iced::Alignment::Center)
        ]
        .align_y(iced::Alignment::Center),
    )
    .height(row_h)
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: if active {
            Some(Background::Color(c::BG_HL()))
        } else {
            None
        },
        ..Default::default()
    })
    .into()
}

pub fn session_row<'a>(
    idx: usize,
    s: &Session,
    wt_name: &str,
    active: bool,
    pending_kill: bool,
) -> Element<'a, Msg> {
    let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
    let agent_color = if active {
        c::CYAN()
    } else if running {
        c::FG()
    } else {
        c::FG_MUTE()
    };

    // "Context" = the PTY's OSC window title with the redundant worktree /
    // agent / session-label noise stripped. The worktree row already shows
    // the worktree name, so repeating it in every child session is wasted
    // pixels.
    let context = session_context(s, wt_name);

    let mut meta_row = row![text(s.agent.label())
        .font(UI_FONT)
        .size(12)
        .color(agent_color),]
    .spacing(6)
    .align_y(iced::Alignment::Center);
    if let Some(ctx) = context.as_deref() {
        let truncated = truncate_ellipsis(ctx, 28);
        meta_row = meta_row.push(text("·").size(11).color(c::FG_MUTE())).push(
            text(truncated)
                .font(UI_FONT)
                .size(11)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::None),
        );
    }

    let meta: Element<'a, Msg> = container(meta_row).width(Length::Fill).clip(true).into();

    let close_btn: Element<'a, Msg> = if pending_kill {
        action_mini_danger("check", Msg::KillSession(idx))
    } else {
        action_mini("close", Msg::RequestKillSession(idx))
    };

    let main_row: Element<'a, Msg> = row![Space::with_width(Length::Fixed(0.0)), meta, close_btn,]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 48.0,
            right: 8.0,
        })
        .into();

    clickable_row(main_row, ROW_H, active, Msg::SelectSession(idx))
}

/// Derive the short "context" string shown next to the agent name.
/// Prefers the OSC title (e.g. `Review pull`) but strips the worktree
/// name, the agent label, and the session label when they appear, so the
/// remainder is the actual task description. Returns `None` if nothing
/// useful is left.
fn session_context(s: &Session, wt_name: &str) -> Option<String> {
    let raw = s.current_title()?;
    let strips: [&str; 3] = [wt_name, &s.label, s.agent.label()];
    let mut out = raw.clone();
    for needle in strips {
        if needle.is_empty() {
            continue;
        }
        out = remove_all_ci(&out, needle);
    }
    let cleaned: String = out
        .chars()
        .filter(|c| {
            // Drop anything the UI font likely can't render: emoji, symbols,
            // box-drawing, private-use, etc. Keep ASCII plus common Latin
            // punctuation/letters.
            *c == ' '
                || *c == '·'
                || (*c >= '\u{0020}' && *c <= '\u{007E}')
                || (*c >= '\u{00A0}' && *c <= '\u{024F}')
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '·' | '-' | ':' | '|' | '/'))
        .to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let take = max_chars.saturating_sub(1);
        let mut out: String = s.chars().take(take).collect();
        out.push('…');
        out
    }
}

/// Remove every case-insensitive occurrence of `needle` from `hay`, returning
/// a UTF-8-safe result.
fn remove_all_ci(hay: &str, needle: &str) -> String {
    let hay_lower = hay.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(hay.len());
    let mut cursor = 0;
    while let Some(rel) = hay_lower[cursor..].find(&needle_lower) {
        let start = cursor + rel;
        out.push_str(&hay[cursor..start]);
        cursor = start + needle_lower.len();
    }
    out.push_str(&hay[cursor..]);
    out
}
