//! Translate iced keyboard events into PTY-bound byte sequences.

use iced::keyboard::{key::Named, Key, Modifiers};

pub fn key_to_bytes(key: &Key, mods: Modifiers) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if mods.alt() {
        out.push(0x1b);
    }
    match key {
        Key::Character(s) => {
            if mods.control() {
                let ch = s.chars().next()?;
                let b = (ch.to_ascii_uppercase() as u8).wrapping_sub(0x40);
                out.push(b & 0x1f);
            } else {
                out.extend_from_slice(s.as_bytes());
            }
        }
        Key::Named(n) => match n {
            Named::Enter => out.push(b'\r'),
            Named::Tab => out.push(b'\t'),
            Named::Backspace => out.push(0x7f),
            Named::Escape => out.push(0x1b),
            Named::Space => out.push(b' '),
            Named::ArrowUp => out.extend_from_slice(b"\x1b[A"),
            Named::ArrowDown => out.extend_from_slice(b"\x1b[B"),
            Named::ArrowRight => out.extend_from_slice(b"\x1b[C"),
            Named::ArrowLeft => out.extend_from_slice(b"\x1b[D"),
            Named::Home => out.extend_from_slice(b"\x1b[H"),
            Named::End => out.extend_from_slice(b"\x1b[F"),
            Named::PageUp => out.extend_from_slice(b"\x1b[5~"),
            Named::PageDown => out.extend_from_slice(b"\x1b[6~"),
            Named::Delete => out.extend_from_slice(b"\x1b[3~"),
            Named::Insert => out.extend_from_slice(b"\x1b[2~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(out)
}
