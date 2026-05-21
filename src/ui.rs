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
    render_toast(f, app, size);

    match &app.modal {
        Modal::None => {}
        Modal::Input { title, buffer, kind, dir_sel } => render_input(f, title, buffer, kind, *dir_sel, size),
        Modal::Confirm { title, prompt, destructive, .. } => {
            render_confirm(f, title, prompt, *destructive, size)
        }
        Modal::Message(msg) => render_message(f, msg, size),
        Modal::Help => render_help(f, size),
        Modal::AgentPicker { project, wt_path, sel } => render_agent_picker(f, app, project, wt_path, *sel, size),
    }
}

fn render_agent_picker(f: &mut Frame, app: &App, project: &str, wt_path: &str, sel: usize, area: Rect) {
    let wt_name = crate::app::path_basename(wt_path);
    let title = if project.is_empty() {
        format!(" Start session · {} ", wt_name)
    } else {
        format!(" Start session · {} / {} ", project, wt_name)
    };
    // Size to fit all agents + chrome (top pad, list, hint).
    let body_rows = Agent::ALL.len() as u16;
    let h = body_rows + 5;
    let r = centered(area, 64, h);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::MAGENTA))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Min(1),    // list
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let items: Vec<ListItem> = Agent::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_default = app.store.default_agent == Some(*a);
            let prefix = if i == sel { "▸ " } else { "  " };
            let label_style = if i == sel {
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG)
            };
            let mut spans = vec![
                Span::styled(prefix, Style::default().fg(theme::MAGENTA)),
                Span::styled(a.label().to_string(), label_style),
            ];
            if is_default {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    "default",
                    Style::default().fg(theme::YELLOW).add_modifier(Modifier::ITALIC),
                ));
            }
            let line = Line::from(spans);
            if i == sel {
                ListItem::new(line).style(Style::default().bg(theme::BG_HIGHLIGHT))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    f.render_widget(List::new(items).style(base_style()), layout[1]);

    f.render_widget(
        Paragraph::new(hint_line(&[
            ("↑↓", "move"),
            ("↵", "launch"),
            ("space", "set default"),
            ("esc", "cancel"),
        ]))
        .style(base_style()),
        layout[2],
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

fn render_toast(f: &mut Frame, app: &App, area: Rect) {
    let Some(toast) = &app.toast else { return };
    if toast.expires_at <= std::time::Instant::now() {
        return;
    }
    let msg = format!(" {} ", toast.message);
    let w = (msg.chars().count() as u16).min(area.width.saturating_sub(2));
    if w == 0 { return; }
    let x = area.x + area.width.saturating_sub(w + 1);
    let y = area.y;
    let r = Rect { x, y, width: w, height: 1 };
    f.render_widget(Clear, r);
    let p = Paragraph::new(msg)
        .style(Style::default().bg(theme::BG).fg(theme::GREEN).add_modifier(Modifier::BOLD));
    f.render_widget(p, r);
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

/// Build a footer hint line: `key  verb   key  verb`.
/// Keys render in FG_DARK, verbs in COMMENT, separated by three spaces.
fn hint_line(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(pairs.len() * 4);
    for (i, (key, verb)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().fg(theme::COMMENT)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default().fg(theme::FG_DARK).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            verb.to_string(),
            Style::default().fg(theme::COMMENT),
        ));
    }
    Line::from(spans)
}

fn render_input(f: &mut Frame, title: &str, buffer: &str, kind: &InputKind, dir_sel: usize, area: Rect) {
    let show_dirs = matches!(kind, InputKind::AddProjectPath);
    let height = if show_dirs { 18 } else { 7 };
    let r = centered(area, 70, height);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::MAGENTA))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };

    // Vertical rhythm: top pad, input row, gap, (optional list), hint row.
    let layout = if show_dirs {
        Layout::vertical([
            Constraint::Length(1), // top pad
            Constraint::Length(1), // input
            Constraint::Length(1), // divider/label
            Constraint::Min(1),    // dir list
            Constraint::Length(1), // hint
        ])
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner)
    };

    // Input line with a real caret (reverse block at end of buffer).
    let input_row = layout[1];
    let input_line = Line::from(vec![
        Span::styled(buffer.to_string(), Style::default().fg(theme::FG)),
        Span::styled(" ", Style::default().bg(theme::FG).fg(theme::BG)),
    ]);
    f.render_widget(
        Paragraph::new(input_line).style(base_style()),
        input_row,
    );

    if show_dirs {
        let label_row = layout[2];
        f.render_widget(
            Paragraph::new(Span::styled(
                "matches",
                Style::default().fg(theme::COMMENT).add_modifier(Modifier::ITALIC),
            ))
            .style(base_style()),
            label_row,
        );

        let list_area = layout[3];
        let entries = crate::app::list_dirs(buffer);
        if entries.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "no matches",
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
                    Style::default().bg(theme::BG_HIGHLIGHT).fg(theme::FG).add_modifier(Modifier::BOLD),
                );
            let mut state = ListState::default();
            state.select(Some(dir_sel));
            f.render_stateful_widget(list, list_area, &mut state);
        }

        let hint_row = layout[4];
        f.render_widget(
            Paragraph::new(hint_line(&[
                ("↵", "submit"),
                ("↑↓", "pick"),
                ("esc", "cancel"),
            ]))
            .style(base_style()),
            hint_row,
        );
    } else {
        let hint_row = layout[3];
        f.render_widget(
            Paragraph::new(hint_line(&[("↵", "submit"), ("esc", "cancel")]))
                .style(base_style()),
            hint_row,
        );
    }
}

fn render_confirm(f: &mut Frame, title: &str, prompt: &str, destructive: bool, area: Rect) {
    let border = if destructive { theme::RED } else { theme::MAGENTA };
    // Size to content: width tracks the longer of title or prompt.
    let content_w = prompt.chars().count().max(title.chars().count() + 2);
    let w = (content_w as u16 + 6).clamp(44, 72);
    let r = centered(area, w, 7);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Min(1),    // prompt
        Constraint::Length(1), // hint
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(prompt.to_string())
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(theme::BG).fg(theme::FG)),
        layout[1],
    );
    let verb = if destructive { "delete" } else { "confirm" };
    f.render_widget(
        Paragraph::new(hint_line(&[("y", verb), ("n / esc", "cancel")])).style(base_style()),
        layout[2],
    );
}

fn render_message(f: &mut Frame, msg: &str, area: Rect) {
    let w = (msg.chars().count() as u16 + 6).clamp(40, 72);
    let r = centered(area, w, 7);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Notice ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::COMMENT))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(msg.to_string())
            .wrap(Wrap { trim: false })
            .style(base_style()),
        layout[1],
    );
    f.render_widget(
        Paragraph::new(hint_line(&[("↵ / esc", "dismiss")])).style(base_style()),
        layout[2],
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let r = centered(area, 68, 26);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::CYAN))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Min(1),    // body
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let body = layout[1];
    let cols = Layout::horizontal([Constraint::Length(16), Constraint::Min(10)]).split(body);
    let keys_col = cols[0];
    let verbs_col = cols[1];

    let section = |s: &'static str| {
        Line::from(Span::styled(
            s,
            Style::default().fg(theme::CYAN).add_modifier(Modifier::BOLD),
        ))
    };
    let key = |s: &'static str| {
        Line::from(Span::styled(
            s,
            Style::default().fg(theme::FG_DARK).add_modifier(Modifier::BOLD),
        ))
    };
    let verb = |s: &'static str| Line::from(Span::styled(s, Style::default().fg(theme::FG)));
    let blank = || Line::from("");

    let entries: Vec<(Line<'static>, Line<'static>)> = vec![
        (section("Browser"), Line::from("")),
        (key("j k  ↑ ↓"), verb("move")),
        (key("h l  ← →"), verb("switch projects ↔ worktrees")),
        (key("tab"), verb("toggle panes")),
        (key("↵"), verb("open session (default agent)")),
        (key("P"), verb("open with agent picker")),
        (key("a  d"), verb("add  delete")),
        (key("r"), verb("refresh")),
        (key("Ctrl-g"), verb("jump to active session")),
        (key("q  esc"), verb("quit")),
        (blank(), blank()),
        (section("Sessions sidebar"), Line::from("")),
        (key("Ctrl-g  ↵"), verb("focus session PTY")),
        (key("esc"), verb("back to browser")),
        (key("j  k"), verb("prev  next session")),
        (key("1–9"), verb("jump to session N")),
        (key("x"), verb("kill active session")),
        (key("c  C"), verb("new session (default  pick)")),
        (key("t"), verb("new terminal in worktree")),
        (blank(), blank()),
        (section("Session pane"), Line::from("")),
        (key("Ctrl-g"), verb("focus sidebar")),
        (
            Line::from(Span::styled(
                "(other keys)",
                Style::default().fg(theme::COMMENT).add_modifier(Modifier::ITALIC),
            )),
            Line::from(Span::styled(
                "forwarded to agent",
                Style::default().fg(theme::COMMENT),
            )),
        ),
    ];

    let (keys_lines, verbs_lines): (Vec<Line<'static>>, Vec<Line<'static>>) =
        entries.into_iter().unzip();

    f.render_widget(Paragraph::new(keys_lines).style(base_style()), keys_col);
    f.render_widget(Paragraph::new(verbs_lines).style(base_style()), verbs_col);

    f.render_widget(
        Paragraph::new(hint_line(&[("↵ / esc", "close")])).style(base_style()),
        layout[2],
    );
}
