//! TUI rendering. Top-level layout lives here; per-region rendering is
//! delegated to the submodules.
//!
//! - [`banner`]   — top banner + empty-state panel
//! - [`browser`]  — projects + worktrees panes
//! - [`sessions`] — sessions sidebar + agent PTY pane
//! - [`chrome`]   — status line, footer hints, toast
//! - [`modals`]   — all modal popups (input, confirm, message, help, tmux, pickers)
//! - [`common`]   — shared styles, layout math, text helpers

mod banner;
mod browser;
mod chrome;
mod common;
mod modals;
mod sessions;

use crate::app::{App, Modal, UiMode};
use crate::theme;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Block,
    Frame,
};

use common::{base_style, chrome_visible};

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
        banner::render_banner(f, app, vchunks[0]);
    }

    if app.store.projects.is_empty() {
        banner::render_empty(f, vchunks[1]);
    } else {
        match app.ui_mode {
            UiMode::Browser => {
                let h = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(vchunks[1]);
                browser::render_projects(f, app, h[0]);
                browser::render_worktrees(f, app, h[1]);
            }
            UiMode::Agent | UiMode::SessionList => {
                if chrome {
                    let h = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                        .split(vchunks[1]);
                    sessions::render_session_list(f, app, h[0]);
                    sessions::render_agent(f, app, h[1]);
                } else {
                    sessions::render_agent(f, app, vchunks[1]);
                }
            }
        }
    }

    chrome::render_status(f, app, vchunks[2]);
    chrome::render_footer(f, app, vchunks[3]);
    chrome::render_toast(f, app, size);

    render_modal(f, app, size);
}

fn render_modal(f: &mut Frame, app: &App, size: Rect) {
    match &app.modal {
        Modal::None => {}
        Modal::Input {
            title,
            buffer,
            kind,
            dir_sel,
        } => modals::render_input(f, title, buffer, kind, *dir_sel, size),
        Modal::Confirm {
            title,
            prompt,
            destructive,
            ..
        } => modals::render_confirm(f, title, prompt, *destructive, size),
        Modal::Message(msg) => modals::render_message(f, msg, size),
        Modal::Help => modals::render_help(f, size),
        Modal::TmuxSetup => modals::render_tmux_setup(f, app, size),
        Modal::TmuxChoice => modals::render_tmux_choice(f, size),
        Modal::AgentPicker {
            project,
            wt_path,
            sel,
        } => modals::render_agent_picker(
            f,
            &app.available_agents,
            app.store.default_agent,
            project,
            wt_path,
            *sel,
            size,
        ),
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
            modals::render_theme_picker(f, sel, *tab, size);
        }
        // GUI-only — never opened from the TUI codepath.
        Modal::RemoveProject { .. } => {}
    }
}
