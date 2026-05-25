use crate::agent::Agent;
use crate::app::{App, InputKind, Modal, Pane, UiMode};
use crate::session::SessionStatus;
use crate::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use tui_term::widget::PseudoTerminal;

fn base_style() -> Style {
    Style::default()
        .bg(theme::current().bg)
        .fg(theme::current().fg)
}

fn chrome_visible(app: &App) -> bool {
    let on_session_page = matches!(app.ui_mode, UiMode::Agent | UiMode::SessionList);
    !on_session_page || app.chrome_visible
}

/// Inner rect of the agent pane for a given full-frame area. Mirrors the
/// layout split in `render`; the event loop uses it to size the active PTY.
pub fn agent_pane_inner(frame: Rect, app: &App) -> Rect {
    let chrome = chrome_visible(app);
    let banner_h = if chrome { 3 } else { 0 };
    let v = Layout::vertical([
        Constraint::Length(banner_h),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame);
    let a = if chrome {
        let h = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(v[1]);
        h[1]
    } else {
        v[1]
    };
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

    let chrome = chrome_visible(app);
    let banner_h: u16 = if chrome { 3 } else { 0 };

    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(banner_h),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(size);

    if chrome {
        render_banner(f, app, vchunks[0]);
    }

    if app.store.projects.is_empty() {
        render_empty(f, vchunks[1]);
    } else {
        match app.ui_mode {
            UiMode::Browser => {
                let hchunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(vchunks[1]);
                render_projects(f, app, hchunks[0]);
                render_worktrees(f, app, hchunks[1]);
            }
            UiMode::Agent | UiMode::SessionList => {
                if chrome {
                    let hchunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                        .split(vchunks[1]);
                    render_session_list(f, app, hchunks[0]);
                    render_agent(f, app, hchunks[1]);
                } else {
                    render_agent(f, app, vchunks[1]);
                }
            }
        }
    }
    render_status(f, app, vchunks[2]);
    render_footer(f, app, vchunks[3]);
    render_toast(f, app, size);

    match &app.modal {
        Modal::None => {}
        Modal::Input {
            title,
            buffer,
            kind,
            dir_sel,
        } => render_input(f, title, buffer, kind, *dir_sel, size),
        Modal::Confirm {
            title,
            prompt,
            destructive,
            ..
        } => render_confirm(f, title, prompt, *destructive, size),
        Modal::Message(msg) => render_message(f, msg, size),
        Modal::Help => render_help(f, size),
        Modal::TmuxSetup => render_tmux_setup(f, app, size),
        Modal::TmuxChoice => render_tmux_choice(f, size),
        Modal::AgentPicker {
            project,
            wt_path,
            sel,
        } => render_agent_picker(f, app, project, wt_path, *sel, size),
        Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } => {
            let sel = match tab {
                theme::ThemeKind::Dark => *sel_dark,
                theme::ThemeKind::Light => *sel_light,
            };
            render_theme_picker(f, sel, *tab, size);
        }
    }
}

fn render_theme_picker(f: &mut Frame, sel: usize, tab: theme::ThemeKind, area: Rect) {
    let t = theme::current();
    let themes = theme::themes_of(tab);
    // Cap modal height; the inner list scrolls when needed.
    let h = (themes.len() as u16 + 6).min(22);
    let r = centered(area, 48, h);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Theme ", t.magenta))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.magenta))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(1), // tabs
        Constraint::Length(1), // spacer
        Constraint::Min(1),    // list
        Constraint::Length(1), // hint separator
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let tab_span = |label: &'static str, active: bool| {
        if active {
            Span::styled(
                label.to_string(),
                Style::default().fg(t.magenta).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(label.to_string(), Style::default().fg(t.comment))
        }
    };
    let tabs = Line::from(vec![
        tab_span("Dark", tab == theme::ThemeKind::Dark),
        Span::raw("   "),
        tab_span("Light", tab == theme::ThemeKind::Light),
    ]);
    f.render_widget(Paragraph::new(tabs).style(base_style()), layout[0]);

    let items: Vec<ListItem> = themes
        .iter()
        .enumerate()
        .map(|(i, th)| {
            let label_style = if i == sel {
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.fg)
            };
            let line = Line::from(Span::styled(th.name.to_string(), label_style));
            if i == sel {
                ListItem::new(line).style(Style::default().bg(t.bg_highlight))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(sel));
    f.render_stateful_widget(List::new(items).style(base_style()), layout[2], &mut state);

    f.render_widget(
        Paragraph::new(hint_line(&[
            ("↑↓", "preview"),
            ("h/l", "tab"),
            ("↵", "apply"),
            ("esc", "cancel"),
        ]))
        .style(base_style()),
        layout[4],
    );
}

fn render_agent_picker(
    f: &mut Frame,
    app: &App,
    project: &str,
    wt_path: &str,
    sel: usize,
    area: Rect,
) {
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
        .title(popup_title(&title, theme::current().magenta))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().magenta))
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
        Constraint::Length(1), // hint separator
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let items: Vec<ListItem> = Agent::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_default = app.store.default_agent == Some(*a);
            let label_style = if i == sel {
                Style::default()
                    .fg(theme::current().fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::current().fg)
            };
            let mut spans = vec![Span::styled(a.label().to_string(), label_style)];
            if is_default {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    "default",
                    Style::default().fg(theme::current().comment),
                ));
            }
            let line = Line::from(spans);
            if i == sel {
                ListItem::new(line).style(Style::default().bg(theme::current().bg_highlight))
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
        layout[3],
    );
}

fn active_style(focused: bool) -> Style {
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

const BANNER: [&str; 3] = ["┌─┐┬─┐┌─┐┬  ┬┌─┐", "│ ┬├┬┘│ │└┐┌┘├┤ ", "└─┘┴└─└─┘ └┘ └─┘"];

fn render_banner(f: &mut Frame, app: &App, area: Rect) {
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

fn render_empty(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" grove ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
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
            Span::styled(
                "a",
                Style::default()
                    .fg(theme::current().cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to add a project, "),
            Span::styled(
                "?",
                Style::default()
                    .fg(theme::current().cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" for help, "),
            Span::styled(
                "q",
                Style::default()
                    .fg(theme::current().cyan)
                    .add_modifier(Modifier::BOLD),
            ),
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

fn render_session_list(f: &mut Frame, app: &App, area: Rect) {
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

        // Worktree paths for this project, in first-seen order.
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

fn render_agent(f: &mut Frame, app: &App, area: Rect) {
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

fn render_toast(f: &mut Frame, app: &App, area: Rect) {
    let Some(toast) = &app.toast else { return };
    if toast.expires_at <= std::time::Instant::now() {
        return;
    }
    let msg = format!(" {} ", toast.message);
    let w = (msg.chars().count() as u16).min(area.width.saturating_sub(2));
    if w == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(w + 1);
    let y = area.y;
    let r = Rect {
        x,
        y,
        width: w,
        height: 1,
    };
    f.render_widget(Clear, r);
    let p = Paragraph::new(msg).style(
        Style::default()
            .bg(theme::current().bg)
            .fg(theme::current().green)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, r);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let p = Paragraph::new(app.status.clone()).style(
        Style::default()
            .bg(theme::current().bg)
            .fg(theme::current().green),
    );
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let zen = matches!(app.ui_mode, UiMode::Agent) && !app.chrome_visible;
    let pairs: &[(&str, &str)] = match app.ui_mode {
        UiMode::Agent if zen => &[
            ("zen", ""),
            ("Ctrl-G", "exit zen"),
            ("(other keys forwarded to agent)", ""),
        ],
        UiMode::Agent => &[
            ("Ctrl-g", "sidebar"),
            ("Ctrl-G", "zen"),
            ("(other keys forwarded to agent)", ""),
        ],
        UiMode::SessionList => &[
            ("Ctrl-g/↵", "session"),
            ("Ctrl-G", "zen"),
            ("j/k", "cycle"),
            ("1-9", "jump"),
            ("d", "kill"),
            ("c/C", "new"),
            ("t", "term"),
            ("esc", "browser"),
            ("?", "help"),
            ("q", "quit"),
        ],
        UiMode::Browser => &[
            ("h/l", "pane"),
            ("a", "add"),
            ("d", "del"),
            ("↵", "open"),
            ("c", "new"),
            ("C", "pick"),
            ("t", "term"),
            ("Ctrl-g", "session"),
            ("T", "theme"),
            ("m", "tmux"),
            ("r", "refresh"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    f.render_widget(Paragraph::new(hint_line(pairs)).style(base_style()), area);
}

fn truncate_title(s: &str, budget: usize) -> String {
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
            continue; // skip the leading space we already added
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
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Build a footer hint line: `key  verb   key  verb`.
/// Keys render in FG_DARK, verbs in COMMENT, separated by three spaces.
fn popup_title(text: &str, color: ratatui::style::Color) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn hint_line(pairs: &[(&str, &str)]) -> Line<'static> {
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

fn render_input(
    f: &mut Frame,
    title: &str,
    buffer: &str,
    kind: &InputKind,
    dir_sel: usize,
    area: Rect,
) {
    let show_dirs = matches!(kind, InputKind::AddProjectPath);
    let height = if show_dirs { 18 } else { 7 };
    let r = centered(area, 70, height);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(
            &format!(" {} ", title),
            theme::current().magenta,
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().magenta))
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
            Constraint::Length(1), // hint separator
            Constraint::Length(1), // hint
        ])
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1), // hint separator
            Constraint::Length(1),
        ])
        .split(inner)
    };

    // Input line with a real caret (reverse block at end of buffer).
    let input_row = layout[1];
    let input_line = Line::from(vec![
        Span::styled(buffer.to_string(), Style::default().fg(theme::current().fg)),
        Span::styled(
            " ",
            Style::default()
                .bg(theme::current().fg)
                .fg(theme::current().bg),
        ),
    ]);
    f.render_widget(Paragraph::new(input_line).style(base_style()), input_row);

    if show_dirs {
        let label_row = layout[2];
        f.render_widget(
            Paragraph::new(Span::styled(
                "matches",
                Style::default()
                    .fg(theme::current().comment)
                    .add_modifier(Modifier::ITALIC),
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
                    Style::default()
                        .fg(theme::current().comment)
                        .add_modifier(Modifier::ITALIC),
                ))
                .style(base_style()),
                list_area,
            );
        } else {
            let items: Vec<ListItem> = entries
                .into_iter()
                .map(|s| ListItem::new(Span::styled(s, Style::default().fg(theme::current().cyan))))
                .collect();
            let list = List::new(items).style(base_style()).highlight_style(
                Style::default()
                    .bg(theme::current().bg_highlight)
                    .fg(theme::current().fg)
                    .add_modifier(Modifier::BOLD),
            );
            let mut state = ListState::default();
            state.select(Some(dir_sel));
            f.render_stateful_widget(list, list_area, &mut state);
        }

        let hint_row = layout[5];
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
        let hint_row = layout[4];
        f.render_widget(
            Paragraph::new(hint_line(&[("↵", "submit"), ("esc", "cancel")])).style(base_style()),
            hint_row,
        );
    }
}

fn render_confirm(f: &mut Frame, title: &str, prompt: &str, destructive: bool, area: Rect) {
    let border = if destructive {
        theme::current().red
    } else {
        theme::current().magenta
    };
    // Size to content: width tracks the longer of title or prompt.
    let content_w = prompt.chars().count().max(title.chars().count() + 2);
    let w = (content_w as u16 + 6).clamp(44, 72);
    let inner_w = (w - 4) as usize;
    let prompt_lines = prompt.chars().count().div_ceil(inner_w.max(1)).max(1) as u16;
    let h = confirm_height(prompt_lines);
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(&format!(" {} ", title), border))
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
        Constraint::Length(1), // hint separator
        Constraint::Length(1), // hint
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(prompt.to_string())
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .bg(theme::current().bg)
                    .fg(theme::current().fg),
            ),
        layout[1],
    );
    let verb = if destructive { "delete" } else { "confirm" };
    f.render_widget(
        Paragraph::new(hint_line(&[("y", verb), ("n / esc", "cancel")])).style(base_style()),
        layout[3],
    );
}

fn confirm_height(prompt_lines: u16) -> u16 {
    (prompt_lines + 5).clamp(6, 10)
}

fn render_message(f: &mut Frame, msg: &str, area: Rect) {
    let w = (msg.chars().count() as u16 + 6).clamp(40, 72);
    let r = centered(area, w, 7);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Notice ", theme::current().comment))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().comment))
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
        Constraint::Length(1), // hint separator
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
        layout[3],
    );
}

#[cfg(test)]
mod tests {
    use super::confirm_height;

    #[test]
    fn confirm_height_reserves_rows_for_prompt() {
        assert_eq!(confirm_height(1), 6);
        assert_eq!(confirm_height(2), 7);
        assert_eq!(confirm_height(5), 10);
    }
}

fn render_tmux_setup(f: &mut Frame, app: &App, area: Rect) {
    let r = centered(area, 78, 22);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Tmux setup ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
    f.render_widget(block, r);

    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let state = if !app.tmux_available {
        "tmux not found: using native sessions"
    } else if app.use_tmux() {
        "tmux enabled: sessions persist across Grove restarts"
    } else {
        "tmux disabled: using native sessions"
    };
    f.render_widget(
        Paragraph::new(state)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme::current().yellow)),
        layout[0],
    );

    f.render_widget(
        Paragraph::new("Grove recommends this config for a smoother tmux experience. It is optional; copy it into your tmux config if you want it.")
            .wrap(Wrap { trim: false })
            .style(base_style()),
        layout[1],
    );

    let code_lines = crate::app::TMUX_SETUP_SNIPPET
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme::current().fg_dark),
            ))
        })
        .collect::<Vec<_>>();
    let code = Paragraph::new(code_lines).style(
        Style::default()
            .bg(theme::current().bg_highlight)
            .fg(theme::current().fg_dark),
    );
    f.render_widget(code, layout[3]);

    let hints = if app.tmux_available {
        hint_line(&[
            ("t / space", "toggle tmux"),
            ("c", "copy"),
            ("↵ / esc / q", "close"),
        ])
    } else {
        hint_line(&[("c", "copy"), ("↵ / esc / q", "close")])
    };
    f.render_widget(Paragraph::new(hints).style(base_style()), layout[5]);
}

fn render_tmux_choice(f: &mut Frame, area: Rect) {
    let r = centered(area, 68, 12);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Session backend ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
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
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("Use tmux for Grove sessions?").style(
            Style::default()
                .fg(theme::current().fg)
                .add_modifier(Modifier::BOLD),
        ),
        layout[0],
    );
    f.render_widget(
        Paragraph::new(
            "Tmux sessions persist across Grove restarts and can be rediscovered later.",
        )
        .wrap(Wrap { trim: false })
        .style(base_style()),
        layout[1],
    );
    f.render_widget(
        Paragraph::new("Native sessions need no tmux dependency, but they end when Grove exits.")
            .wrap(Wrap { trim: false })
            .style(base_style()),
        layout[3],
    );
    f.render_widget(
        Paragraph::new(hint_line(&[
            ("↵ / t / y", "enable tmux"),
            ("n / esc", "use native"),
        ]))
        .style(base_style()),
        layout[5],
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let r = centered(area, 68, 26);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Help ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
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
        Constraint::Length(1), // hint separator
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
            Style::default()
                .fg(theme::current().cyan)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let key = |s: &'static str| {
        Line::from(Span::styled(
            s,
            Style::default()
                .fg(theme::current().fg_dark)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let verb =
        |s: &'static str| Line::from(Span::styled(s, Style::default().fg(theme::current().fg)));
    let blank = || Line::from("");

    let entries: Vec<(Line<'static>, Line<'static>)> = vec![
        (section("Browser"), Line::from("")),
        (key("j k  ↑ ↓"), verb("move")),
        (key("h l  ← →"), verb("switch projects ↔ worktrees")),
        (key("tab"), verb("toggle panes")),
        (key("↵"), verb("open session (default agent)")),
        (key("c  C"), verb("new session (default  pick)")),
        (key("t"), verb("new terminal in worktree")),
        (key("a  d"), verb("add  delete")),
        (key("r"), verb("refresh")),
        (key("m"), verb("tmux setup")),
        (key("Ctrl-g"), verb("jump to active session")),
        (key("T"), verb("theme picker")),
        (key("q  esc"), verb("quit")),
        (blank(), blank()),
        (section("Sessions sidebar"), Line::from("")),
        (key("Ctrl-g  ↵"), verb("focus session PTY")),
        (key("Ctrl-G"), verb("toggle zen (hide chrome)")),
        (key("esc"), verb("back to browser")),
        (key("j  k"), verb("prev  next session")),
        (key("1–9"), verb("jump to session N")),
        (key("d"), verb("kill active session")),
        (key("c  C"), verb("new session (default  pick)")),
        (key("t"), verb("new terminal in worktree")),
        (blank(), blank()),
        (section("Session pane"), Line::from("")),
        (key("Ctrl-g"), verb("focus sidebar")),
        (key("Ctrl-G"), verb("toggle zen (hide chrome)")),
        (
            Line::from(Span::styled(
                "(other keys)",
                Style::default()
                    .fg(theme::current().comment)
                    .add_modifier(Modifier::ITALIC),
            )),
            Line::from(Span::styled(
                "forwarded to agent",
                Style::default().fg(theme::current().comment),
            )),
        ),
    ];

    let (keys_lines, verbs_lines): (Vec<Line<'static>>, Vec<Line<'static>>) =
        entries.into_iter().unzip();

    f.render_widget(Paragraph::new(keys_lines).style(base_style()), keys_col);
    f.render_widget(Paragraph::new(verbs_lines).style(base_style()), verbs_col);

    f.render_widget(
        Paragraph::new(hint_line(&[("↵ / esc", "close")])).style(base_style()),
        layout[3],
    );
}
