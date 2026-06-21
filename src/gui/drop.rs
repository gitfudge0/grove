//! File drag-and-drop support: converts dropped file paths into text typed
//! into the focused session's PTY.

use std::path::{Path, PathBuf};

/// File paths sitting on the Wayland clipboard as a `text/uri-list`, e.g. after
/// "Copy" in a file manager. Empty when not on Wayland, when `wl-paste` is
/// missing, or when the clipboard holds no file URIs. Lets Wayland users paste
/// a path where X11/macOS get native drag-and-drop (winit has no Wayland DnD).
pub fn clipboard_paths() -> Vec<PathBuf> {
    match std::process::Command::new("wl-paste")
        .args(["--no-newline", "--type", "text/uri-list"])
        .output()
    {
        Ok(o) if o.status.success() => parse_uri_list(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Parse a `text/uri-list` into local paths: keep `file://` entries, drop
/// comment lines (`#`) and non-file schemes, percent-decode the rest. Local
/// file URIs carry an empty authority (`file:///path`), so stripping the scheme
/// leaves the absolute path.
fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|p| PathBuf::from(percent_decode(p)))
        .collect()
}

/// Decode `%XX` byte escapes in a URI path. Invalid escapes pass through.
fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut it = s.bytes().enumerate();
    let raw = s.as_bytes();
    while let Some((i, b)) = it.next() {
        if b == b'%' {
            if let (Some(hi), Some(lo)) =
                (hex(raw.get(i + 1).copied()), hex(raw.get(i + 2).copied()))
            {
                bytes.push(hi << 4 | lo);
                it.next();
                it.next();
                continue;
            }
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex(b: Option<u8>) -> Option<u8> {
    match b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Render a dropped path as terminal input: the path, shell-escaped when it
/// contains characters special to a shell, followed by a single space so
/// consecutive drops (and the user's next keystroke) stay separated.
pub fn dropped_path_text(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("{} ", shell_escape(&raw))
}

/// Quote `s` for POSIX shells. Plain paths pass through untouched; anything
/// containing shell metacharacters or whitespace is single-quoted, with
/// embedded single quotes escaped as `'\''`.
fn shell_escape(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '/' | '.' | '-' | '_' | '+' | ':' | '@' | '%' | ',' | '~' | '='
                )
        });
    if plain {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn plain_path_passes_through_with_trailing_space() {
        let p = PathBuf::from("/Users/dev/project/src/main.rs");
        assert_eq!(dropped_path_text(&p), "/Users/dev/project/src/main.rs ");
    }

    #[test]
    fn path_with_spaces_is_single_quoted() {
        let p = PathBuf::from("/Users/dev/My Docs/file 1.txt");
        assert_eq!(dropped_path_text(&p), "'/Users/dev/My Docs/file 1.txt' ");
    }

    #[test]
    fn embedded_single_quote_is_escaped() {
        let p = PathBuf::from("/tmp/it's here.txt");
        assert_eq!(dropped_path_text(&p), r"'/tmp/it'\''s here.txt' ");
    }

    #[test]
    fn shell_metacharacters_trigger_quoting() {
        let p = PathBuf::from("/tmp/a&b(c).txt");
        assert_eq!(dropped_path_text(&p), "'/tmp/a&b(c).txt' ");
    }

    #[test]
    fn unicode_path_is_quoted() {
        let p = PathBuf::from("/tmp/résumé.pdf");
        assert_eq!(dropped_path_text(&p), "'/tmp/résumé.pdf' ");
    }

    #[test]
    fn uri_list_decodes_and_filters() {
        let list = "#comment\n\
                    file:///tmp/My%20Docs/file%201.txt\n\
                    https://example.com/skip\n\
                    file:///tmp/r%C3%A9sum%C3%A9.pdf\n";
        assert_eq!(
            parse_uri_list(list),
            vec![
                PathBuf::from("/tmp/My Docs/file 1.txt"),
                PathBuf::from("/tmp/résumé.pdf"),
            ]
        );
    }

    #[test]
    fn uri_list_empty_when_no_file_uris() {
        assert!(parse_uri_list("https://x.com\n# only comments\n").is_empty());
    }

    #[test]
    fn percent_decode_leaves_invalid_escapes() {
        assert_eq!(percent_decode("100%done%2Fok"), "100%done/ok");
    }
}
