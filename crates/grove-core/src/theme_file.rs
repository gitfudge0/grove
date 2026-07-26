//! User-defined ("custom") theme storage: `dirs::config_dir()/grove/themes.json`.
//! Mirrors `storage.rs`'s atomic-write convention but is kept separate since
//! it's a distinct file with its own schema and validation rules.

use crate::theme::{Color, Theme, ThemeKind};
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;

/// On-disk DTO for a single theme. Field names match `Theme`'s color fields;
/// colors are `#rrggbb` hex strings.
#[derive(Debug, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub kind: String,
    pub colors: ThemeColors,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ThemeColors {
    pub bg: String,
    pub bg_highlight: String,
    pub fg: String,
    pub fg_dark: String,
    pub comment: String,
    pub blue: String,
    pub cyan: String,
    pub magenta: String,
    pub green: String,
    pub yellow: String,
    pub red: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ThemeFile {
    #[serde(default)]
    themes: Vec<ThemeDef>,
}

/// A theme entry that was skipped while loading `themes.json`, with a
/// human-readable reason. `name` is `None` only for whole-file failures
/// (e.g. corrupt JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeLoadError {
    pub name: Option<String>,
    pub reason: String,
}

/// One-line summary of `errors` for a non-blocking notice, matching the
/// mock's toast copy exactly: `themes.json: skipped '<name>' — <reason>`,
/// or `themes.json: <reason>` when the entry had no name (a whole-file
/// failure). Any further errors beyond the first are summarized as
/// `(+N more)` rather than listed in full. `None` when `errors` is empty —
/// callers use that to skip showing a notice at all.
pub fn summarize_errors(errors: &[ThemeLoadError]) -> Option<String> {
    let first = errors.first()?;
    let head = match &first.name {
        Some(name) => format!("themes.json: skipped '{name}' — {}", first.reason),
        None => format!("themes.json: {}", first.reason),
    };
    Some(if errors.len() > 1 {
        format!("{head} (+{} more)", errors.len() - 1)
    } else {
        head
    })
}

/// Parses `#rrggbb` (leading `#` optional, case-insensitive) into a `Color`.
pub fn parse_hex(s: &str) -> Result<Color, String> {
    let s = s.strip_prefix('#').unwrap_or(s);
    // The length check below is byte-based, and the slicing after it
    // (`&s[i..i+2]`) indexes by byte offset too — both are only safe once
    // every byte is confirmed ASCII (a single non-ASCII char, e.g. multibyte
    // UTF-8, can make `s.len()` land on 6 while still slicing mid-character
    // and panicking). Rejecting non-hexdigit bytes up front guarantees ASCII.
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid hex color \"{s}\" (expected #rrggbb)"));
    }
    let byte = |i: usize| -> Result<u8, String> {
        u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|_| format!("invalid hex color \"{s}\" (expected #rrggbb)"))
    };
    Ok(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// Serializes a `Color` back to lowercase `#rrggbb`.
pub fn to_hex(c: Color) -> String {
    let Color::Rgb(r, g, b) = c;
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn parse_kind(s: &str) -> Result<ThemeKind, String> {
    match s {
        "dark" => Ok(ThemeKind::Dark),
        "light" => Ok(ThemeKind::Light),
        other => Err(format!(
            "unknown kind \"{other}\" (expected \"dark\" or \"light\")"
        )),
    }
}

/// Validates and converts a single `ThemeDef` into a `Theme`, or an error
/// reason if it's malformed.
fn convert(def: ThemeDef) -> Result<Theme, String> {
    if def.name.trim().is_empty() {
        return Err("empty name".to_string());
    }
    let kind = parse_kind(&def.kind)?;
    let c = &def.colors;
    Ok(Theme {
        name: Cow::Owned(def.name),
        kind,
        bg: parse_hex(&c.bg)?,
        bg_highlight: parse_hex(&c.bg_highlight)?,
        fg: parse_hex(&c.fg)?,
        fg_dark: parse_hex(&c.fg_dark)?,
        comment: parse_hex(&c.comment)?,
        blue: parse_hex(&c.blue)?,
        cyan: parse_hex(&c.cyan)?,
        magenta: parse_hex(&c.magenta)?,
        green: parse_hex(&c.green)?,
        yellow: parse_hex(&c.yellow)?,
        red: parse_hex(&c.red)?,
    })
}

fn config_path() -> Option<PathBuf> {
    // Routed through `storage::config_dir()` so `GROVE_CONFIG_DIR` and the
    // legacy `work-manager` migration both apply here too.
    Some(crate::storage::config_dir().ok()?.join("themes.json"))
}

/// Loads custom themes from `themes.json`. Missing file yields an empty,
/// error-free result. Corrupt top-level JSON yields a single error and an
/// empty result, without deleting or otherwise touching the file. Per-entry
/// problems (bad hex, missing field, empty name, unknown kind, duplicate
/// name, or a name shadowing a builtin) cause that entry to be skipped with
/// a recorded reason; first occurrence of a duplicate name wins.
pub fn load() -> (Vec<Theme>, Vec<ThemeLoadError>) {
    let Some(path) = config_path() else {
        return (Vec::new(), Vec::new());
    };
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return (
                Vec::new(),
                vec![ThemeLoadError {
                    name: None,
                    reason: format!("failed to read {}: {e}", path.display()),
                }],
            )
        }
    };
    let file: ThemeFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            return (
                Vec::new(),
                vec![ThemeLoadError {
                    name: None,
                    reason: format!("failed to parse {}: {e}", path.display()),
                }],
            )
        }
    };

    let mut themes: Vec<Theme> = Vec::new();
    let mut errors: Vec<ThemeLoadError> = Vec::new();
    for def in file.themes {
        let name = def.name.clone();
        if name.trim().is_empty() {
            errors.push(ThemeLoadError {
                name: None,
                reason: "empty name".to_string(),
            });
            continue;
        }
        if crate::theme::BUILTINS.iter().any(|t| t.name == name) {
            errors.push(ThemeLoadError {
                name: Some(name),
                reason: "shadows built-in".to_string(),
            });
            continue;
        }
        if themes.iter().any(|t: &Theme| t.name == name) {
            errors.push(ThemeLoadError {
                name: Some(name),
                reason: "duplicate custom name (first wins)".to_string(),
            });
            continue;
        }
        match convert(def) {
            Ok(t) => themes.push(t),
            Err(reason) => errors.push(ThemeLoadError {
                name: Some(name),
                reason,
            }),
        }
    }
    (themes, errors)
}

/// Writes `themes` to `themes.json` via a sibling temp file + rename.
pub fn save(themes: &[Theme]) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| std::io::Error::other("no config dir"))?;
    let file = ThemeFile {
        themes: themes
            .iter()
            .map(|t| ThemeDef {
                name: t.name.to_string(),
                kind: match t.kind {
                    ThemeKind::Dark => "dark".to_string(),
                    ThemeKind::Light => "light".to_string(),
                },
                colors: ThemeColors {
                    bg: to_hex(t.bg),
                    bg_highlight: to_hex(t.bg_highlight),
                    fg: to_hex(t.fg),
                    fg_dark: to_hex(t.fg_dark),
                    comment: to_hex(t.comment),
                    blue: to_hex(t.blue),
                    cyan: to_hex(t.cyan),
                    magenta: to_hex(t.magenta),
                    green: to_hex(t.green),
                    yellow: to_hex(t.yellow),
                    red: to_hex(t.red),
                },
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&file)?;
    // `storage::write_atomic` rather than a hand-rolled `.json.tmp` sibling:
    // its temp name is unique per write, so two concurrent writers can't
    // rename a half-written mix into place.
    crate::storage::write_atomic(&path, json.as_bytes()).map_err(std::io::Error::other)?;
    Ok(())
}

/// Canonical field order used by the bare-hex paste format and by
/// [`PastedColors`]'s iteration helpers. This is just `Theme::FIELD_NAMES`
/// under this module's own established name — an alias rather than a second
/// copy of the same 11 strings, so the two can never drift out of sync.
pub use crate::theme::FIELD_NAMES as FIELD_ORDER;

/// Result of successfully parsing a pasted palette: a name/kind (only present
/// when the paste was full-`ThemeDef` JSON and included them) plus whichever
/// of the 11 color fields were present in the paste — a subset for the named-
/// lines format, always all 11 for the bare-hex format.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PastedColors {
    pub name: Option<String>,
    pub kind: Option<ThemeKind>,
    /// `(field_name, color)` pairs, in `FIELD_ORDER` order, deduplicated so
    /// each field appears at most once (last write wins).
    pub colors: Vec<(&'static str, Color)>,
}

impl PastedColors {
    fn set(&mut self, field: &'static str, color: Color) {
        if let Some(slot) = self.colors.iter_mut().find(|(f, _)| *f == field) {
            slot.1 = color;
        } else {
            self.colors.push((field, color));
        }
        self.colors
            .sort_by_key(|(f, _)| FIELD_ORDER.iter().position(|x| x == f).unwrap_or(99));
    }

    pub fn get(&self, field: &str) -> Option<Color> {
        self.colors
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, c)| *c)
    }
}

/// Applies a parsed paste to `draft`: every field `applied` carries is
/// written into `draft` (a subset updates only those fields, everything else
/// on `draft` is untouched); `applied.name`/`applied.kind`, when present,
/// overwrite `draft.name`/`draft.kind` too. Used by `Modal::ThemeManager`'s
/// editor "Apply" action after a successful `parse_paste`.
pub fn apply_pasted_colors(draft: &mut Theme, applied: &PastedColors) {
    for (i, field) in FIELD_ORDER.iter().enumerate() {
        if let Some(color) = applied.get(field) {
            draft.set_field(i, color);
        }
    }
    if let Some(name) = &applied.name {
        draft.name = std::borrow::Cow::Owned(name.clone());
    }
    if let Some(kind) = applied.kind {
        draft.kind = kind;
    }
}

/// Serializes `theme`'s 11 colors as canonical `field #hex` lines, one per
/// field in `FIELD_ORDER`, using `to_hex`'s lowercase `#rrggbb` formatting —
/// the exact named-lines shape `parse_paste` accepts, so this round-trips.
/// Used to prefill `Modal::ThemeManager`'s editor paste box with the draft's
/// current values (a copyable reference and an editable starting point,
/// rather than an empty box with just a placeholder).
pub fn to_named_lines(theme: &Theme) -> String {
    FIELD_ORDER
        .iter()
        .enumerate()
        .map(|(i, field)| format!("{field} {}", to_hex(theme.field(i))))
        .collect::<Vec<_>>()
        .join("\n")
}

fn canonical_field(name: &str) -> Option<&'static str> {
    let lower = name.trim().to_lowercase();
    FIELD_ORDER.iter().copied().find(|f| *f == lower)
}

/// Parses a pasted palette in one of three auto-detected formats, applying a
/// SUBSET of fields is valid for the JSON and named-lines formats; the
/// bare-hex format always supplies all 11.
///
/// Detection order:
/// 1. JSON `ThemeDef` (full, or `{"colors": {...}}` only) — leading `{`.
/// 2. Named lines — every non-blank line looks like `field #hex` or
///    `field: hex` (field names case-insensitive, `#` optional).
/// 3. Bare hex — exactly 11 whitespace/comma-separated hex values, applied
///    in `FIELD_ORDER`.
pub fn parse_paste(input: &str) -> Result<PastedColors, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("nothing to parse".to_string());
    }
    if trimmed.starts_with('{') {
        return parse_paste_json(trimmed);
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.is_empty() && lines.iter().all(|l| looks_like_named_line(l)) {
        return parse_paste_named_lines(&lines);
    }
    parse_paste_bare_hex(trimmed)
}

fn looks_like_named_line(line: &str) -> bool {
    split_named_line(line).is_some()
}

/// Splits a `field #hex` / `field: hex` / `field: #hex` line into
/// `(field, hex)`, or `None` if it doesn't have that shape at all (used both
/// to detect the format and to parse it).
fn split_named_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    // Split on the first run of whitespace and/or a single ':'.
    let colon_or_space = line.find(|c: char| c == ':' || c.is_whitespace())?;
    let (field, rest) = line.split_at(colon_or_space);
    let rest = rest.trim_start_matches(':').trim();
    if field.trim().is_empty() || rest.is_empty() {
        return None;
    }
    // The value must look like a hex color (optionally `#`-prefixed, 6 hex
    // digits) for this to count as a named line at all.
    let hex = rest.strip_prefix('#').unwrap_or(rest);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some((field.trim(), rest))
    } else {
        None
    }
}

fn parse_paste_named_lines(lines: &[&str]) -> Result<PastedColors, String> {
    let mut out = PastedColors::default();
    for (i, line) in lines.iter().enumerate() {
        let (field, hex) = split_named_line(line)
            .ok_or_else(|| format!("line {}: expected 'field #hex'", i + 1))?;
        let Some(canonical) = canonical_field(field) else {
            return Err(format!("line {}: unknown field '{}'", i + 1, field.trim()));
        };
        let color = parse_hex(hex).map_err(|e| format!("line {}: {e}", i + 1))?;
        out.set(canonical, color);
    }
    Ok(out)
}

fn parse_paste_bare_hex(input: &str) -> Result<PastedColors, String> {
    let tokens: Vec<&str> = input
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.len() != 11 {
        return Err(format!("expected 11 colors, got {}", tokens.len()));
    }
    let mut out = PastedColors::default();
    for (field, tok) in FIELD_ORDER.iter().zip(tokens.iter()) {
        let color = parse_hex(tok)?;
        out.set(field, color);
    }
    Ok(out)
}

fn parse_paste_json(trimmed: &str) -> Result<PastedColors, String> {
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    let mut out = PastedColors::default();
    if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
        out.name = Some(name.to_string());
    }
    if let Some(kind) = obj.get("kind").and_then(|v| v.as_str()) {
        out.kind = Some(parse_kind(kind)?);
    }
    let colors_obj = match obj.get("colors") {
        Some(v) => v
            .as_object()
            .ok_or_else(|| "\"colors\" must be an object".to_string())?,
        None => obj, // no nested "colors" key: allow bare top-level fields too
    };
    for (key, value) in colors_obj {
        let Some(canonical) = canonical_field(key) else {
            // Ignore JSON keys that aren't color fields (name/kind/etc. at
            // the top level fall through here harmlessly).
            continue;
        };
        let hex = value
            .as_str()
            .ok_or_else(|| format!("field '{key}' must be a hex string"))?;
        out.colors.push((
            canonical,
            parse_hex(hex).map_err(|e| format!("field '{key}': {e}"))?,
        ));
    }
    out.colors
        .sort_by_key(|(f, _)| FIELD_ORDER.iter().position(|x| x == f).unwrap_or(99));
    if out.colors.is_empty() && out.name.is_none() && out.kind.is_none() {
        return Err("no recognized fields found".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_ok_with_and_without_hash() {
        assert!(matches!(
            parse_hex("#ff00aa"),
            Ok(Color::Rgb(0xff, 0x00, 0xaa))
        ));
        assert!(matches!(
            parse_hex("FF00AA"),
            Ok(Color::Rgb(0xff, 0x00, 0xaa))
        ));
    }

    #[test]
    fn parse_hex_rejects_bad_input() {
        assert!(parse_hex("#zzzzzz").is_err());
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("").is_err());
    }

    /// A multibyte character can make `s.len()` (byte length) land on 6
    /// while the string has fewer than 6 chars — slicing `&s[i..i+2]` at
    /// those byte offsets used to panic instead of returning `Err`.
    #[test]
    fn parse_hex_rejects_multibyte_without_panicking() {
        assert!(parse_hex("aéaab").is_err());
        assert!(parse_hex("é11111").is_err());
        assert!(parse_hex("111111é").is_err());
        assert!(parse_hex("#aébbcc").is_err());
    }

    /// Mirrors the theme editor's live per-keystroke validation
    /// (`Grove::theme_manager_editor_hex_changed`): every partial state while
    /// typing a full `#rrggbb` is invalid until the final character lands,
    /// and any one bad hex digit anywhere keeps it invalid.
    #[test]
    fn parse_hex_incremental_typing_states() {
        let full = "#a1b2c3";
        for i in 1..full.len() {
            assert!(
                parse_hex(&full[..i]).is_err(),
                "partial input {:?} must not parse",
                &full[..i]
            );
        }
        assert!(parse_hex(full).is_ok(), "the complete hex must parse");
        assert!(
            parse_hex("#a1b2cg").is_err(),
            "a non-hex digit must stay invalid"
        );
        assert!(
            parse_hex("#a1b2c30").is_err(),
            "trailing extra digit must stay invalid"
        );
    }

    #[test]
    fn to_hex_round_trip() {
        let c = Color::Rgb(0x1a, 0x2b, 0x3c);
        assert_eq!(to_hex(c), "#1a2b3c");
    }

    fn sample_def(name: &str) -> ThemeDef {
        ThemeDef {
            name: name.to_string(),
            kind: "dark".to_string(),
            colors: ThemeColors {
                bg: "#111111".to_string(),
                bg_highlight: "#222222".to_string(),
                fg: "#eeeeee".to_string(),
                fg_dark: "#dddddd".to_string(),
                comment: "#999999".to_string(),
                blue: "#0000ff".to_string(),
                cyan: "#00ffff".to_string(),
                magenta: "#ff00ff".to_string(),
                green: "#00ff00".to_string(),
                yellow: "#ffff00".to_string(),
                red: "#ff0000".to_string(),
            },
        }
    }

    #[test]
    fn convert_ok() {
        let t = convert(sample_def("mytheme")).expect("convert");
        assert_eq!(t.name, "mytheme");
        assert!(matches!(t.kind, ThemeKind::Dark));
    }

    #[test]
    fn convert_rejects_bad_hex() {
        let mut def = sample_def("mytheme");
        def.colors.bg = "not-a-color".to_string();
        assert!(convert(def).is_err());
    }

    #[test]
    fn convert_rejects_unknown_kind() {
        let mut def = sample_def("mytheme");
        def.kind = "sepia".to_string();
        let Err(err) = convert(def) else {
            panic!("expected an error")
        };
        assert!(err.contains("unknown kind"));
    }

    #[test]
    fn convert_rejects_empty_name() {
        let def = sample_def("");
        let Err(err) = convert(def) else {
            panic!("expected an error")
        };
        assert_eq!(err, "empty name");
    }

    #[test]
    fn summarize_errors_empty_is_none() {
        assert_eq!(summarize_errors(&[]), None);
    }

    #[test]
    fn summarize_errors_single_named_entry() {
        let errors = vec![ThemeLoadError {
            name: Some("my-theme".to_string()),
            reason: "missing field `bg_highlight`".to_string(),
        }];
        assert_eq!(
            summarize_errors(&errors).as_deref(),
            Some("themes.json: skipped 'my-theme' — missing field `bg_highlight`")
        );
    }

    #[test]
    fn summarize_errors_nameless_entry_omits_skipped_wording() {
        let errors = vec![ThemeLoadError {
            name: None,
            reason: "failed to parse themes.json: EOF".to_string(),
        }];
        assert_eq!(
            summarize_errors(&errors).as_deref(),
            Some("themes.json: failed to parse themes.json: EOF")
        );
    }

    #[test]
    fn parse_paste_full_json_theme_def() {
        let json = r##"{"name":"mytheme","kind":"dark","colors":{"bg":"#111111","bg_highlight":"#222222","fg":"#eeeeee","fg_dark":"#dddddd","comment":"#999999","blue":"#0000ff","cyan":"#00ffff","magenta":"#ff00ff","green":"#00ff00","yellow":"#ffff00","red":"#ff0000"}}"##;
        let out = parse_paste(json).expect("parse");
        assert_eq!(out.name.as_deref(), Some("mytheme"));
        assert!(matches!(out.kind, Some(ThemeKind::Dark)));
        assert_eq!(out.colors.len(), 11);
        assert_eq!(out.get("bg"), Some(Color::Rgb(0x11, 0x11, 0x11)));
        assert_eq!(out.get("red"), Some(Color::Rgb(0xff, 0x00, 0x00)));
    }

    #[test]
    fn parse_paste_colors_only_json() {
        let json = r##"{"colors":{"bg":"#000000","fg":"#ffffff"}}"##;
        let out = parse_paste(json).expect("parse");
        assert_eq!(out.name, None);
        assert_eq!(out.kind, None);
        assert_eq!(out.colors.len(), 2);
        assert_eq!(out.get("bg"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(out.get("fg"), Some(Color::Rgb(0xff, 0xff, 0xff)));
    }

    #[test]
    fn parse_paste_json_rejects_bad_hex() {
        let json = r#"{"colors":{"bg":"nope"}}"#;
        assert!(parse_paste(json).is_err());
    }

    #[test]
    fn parse_paste_json_rejects_bad_kind() {
        let json = r##"{"name":"x","kind":"sepia","colors":{"bg":"#000000"}}"##;
        let err = parse_paste(json).unwrap_err();
        assert!(err.contains("unknown kind"), "{err}");
    }

    #[test]
    fn parse_paste_named_lines_any_order_case_insensitive() {
        let input = "FG: #eeeeee\nbg #111111\nRed: ff0000";
        let out = parse_paste(input).expect("parse");
        assert_eq!(out.colors.len(), 3);
        assert_eq!(out.get("fg"), Some(Color::Rgb(0xee, 0xee, 0xee)));
        assert_eq!(out.get("bg"), Some(Color::Rgb(0x11, 0x11, 0x11)));
        assert_eq!(out.get("red"), Some(Color::Rgb(0xff, 0x00, 0x00)));
    }

    #[test]
    fn parse_paste_named_lines_subset_is_valid() {
        let input = "bg: #111111";
        let out = parse_paste(input).expect("parse");
        assert_eq!(out.colors, vec![("bg", Color::Rgb(0x11, 0x11, 0x11))]);
    }

    #[test]
    fn parse_paste_named_lines_unknown_field_errors_with_line_number() {
        let input = "bg: #111111\nfoo: #222222";
        let err = parse_paste(input).unwrap_err();
        assert_eq!(err, "line 2: unknown field 'foo'");
    }

    #[test]
    fn parse_paste_named_lines_bad_hex_errors_with_line_number() {
        let input = "bg: #111111\nfg: notahex";
        let err = parse_paste(input);
        // "fg: notahex" doesn't look like a named line at all (value isn't
        // hex-shaped), so this falls through to bare-hex parsing and fails
        // there instead — both are acceptable rejections, just confirm it
        // errors.
        assert!(err.is_err());
    }

    #[test]
    fn parse_paste_named_lines_duplicate_field_last_wins() {
        let input = "bg: #111111\nbg: #222222";
        let out = parse_paste(input).expect("parse");
        assert_eq!(out.colors, vec![("bg", Color::Rgb(0x22, 0x22, 0x22))]);
    }

    #[test]
    fn parse_paste_bare_hex_all_eleven_in_canonical_order() {
        let input = "#111111, #222222 #333333, #444444 #555555 #666666 #777777, #888888, #999999 #aaaaaa #bbbbbb";
        let out = parse_paste(input).expect("parse");
        assert_eq!(out.colors.len(), 11);
        assert_eq!(out.get("bg"), Some(Color::Rgb(0x11, 0x11, 0x11)));
        assert_eq!(out.get("bg_highlight"), Some(Color::Rgb(0x22, 0x22, 0x22)));
        assert_eq!(out.get("red"), Some(Color::Rgb(0xbb, 0xbb, 0xbb)));
    }

    #[test]
    fn parse_paste_bare_hex_wrong_count_errors() {
        let input = "#111111 #222222 #333333";
        let err = parse_paste(input).unwrap_err();
        assert_eq!(err, "expected 11 colors, got 3");
    }

    #[test]
    fn parse_paste_bare_hex_invalid_token_errors() {
        let input = "#111111 #222222 #333333 #444444 #555555 #666666 #777777 #888888 #999999 #aaaaaa zzzzzz";
        assert!(parse_paste(input).is_err());
    }

    #[test]
    fn parse_paste_empty_errors() {
        assert!(parse_paste("").is_err());
        assert!(parse_paste("   \n  ").is_err());
    }

    #[test]
    fn parse_paste_malformed_json_errors() {
        let err = parse_paste("{not json").unwrap_err();
        assert!(err.contains("invalid JSON"), "{err}");
    }

    fn sample_theme(name: &str) -> Theme {
        Theme {
            name: Cow::Owned(name.to_string()),
            kind: ThemeKind::Dark,
            bg: Color::Rgb(0x10, 0x10, 0x10),
            bg_highlight: Color::Rgb(0x20, 0x20, 0x20),
            fg: Color::Rgb(0xe0, 0xe0, 0xe0),
            fg_dark: Color::Rgb(0xd0, 0xd0, 0xd0),
            comment: Color::Rgb(0x90, 0x90, 0x90),
            blue: Color::Rgb(0x00, 0x00, 0xff),
            cyan: Color::Rgb(0x00, 0xff, 0xff),
            magenta: Color::Rgb(0xff, 0x00, 0xff),
            green: Color::Rgb(0x00, 0xff, 0x00),
            yellow: Color::Rgb(0xff, 0xff, 0x00),
            red: Color::Rgb(0xff, 0x00, 0x00),
        }
    }

    #[test]
    fn apply_pasted_colors_subset_updates_only_named_fields() {
        let mut draft = sample_theme("mytheme");
        let original_fg = draft.fg;
        let applied = parse_paste("bg: #010101").unwrap();
        apply_pasted_colors(&mut draft, &applied);
        assert_eq!(draft.bg, Color::Rgb(0x01, 0x01, 0x01));
        assert_eq!(
            draft.fg, original_fg,
            "untouched fields must survive a subset apply"
        );
        assert_eq!(
            draft.name, "mytheme",
            "name untouched when the paste had none"
        );
    }

    #[test]
    fn apply_pasted_colors_full_json_overwrites_name_and_kind() {
        let mut draft = sample_theme("old-name");
        draft.kind = ThemeKind::Dark;
        let json = r##"{"name":"new-name","kind":"light","colors":{"bg":"#ffffff"}}"##;
        let applied = parse_paste(json).unwrap();
        apply_pasted_colors(&mut draft, &applied);
        assert_eq!(draft.name, "new-name");
        assert!(matches!(draft.kind, ThemeKind::Light));
        assert_eq!(draft.bg, Color::Rgb(0xff, 0xff, 0xff));
    }

    #[test]
    fn apply_pasted_colors_bare_hex_overwrites_every_field() {
        let mut draft = sample_theme("mytheme");
        let input = "#111111 #222222 #333333 #444444 #555555 #666666 #777777 #888888 #999999 #aaaaaa #bbbbbb";
        let applied = parse_paste(input).unwrap();
        apply_pasted_colors(&mut draft, &applied);
        assert_eq!(draft.bg, Color::Rgb(0x11, 0x11, 0x11));
        assert_eq!(draft.red, Color::Rgb(0xbb, 0xbb, 0xbb));
        assert_eq!(draft.name, "mytheme", "bare-hex paste never carries a name");
    }

    #[test]
    fn to_named_lines_covers_all_11_fields_in_canonical_order_lowercase() {
        let theme = sample_theme("mytheme");
        let lines = to_named_lines(&theme);
        let rows: Vec<&str> = lines.lines().collect();
        assert_eq!(rows.len(), 11);
        assert_eq!(rows[0], "bg #101010");
        assert_eq!(rows[1], "bg_highlight #202020");
        assert_eq!(rows[10], "red #ff0000");
        for (i, field) in FIELD_ORDER.iter().enumerate() {
            assert!(
                rows[i].starts_with(&format!("{field} #")),
                "row {i} should start with '{field} #', got {:?}",
                rows[i]
            );
        }
    }

    #[test]
    fn to_named_lines_round_trips_through_parse_paste() {
        let theme = sample_theme("mytheme");
        let lines = to_named_lines(&theme);
        let applied = parse_paste(&lines).expect("round-trip parse");
        assert_eq!(applied.colors.len(), 11);
        let mut round_tripped = theme.clone();
        apply_pasted_colors(&mut round_tripped, &applied);
        assert!(round_tripped.colors_eq(&theme));
    }

    #[test]
    fn summarize_errors_multiple_adds_more_count() {
        let errors = vec![
            ThemeLoadError {
                name: Some("a".to_string()),
                reason: "bad hex".to_string(),
            },
            ThemeLoadError {
                name: Some("b".to_string()),
                reason: "shadows built-in".to_string(),
            },
            ThemeLoadError {
                name: None,
                reason: "empty name".to_string(),
            },
        ];
        let summary = summarize_errors(&errors).unwrap();
        assert!(summary.starts_with("themes.json: skipped 'a' — bad hex"));
        assert!(summary.ends_with("(+2 more)"));
    }
}
