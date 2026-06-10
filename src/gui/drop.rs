//! File drag-and-drop support: converts dropped file paths into text typed
//! into the focused session's PTY.

use std::path::Path;

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
}
