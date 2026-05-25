//! Browser-mode panes: the project list and the worktree list.

use super::common::{active_style, base_style, format_age};
use crate::app::{App, Pane};
use crate::session::SessionStatus;
use crate::theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState},
    Frame,
};

pub fn render_projects(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Pane::Projects;
    let running_by_project: Vec<usize> = app
        .store
        .projects
        .iter()
        .map(|p| {
            app.sessions
                .iter()
                .filter(|s| s.project == p.name && s.status() == SessionStatus::Running)
                .count()
        })
        .collect();
    let count_width = running_by_project
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .to_string()
        .len()
        .max(1);

    let items: Vec<ListItem> = app
        .store
        .projects
        .iter()
        .zip(running_by_project)
        .map(|(p, running)| {
            let count_style = if running > 0 {
                Style::default()
                    .fg(theme::current().green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::current().comment)
            };
            let spans = vec![
                Span::styled(format!("{running:>count_width$}"), count_style),
                Span::raw("  "),
                Span::styled(
                    p.name.clone(),
                    Style::default()
                        .fg(theme::current().fg)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::default()
        .title(" Projects ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(active_style(focused))
        .style(base_style());

    let list = List::new(items)
        .block(block)
        .style(base_style())
        .highlight_style(
            Style::default()
                .bg(if focused {
                    theme::current().blue
                } else {
                    theme::current().bg_highlight
                })
                .fg(theme::current().fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if app.store.projects.is_empty() {
            "  "
        } else {
            "▶ "
        });

    let mut state = ListState::default();
    if !app.store.projects.is_empty() {
        state.select(Some(app.proj_idx));
    }
    f.render_stateful_widget(list, area, &mut state);
}

pub fn render_worktrees(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Pane::Worktrees;
    let title = match app.selected_project() {
        Some(p) => format!(" Worktrees · {} ", p.name),
        None => " Worktrees ".into(),
    };

    let placeholder = if app.selected_project().is_none() {
        Some("no project selected")
    } else if app.worktrees.is_empty() {
        Some("no worktrees — press 'a' to add")
    } else {
        None
    };

    let items: Vec<ListItem> = if let Some(msg) = placeholder {
        vec![ListItem::new(Line::from(Span::styled(
            msg,
            Style::default()
                .fg(theme::current().fg_dark)
                .add_modifier(Modifier::ITALIC),
        )))]
    } else {
        app.worktrees
            .iter()
            .map(|w| {
                let short = crate::app::path_basename(&w.path);
                let age = w.mtime.map(format_age).unwrap_or_else(|| "—".into());
                let mut spans = vec![
                    Span::styled(
                        short,
                        Style::default()
                            .fg(theme::current().fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("[{}]", w.branch),
                        Style::default().fg(theme::current().yellow),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:>5} ago", age),
                        Style::default().fg(theme::current().green),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        w.path.clone(),
                        Style::default().fg(theme::current().comment),
                    ),
                ];
                if w.is_main {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        "● main checkout",
                        Style::default()
                            .fg(theme::current().cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                let running = app
                    .sessions
                    .iter()
                    .filter(|s| s.wt_path == w.path && s.status() == SessionStatus::Running)
                    .count();
                if running > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("●{running}"),
                        Style::default()
                            .fg(theme::current().green)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(active_style(focused))
        .style(base_style());

    let list = List::new(items)
        .block(block)
        .style(base_style())
        .highlight_style(
            Style::default()
                .bg(if focused {
                    theme::current().blue
                } else {
                    theme::current().bg_highlight
                })
                .fg(theme::current().fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if placeholder.is_some() { "  " } else { "▶ " });

    let mut state = ListState::default();
    if !app.worktrees.is_empty() {
        state.select(Some(app.wt_idx));
    }
    f.render_stateful_widget(list, area, &mut state);
}
