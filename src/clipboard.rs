//! Clipboard integration for grove.
//!
//! `copy` writes to two backends so it works locally and over SSH:
//! * the OS clipboard via `arboard`, and
//! * the host terminal via an OSC 52 escape sequence (survives SSH/tmux).
//!
//! `paste` reads from the OS clipboard only (arboard); OSC 52 read support
//! is terminal-dependent and not universally available.

use base64::Engine;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

fn clipboard() -> &'static Mutex<Option<arboard::Clipboard>> {
    static CB: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    CB.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()))
}

/// Copy `text` to the clipboard. Both backends are best-effort; failures are
/// swallowed since at least one usually lands.
pub fn copy(text: &str) {
    if let Ok(mut guard) = clipboard().lock() {
        if let Some(cb) = guard.as_mut() {
            let _ = cb.set_text(text.to_owned());
        }
    }

    // OSC 52 only makes sense when stdout is actually a terminal; writing it
    // into a pipe/file would just inject escape garbage.
    use std::io::IsTerminal;
    if io::stdout().is_terminal() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b]52;c;{}\x07", b64);
        let _ = stdout.flush();
    }
}

/// Read text from the OS clipboard. Returns `None` if the clipboard is
/// unavailable, empty, or does not contain text.
pub fn paste() -> Option<String> {
    let mut guard = clipboard().lock().ok()?;
    let cb = guard.as_mut()?;
    cb.get_text().ok().filter(|s| !s.is_empty())
}
