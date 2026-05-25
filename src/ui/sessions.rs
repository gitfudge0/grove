//! Sessions sidebar (grouped by project/worktree) and the active-session
//! PTY pane.

use super::common::{active_style, base_style, hint_line, truncate_title};
use crate::app::{App, UiMode};
use crate::session::SessionStatus;
use crate::theme;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;

pub fn render_session_list(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.ui_mode == UiMode::SessionList;
    let running = app
        .sessions
        .iter()
        .filter(|s| s.status() == SessionStatus::Running)
        .count();
    let title = if app.sessions.is_empty() {
        " Sessions ".to_string()
    } else {
        format!(" Sessions · {} ", running)
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(active_style(focused))
        .style(base_style());

    if app.sessions.is_empty() {
        f.render_widget(block.clone(), area);
        let inner = block.inner(area);
        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
        f.render_widget(
            Paragraph::new(Span::styled(
                "no sessions yet",
                Style::default()
                    .fg(theme::current().fg_dark)
                    .add_modifier(Modifier::ITALIC),
            ))
            .alignment(Alignment::Center)
            .style(base_style()),
            layout[1],
        );
        f.render_widget(
            Paragraph::new(hint_line(&[("esc", "browser"), ("?", "help")]))
                .alignment(Alignment::Center)
                .style(base_style()),
            layout[2],
        );
        return;
    }

    // Build a project → worktree → session tree, preserving first-seen order.
    // Each emitted row is either a non-selectable header or a session row that
    // carries its original `app.sessions` index (for selection / jump keys).
    let mut items: Vec<ListItem> = Vec::new();
    let mut sel_display: Option<usize> = None;

    let mut projects: Vec<&str> = Vec::new();
    for s in &app.sessions {
        if !projects.contains(&s.project.as_str()) {
            projects.push(&s.project);
        }
    }

    for (pi, proj) in projects.iter().enumerate() {
        if pi > 0 {
            items.push(ListItem::new(Line::from("")));
        }
        items.push(ListItem::new(Line::from(Span::styled(
            proj.to_string(),
            Style::default()
                .fg(theme::current().fg)
                .add_modifier(Modifier::BOLD),
        ))));

        let mut wts: Vec<&str> = Vec::new();
        for s in &app.sessions {
            if s.project == *proj && !wts.contains(&s.wt_path.as_str()) {
                wts.push(&s.wt_path);
            }
        }

        for (wi, wt) in wts.iter().enumerate() {
            let last_wt = wi + 1 == wts.len();
            let (branch, child) = if last_wt {
                ("└ ", "   ")
            } else {
                ("├ ", "│  ")
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(branch, Style::default().fg(theme::current().comment)),
                Span::styled(
                    crate::app::path_basename(wt),
                    Style::default().fg(theme::current().cyan),
                ),
            ])));

            for (i, s) in app.sessions.iter().enumerate() {
                if s.project != *proj || s.wt_path != **wt {
                    continue;
                }
                if app.active_session == Some(i) {
                    sel_display = Some(items.len());
                }
                let (dot, dot_style) = match s.status() {
                    SessionStatus::Running => ("●", Style::default().fg(theme::current().green)),
                    SessionStatus::Exited(Some(c)) if c != 0 => {
                        ("○", Style::default().fg(theme::current().red))
                    }
                    SessionStatus::Exited(_) => {
                        ("○", Style::default().fg(theme::current().fg_dark))
                    }
                };
                let idx_str = format!("{} ", i + 1);
                let mut header_spans = vec![
                    Span::styled(child, Style::default().fg(theme::current().comment)),
                    Span::styled(
                        idx_str.clone(),
                        Style::default().fg(theme::current().comment),
                    ),
                    Span::styled(dot, dot_style),
                    Span::raw(" "),
                    Span::styled(
                        s.agent.label(),
                        Style::default()
                            .fg(theme::current().fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];
                if !s.branch.is_empty() {
                    let branch_str = format!("[{}]", s.branch);
                    // List inner width = area.width - 2 (borders). No highlight symbol.
                    let usable = area.width.saturating_sub(2) as usize;
                    let used = child.chars().count()
                        + idx_str.chars().count()
                        + 2  // dot + space
                        + s.agent.label().chars().count();
                    let pad = usable
                        .saturating_sub(used)
                        .saturating_sub(branch_str.chars().count())
                        .max(1);
                    header_spans.push(Span::raw(" ".repeat(pad)));
                    header_spans.push(Span::styled(
                        branch_str,
                        Style::default().fg(theme::current().cyan),
                    ));
                }
                let header = Line::from(header_spans);
                let subtitle = s.current_title().filter(|t| {
                    !t.eq_ignore_ascii_case(&s.label) && !t.eq_ignore_ascii_case(s.agent.label())
                });
                let lines = match subtitle {
                    Some(t) => vec![
                        header,
                        Line::from(vec![
                            Span::styled(child, Style::default().fg(theme::current().comment)),
                            Span::raw("     "),
                            Span::styled(t, Style::default().fg(theme::current().fg_dark)),
                        ]),
                    ],
                    None => vec![header],
                };
                items.push(ListItem::new(lines));
            }
        }
    }

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
        .highlight_symbol("");

    let mut state = ListState::default();
    state.select(sel_display);
    f.render_stateful_widget(list, area, &mut state);
}

pub fn render_agent(f: &mut Frame, app: &App, area: Rect) {
    let Some(idx) = app.active_session else {
        return;
    };
    let Some(session) = app.sessions.get(idx) else {
        return;
    };

    let focused = app.ui_mode == UiMode::Agent;
    let title_budget = area.width.saturating_sub(4) as usize;
    let (title, border) = match session.status() {
        SessionStatus::Running => {
            let style = if focused {
                Style::default()
                    .fg(theme::current().cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::current().comment)
            };
            let osc = session.current_title().filter(|t| {
                !t.eq_ignore_ascii_case(&session.label)
                    && !t.eq_ignore_ascii_case(session.agent.label())
            });
            let base = match &osc {
                Some(osc) => format!(" {} · {} ({}) ", session.project, session.label, osc),
                None => format!(" {} · {} ", session.project, session.label),
            };
            let t = if base.chars().count() > title_budget {
                let short = format!(" {} · {} ", session.project, session.label);
                if short.chars().count() > title_budget {
                    truncate_title(&short, title_budget)
                } else {
                    short
                }
            } else {
                base
            };
            (t, style)
        }
        SessionStatus::Exited(code) => {
            let failed = matches!(code, Some(c) if c != 0);
            let style = if failed {
                if focused {
                    Style::default()
                        .fg(theme::current().red)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::current().red)
                }
            } else if focused {
                Style::default()
                    .fg(theme::current().fg_dark)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::current().comment)
            };
            let raw = format!(
                " {} · {} — exited ({}) ",
                session.project,
                session.label,
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            );
            (truncate_title(&raw, title_budget), style)
        }
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .style(base_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Ok(parser) = session.parser.lock() {
        let screen = parser.screen();
        f.render_widget(PseudoTerminal::new(screen), inner);
    }

    // Overlay the active text selection by reversing the affected cells.
    if let Some(sel) = app.selection {
        let ((sc, sr), (ec, er)) = sel.normalized();
        let last_col = inner.width.saturating_sub(1);
        let buf = f.buffer_mut();
        for row in sr..=er.min(inner.height.saturating_sub(1)) {
            let (from, to) = if sr == er {
                (sc, ec)
            } else if row == sr {
                (sc, last_col)
            } else if row == er {
                (0, ec)
            } else {
                (0, last_col)
            };
            for col in from..=to.min(last_col) {
                if let Some(cell) = buf.cell_mut((inner.x + col, inner.y + row)) {
                    cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}
