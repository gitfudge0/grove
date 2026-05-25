//! Top banner shown above the panes and the empty-state panel when there
//! are no projects yet.

use super::common::base_style;
use crate::app::App;
use crate::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

const BANNER: [&str; 3] = ["┌─┐┬─┐┌─┐┬  ┬┌─┐", "│ ┬├┬┘│ │└┐┌┘├┤ ", "└─┘┴└─└─┘ └┘ └─┘"];

pub fn render_banner(f: &mut Frame, app: &App, area: Rect) {
    let banner_w: u16 = 18;
    let hchunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(banner_w), Constraint::Min(10)])
        .split(area);

    let banner_lines: Vec<Line> = BANNER
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                *l,
                Style::default()
                    .fg(theme::current().green)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(banner_lines).style(base_style()), hchunks[0]);

    let version = env!("CARGO_PKG_VERSION");
    let key = Style::default().fg(theme::current().comment);
    let accent = Style::default()
        .fg(theme::current().cyan)
        .add_modifier(Modifier::BOLD);

    let backend_line = if !app.tmux_available {
        Line::from(vec![
            Span::styled(
                "tmux recommended: ",
                Style::default()
                    .fg(theme::current().yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("native sessions will not persist", key),
        ])
    } else if !app.use_tmux() {
        Line::from(vec![
            Span::styled(
                "native sessions",
                Style::default().fg(theme::current().yellow),
            ),
            Span::styled("  · press ", key),
            Span::styled("m", accent),
            Span::styled(" to enable tmux", key),
        ])
    } else {
        Line::from(vec![
            Span::styled("press ", key),
            Span::styled("?", accent),
            Span::styled(" for help", key),
        ])
    };

    let info = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Grove", accent),
            Span::raw("  "),
            Span::styled(
                format!("v{}", version),
                Style::default().fg(theme::current().yellow),
            ),
            Span::raw("  "),
            Span::styled("· worktree launchpad for ai agents", key),
        ]),
        backend_line,
    ];
    f.render_widget(Paragraph::new(info).style(base_style()), hchunks[1]);
}

pub fn render_empty(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" grove ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
    let accent = Style::default()
        .fg(theme::current().cyan)
        .add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No projects yet.",
            Style::default()
                .fg(theme::current().fg)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("Press "),
            Span::styled("a", accent),
            Span::raw(" to add a project, "),
            Span::styled("?", accent),
            Span::raw(" for help, "),
            Span::styled("q", accent),
            Span::raw(" to quit."),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center)
            .style(base_style()),
        area,
    );
}
