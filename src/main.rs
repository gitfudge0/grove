mod agent;
mod app;
mod clipboard;
mod git;
mod launch;
mod session;
mod session_meta;
mod storage;
mod tmux;
mod theme;
mod ui;

use anyhow::Result;
use app::{App, Modal, Selection, UiMode};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
        EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use crossterm::event::MouseEvent;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Position, Rect},
    Terminal,
};
use std::io::stdout;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn main() -> Result<()> {
    if !tmux::available() {
        eprintln!("grove requires tmux on PATH (used for persistent agent sessions).");
        std::process::exit(1);
    }
    let mut app = App::new()?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    execute!(stdout(), EnableMouseCapture)?;
    execute!(stdout(), EnableBracketedPaste)?;
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
    execute!(stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res?;
    Ok(())
}

fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut needs_draw = true;
    loop {
        if app.active_session.is_some() {
            let size = terminal.size()?;
            let inner = ui::agent_pane_inner(
                Rect::new(0, 0, size.width, size.height),
                app,
            );
            if let Some(s) = app.active_session_mut() {
                s.resize(inner.height, inner.width);
            }
        }

        // Redraw only on input, a resize, or fresh PTY output — otherwise the
        // loop idles without burning a full render every poll tick.
        let session_dirty = app
            .sessions
            .iter()
            .any(|s| s.dirty.swap(false, Ordering::Relaxed));
        if let Some(t) = &app.toast {
            if t.expires_at <= std::time::Instant::now() {
                app.toast = None;
                needs_draw = true;
            }
        }

        if needs_draw || session_dirty {
            terminal.draw(|f| ui::render(f, app))?;
        }
        needs_draw = false;

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if matches!(app.modal, Modal::None) {
                        match app.ui_mode {
                            UiMode::Agent => handle_agent_key(app, key),
                            UiMode::SessionList => handle_session_list_key(app, key),
                            UiMode::Browser => handle_browser_key(app, key)?,
                        }
                    } else {
                        handle_browser_key(app, key)?;
                    }
                    needs_draw = true;
                }
                Event::Paste(text) => {
                    handle_paste(app, text);
                    needs_draw = true;
                }
                Event::Mouse(me) => {
                    let size = terminal.size()?;
                    needs_draw |= handle_mouse(app, size, me);
                }
                Event::Resize(_, _) => needs_draw = true,
                _ => {}
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Route a mouse event over the active agent pane: wheel scrolls the
/// scrollback, and left-button drag selects text to copy. Returns whether a
/// redraw is needed.
fn handle_mouse(app: &mut App, size: ratatui::layout::Size, me: MouseEvent) -> bool {
    if app.ui_mode != UiMode::Agent || !matches!(app.modal, Modal::None) {
        return false;
    }
    let pane = ui::agent_pane_inner(Rect::new(0, 0, size.width, size.height), app);
    let in_pane = pane.contains(Position::new(me.column, me.row));

    // Clamp a possibly out-of-bounds pointer to the nearest pane cell so a
    // drag that strays past the edge still extends the selection.
    let rel = |col: u16, row: u16| -> (u16, u16) {
        let c = col.clamp(pane.x, pane.x + pane.width.saturating_sub(1)) - pane.x;
        let r = row.clamp(pane.y, pane.y + pane.height.saturating_sub(1)) - pane.y;
        (c, r)
    };

    match me.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if !in_pane {
                return false;
            }
            let up = me.kind == MouseEventKind::ScrollUp;
            let Some(s) = app.active_session_mut() else { return false };
            s.scroll(up, me.column - pane.x, me.row - pane.y);
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if !in_pane {
                app.selection = None;
                return false;
            }
            let p = rel(me.column, me.row);
            app.selection = Some(Selection {
                anchor: p,
                head: p,
                dragging: true,
            });
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(sel) = app.selection.as_mut() else { return false };
            if !sel.dragging {
                return false;
            }
            sel.head = rel(me.column, me.row);
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let Some(mut sel) = app.selection else { return false };
            if !sel.dragging {
                return false;
            }
            sel.dragging = false;

            if sel.is_empty() {
                // A bare click: forward it to the inner app if it wants the
                // mouse, so clickable agent UIs still work.
                let (c, r) = sel.anchor;
                app.selection = None;
                if let Some(s) = app.active_session_mut() {
                    if s.wants_mouse() {
                        s.forward_click(c, r);
                    }
                }
                return true;
            }

            let (start, end) = sel.normalized();
            let text = app
                .active_session_mut()
                .and_then(|s| s.selection_text(start, end));
            if let Some(text) = text {
                let n = text.len();
                clipboard::copy(&text);
                app.set_toast(format!("Copied {} bytes", n));
            }
            app.selection = None;
            true
        }
        _ => false,
    }
}

/// True for Ctrl-Shift-G, accepting both terminal encodings (Ctrl+'G' with
/// or without an explicit SHIFT modifier, and Ctrl+Shift+'g').
fn is_ctrl_shift_g(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    match key.code {
        KeyCode::Char('G') => true,
        KeyCode::Char('g') => key.modifiers.contains(KeyModifiers::SHIFT),
        _ => false,
    }
}

/// Agent mode: Ctrl-g focuses the sidebar; Ctrl-Shift-g toggles zen mode.
fn handle_agent_key(app: &mut App, key: KeyEvent) {
    // Typing dismisses any lingering copy highlight, like a real terminal.
    app.selection = None;

    if is_ctrl_shift_g(&key) {
        app.chrome_visible = !app.chrome_visible;
        return;
    }

    if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.chrome_visible = true;
        app.enter_session_list();
        return;
    }

    if let Some(bytes) = key_to_bytes(key) {
        if let Some(s) = app.active_session_mut() {
            s.send(&bytes);
        }
    }
}

/// Sessions-sidebar mode: keys navigate / manage sessions, the highlighted
/// session's PTY is still rendered on the right but doesn't receive input.
fn handle_session_list_key(app: &mut App, key: KeyEvent) {
    if is_ctrl_shift_g(&key) {
        app.chrome_visible = !app.chrome_visible;
        if !app.chrome_visible {
            app.focus_active_session();
        }
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('g') if ctrl => app.focus_active_session(),
        KeyCode::Enter => app.focus_active_session(),
        KeyCode::Esc => app.enter_browser(),
        KeyCode::Char('j') | KeyCode::Down => app.session_cycle(1),
        KeyCode::Char('k') | KeyCode::Up => app.session_cycle(-1),
        KeyCode::Char('n') => app.session_cycle(1),
        KeyCode::Char('p') => app.session_cycle(-1),
        KeyCode::Char(ch @ '1'..='9') => app.session_select(ch as usize - '1' as usize),
        KeyCode::Char('x') => app.kill_active_session(),
        KeyCode::Char('c') if ctrl => app.should_quit = true,
        KeyCode::Char('c') => app.new_session_for_active(),
        KeyCode::Char('C') => app.new_session_for_active_pick(),
        KeyCode::Char('t') => app.new_terminal_for_active(),
        KeyCode::Char('T') => app.open_theme_picker(),
        KeyCode::Char('?') => app.modal = Modal::Help,
        KeyCode::Char('q') => app.should_quit = true,
        _ => {}
    }
}

/// Browser mode: the original sidebar/modal key handling.
fn handle_browser_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match &mut app.modal {
        Modal::Input { .. } => match key.code {
            KeyCode::Esc => app.modal = Modal::None,
            KeyCode::Enter => app.submit_input()?,
            KeyCode::Down => app.input_dir_move(1),
            KeyCode::Up => app.input_dir_move(-1),
            KeyCode::Tab | KeyCode::Right => app.input_dir_pick(),
            KeyCode::Backspace => app.input_buffer_edit(|b| {
                b.pop();
            }),
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
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.submit_confirm(false)?,
            _ => {}
        },
        Modal::Message(_) | Modal::Help => match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => {
                app.modal = Modal::None;
            }
            _ => {}
        },
        Modal::AgentPicker { .. } => match key.code {
            KeyCode::Esc => app.modal = Modal::None,
            KeyCode::Char('j') | KeyCode::Down => app.picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.picker_move(-1),
            KeyCode::Char(' ') => app.picker_toggle_default()?,
            KeyCode::Enter => app.picker_submit(),
            _ => {}
        },
        Modal::ThemePicker { .. } => match key.code {
            KeyCode::Esc => app.theme_picker_cancel(),
            KeyCode::Char('j') | KeyCode::Down => app.theme_picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.theme_picker_move(-1),
            KeyCode::Char('h') | KeyCode::Left
            | KeyCode::Char('l') | KeyCode::Right
            | KeyCode::Tab | KeyCode::BackTab => app.theme_picker_switch_tab(),
            KeyCode::Enter => app.theme_picker_submit()?,
            _ => {}
        },
        Modal::None => match key.code {
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.focus_active_session();
            }
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true
            }
            KeyCode::Char('j') | KeyCode::Down
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                app.move_down()
            }
            KeyCode::Char('k') | KeyCode::Up
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                app.move_up()
            }
            KeyCode::Tab => app.toggle_focus(),
            KeyCode::Char('h') | KeyCode::Left => app.focus_pane(app::Pane::Projects),
            KeyCode::Char('l') | KeyCode::Right => app.focus_pane(app::Pane::Worktrees),
            KeyCode::Char('a') => app.start_add(),
            KeyCode::Char('d') => app.start_delete(),
            KeyCode::Char('r') => app.refresh_worktrees(),
            KeyCode::Char('?') => app.modal = Modal::Help,
            KeyCode::Char('T') => app.open_theme_picker(),
            KeyCode::Char('c') if matches!(app.focus, app::Pane::Worktrees) => {
                app.open_worktree(false);
            }
            KeyCode::Char('C') if matches!(app.focus, app::Pane::Worktrees) => {
                app.open_worktree(true);
            }
            KeyCode::Char('t') if matches!(app.focus, app::Pane::Worktrees) => {
                app.open_worktree_terminal();
            }
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
    Ok(())
}

/// Route a paste event. In the agent pane we forward it to the PTY wrapped
/// in bracketed-paste markers so the inner CLI inserts it as a single block
/// instead of treating embedded newlines as submits. In the worktree-input
/// modal we splice the text into the input buffer (single-line, so newlines
/// are stripped). Other modes ignore pastes.
fn handle_paste(app: &mut App, text: String) {
    if matches!(app.modal, Modal::Input { .. }) {
        let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        app.input_buffer_edit(|b| b.push_str(&cleaned));
        return;
    }
    if !matches!(app.modal, Modal::None) {
        return;
    }
    if !matches!(app.ui_mode, UiMode::Agent) {
        return;
    }
    let Some(s) = app.active_session_mut() else { return };
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::with_capacity(normalized.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(normalized.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    s.send(&bytes);
}

/// Encode a key event into the byte sequence a PTY application expects.
fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut out: Vec<u8> = Vec::new();
    if alt {
        out.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = (c.to_ascii_uppercase() as u8).wrapping_sub(0x40);
                out.push(b & 0x1f);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        _ => return None,
    }
    Some(out)
}
