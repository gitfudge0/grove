//! `copy` writes both the OS clipboard and an OSC 52 escape sequence so it works over SSH/tmux; `paste` reads only the OS clipboard.

use base64::Engine as _;
use std::io::{self, Write as _};
use std::sync::{Mutex, OnceLock};

fn clipboard() -> &'static Mutex<Option<arboard::Clipboard>> {
    static CB: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    CB.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()))
}

/// Both backends are best-effort; failures are swallowed since at least one usually lands.
pub fn copy(text: &str) {
    use std::io::IsTerminal as _;

    if let Ok(mut guard) = clipboard().lock() {
        if let Some(cb) = guard.as_mut() {
            let _ = cb.set_text(text.to_owned());
        }
    }

    // Writing OSC 52 into a pipe or file would just inject escape garbage.
    if io::stdout().is_terminal() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
        let _ = stdout.flush();
    }
}

/// `None` when the clipboard is unavailable, empty, or holds no text.
pub fn paste() -> Option<String> {
    let mut guard = clipboard().lock().ok()?;
    let cb = guard.as_mut()?;
    cb.get_text().ok().filter(|s| !s.is_empty())
}

/// Wraps in `\x1b[200~`…`\x1b[201~` so the inner app sees one paste rather than a burst of typing that would auto-indent every line (`src/gui/update/mod.rs:831-838`).
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
