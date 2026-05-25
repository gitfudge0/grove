//! Bottom-of-screen chrome: status line, key-hint footer, and the
//! transient toast notification.

use super::common::{base_style, hint_line};
use crate::app::{App, UiMode};
use crate::theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Clear, Paragraph},
    Frame,
};

pub fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let p = Paragraph::new(app.status.clone()).style(
        Style::default()
            .bg(theme::current().bg)
            .fg(theme::current().green),
    );
    f.render_widget(p, area);
}

pub fn render_footer(f: &mut Frame, app: &App, area: Rect) {
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

pub fn render_toast(f: &mut Frame, app: &App, area: Rect) {
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
