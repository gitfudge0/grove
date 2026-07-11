//! Sidebar row builders — projects, worktrees, and sessions.

use super::activity::ActivityState;
use super::icons::{icon, spinner};
use super::metrics::{ROW_H, SUBTITLE_H, UI_BOLD, UI_FONT};
use super::palette as c;
use super::state::Msg;
use super::widgets::{action_mini, action_mini_danger, clickable_row, split_start_button};
use crate::agent::Agent;
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::time::Duration;

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

pub fn project_row<'a>(
    idx: usize,
    name: &str,
    count: usize,
    expanded: bool,
    is_git: bool,
    rollup: Option<ActivityState>,
    tick: u32,
) -> Element<'a, Msg> {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let has_sessions = count > 0;
    let count_color = if has_sessions {
        c::GREEN()
    } else {
        c::FG_MUTE()
    };

    // Same dot-plus-numeral vocabulary as the statusbar's "● N running".
    let inline_count = row![
        super::widgets::dot(count_color),
        text(format!("{count}"))
            .font(UI_FONT)
            .size(11)
            .color(count_color),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

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

    let scripts_btn = button(
        container(icon("cog", 12.0, c::FG_MUTE()))
            .center_x(22)
            .center_y(22),
    )
    .on_press(Msg::EditScripts { proj: idx })
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

    let remove_btn = button(
        container(icon("trash", 12.0, c::FG_MUTE()))
            .center_x(22)
            .center_y(22),
    )
    .on_press(Msg::RemoveProject { proj: idx })
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if hovered { c::RED() } else { c::FG_MUTE() },
            border: Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    });

    let rollup_el: Element<'a, Msg> = match rollup {
        Some(st) => state_glyph(st, tick),
        None => Space::with_width(Length::Fixed(0.0)).into(),
    };
    // Worktrees and worktree-lifecycle scripts are git-only — omit the add and
    // settings buttons for non-git projects.
    let mut right = row![rollup_el].spacing(6).align_y(iced::Alignment::Center);
    if is_git {
        right = right.push(add_btn).push(scripts_btn);
    }
    right = right.push(remove_btn).padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 0.0,
        right: 8.0,
    });

    container(row![project_btn, right].align_y(iced::Alignment::Center))
        .height(ROW_H)
        .width(Length::Fill)
        .into()
}

/// Only show a branch chip for non-default worktrees. The main worktree's
/// branch (typically `main` / `master`) is redundant with the project name.
pub fn worktree_shows_branch(is_main: bool, branch: &str, name: &str) -> bool {
    !is_main && branch != name && !branch.is_empty()
}

/// Right-aligned `main` marker for the main worktree row, matching the
/// launcher's worktree-column tag (see `session_launcher_modal`). Shares a
/// fixed slot with the on-hover spawn icons — only one of the two is ever
/// shown, so they never compete for width.
fn main_tag<'a>() -> Element<'a, Msg> {
    text("main")
        .font(UI_FONT)
        .size(10)
        .color(c::GREEN())
        .wrapping(iced::widget::text::Wrapping::None)
        .into()
}

/// Rendered height of a worktree row. Must stay in sync with `worktree_row`'s
/// layout — the agent-menu overlay position is computed from this.
pub fn worktree_row_height(show_branch: bool) -> f32 {
    if show_branch {
        ROW_H + 14.0
    } else {
        ROW_H
    }
}

#[allow(clippy::too_many_arguments)]
pub fn worktree_row<'a>(
    proj: usize,
    wt: usize,
    name: &str,
    branch: &str,
    active: bool,
    is_main: bool,
    is_git: bool,
    hovered: bool,
    expanded: bool,
    has_run: bool,
    rollup: Option<ActivityState>,
    tick: u32,
    available: &[Agent],
    git_suffix: Option<String>,
) -> Element<'a, Msg> {
    // (Height logic shared with the agent-menu overlay positioning in view.rs
    // via `worktree_shows_branch` / `worktree_row_height`.)
    // Split layout — action buttons are siblings of the left button, NOT
    // nested inside it. Nesting buttons inside a button causes both to fire
    // on a single click (iced 0.13 does not propagate captured-event status
    // through the outer button's on_event handler).
    let show_branch = worktree_shows_branch(is_main, branch, name);

    let name_text = text(name.to_string())
        .size(13)
        .color(c::FG_DIM())
        .wrapping(iced::widget::text::Wrapping::None);

    // Non-git project root: flag it so the user knows sessions run directly in
    // the project path with no branch isolation / worktrees.
    let no_git = is_main && !is_git;
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
    } else if no_git {
        row![
            container(name_text).clip(true),
            icon("no-git", 11.0, c::FG_MUTE()),
            text("no git").size(10).color(c::FG_MUTE()),
        ]
        .spacing(5)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        name_text.into()
    };

    let row_h = worktree_row_height(show_branch);

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

    // The `main` tag and the hover spawn icons share this one fixed
    // right-hand slot: at rest the git main worktree shows `main`, on hover
    // it's replaced by the spawn icons — the two never render at once, so
    // there's no collision between the tag and the hover actions.
    let actions: Element<'a, Msg> = if hovered {
        row![split_start_button(proj, wt, is_main, has_run, available)]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 8.0,
            })
            .into()
    } else if is_main && is_git {
        container(main_tag())
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 4.0,
                right: 12.0,
            })
            .into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let rollup_el: Element<'a, Msg> = match rollup {
        Some(st) => state_glyph(st, tick),
        None => Space::with_width(Length::Fixed(0.0)).into(),
    };
    // Compact git-state suffix (`*` dirty, `↑N`/`↓M` ahead/behind) — muted so
    // it reads as secondary metadata, not competing with the name or the
    // activity glyph.
    let git_suffix_el: Element<'a, Msg> = match git_suffix {
        Some(s) => text(s)
            .size(11)
            .color(c::FG_MUTE())
            .wrapping(iced::widget::text::Wrapping::None)
            .into(),
        None => Space::with_width(Length::Fixed(0.0)).into(),
    };
    container(
        row![left_btn, git_suffix_el, rollup_el, actions]
            .spacing(4)
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
    state: ActivityState,
    tick: u32,
) -> Element<'a, Msg> {
    let agent_color = if active {
        c::CYAN()
    } else {
        match state {
            ActivityState::Working | ActivityState::WaitingForInput => c::FG(),
            ActivityState::Done | ActivityState::Idle => c::FG_DIM(),
            ActivityState::Exited => c::FG_MUTE(),
        }
    };

    // "Context" = the PTY's OSC window title with the redundant worktree /
    // agent / session-label noise stripped. The worktree row already shows
    // the worktree name, so repeating it in every child session is wasted
    // pixels.
    let context = session_context(s, wt_name);

    let mut meta_row = row![
        icon(s.agent.icon_name(), 12.0, agent_color),
        text(s.agent.label())
            .font(UI_FONT)
            .size(12)
            .color(agent_color),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);
    if let Some(ctx) = context.as_deref() {
        let truncated = truncate_ellipsis(ctx, 28);
        meta_row = meta_row
            .push(text("·").size(11).color(c::FG_MUTE()))
            .push(single_line(
                text(truncated)
                    .font(UI_FONT)
                    .size(11)
                    .color(c::FG_MUTE())
                    .wrapping(iced::widget::text::Wrapping::None),
                11.0,
            ));
    }

    let meta: Element<'a, Msg> = container(meta_row).width(Length::Fill).clip(true).into();

    let close_btn: Element<'a, Msg> = if pending_kill {
        action_mini_danger("check", Msg::KillSession(idx))
    } else {
        action_mini("close", Msg::RequestKillSession(idx))
    };

    let main_row: Element<'a, Msg> = row![state_glyph(state, tick), meta, close_btn,]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            // Aligns under the worktree row's label text (26 + 14 + 6).
            left: 46.0,
            right: 8.0,
        })
        .into();

    let row_el = clickable_row(main_row, ROW_H, active, Msg::SelectSession(idx));

    // WaitingForInput: amber tint background + 3px solid left accent bar.
    // The bar is overlaid via stack! so it never shifts the row content.
    if matches!(state, ActivityState::WaitingForInput) {
        let tint = Color {
            a: 0.12,
            ..c::AMBER()
        };
        let bar: Element<'a, Msg> = container(
            container(Space::with_width(3.0))
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
        container(iced::widget::stack![row_el, bar])
            .height(ROW_H)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(tint)),
                ..Default::default()
            })
            .into()
    } else {
        row_el
    }
}

/// One row in the terminal tab's sidebar list. Shows the terminal's label and
/// its contextual title (the shell's OSC window title); highlights the active
/// terminal and reveals a close button when `show_close` is set.
pub fn terminal_row<'a>(
    idx: usize,
    s: &Session,
    active: bool,
    show_close: bool,
) -> Element<'a, Msg> {
    let running = matches!(
        *s.status.lock().unwrap_or_else(|e| e.into_inner()),
        SessionStatus::Running
    );
    let name_color = if active {
        c::CYAN()
    } else if running {
        c::FG()
    } else {
        c::FG_MUTE()
    };

    // No synthetic "terminal N" name — just the icon and the shell's own
    // contextual title (cwd / running command), falling back to "~" so a
    // title-less shell still reads as the home terminal.
    let ctx = terminal_context(s).unwrap_or_else(|| "~".to_string());
    let meta_row = row![
        icon("term", 12.0, name_color),
        single_line(
            text(truncate_ellipsis(&ctx, 28))
                .font(UI_FONT)
                .size(12)
                .color(name_color)
                .wrapping(iced::widget::text::Wrapping::None),
            12.0,
        ),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let meta: Element<'a, Msg> = container(meta_row).width(Length::Fill).clip(true).into();

    let close_btn: Element<'a, Msg> = if show_close {
        action_mini("close", Msg::CloseHomeTerminal(idx))
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let main_row: Element<'a, Msg> = row![meta, close_btn]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 16.0,
            right: 8.0,
        })
        .into();

    clickable_row(main_row, ROW_H, active, Msg::SelectHomeTerminal(idx))
}

/// Contextual title for a home terminal: its OSC window title (typically the
/// current directory or running command), with the redundant terminal label
/// stripped and unrenderable characters removed.
pub fn terminal_context(s: &Session) -> Option<String> {
    let raw = s.current_title()?;
    let out = remove_all_ci(&raw, &s.label);
    sanitize_ui_text(&out)
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
    sanitize_ui_text(&out)
}

/// Strip characters the UI font (IBM Plex Sans) can't render — emoji, box
/// drawing, private-use, etc. — and collapse the resulting whitespace. Used
/// by both the sidebar session row and the workspace session bar so that
/// OSC titles emitted by agents (which often include emoji prefixes) never
/// produce tofu boxes.
pub fn sanitize_ui_text(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
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

/// Force an element to render on a single line.
///
/// iced 0.13's plain `text` widget ignores `.wrapping(Wrapping::None)` — the
/// graphics `Paragraph` stores the wrapping mode but never calls cosmic-text's
/// `set_wrap`, so a long label always word-wraps to a second line regardless.
/// Clipping the element to exactly one line height (iced's default line height
/// is `1.3 × size`) hides any wrapped overflow; paired with `truncate_ellipsis`
/// this gives "one line, `…` when the text is too long".
pub(super) fn single_line<'a>(
    elem: impl Into<Element<'a, Msg>>,
    text_size: f32,
) -> Element<'a, Msg> {
    container(elem)
        .height(Length::Fixed(text_size * 1.3))
        .clip(true)
        .into()
}

pub(super) fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
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

// ── activity-stream rows ────────────────────────────────────────────────

/// Status glyph replacing the old state dot. All states render as font-free
/// SVG icons (the bundled fonts carry no arc/braille/circle glyphs). `tick` is
/// `Grove::blink_tick` (~60ms): the Working arc spins continuously, and the
/// waiting glyph blinks at ~1Hz (8 ticks on, 8 off) by dimming — never hiding
/// — the icon, so row layout stays stable.
pub fn state_glyph<'a>(state: ActivityState, tick: u32) -> Element<'a, Msg> {
    let inner: Element<'a, Msg> = match state {
        ActivityState::Working => spinner(11.0, c::GREEN(), tick),
        ActivityState::WaitingForInput => {
            // Gentle opacity pulse (~2s): triangle wave over 32 ticks,
            // alpha oscillates between 0.55 and 1.0. No on/off blink.
            let phase = (tick % 32) as f32;
            let t = (phase - 16.0).abs() / 16.0;
            let alpha = 0.55 + 0.45 * t;
            icon(
                "question",
                11.0,
                Color {
                    a: alpha,
                    ..c::AMBER()
                },
            )
        }
        ActivityState::Done => icon("check", 11.0, c::GREEN()),
        ActivityState::Idle => icon("dot", 11.0, c::FG_MUTE()),
        ActivityState::Exited => icon("ring", 11.0, c::FG_MUTE()),
    };
    container(inner).width(14).center_x(14).into()
}

/// Group header row used between activity-stream sections: mono uppercase
/// label, dim count, and a 1px BORDER_SOFT rule extending to the right.
pub fn activity_group_header<'a>(
    label: &str,
    count: usize,
    expanded: bool,
    on_toggle: Option<Msg>,
) -> Element<'a, Msg> {
    let chev = if expanded { "chev-down" } else { "chev-right" };
    let chev_el: Element<'a, Msg> = if on_toggle.is_some() {
        container(icon(chev, 9.0, c::FG_MUTE())).width(12).into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let rule: Element<'a, Msg> = container(Space::with_height(1.0))
        .height(1.0)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BORDER_SOFT())),
            ..Default::default()
        })
        .into();

    let inner = row![
        chev_el,
        text(label.to_uppercase())
            .font(UI_BOLD)
            .size(10)
            .color(c::FG_MUTE())
            .wrapping(iced::widget::text::Wrapping::None),
        text(format!("{count}"))
            .font(UI_FONT)
            .size(10)
            .color(c::FG_MUTE()),
        container(rule).width(Length::Fill).padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 6.0,
            right: 0.0,
        }),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 12.0,
        right: 12.0,
    });

    let h = 26.0;
    let body: Element<'a, Msg> = container(inner)
        .height(h)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        .into();

    if let Some(msg) = on_toggle {
        button(body)
            .on_press(msg)
            .width(Length::Fill)
            .padding(0)
            .style(|_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: c::FG_MUTE(),
                    border: Border::default(),
                    shadow: Shadow::default(),
                }
            })
            .into()
    } else {
        body
    }
}

/// Render a session as a two-line activity row: agent + label up top,
/// `project / worktree · branch` subtitle below, relative time on the right.
/// Active state paints `bg_hl` plus a 2px cyan left-rail.
#[allow(clippy::too_many_arguments)]
pub fn session_activity_row<'a>(
    idx: usize,
    s: &Session,
    project: &str,
    worktree: &str,
    active: bool,
    pending_kill: bool,
    last_activity: Option<Duration>,
    hovered: bool,
    spawn_coords: Option<(usize, usize)>,
    state: ActivityState,
    tick: u32,
    available: &[Agent],
) -> Element<'a, Msg> {
    activity_row_inner(
        Some(idx),
        Some(s),
        state,
        tick,
        s.agent.label(),
        project,
        worktree,
        Some(&s.branch),
        active,
        pending_kill,
        last_activity,
        hovered,
        spawn_coords,
        available,
    )
}

/// Activity-stream row representing a worktree itself: shows
/// `project / worktree` with a session-count subtitle. On hover the
/// new-session action chips appear at the row's right edge; the label fills
/// the remaining width and clips, so a long name never wraps to a second line.
#[allow(clippy::too_many_arguments)]
pub fn worktree_activity_row<'a>(
    proj: usize,
    wt: usize,
    project: &str,
    worktree: &str,
    is_main: bool,
    is_git: bool,
    session_count: usize,
    hovered: bool,
    available: &[Agent],
) -> Element<'a, Msg> {
    let label = format!("{project} / {worktree}");

    let top = text(label)
        .font(UI_FONT)
        .size(12)
        .color(c::FG_DIM())
        .wrapping(iced::widget::text::Wrapping::None);

    let count_label = match session_count {
        0 => "no sessions".to_string(),
        1 => "1 session".to_string(),
        n => format!("{n} sessions"),
    };
    let count_text = text(count_label)
        .font(UI_FONT)
        .size(10)
        .color(c::FG_MUTE())
        .wrapping(iced::widget::text::Wrapping::None);

    let h = ROW_H + SUBTITLE_H;

    // The label fills the row and clips, so a long `project / worktree` name
    // never reflows onto a second line. The session-count subtitle sits below.
    let body = column![
        container(top).width(Length::Fill).clip(true),
        container(count_text).width(Length::Fill).clip(true),
    ]
    .spacing(0);

    // The clickable label is itself the fill target — clicking anywhere left of
    // the (right-edge) chips jumps to the worktree.
    let label_btn = button(body)
        .on_press(Msg::WorktreeClicked { proj, wt })
        .width(Length::Fill)
        .padding(0)
        .style(move |_, _status| button::Style {
            background: None,
            text_color: c::FG_DIM(),
            border: Border::default(),
            shadow: Shadow::default(),
        });

    // On hover the spawn chips occupy a fixed slot at the row's right edge
    // (matching the session rows) rather than growing inline next to the count,
    // which previously pushed long labels into a second line. At rest, the
    // git main worktree shows a `main` tag in that same slot instead — the
    // two never render at once, so there's no collision on hover.
    let right_chips: Element<'a, Msg> = if hovered {
        super::widgets::split_start_button_flat(proj, wt, is_main, 10.0, available)
    } else if is_main && is_git {
        container(main_tag())
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 4.0,
                right: 4.0,
            })
            .into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let inner = row![
        container(state_glyph(ActivityState::Exited, 0))
            .width(14)
            .center_y(Length::Fill),
        label_btn,
        right_chips,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 4.0,
        bottom: 4.0,
        left: 12.0,
        right: 12.0,
    });

    container(inner)
        .height(h)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            ..Default::default()
        })
        .into()
}

#[allow(clippy::too_many_arguments)]
fn activity_row_inner<'a>(
    session_idx: Option<usize>,
    session: Option<&Session>,
    state: ActivityState,
    tick: u32,
    agent_label: &str,
    project: &str,
    worktree: &str,
    branch: Option<&str>,
    active: bool,
    pending_kill: bool,
    last_activity: Option<Duration>,
    hovered: bool,
    spawn_coords: Option<(usize, usize)>,
    available: &[Agent],
) -> Element<'a, Msg> {
    let agent_color = match state {
        ActivityState::Working | ActivityState::WaitingForInput => c::FG(),
        ActivityState::Done | ActivityState::Idle => c::FG_DIM(),
        ActivityState::Exited => c::FG_MUTE(),
    };
    let agent_color = if active { c::CYAN() } else { agent_color };

    // Line 1 is just the agent: the session label is almost always the branch,
    // which already appears in the `project / worktree · branch` subtitle, so
    // repeating it here is noise and steals width from the hover spawn chips.
    let mut top_row = row![].spacing(6).align_y(iced::Alignment::Center);
    if let Some(s) = session {
        top_row = top_row.push(icon(s.agent.icon_name(), 12.0, agent_color));
    }
    top_row = top_row.push(
        text(agent_label.to_string())
            .font(UI_FONT)
            .size(12)
            .color(agent_color)
            .wrapping(iced::widget::text::Wrapping::None),
    );

    // Subtitle: mono "project / worktree" plus optional " · branch" when
    // the branch isn't redundant with the worktree folder name.
    let mut subtitle = format!("{project} / {worktree}");
    if let Some(b) = branch {
        if !b.is_empty() && b != worktree {
            subtitle.push_str(" · ");
            subtitle.push_str(b);
        }
    }

    let sub = text(subtitle)
        .font(UI_FONT)
        .size(10)
        .color(c::FG_MUTE())
        .wrapping(iced::widget::text::Wrapping::None);

    let body = column![container(top_row).clip(true), container(sub).clip(true)].spacing(0);

    let time_label = last_activity.map(format_relative).unwrap_or_default();
    let time_el: Element<'a, Msg> = text(time_label)
        .font(UI_FONT)
        .size(10)
        .color(c::FG_MUTE())
        .into();

    // On hover, the relative time yields its slot to the inline spawn chips —
    // "start another agent in this session's worktree" (play / agent picker /
    // terminal), the same control the worktree rows reveal. Off-hover the time
    // shows, so the row keeps full label width at rest.
    let right_el: Element<'a, Msg> = match (hovered, spawn_coords) {
        (true, Some((pi, wi))) => super::widgets::session_spawn_chips_flat(pi, wi, 10.0, available),
        _ => time_el,
    };

    let close_btn: Element<'a, Msg> = match session_idx {
        Some(i) if pending_kill => action_mini_danger("check", Msg::KillSession(i)),
        Some(i) => action_mini("close", Msg::RequestKillSession(i)),
        None => Space::with_width(Length::Fixed(0.0)).into(),
    };

    let inner = row![
        container(state_glyph(state, tick))
            .width(14)
            .center_y(Length::Fill),
        container(body).width(Length::Fill).clip(true),
        right_el,
        close_btn,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 4.0,
        bottom: 4.0,
        left: 12.0,
        right: 12.0,
    });

    let h = ROW_H + SUBTITLE_H;

    let row_btn: Element<'a, Msg> = if let Some(i) = session_idx {
        clickable_row(inner, h, active, Msg::SelectSession(i))
    } else {
        container(inner).height(h).width(Length::Fill).into()
    };

    let waiting = matches!(state, ActivityState::WaitingForInput);
    let bg = if waiting {
        Some(Background::Color(Color {
            a: 0.12,
            ..c::AMBER()
        }))
    } else if active {
        Some(Background::Color(c::BG_HL()))
    } else {
        None
    };

    if waiting {
        let bar: Element<'a, Msg> = container(
            container(Space::with_width(3.0))
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
        container(iced::widget::stack![row_btn, bar])
            .height(h)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: bg,
                ..Default::default()
            })
            .into()
    } else {
        container(row_btn)
            .height(h)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: bg,
                ..Default::default()
            })
            .into()
    }
}

/// Format a Duration into the short relative-time labels the activity stream
/// uses: `now`, `2m`, `1h`, `3h`, `2d`.
fn format_relative(d: Duration) -> String {
    let s = d.as_secs();
    if s < 30 {
        "now".to_string()
    } else if s < 60 * 60 {
        format!("{}m", (s / 60).max(1))
    } else if s < 60 * 60 * 24 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── worktree_shows_branch ────────────────────────────────────────────────

    /// The main worktree never shows a branch chip, regardless of branch name.
    #[test]
    fn main_worktree_never_shows_branch() {
        assert!(!worktree_shows_branch(true, "main", "main"));
        assert!(!worktree_shows_branch(true, "feature-x", "feature-x"));
        // Even if branch and name differ, is_main wins.
        assert!(!worktree_shows_branch(true, "develop", "myworktree"));
    }

    /// A linked worktree whose branch equals its name is not shown (redundant).
    #[test]
    fn branch_equal_to_name_hidden() {
        assert!(!worktree_shows_branch(false, "feature-x", "feature-x"));
    }

    /// A linked worktree with an empty branch string should not show the chip.
    #[test]
    fn empty_branch_hidden() {
        assert!(!worktree_shows_branch(false, "", "myworktree"));
    }

    /// A linked worktree whose branch differs from its name shows the chip.
    #[test]
    fn branch_different_from_name_shown() {
        assert!(worktree_shows_branch(false, "feature/awesome", "awesome"));
        assert!(worktree_shows_branch(false, "main", "awesome-wt"));
    }

    // ── sanitize_ui_text ─────────────────────────────────────────────────────

    /// Emoji and non-Latin glyphs are stripped; runs of whitespace are collapsed.
    #[test]
    fn sanitize_strips_emoji_and_collapses_spaces() {
        // Input: emoji prefix + content that survives
        let result = sanitize_ui_text("🚀 Review pull request");
        assert_eq!(result, Some("Review pull request".into()));
    }

    /// A string that is only stripped characters returns `None`.
    #[test]
    fn sanitize_empty_after_strip_returns_none() {
        assert_eq!(sanitize_ui_text("🎉🎊"), None);
        assert_eq!(sanitize_ui_text(""), None);
    }

    /// Plain ASCII content passes through unchanged.
    #[test]
    fn sanitize_plain_ascii_passes_through() {
        let result = sanitize_ui_text("fix: handle edge case");
        assert_eq!(result, Some("fix: handle edge case".into()));
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
