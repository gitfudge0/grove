//! gpui `Keystroke` → PTY bytes, plus chord predicates. Port of `src/gui/keys.rs:5-45`/`pty_input.rs:400-449`; iced is the oracle, so modified arrows stay plain and DECCKM never affects encoding.

use gpui::Keystroke;

/// `src/gui/keys.rs:5-45`, adapted to gpui's `(key, key_char, modifiers)` shape. Returns `None` when the key produces no PTY bytes.
pub fn key_to_bytes(keystroke: &Keystroke, _app_cursor: bool) -> Option<Vec<u8>> {
    // `modifiers.platform` is Super/Cmd: app chords never reach the PTY (findings §S1 Step 4).
    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }

    let mut out = Vec::new();
    // `keys.rs:7-9` — Alt is an ESC prefix, making Alt+Escape arrive as ESC ESC.
    if keystroke.modifiers.alt {
        out.push(0x1b);
    }

    // Ctrl runs first, but only over single printable chars; space is spelled out or the named table would shadow it.
    if keystroke.modifiers.control {
        // Ctrl+Shift is the app's global-shortcut modifier on non-mac; unbound ctrl-shift-letter chords must never fall through to the arithmetic below.
        if cfg!(not(target_os = "macos"))
            && keystroke.modifiers.shift
            && keystroke.key.chars().count() == 1
            && keystroke
                .key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            return None;
        }
        let ctrl_char = if keystroke.key == "space" {
            Some(' ')
        } else if keystroke.key.chars().count() == 1 {
            keystroke.key.chars().next()
        } else {
            None
        };
        if let Some(ch) = ctrl_char {
            // `keys.rs:14-17` — runs on `key`, never `key_char`: on Linux Ctrl+V's `key_char` is already the control char, which would fold to garbage.
            if !ch.is_ascii() {
                return None;
            }
            let b = (ch.to_ascii_uppercase() as u8).wrapping_sub(0x40);
            out.push(b & 0x1f);
            return Some(out);
        }
    }

    if let Some(bytes) = named_key_bytes(&keystroke.key) {
        out.extend_from_slice(bytes);
        return Some(out);
    }

    // `key_char` carries the layout's actual output (option-s → "ß"); `key` deliberately does not.
    let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
    if text.is_empty() {
        return None;
    }
    // A multi-char `key` with no `key_char` is an unnamed special key (`keys.rs:40`).
    if keystroke.key_char.is_none() && keystroke.key.chars().count() != 1 {
        return None;
    }
    out.extend_from_slice(text.as_bytes());
    Some(out)
}

/// `src/gui/keys.rs:25-40`, key for key. gpui names these keys in lowercase.
fn named_key_bytes(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "enter" => b"\r",
        "tab" => b"\t",
        "backspace" => b"\x7f",
        "escape" => b"\x1b",
        "space" => b" ",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        _ => return None,
    })
}

/// Synthesize arrow bytes for caret movement (`session.rs:1041-1060`). Horizontal only — Up/Down recall shell history instead. The one place DECCKM matters: `app_cursor` selects SS3 vs. CSI.
pub fn arrow_moves(cur_col: u16, t_col: u16, app_cursor: bool) -> Vec<u8> {
    let prefix: &[u8] = if app_cursor { b"\x1bO" } else { b"\x1b[" };
    let mut out = Vec::new();
    let (col_ch, col_n) = if t_col > cur_col {
        (b'C', t_col - cur_col) // Right
    } else {
        (b'D', cur_col - t_col) // Left
    };
    for _ in 0..col_n {
        out.extend_from_slice(prefix);
        out.push(col_ch);
    }
    out
}

/// How far a keyboard scroll chord moves (`pty_input.rs:385-391`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAmount {
    /// One screenful.
    Page,
    /// The full scrollback: jump to the top or back to the bottom.
    All,
}

/// `pty_input.rs:400-411`. `None` when Ctrl/Alt/Super is held (don't steal TUI chords) or Shift is absent (plain PageUp/etc. reach the PTY).
pub fn keyboard_scroll_intent(keystroke: &Keystroke) -> Option<(bool, ScrollAmount)> {
    let m = keystroke.modifiers;
    if !m.shift || m.control || m.platform || m.alt {
        return None;
    }
    match keystroke.key.as_str() {
        "pageup" => Some((true, ScrollAmount::Page)),
        "pagedown" => Some((false, ScrollAmount::Page)),
        "home" => Some((true, ScrollAmount::All)),
        "end" => Some((false, ScrollAmount::All)),
        _ => None,
    }
}

/// `pty_input.rs:427-437`. macOS: Cmd+C without Ctrl. Elsewhere: Ctrl+Shift+C.
pub fn is_copy_shortcut(keystroke: &Keystroke) -> bool {
    chord(keystroke, "c")
}

/// `pty_input.rs:439-449`. macOS: Cmd+V without Ctrl. Elsewhere: Ctrl+Shift+V — plain Ctrl+V is deliberately left for the PTY (literal insert in vim/readline).
pub fn is_paste_shortcut(keystroke: &Keystroke) -> bool {
    chord(keystroke, "v")
}

fn chord(keystroke: &Keystroke, letter: &str) -> bool {
    if !keystroke.key.eq_ignore_ascii_case(letter) {
        return false;
    }
    let m = keystroke.modifiers;
    #[cfg(target_os = "macos")]
    {
        m.platform && !m.control
    }
    #[cfg(not(target_os = "macos"))]
    {
        m.control && m.shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn ks(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: None,
        }
    }

    /// A printable key as the platform delivers it: `key` is the layout key, `key_char` the character actually typed.
    fn typed(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: Some(key.to_string()),
        }
    }

    fn none() -> Modifiers {
        Modifiers::default()
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn bytes(key: &str, modifiers: Modifiers) -> Option<Vec<u8>> {
        key_to_bytes(&ks(key, modifiers), false)
    }

    /// The named-key table, row for row against `src/gui/keys.rs:25-39`.
    #[test]
    fn named_key_byte_table() {
        let table: &[(&str, &[u8])] = &[
            ("enter", b"\r"),         // keys.rs:25
            ("tab", b"\t"),           // :26
            ("backspace", b"\x7f"),   // :27
            ("escape", b"\x1b"),      // :28
            ("space", b" "),          // :29
            ("up", b"\x1b[A"),        // :30
            ("down", b"\x1b[B"),      // :31
            ("right", b"\x1b[C"),     // :32
            ("left", b"\x1b[D"),      // :33
            ("home", b"\x1b[H"),      // :34
            ("end", b"\x1b[F"),       // :35
            ("pageup", b"\x1b[5~"),   // :36
            ("pagedown", b"\x1b[6~"), // :37
            ("delete", b"\x1b[3~"),   // :38
            ("insert", b"\x1b[2~"),   // :39
        ];
        for (key, expected) in table {
            assert_eq!(
                bytes(key, none()).as_deref(),
                Some(*expected),
                "named key {key}"
            );
        }
    }

    /// `keys.rs:40` — any other named key emits nothing.
    #[test]
    fn unknown_named_keys_emit_nothing() {
        for key in ["f1", "f12", "capslock", "printscreen", "menu"] {
            assert_eq!(bytes(key, none()), None, "named key {key}");
        }
    }

    /// `keys.rs:21` — a plain character key is its own UTF-8 bytes.
    #[test]
    fn plain_characters_emit_their_utf8() {
        assert_eq!(key_to_bytes(&typed("a", none()), false), Some(vec![b'a']));
        assert_eq!(key_to_bytes(&typed("Z", none()), false), Some(vec![b'Z']));
        assert_eq!(
            key_to_bytes(&typed("é", none()), false),
            Some("é".as_bytes().to_vec())
        );
    }

    /// `keys.rs:16-19` — fold to uppercase, subtract 0x40, mask to 5 bits.
    #[test]
    fn ctrl_letters_span_one_to_twenty_six() {
        for (i, ch) in ('a'..='z').enumerate() {
            let expected = u8::try_from(i + 1).unwrap_or(0);
            assert_eq!(
                bytes(&ch.to_string(), ctrl()),
                Some(vec![expected]),
                "ctrl-{ch}"
            );
        }
        // keys.rs:69-74 — Ctrl+C is 0x03 in either case.
        assert_eq!(bytes("c", ctrl()), Some(vec![0x03]));
        assert_eq!(bytes("C", ctrl()), Some(vec![0x03]));
        assert_eq!(bytes("a", ctrl()), Some(vec![0x01]));
        assert_eq!(bytes("z", ctrl()), Some(vec![0x1a]));
    }

    /// `keys.rs:18-19` arithmetic over `' '`: (0x20 - 0x40) & 0x1f == 0.
    #[test]
    fn ctrl_space_is_nul() {
        // gpui names the space bar "space", so the named table would claim it first. Ctrl+Space must still be NUL — the table must not shadow the Ctrl arithmetic.
        assert_eq!(bytes("space", ctrl()), Some(vec![0x00]));
        assert_eq!(bytes(" ", ctrl()), Some(vec![0x00]));
        // …and Ctrl over a *named* key that is not the space bar still takes the named arm, exactly as iced's `Key::Named` match does.
        assert_eq!(bytes("enter", ctrl()).as_deref(), Some(&b"\r"[..]));
        assert_eq!(bytes("up", ctrl()).as_deref(), Some(&b"\x1b[A"[..]));
    }

    /// `keys.rs:14-17`, iced test `keys.rs:85` — no garbage bytes.
    #[test]
    fn ctrl_non_ascii_emits_nothing() {
        assert_eq!(bytes("é", ctrl()), None);
    }

    /// `keys.rs:7-9`, iced test `:96` — Alt prefixes ESC onto the unmodified sequence.
    #[test]
    fn alt_prefixes_esc() {
        assert_eq!(bytes("up", alt()).as_deref(), Some(&b"\x1b\x1b[A"[..]));
        assert_eq!(
            key_to_bytes(&typed("a", alt()), false).as_deref(),
            Some(&b"\x1ba"[..])
        );
        // The spec's "Alt+Escape reaches the PTY as ESC ESC" (`:7-9` + `:28`).
        assert_eq!(bytes("escape", alt()).as_deref(), Some(&b"\x1b\x1b"[..]));
    }

    /// Super/Cmd chords are app chords and never reach the PTY (findings §S1 Step 4). iced never saw them; gpui does, so the filter lives here.
    #[test]
    fn platform_chords_never_reach_the_pty() {
        let sup = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        assert_eq!(bytes("a", sup), None);
        assert_eq!(bytes("enter", sup), None);
    }

    /// An unbound ctrl-shift-letter chord must never fall through to the Ctrl arithmetic; named TUI chords are unaffected.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_shift_letters_are_swallowed_on_non_mac() {
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };
        for key in ["j", "h", "g"] {
            assert_eq!(bytes(key, ctrl_shift), None, "ctrl-shift-{key}");
        }
        // The TUI-chord contract still holds for named keys.
        assert_eq!(bytes("enter", ctrl_shift).as_deref(), Some(&b"\r"[..]));
        assert_eq!(
            bytes("pageup", ctrl_shift).as_deref(),
            Some(&b"\x1b[5~"[..])
        );
    }

    /// DECCKM must NOT change keypress encoding: `keys.rs` is app-cursor unaware.
    #[test]
    fn app_cursor_does_not_change_keypresses() {
        for app_cursor in [false, true] {
            assert_eq!(
                key_to_bytes(&ks("up", none()), app_cursor).as_deref(),
                Some(&b"\x1b[A"[..]),
                "plain Up must be CSI in both cursor modes (app_cursor={app_cursor})"
            );
        }
    }

    /// No CSI-modifier form: the spike emitted `\x1b[1;2A` for Shift+Up; iced does not.
    #[test]
    fn modified_arrows_stay_plain() {
        assert_eq!(bytes("up", shift()).as_deref(), Some(&b"\x1b[A"[..]));
        assert_eq!(bytes("up", ctrl()).as_deref(), Some(&b"\x1b[A"[..]));
        assert_eq!(bytes("left", shift()).as_deref(), Some(&b"\x1b[D"[..]));
    }

    #[test]
    fn arrow_moves_walk_right_in_csi_mode() {
        assert_eq!(arrow_moves(5, 8, false), b"\x1b[C\x1b[C\x1b[C".to_vec());
    }

    #[test]
    fn arrow_moves_walk_left_in_ss3_mode() {
        assert_eq!(arrow_moves(8, 5, true), b"\x1bOD\x1bOD\x1bOD".to_vec());
    }

    #[test]
    fn arrow_moves_to_the_same_column_emit_nothing() {
        assert!(arrow_moves(4, 4, false).is_empty());
        assert!(arrow_moves(4, 4, true).is_empty());
    }

    #[test]
    fn scroll_chords_match_shift_only() {
        assert_eq!(
            keyboard_scroll_intent(&ks("pageup", shift())),
            Some((true, ScrollAmount::Page))
        );
        assert_eq!(
            keyboard_scroll_intent(&ks("pagedown", shift())),
            Some((false, ScrollAmount::Page))
        );
        assert_eq!(
            keyboard_scroll_intent(&ks("home", shift())),
            Some((true, ScrollAmount::All))
        );
        assert_eq!(
            keyboard_scroll_intent(&ks("end", shift())),
            Some((false, ScrollAmount::All))
        );
    }

    #[test]
    fn plain_navigation_keys_fall_through_to_the_pty() {
        for key in ["pageup", "pagedown", "home", "end"] {
            assert_eq!(keyboard_scroll_intent(&ks(key, none())), None, "{key}");
        }
    }

    #[test]
    fn extra_modifiers_are_not_stolen_from_tui_chords() {
        for extra in [
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
            Modifiers {
                platform: true,
                shift: true,
                ..Modifiers::default()
            },
            Modifiers {
                alt: true,
                shift: true,
                ..Modifiers::default()
            },
        ] {
            assert_eq!(keyboard_scroll_intent(&ks("pageup", extra)), None);
        }
    }

    #[test]
    fn copy_and_paste_chords_match_the_platform_rule() {
        #[cfg(target_os = "macos")]
        let (hit, letter_only) = (
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
            Modifiers::default(),
        );
        #[cfg(not(target_os = "macos"))]
        let (hit, letter_only) = (
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
            Modifiers::default(),
        );

        assert!(is_copy_shortcut(&ks("c", hit)));
        assert!(is_copy_shortcut(&ks("C", hit)));
        assert!(is_paste_shortcut(&ks("v", hit)));
        assert!(!is_copy_shortcut(&ks("v", hit)));
        assert!(!is_copy_shortcut(&ks("c", letter_only)));
        // Plain Ctrl+V is deliberately left to the PTY.
        assert!(!is_paste_shortcut(&ks("v", ctrl())));
    }
}
