//! Shared TUI helpers: styles, layout math, and text formatters used by
//! every render module.

use crate::app::{App, UiMode};
use crate::theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

pub fn base_style() -> Style {
    Style::default()
        .bg(theme::current().bg)
        .fg(theme::current().fg)
}

pub fn active_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .bg(theme::current().bg)
            .fg(theme::current().cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(theme::current().bg)
            .fg(theme::current().comment)
    }
}

pub fn chrome_visible(app: &App) -> bool {
    let on_session_page = matches!(app.ui_mode, UiMode::Agent | UiMode::SessionList);
    !on_session_page || app.chrome_visible
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub fn popup_title(text: &str, color: ratatui::style::Color) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

/// Build a footer hint line: `key  verb   key  verb` with FG_DARK keys and
/// COMMENT verbs separated by three spaces.
pub fn hint_line(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(pairs.len() * 4);
    for (i, (key, verb)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                "   ",
                Style::default().fg(theme::current().comment),
            ));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(theme::current().fg_dark)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            verb.to_string(),
            Style::default().fg(theme::current().comment),
        ));
    }
    Line::from(spans)
}

pub fn truncate_title(s: &str, budget: usize) -> String {
    if budget < 4 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= budget {
        return s.to_string();
    }
    let keep = budget.saturating_sub(2); // leading space + ellipsis
    let mut out = String::with_capacity(keep + 2);
    out.push(' ');
    for (i, ch) in s.chars().enumerate() {
        if i == 0 {
            continue;
        }
        if out.chars().count() >= keep {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out.push(' ');
    out
}

pub fn format_age(t: std::time::SystemTime) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 86_400 * 30 {
        format!("{}d", secs / 86_400)
    } else if secs < 86_400 * 365 {
        format!("{}mo", secs / (86_400 * 30))
    } else {
        format!("{}y", secs / (86_400 * 365))
    }
}
