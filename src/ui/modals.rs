//! All modal popups: input, confirm, message, help, tmux setup/choice,
//! agent picker, theme picker.

use super::common::{base_style, centered, hint_line, popup_title};
use crate::agent::Agent;
use crate::app::{App, InputKind};
use crate::theme;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn render_input(
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

    let inner = inner_rect(r);

    // Vertical rhythm: top pad, input row, gap, (optional list), hint row.
    let layout = if show_dirs {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner)
    };

    // Input line with a real caret (reverse block at end of buffer).
    let input_line = Line::from(vec![
        Span::styled(buffer.to_string(), Style::default().fg(theme::current().fg)),
        Span::styled(
            " ",
            Style::default()
                .bg(theme::current().fg)
                .fg(theme::current().bg),
        ),
    ]);
    f.render_widget(Paragraph::new(input_line).style(base_style()), layout[1]);

    if show_dirs {
        f.render_widget(
            Paragraph::new(Span::styled(
                "matches",
                Style::default()
                    .fg(theme::current().comment)
                    .add_modifier(Modifier::ITALIC),
            ))
            .style(base_style()),
            layout[2],
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

        f.render_widget(
            Paragraph::new(hint_line(&[
                ("↵", "submit"),
                ("↑↓", "pick"),
                ("esc", "cancel"),
            ]))
            .style(base_style()),
            layout[5],
        );
    } else {
        f.render_widget(
            Paragraph::new(hint_line(&[("↵", "submit"), ("esc", "cancel")])).style(base_style()),
            layout[4],
        );
    }
}

pub fn render_confirm(f: &mut Frame, title: &str, prompt: &str, destructive: bool, area: Rect) {
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

    let inner = inner_rect(r);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
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

pub fn confirm_height(prompt_lines: u16) -> u16 {
    (prompt_lines + 5).clamp(6, 10)
}

pub fn render_message(f: &mut Frame, msg: &str, area: Rect) {
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

    let inner = inner_rect(r);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
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

pub fn render_tmux_setup(f: &mut Frame, app: &App, area: Rect) {
    let r = centered(area, 78, 22);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Tmux setup ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
    f.render_widget(block, r);

    let inner = inner_rect(r);
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

pub fn render_tmux_choice(f: &mut Frame, area: Rect) {
    let r = centered(area, 68, 12);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Session backend ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
    f.render_widget(block, r);

    let inner = inner_rect(r);
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

pub fn render_agent_picker(
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

    let inner = inner_rect(r);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
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

pub fn render_theme_picker(f: &mut Frame, sel: usize, tab: theme::ThemeKind, area: Rect) {
    let t = theme::current();
    let themes = theme::themes_of(tab);
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

    let inner = inner_rect(r);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
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

pub fn render_help(f: &mut Frame, area: Rect) {
    let r = centered(area, 68, 26);
    f.render_widget(Clear, r);
    let block = Block::default()
        .title(popup_title(" Help ", theme::current().cyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::current().cyan))
        .style(base_style());
    f.render_widget(block, r);

    let inner = inner_rect(r);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
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

/// Two-character inset from a block's border to its inner content area —
/// matches the manual inset every modal here was open-coding.
fn inner_rect(r: Rect) -> Rect {
    Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    }
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
