use crate::agent::Agent;
use crate::app::{App, InputKind, Modal, Pane};
use crate::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

fn base_style() -> Style {
    Style::default().bg(theme::BG).fg(theme::FG)
}

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    // Paint background across the whole frame.
    f.render_widget(Block::default().style(base_style()), size);

    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(size);

    render_banner(f, app, vchunks[0]);

    if app.store.projects.is_empty() {
        render_empty(f, vchunks[1]);
    } else {
        let hchunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(vchunks[1]);

        render_projects(f, app, hchunks[0]);
        render_worktrees(f, app, hchunks[1]);
    }
    render_status(f, app, vchunks[2]);
    render_footer(f, vchunks[3]);

    match &app.modal {
        Modal::None => {}
        Modal::Input { title, buffer, kind, dir_sel } => render_input(f, title, buffer, kind, *dir_sel, size),
        Modal::Confirm { prompt, .. } => render_confirm(f, prompt, size),
        Modal::Message(msg) => render_message(f, msg, size),
        Modal::Help => render_help(f, size),
        Modal::AgentPicker { sel, .. } => render_agent_picker(f, app, *sel, size),
    }
}

fn render_agent_picker(f: &mut Frame, app: &App, sel: usize, area: Rect) {
    let r = centered(area, 60, 10);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Start agent ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::MAGENTA))
        .style(base_style());

    let items: Vec<ListItem> = Agent::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_default = app.store.default_agent == Some(*a);
            let marker = if is_default { " ★ default" } else { "" };
            let prefix = if i == sel { "▶ " } else { "  " };
            let style = if i == sel {
                Style::default().fg(theme::FG).bg(theme::BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG)
            };
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::styled(a.label(), style),
                Span::styled(marker, Style::default().fg(theme::YELLOW)),
            ]))
        })
        .collect();

    f.render_widget(block, r);
    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    f.render_widget(List::new(items).style(base_style()), inner);

    let hint = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↵ launch   d toggle default   esc cancel",
            Style::default().fg(theme::COMMENT),
        )))
        .style(base_style()),
        hint,
    );
}

fn active_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(theme::BG).fg(theme::CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(theme::BG).fg(theme::COMMENT)
    }
}

const BANNER: [&str; 5] = [
    "   ____                       ",
    "  / ___|_ __ _____   _____    ",
    " | |  _| '__/ _ \\ \\ / / _ \\ ",
    " | |_| | | | (_) \\ V /  __/  ",
    "  \\____|_|  \\___/ \\_/ \\___| ",
];

fn render_banner(f: &mut Frame, app: &App, area: Rect) {
    let hchunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(10)])
        .split(area);

    let banner_lines: Vec<Line> = BANNER
        .iter()
        .map(|l| Line::from(Span::styled(*l, Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD))))
        .collect();
    f.render_widget(
        Paragraph::new(banner_lines).style(base_style()),
        hchunks[0],
    );

    let version = env!("CARGO_PKG_VERSION");
    let projects = app.store.projects.len();
    let worktrees: usize = app
        .store
        .projects
        .iter()
        .map(|p| {
            crate::git::list_worktrees(&p.path)
                .iter()
                .filter(|w| !w.is_main)
                .count()
        })
        .sum();
    let default_agent = app
        .store
        .default_agent
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| "—".into());

    let key = Style::default().fg(theme::COMMENT);
    let val = Style::default().fg(theme::FG).add_modifier(Modifier::BOLD);
    let accent = Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD);

    let info = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Grove", accent),
            Span::raw("  "),
            Span::styled(format!("v{}", version), Style::default().fg(theme::YELLOW)),
            Span::raw("  "),
            Span::styled("· worktree launchpad for ai agents", key),
        ]),
        Line::from(vec![
            Span::styled("projects ", key),
            Span::styled(format!("{:>3}", projects), val),
            Span::raw("  "),
            Span::styled("worktrees ", key),
            Span::styled(format!("{:>3}", worktrees), val),
            Span::raw("  "),
            Span::styled("default agent ", key),
            Span::styled(default_agent, val),
        ]),
        Line::from(vec![
            Span::styled("press ", key),
            Span::styled("?", accent),
            Span::styled(" for help", key),
        ]),
    ];
    f.render_widget(
        Paragraph::new(info).style(base_style()),
        hchunks[1],
    );
}

fn render_empty(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" grove ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::CYAN))
        .style(base_style());
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No projects yet.",
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("Press "),
            Span::styled("a", Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD)),
            Span::raw(" to add a project, "),
            Span::styled("?", Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD)),
            Span::raw(" for help, "),
            Span::styled("q", Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD)),
            Span::raw(" to quit."),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).alignment(Alignment::Center).style(base_style()),
        area,
    );
}

fn render_projects(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Pane::Projects;
    let items: Vec<ListItem> = {
        app.store
            .projects
            .iter()
            .map(|p| {
                ListItem::new(Line::from(vec![
                    Span::styled(p.name.clone(), Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled(p.path.clone(), Style::default().fg(theme::COMMENT)),
                ]))
            })
            .collect()
    };

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
                .bg(if focused { theme::BLUE } else { theme::BG_HIGHLIGHT })
                .fg(theme::FG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if app.store.projects.is_empty() { "  " } else { "▶ " });

    let mut state = ListState::default();
    if !app.store.projects.is_empty() {
        state.select(Some(app.proj_idx));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn render_worktrees(f: &mut Frame, app: &App, area: Rect) {
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
            Style::default().fg(theme::FG_DARK).add_modifier(Modifier::ITALIC),
        )))]
    } else { app
        .worktrees
        .iter()
        .map(|w| {
            let short = std::path::Path::new(&w.path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&w.path)
                .to_string();
            let age = w.mtime.map(format_age).unwrap_or_else(|| "—".into());
            let mut spans = vec![
                Span::styled(short, Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(
                    format!("[{}]", w.branch),
                    Style::default().fg(theme::YELLOW),
                ),
                Span::raw("  "),
                Span::styled(format!("{:>5} ago", age), Style::default().fg(theme::GREEN)),
                Span::raw("  "),
                Span::styled(w.path.clone(), Style::default().fg(theme::COMMENT)),
            ];
            if w.is_main {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    "● main checkout",
                    Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD),
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
                .bg(if focused { theme::BLUE } else { theme::BG_HIGHLIGHT })
                .fg(theme::FG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if placeholder.is_some() { "  " } else { "▶ " });

    let mut state = ListState::default();
    if !app.worktrees.is_empty() {
        state.select(Some(app.wt_idx));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let p = Paragraph::new(app.status.clone())
        .style(Style::default().bg(theme::BG).fg(theme::GREEN));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let key = Style::default().fg(theme::CYAN);
    let dim = Style::default().fg(theme::FG_DARK);
    let line = Line::from(vec![
        Span::styled("tab", key),
        Span::styled(" switch  ", dim),
        Span::styled("a", key),
        Span::styled(" add  ", dim),
        Span::styled("d", key),
        Span::styled(" del  ", dim),
        Span::styled("↵", key),
        Span::styled(" open  ", dim),
        Span::styled("r", key),
        Span::styled(" refresh  ", dim),
        Span::styled("?", key),
        Span::styled(" help  ", dim),
        Span::styled("q", key),
        Span::styled(" quit", dim),
    ]);
    f.render_widget(Paragraph::new(line).style(base_style()), area);
}

fn format_age(t: std::time::SystemTime) -> String {
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

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

fn render_input(f: &mut Frame, title: &str, buffer: &str, kind: &InputKind, dir_sel: usize, area: Rect) {
    let show_dirs = matches!(kind, InputKind::AddProjectPath);
    let height = if show_dirs { 16 } else { 5 };
    let r = centered(area, 70, height);
    f.render_widget(Clear, r);
    f.render_widget(Block::default().style(base_style()), r);
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::MAGENTA))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 1,
        y: r.y + 1,
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(2),
    };

    let input_area = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
    let text = format!("{}_", buffer);
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).style(base_style()),
        input_area,
    );

    if show_dirs && inner.height > 2 {
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height - 2,
        };
        let entries = crate::app::list_dirs(buffer);
        if entries.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "(no matches)",
                    Style::default().fg(theme::COMMENT).add_modifier(Modifier::ITALIC),
                ))
                .style(base_style()),
                list_area,
            );
        } else {
            let items: Vec<ListItem> = entries
                .into_iter()
                .map(|s| ListItem::new(Span::styled(s, Style::default().fg(theme::CYAN))))
                .collect();
            let list = List::new(items)
                .style(base_style())
                .highlight_style(
                    Style::default().bg(theme::BLUE).fg(theme::FG).add_modifier(Modifier::BOLD),
                );
            let mut state = ListState::default();
            state.select(Some(dir_sel));
            f.render_stateful_widget(list, list_area, &mut state);
        }
    }
}

fn render_confirm(f: &mut Frame, prompt: &str, area: Rect) {
    let r = centered(area, 70, 6);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::RED))
        .style(base_style());
    let lines = vec![
        Line::from(prompt.to_string()),
        Line::from(""),
        Line::from(Span::styled(
            "y = yes    n / esc = no",
            Style::default().fg(theme::COMMENT),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }).style(base_style()),
        r,
    );
}

fn render_message(f: &mut Frame, msg: &str, area: Rect) {
    let r = centered(area, 70, 6);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Notice ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::YELLOW))
        .style(base_style());
    let lines = vec![
        Line::from(msg.to_string()),
        Line::from(""),
        Line::from(Span::styled(
            "press any key",
            Style::default().fg(theme::COMMENT),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }).style(base_style()),
        r,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let r = centered(area, 60, 14);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::CYAN))
        .style(base_style());
    let lines = vec![
        Line::from("j/k or ↑/↓     move"),
        Line::from("tab / h / l    switch pane"),
        Line::from("enter          focus worktrees / open agent (uses default)"),
        Line::from("shift+enter    open worktree, always show agent picker"),
        Line::from("a              add project / worktree"),
        Line::from("d              delete (with confirm)"),
        Line::from("r              refresh worktrees"),
        Line::from("?              this help"),
        Line::from("q / esc        quit"),
        Line::from(""),
        Line::from(Span::styled(
            "press any key to close",
            Style::default().fg(theme::COMMENT),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).alignment(Alignment::Left).style(base_style()),
        r,
    );
}
