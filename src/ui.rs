use crate::agent::Agent;
use crate::app::{App, InputKind, Modal, Pane, UiMode};
use crate::session::SessionStatus;
use crate::theme;
use tui_term::widget::PseudoTerminal;
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

/// Inner rect of the agent pane for a given full-frame area. Mirrors the
/// layout split in `render`; the event loop uses it to size the active PTY.
pub fn agent_pane_inner(frame: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame);
    let h = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(v[1]);
    let a = h[1];
    Rect {
        x: a.x + 1,
        y: a.y + 1,
        width: a.width.saturating_sub(2),
        height: a.height.saturating_sub(2),
    }
}

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    // Paint background across the whole frame.
    f.render_widget(Block::default().style(base_style()), size);

    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(size);

    render_banner(f, app, vchunks[0]);

    if app.store.projects.is_empty() {
        render_empty(f, vchunks[1]);
    } else {
        let constraints = match app.ui_mode {
            UiMode::Browser => [Constraint::Percentage(30), Constraint::Percentage(70)],
            UiMode::Agent | UiMode::SessionList => {
                [Constraint::Percentage(30), Constraint::Percentage(70)]
            }
        };
        let hchunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(vchunks[1]);

        match app.ui_mode {
            UiMode::Browser => {
                render_projects(f, app, hchunks[0]);
                render_worktrees(f, app, hchunks[1]);
            }
            UiMode::Agent | UiMode::SessionList => {
                render_session_list(f, app, hchunks[0]);
                render_agent(f, app, hchunks[1]);
            }
        }
    }
    render_status(f, app, vchunks[2]);
    render_footer(f, app, vchunks[3]);

    match &app.modal {
        Modal::None => {}
        Modal::Input { title, buffer, kind, dir_sel } => render_input(f, title, buffer, kind, *dir_sel, size),
        Modal::Confirm { prompt, .. } => render_confirm(f, prompt, size),
        Modal::Message(msg) => render_message(f, msg, size),
        Modal::Help => render_help(f, size),
        Modal::AgentPicker { project, wt_path, sel } => render_agent_picker(f, app, project, wt_path, *sel, size),
    }
}

fn render_agent_picker(f: &mut Frame, app: &App, project: &str, wt_path: &str, sel: usize, area: Rect) {
    let r = centered(area, 60, 10);
    f.render_widget(Clear, r);
    let wt_name = crate::app::path_basename(wt_path);
    let title = if project.is_empty() {
        format!(" Start session · {} ", wt_name)
    } else {
        format!(" Start session · {} / {} ", project, wt_name)
    };
    let block = Block::default()
        .title(title)
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
            "j/k move   ↵ launch   space default   esc cancel",
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

const BANNER: [&str; 3] = [
    "┌─┐┬─┐┌─┐┬  ┬┌─┐",
    "│ ┬├┬┘│ │└┐┌┘├┤ ",
    "└─┘┴└─└─┘ └┘ └─┘",
];

fn render_banner(f: &mut Frame, app: &App, area: Rect) {
    let _ = app;
    let banner_w: u16 = 18;
    let hchunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(banner_w), Constraint::Min(10)])
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

    let key = Style::default().fg(theme::COMMENT);
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

        app.store
            .projects
            .iter()
            .zip(running_by_project)
            .map(|(p, running)| {
                let count_style = if running > 0 {
                    Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::COMMENT)
                };
                let spans = vec![
                    Span::styled(format!("{running:>count_width$}"), count_style),
                    Span::raw("  "),
                    Span::styled(p.name.clone(), Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
                ];
                ListItem::new(Line::from(spans))
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
            let short = crate::app::path_basename(&w.path);
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
            let running = app
                .sessions
                .iter()
                .filter(|s| s.wt_path == w.path && s.status() == SessionStatus::Running)
                .count();
            if running > 0 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("●{running}"),
                    Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD),
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

fn render_session_list(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.ui_mode == UiMode::SessionList;
    let block = Block::default()
        .title(" Sessions ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(active_style(focused))
        .style(base_style());

    if app.sessions.is_empty() {
        let items = vec![ListItem::new(Line::from(Span::styled(
            "no sessions",
            Style::default().fg(theme::FG_DARK).add_modifier(Modifier::ITALIC),
        )))];
        let list = List::new(items).block(block).style(base_style()).highlight_symbol("  ");
        f.render_stateful_widget(list, area, &mut ListState::default());
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

    for proj in projects {
        items.push(ListItem::new(Line::from(Span::styled(
            proj.to_string(),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ))));

        // Worktree paths for this project, in first-seen order.
        let mut wts: Vec<&str> = Vec::new();
        for s in &app.sessions {
            if s.project == proj && !wts.contains(&s.wt_path.as_str()) {
                wts.push(&s.wt_path);
            }
        }

        for (wi, wt) in wts.iter().enumerate() {
            let last_wt = wi + 1 == wts.len();
            let (branch, child) = if last_wt { ("└ ", "   ") } else { ("├ ", "│  ") };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(branch, Style::default().fg(theme::COMMENT)),
                Span::styled(
                    crate::app::path_basename(wt),
                    Style::default().fg(theme::CYAN),
                ),
            ])));

            for (i, s) in app.sessions.iter().enumerate() {
                if s.project != proj || s.wt_path != **wt {
                    continue;
                }
                if app.active_session == Some(i) {
                    sel_display = Some(items.len());
                }
                let (dot, dot_style) = match s.status() {
                    SessionStatus::Running => ("●", Style::default().fg(theme::GREEN)),
                    SessionStatus::Exited(_) => ("○", Style::default().fg(theme::FG_DARK)),
                };
                let idx_str = format!("{} ", i + 1);
                let mut header_spans = vec![
                    Span::styled(child, Style::default().fg(theme::COMMENT)),
                    Span::styled(idx_str.clone(), Style::default().fg(theme::COMMENT)),
                    Span::styled(dot, dot_style),
                    Span::raw(" "),
                    Span::styled(
                        s.agent.label(),
                        Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                    ),
                ];
                if !s.branch.is_empty() {
                    let branch_str = format!("[{}]", s.branch);
                    // List inner width = area.width - 2 (borders) - 2 (highlight symbol).
                    let usable = area.width.saturating_sub(4) as usize;
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
                        Style::default().fg(theme::CYAN),
                    ));
                }
                let header = Line::from(header_spans);
                let subtitle = s.current_title().filter(|t| {
                    !t.eq_ignore_ascii_case(&s.label)
                        && !t.eq_ignore_ascii_case(s.agent.label())
                });
                let lines = match subtitle {
                    Some(t) => vec![
                        header,
                        Line::from(vec![
                            Span::styled(child, Style::default().fg(theme::COMMENT)),
                            Span::raw("     "),
                            Span::styled(
                                t,
                                Style::default()
                                    .fg(theme::FG_DARK)
                                    .add_modifier(Modifier::DIM),
                            ),
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
                .bg(theme::BLUE)
                .fg(theme::FG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(sel_display);
    f.render_stateful_widget(list, area, &mut state);
}

fn render_agent(f: &mut Frame, app: &App, area: Rect) {
    let Some(idx) = app.active_session else { return };
    let Some(session) = app.sessions.get(idx) else { return };

    let focused = app.ui_mode == UiMode::Agent;
    let (title, border) = match session.status() {
        SessionStatus::Running => {
            let style = if focused {
                Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::COMMENT)
            };
            let osc = session.current_title().filter(|t| {
                !t.eq_ignore_ascii_case(&session.label)
                    && !t.eq_ignore_ascii_case(session.agent.label())
            });
            let t = match osc {
                Some(osc) => format!(" {} · {} ({}) ", session.project, session.label, osc),
                None => format!(" {} · {} ", session.project, session.label),
            };
            (t, style)
        }
        SessionStatus::Exited(code) => {
            let style = if focused {
                Style::default().fg(theme::RED).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::COMMENT)
            };
            (
                format!(
                    " {} · {} — exited ({}) · Ctrl-g for sidebar ",
                    session.project,
                    session.label,
                    code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
                style,
            )
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

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let p = Paragraph::new(app.status.clone())
        .style(Style::default().bg(theme::BG).fg(theme::GREEN));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let pairs: &[(&str, &str)] = match app.ui_mode {
        UiMode::Agent => &[
            ("Ctrl-g", " sidebar  "),
            ("(all other keys forwarded to the agent)", ""),
        ],
        UiMode::SessionList => &[
            ("Ctrl-g/↵", " session  "),
            ("j/k", " cycle  "),
            ("1-9", " jump  "),
            ("x", " kill  "),
            ("c/C", " new  "),
            ("t", " term  "),
            ("esc", " browser  "),
            ("?", " help  "),
            ("q", " quit"),
        ],
        UiMode::Browser => &[
            ("h/l", " pane  "),
            ("a", " add  "),
            ("d", " del  "),
            ("↵", " open  "),
            ("P", " pick  "),
            ("Ctrl-g", " session  "),
            ("r", " refresh  "),
            ("?", " help  "),
            ("q", " quit"),
        ],
    };
    let key = Style::default().fg(theme::CYAN);
    let dim = Style::default().fg(theme::FG_DARK);
    let spans: Vec<Span> = pairs
        .iter()
        .flat_map(|(k, d)| [Span::styled(*k, key), Span::styled(*d, dim)])
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)).style(base_style()), area);
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
    let r = centered(area, 64, 24);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::CYAN))
        .style(base_style());
    let lines = vec![
        Line::from(Span::styled(
            "Browser",
            Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from("j/k or ↑/↓     move"),
        Line::from("h / l / ← / →  focus projects / worktrees   tab toggles"),
        Line::from("enter          focus worktrees / open session (uses default)"),
        Line::from("P              open worktree, always show session picker"),
        Line::from("a / d          add / delete   r  refresh"),
        Line::from("Ctrl-g         jump to active session pane"),
        Line::from("q / esc        quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Sessions sidebar",
            Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from("Ctrl-g / ↵     focus the session pane"),
        Line::from("esc            back to projects browser"),
        Line::from("j / k          next / prev session"),
        Line::from("1-9            jump to session N"),
        Line::from("x              kill active session"),
        Line::from("c / C          new session for active (default / pick)"),
        Line::from("t              new terminal for active session's worktree"),
        Line::from(""),
        Line::from(Span::styled(
            "Session pane",
            Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from("Ctrl-g         move focus to the Sessions sidebar"),
        Line::from("(all other keys are forwarded to the agent)"),
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
