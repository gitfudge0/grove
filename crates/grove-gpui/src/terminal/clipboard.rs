//! Clipboard integration — a port of `src/clipboard.rs`.
//!
//! `copy` writes to **two** backends so it works locally and over SSH: the OS
//! clipboard via `arboard`, and the host terminal via an OSC 52 escape
//! sequence (which survives SSH and tmux). `paste` reads the OS clipboard only
//! — OSC 52 read support is terminal-dependent and not universally available.
//!
//! OSC 52 is part of the same clipboard story on the iced side, so it is
//! carried over rather than dropped (Plan 04 Task 5 Step 5).

use base64::Engine as _;
use std::io::{self, Write as _};
use std::sync::{Mutex, OnceLock};

fn clipboard() -> &'static Mutex<Option<arboard::Clipboard>> {
    static CB: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    CB.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()))
}

/// Copy `text`. Both backends are best-effort; failures are swallowed since at
/// least one usually lands.
pub fn copy(text: &str) {
    use std::io::IsTerminal as _;

    if let Ok(mut guard) = clipboard().lock() {
        if let Some(cb) = guard.as_mut() {
            let _ = cb.set_text(text.to_owned());
        }
    }

    // OSC 52 only makes sense when stdout is actually a terminal; writing it
    // into a pipe or file would just inject escape garbage.
    if io::stdout().is_terminal() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
        let _ = stdout.flush();
    }
}

/// Read text from the OS clipboard. `None` when the clipboard is unavailable,
/// empty, or holds no text.
pub fn paste() -> Option<String> {
    let mut guard = clipboard().lock().ok()?;
    let cb = guard.as_mut()?;
    cb.get_text().ok().filter(|s| !s.is_empty())
}

/// Wrap `text` as a bracketed paste (`src/gui/update/mod.rs:831-838`).
///
/// Line endings are normalized to `\r` — `\r\n` first, then bare `\n`, so a
/// CRLF block does not become a double newline — and the whole thing is wrapped
/// in `\x1b[200~` … `\x1b[201~` so the inner app sees one paste rather than a
/// burst of typing (which is what stops an editor auto-indenting every line).
pub fn bracketed_paste(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::with_capacity(normalized.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(normalized.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_wraps_and_normalizes_line_endings() {
        assert_eq!(bracketed_paste("a\nb"), b"\x1b[200~a\rb\x1b[201~".to_vec());
        // CRLF collapses to a single \r, not two.
        assert_eq!(
            bracketed_paste("a\r\nb"),
            b"\x1b[200~a\rb\x1b[201~".to_vec()
        );
        assert_eq!(bracketed_paste(""), b"\x1b[200~\x1b[201~".to_vec());
    }
}
