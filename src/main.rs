mod agent;
mod app;
mod git;
mod launch;
mod storage;
mod theme;
mod ui;

use anyhow::Result;
use app::{App, Modal};
use crossterm::{
    event::{
        self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::stdout;
use std::time::Duration;

fn main() -> Result<()> {
    let mut app = App::new()?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let kbd_enhanced = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal, &mut app);

    if kbd_enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res?;

    if let Some(pending) = app.pending_exec.take() {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&pending.program)
            .args(&pending.args)
            .current_dir(&pending.cwd)
            .exec();
        return Err(err.into());
    }

    Ok(())
}

fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match &mut app.modal {
            Modal::Input { .. } => match key.code {
                KeyCode::Esc => app.modal = Modal::None,
                KeyCode::Enter => app.submit_input()?,
                KeyCode::Down => app.input_dir_move(1),
                KeyCode::Up => app.input_dir_move(-1),
                KeyCode::Tab | KeyCode::Right => app.input_dir_pick(),
                KeyCode::Backspace => app.input_buffer_edit(|b| { b.pop(); }),
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.input_buffer_edit(|b| b.push(c));
                    } else if c == 'u' {
                        app.input_buffer_edit(|b| b.clear());
                    } else if c == 'c' {
                        app.modal = Modal::None;
                    }
                }
                _ => {}
            },
            Modal::Confirm { .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => app.submit_confirm(true)?,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    app.submit_confirm(false)?
                }
                _ => {}
            },
            Modal::Message(_) | Modal::Help => {
                app.modal = Modal::None;
            }
            Modal::AgentPicker { .. } => match key.code {
                KeyCode::Esc => app.modal = Modal::None,
                KeyCode::Char('j') | KeyCode::Down => app.picker_move(1),
                KeyCode::Char('k') | KeyCode::Up => app.picker_move(-1),
                KeyCode::Char('d') | KeyCode::Char('s') => app.picker_toggle_default()?,
                KeyCode::Enter => app.picker_submit(),
                _ => {}
            },
            Modal::None => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.should_quit = true
                }
                KeyCode::Char('j') | KeyCode::Down => app.move_down(),
                KeyCode::Char('k') | KeyCode::Up => app.move_up(),
                KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l')
                | KeyCode::Left | KeyCode::Right => app.toggle_focus(),
                KeyCode::Char('a') => app.start_add(),
                KeyCode::Char('d') => app.start_delete(),
                KeyCode::Char('r') => app.refresh_worktrees(),
                KeyCode::Char('?') => app.modal = Modal::Help,
                KeyCode::Enter => {
                    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                    if shift && matches!(app.focus, app::Pane::Worktrees) {
                        app.open_worktree(true);
                    } else {
                        app.on_enter()?;
                    }
                }
                _ => {}
            },
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
