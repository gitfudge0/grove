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
                // Ctrl-<key> arithmetic is only meaningful for ASCII.
                if !ch.is_ascii() {
                    return None;
                }
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
        Key::Unidentified => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smol_str::SmolStr;

    fn ctrl(s: &str) -> Option<Vec<u8>> {
        key_to_bytes(&Key::Character(SmolStr::new(s)), Modifiers::CTRL)
    }

    fn plain(s: &str) -> Option<Vec<u8>> {
        key_to_bytes(&Key::Character(SmolStr::new(s)), Modifiers::empty())
    }

    fn alt_named(n: Named) -> Option<Vec<u8>> {
        key_to_bytes(&Key::Named(n), Modifiers::ALT)
    }

    fn named(n: Named) -> Option<Vec<u8>> {
        key_to_bytes(&Key::Named(n), Modifiers::empty())
    }

    /// Ctrl+C must produce the single byte 0x03 (ETX / interrupt).
    #[test]
    fn ctrl_c_produces_etx() {
        assert_eq!(ctrl("c"), Some(vec![0x03]));
        // Upper and lower case must both work (the code folds to uppercase).
        assert_eq!(ctrl("C"), Some(vec![0x03]));
    }

    /// Ctrl+A → 0x01, Ctrl+Z → 0x1A — verify the arithmetic covers the range.
    #[test]
    fn ctrl_ascii_range() {
        assert_eq!(ctrl("a"), Some(vec![0x01]));
        assert_eq!(ctrl("z"), Some(vec![0x1a]));
    }

    /// Ctrl+non-ASCII must return `None` — no garbage bytes emitted.
    #[test]
    fn ctrl_non_ascii_returns_none() {
        // "é" is a multi-byte non-ASCII character.
        assert_eq!(
            ctrl("é"),
            None,
            "Ctrl+non-ASCII must return None, not garbage bytes"
        );
    }

    /// Alt-prefix adds an ESC byte (0x1b) before the sequence.
    #[test]
    fn alt_adds_esc_prefix() {
        let bytes = alt_named(Named::ArrowUp).expect("alt+up");
        // Must start with ESC.
        assert_eq!(bytes[0], 0x1b, "Alt should prepend ESC");
        // The rest must be the normal Up sequence \x1b[A.
        assert_eq!(&bytes[1..], b"\x1b[A");
    }

    /// Named keys produce the correct VT sequences.
    #[test]
    fn named_keys_correct_sequences() {
        assert_eq!(named(Named::Enter), Some(vec![b'\r']));
        assert_eq!(named(Named::Tab), Some(vec![b'\t']));
        assert_eq!(named(Named::Backspace), Some(vec![0x7f]));
        assert_eq!(named(Named::Escape), Some(vec![0x1b]));
        assert_eq!(named(Named::ArrowUp), Some(b"\x1b[A".to_vec()));
        assert_eq!(named(Named::ArrowDown), Some(b"\x1b[B".to_vec()));
        assert_eq!(named(Named::ArrowRight), Some(b"\x1b[C".to_vec()));
        assert_eq!(named(Named::ArrowLeft), Some(b"\x1b[D".to_vec()));
    }

    /// A plain character key produces its UTF-8 bytes without modification.
    #[test]
    fn plain_character_bytes() {
        assert_eq!(plain("a"), Some(vec![b'a']));
        assert_eq!(plain("Z"), Some(vec![b'Z']));
    }
}
