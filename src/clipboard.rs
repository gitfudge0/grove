//! Copying selected text out of grove. We write to two places so a copy
//! works whether grove runs locally or over SSH:
//!
//! * the OS clipboard via `arboard` (best-effort; fails on headless boxes), and
//! * the host terminal's clipboard via an OSC 52 escape sequence, which
//!   survives SSH/tmux as long as the outer terminal allows it.

use base64::Engine;
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

    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let _ = crossterm::execute!(std::io::stdout(), crossterm::style::Print(format!("\x1b]52;c;{}\x07", b64)));
}
