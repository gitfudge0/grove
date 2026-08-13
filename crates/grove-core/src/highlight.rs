//! Whole-file syntax highlighting for the diff viewer, pure and
//! gpui-free: a file's text goes in, one `Vec<Span>` per line comes out, each
//! span carrying a [`CodeScope`] rather than a colour — Grove's palette
//! (`src/theme.rs`'s seven `CODE_*` colours) is the only place colour is
//! decided, never syntect's own themes.
//!
//! We use syntect purely for its parser (`SyntaxSet`, `ParseState`,
//! `ScopeStack`) and never touch its `highlighting`/`Theme` machinery — there
//! is no `Theme` in this module at all.

use std::sync::OnceLock;

use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::diff::{DIFF_MAX_BYTES, DIFF_MAX_LINES};

/// The seven `CODE_*` theme targets, plus `Plain` for text no rule below
/// claims (including any scope on an unrecognised file extension).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeScope {
    Keyword,
    StringLit,
    Number,
    Comment,
    Type,
    Func,
    Punct,
    Plain,
}

/// One highlighted run of a line. `start`/`len` are **char offsets**, not
/// byte offsets, so a view slicing multibyte text (CJK, emoji) by these
/// bounds can never land mid-character and panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub len: usize,
    pub scope: CodeScope,
}

/// The bundled syntax definitions, parsed once and reused for every call —
/// `SyntaxSet::load_defaults_newlines` walks and compiles a sizeable bundled
/// dump, so paying that cost per file would make highlighting far more
/// expensive than the git shelling it rides alongside.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Map one syntect scope selector onto Grove's semantic vocabulary. Checked
/// top-of-stack-down (most specific scope first) so a nested rule (e.g. a
/// number literal inside a string interpolation) wins over its container.
/// Each arm's comment says why the sublime-syntax convention maps where it
/// does:
fn map_scope(scope: Scope) -> Option<CodeScope> {
    let name = scope.build_string();
    // `keyword.*` (control flow, `let`/`fn`/...) and `storage.*` (type/
    // modifier keywords like `struct`, `const`, `static`) both read as
    // "reserved word" to a reader, so both map to Keyword.
    if name.starts_with("keyword.operator") || name.starts_with("punctuation") {
        // Operators and punctuation are visually closer to structural noise
        // than to a reserved word — kept as their own Punct scope rather
        // than folding into Keyword, checked before the general
        // `keyword.*` rule below since `keyword.operator` also starts with
        // `keyword.`.
        return Some(CodeScope::Punct);
    }
    if name.starts_with("keyword") || name.starts_with("storage") {
        return Some(CodeScope::Keyword);
    }
    // `string.*` covers quoted literals in every bundled syntax, including
    // multi-line and interpolated forms.
    if name.starts_with("string") {
        return Some(CodeScope::StringLit);
    }
    // `constant.numeric.*` is numbers; `constant.language`/`constant.other`
    // (booleans, `nil`, named constants) read more like keywords than
    // numbers to a reader, so those fall through to the generic
    // `constant` case below... but sublime-syntax has no bare "constant"
    // scope used alone in practice, so treat non-numeric constants as
    // Keyword (`true`/`false`/`null` sit with the other reserved words).
    if name.starts_with("constant.numeric") {
        return Some(CodeScope::Number);
    }
    if name.starts_with("constant") {
        return Some(CodeScope::Keyword);
    }
    // `comment.*` — line and block comments, and their punctuation.
    if name.starts_with("comment") {
        return Some(CodeScope::Comment);
    }
    // Type names: declared types/classes (`entity.name.type`,
    // `entity.name.class`) and referenced/built-in types
    // (`support.type`, `support.class`, `storage.type` is *already*
    // Keyword above since it covers the `struct`/`class` keyword itself,
    // not the name that follows it).
    if name.starts_with("entity.name.type")
        || name.starts_with("entity.name.class")
        || name.starts_with("entity.other.inherited-class")
        || name.starts_with("support.type")
        || name.starts_with("support.class")
    {
        return Some(CodeScope::Type);
    }
    // Function/method names, declared or called, plus built-in functions.
    if name.starts_with("entity.name.function") || name.starts_with("support.function") {
        return Some(CodeScope::Func);
    }
    None
}

/// Pick the innermost (top-of-stack) scope in `stack` that [`map_scope`]
/// recognises, falling back to [`CodeScope::Plain`] when nothing in the
/// stack maps to anything — including an entirely unrecognised language,
/// whose only scope is the syntax's own root.
fn scope_for_stack(stack: &ScopeStack) -> CodeScope {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|&s| map_scope(s))
        .unwrap_or(CodeScope::Plain)
}

/// Highlight one line given the ops syntect's parser produced for it,
/// coalescing adjacent same-scope regions into one [`Span`] each. `line`
/// includes its trailing newline (`ScopeRegionIterator`'s byte offsets are
/// relative to exactly the string `parse_line` was called with); the
/// trailing `\n`/`\r\n` is then trimmed off the *char* count so the returned
/// spans cover only `visible_len` chars — the line's own text, without the
/// terminator.
fn spans_for_line(
    line: &str,
    visible_len: usize,
    ops: &[(usize, syntect::parsing::ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut char_pos = 0usize;
    for (text, op) in ScopeRegionIterator::new(ops, line) {
        // `ScopeRegionIterator` pairs each text region with the op that
        // takes it *into* scope — the op must be applied before reading the
        // stack for this region, not after (verified against syntect
        // 5.3.0's own behaviour: the `Push` yielded alongside a token's text
        // is the push that puts that very token inside the new scope).
        let _ = stack.apply(op);
        if !text.is_empty() && char_pos < visible_len {
            let scope = scope_for_stack(stack);
            let len = text.chars().count().min(visible_len - char_pos);
            if len > 0 {
                match spans.last_mut() {
                    Some(last) if last.scope == scope => last.len += len,
                    _ => spans.push(Span {
                        start: char_pos,
                        len,
                        scope,
                    }),
                }
            }
            char_pos += text.chars().count();
        }
    }
    spans
}

/// Highlight every line of `text`, choosing a language by `path`'s
/// extension; an unrecognised extension (or no extension) highlights every
/// line as entirely [`CodeScope::Plain`] rather than guessing. Files over
/// [`DIFF_MAX_LINES`] / [`DIFF_MAX_BYTES`] return an empty vector — no
/// highlighting — matching the diff viewer's existing oversize-guard stub
/// behaviour, since a file that large never reaches the point of rendering
/// content lines at all.
pub fn highlight_file(text: &str, path: &str) -> Vec<Vec<Span>> {
    if text.len() as u64 > DIFF_MAX_BYTES {
        return Vec::new();
    }
    let line_count = text.lines().count();
    if line_count > DIFF_MAX_LINES as usize {
        return Vec::new();
    }
    if text.is_empty() {
        return Vec::new();
    }

    let set = syntax_set();
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let syntax = set.find_syntax_by_extension(ext);

    let Some(syntax) = syntax else {
        // Unknown extension: every line entirely Plain, one span each,
        // still char-offset-correct.
        return text
            .lines()
            .map(|l| {
                let len = l.chars().count();
                if len == 0 {
                    Vec::new()
                } else {
                    vec![Span {
                        start: 0,
                        len,
                        scope: CodeScope::Plain,
                    }]
                }
            })
            .collect();
    };

    let mut parse_state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut result = Vec::with_capacity(line_count);
    for line in syntect::util::LinesWithEndings::from(text) {
        let visible_len = line.trim_end_matches(['\n', '\r']).chars().count();
        let Ok(ops) = parse_state.parse_line(line, set) else {
            result.push(Vec::new());
            continue;
        };
        result.push(spans_for_line(line, visible_len, &ops, &mut stack));
    }
    result
}

// ── projection onto diff lines ──────────────────────────────────────────

/// Attach the right line's spans to a diff [`crate::diff::Line`]: a `Del`
/// line (only exists on the old side) gets `old_spans`, an `Add` line (only
/// exists on the new side) gets `new_spans`, and a `Context` line (present
/// on both, same text) reads from `new_spans` — an arbitrary but consistent
/// choice, since the two sides highlight identical text.
///
/// Line numbers are 1-based in [`crate::diff::Line`]; `old_spans`/
/// `new_spans` are 0-indexed per-line vectors from [`highlight_file`]. Out of
/// range (a stale index, or highlighting was skipped by the oversize guard)
/// yields no spans rather than panicking.
pub fn line_spans(
    line: &crate::diff::Line,
    old_spans: &[Vec<Span>],
    new_spans: &[Vec<Span>],
) -> Vec<Span> {
    use crate::diff::LineKind;
    let (no, spans) = match line.kind {
        LineKind::Del => (line.old_no, old_spans),
        LineKind::Add | LineKind::Context => (line.new_no, new_spans),
    };
    let Some(no) = no else {
        return Vec::new();
    };
    let idx = (no as usize).saturating_sub(1);
    spans.get(idx).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::diff::{Line, LineKind};

    fn total_len(spans: &[Span]) -> usize {
        spans.iter().map(|s| s.len).sum()
    }

    fn has_scope(lines: &[Vec<Span>], scope: CodeScope) -> bool {
        lines
            .iter()
            .any(|spans| spans.iter().any(|s| s.scope == scope))
    }

    #[test]
    fn rust_sample_finds_keyword_string_number_comment() {
        let src = "// a comment\nfn main() {\n    let x: u32 = 42;\n    let s = \"hi\";\n}\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 5);
        assert!(has_scope(&lines, CodeScope::Comment));
        assert!(has_scope(&lines, CodeScope::Keyword));
        assert!(has_scope(&lines, CodeScope::Number));
        assert!(has_scope(&lines, CodeScope::StringLit));
    }

    #[test]
    fn javascript_sample_finds_keyword_and_string() {
        // syntect's bundled `load_defaults_newlines` set has no TypeScript
        // definition (only the Sublime "default" package, which stops at
        // JavaScript) — a `.ts` file falls back to all-Plain, correctly, per
        // `unknown_extension_is_all_plain` below. JavaScript is bundled, so
        // it stands in as the "web/TS-family" language sample.
        let src = "function greet(name) {\n  return `hi ${name}`;\n}\n";
        let lines = highlight_file(src, "sample.js");
        assert_eq!(lines.len(), 3);
        assert!(has_scope(&lines, CodeScope::Keyword));
    }

    #[test]
    fn json_sample_highlights() {
        let src = "{\n  \"a\": 1,\n  \"b\": \"two\"\n}\n";
        let lines = highlight_file(src, "sample.json");
        assert_eq!(lines.len(), 4);
        // Every non-empty line reconstructs to its own char length.
        for (line, spans) in src.lines().zip(&lines) {
            assert_eq!(total_len(spans), line.chars().count());
        }
    }

    #[test]
    fn markdown_sample_highlights() {
        let src = "# Title\n\nSome *text* and `code`.\n";
        let lines = highlight_file(src, "sample.md");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn unknown_extension_is_all_plain() {
        let src = "whatever this is\nsecond line\n";
        let lines = highlight_file(src, "sample.zzzz");
        assert_eq!(lines.len(), 2);
        for spans in &lines {
            assert!(spans.iter().all(|s| s.scope == CodeScope::Plain));
        }
    }

    #[test]
    fn no_extension_is_all_plain() {
        let lines = highlight_file("plain text\n", "Makefile.notreal.noext");
        for spans in &lines {
            assert!(spans.iter().all(|s| s.scope == CodeScope::Plain));
        }
    }

    #[test]
    fn multibyte_cjk_lines_land_on_char_boundaries_and_cover_the_line() {
        let src = "// 日本語のコメント\nlet 名前 = \"こんにちは\";\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 2);
        for (line, spans) in src.lines().zip(&lines) {
            assert_eq!(total_len(spans), line.chars().count());
            // Every span's start must itself be a valid char index (i.e.
            // within char count range) — proven simply by successfully
            // reconstructing the exact char length above, since a
            // byte-offset bug would over/under count against multibyte
            // text.
        }
    }

    /// Table-driven coverage of [`spans_for_line`]'s coalescing, exercised
    /// through the ops-free path (`unknown_extension`'s all-Plain branch and
    /// the syntect-parsed path below), asserting each line's spans (a) never
    /// have two adjacent same-scope spans and (b) cover the line exactly
    /// (no gaps, no overlaps, in order) — merging must never drop or
    /// duplicate a char.
    fn assert_spans_are_merged_and_exact(spans: &[Span], visible_len: usize) {
        let mut expected_start = 0usize;
        for pair in spans.windows(2) {
            assert_ne!(
                pair[0].scope, pair[1].scope,
                "adjacent spans share a scope: {pair:?}"
            );
        }
        for s in spans {
            assert_eq!(s.start, expected_start, "gap or overlap before {s:?}");
            expected_start += s.len;
        }
        assert_eq!(
            expected_start, visible_len,
            "spans do not cover the whole line exactly"
        );
    }

    #[test]
    fn merge_table_all_same_scope_line_yields_a_single_span() {
        // A comment *body* — deliberately excluding the `//` marker, which
        // Rust's sublime-syntax tags as its own
        // `punctuation.definition.comment` scope ahead of the comment text
        // itself — must coalesce to exactly one span, not one per syntect
        // token.
        let src = "this whole line is one comment scope end to end\n";
        let lines = highlight_file(&format!("// {src}"), "sample.rs");
        assert_eq!(lines.len(), 1);
        // `Punct` for the `//` marker, `Comment` for everything after —
        // exactly two merged spans, proving same-scope runs within each
        // collapse to one while genuinely different adjacent scopes do not.
        assert_eq!(
            lines[0].len(),
            2,
            "expected exactly one Punct span + one merged Comment span: {:?}",
            lines[0]
        );
        assert_eq!(lines[0][0].scope, CodeScope::Punct);
        assert_eq!(lines[0][1].scope, CodeScope::Comment);
        assert_spans_are_merged_and_exact(
            &lines[0],
            format!("// {src}").trim_end().chars().count(),
        );
    }

    #[test]
    fn merge_table_alternating_scopes_yields_one_span_per_change() {
        // keyword, plain space, type name, plain punctuation... — scope
        // changes several times, so several spans are expected, but no two
        // adjacent ones may share a scope.
        let src = "fn main() -> Vec<u32> { 42 }\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].len() > 1,
            "expected multiple spans: {:?}",
            lines[0]
        );
        assert_spans_are_merged_and_exact(
            &lines[0],
            "fn main() -> Vec<u32> { 42 }".chars().count(),
        );
    }

    #[test]
    fn merge_table_single_span_line() {
        let src = "x\n";
        let lines = highlight_file(src, "sample.zzzz");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_spans_are_merged_and_exact(&lines[0], 1);
    }

    #[test]
    fn merge_table_empty_line_yields_no_spans() {
        let src = "fn main() {\n\n}\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 3);
        assert!(lines[1].is_empty(), "blank line should have no spans");
    }

    #[test]
    fn merge_table_multibyte_cjk_offsets_are_exact_after_merging() {
        let src = "// 日本語のコメントです、全部同じスコープ\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 1);
        assert_spans_are_merged_and_exact(
            &lines[0],
            "// 日本語のコメントです、全部同じスコープ".chars().count(),
        );
    }

    #[test]
    fn merge_table_spans_cover_the_line_exactly_no_gaps_or_overlaps() {
        let src = "    let s = \"a string with punctuation: (a, b, c);\"; // trailing comment\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 1);
        assert_spans_are_merged_and_exact(
            &lines[0],
            "    let s = \"a string with punctuation: (a, b, c);\"; // trailing comment"
                .chars()
                .count(),
        );
    }

    #[test]
    fn adjacent_spans_never_share_a_scope() {
        // syntect splits a line like this into many regions inside one
        // `CodeScope` (e.g. several `punctuation.*`/`keyword.*` regions in a
        // row); `spans_for_line`'s coalescing must fold each run into a
        // single span, so no two neighbours can carry the same scope.
        let src = "fn main() {\n    let v: Vec<(u32, u32)> = vec![(1, 2), (3, 4)];\n}\n";
        let lines = highlight_file(src, "sample.rs");
        for spans in &lines {
            for pair in spans.windows(2) {
                assert_ne!(
                    pair[0].scope, pair[1].scope,
                    "adjacent spans share a scope: {pair:?}"
                );
            }
        }
    }

    #[test]
    fn empty_file_yields_no_lines() {
        assert!(highlight_file("", "sample.rs").is_empty());
    }

    #[test]
    fn crlf_line_endings_do_not_include_the_cr_in_spans() {
        let src = "let a = 1;\r\nlet b = 2;\r\n";
        let lines = highlight_file(src, "sample.rs");
        assert_eq!(lines.len(), 2);
        assert_eq!(total_len(&lines[0]), "let a = 1;".chars().count());
        assert_eq!(total_len(&lines[1]), "let b = 2;".chars().count());
    }

    #[test]
    fn oversize_guard_returns_empty_for_too_many_lines() {
        let src = "x\n".repeat(DIFF_MAX_LINES as usize + 1);
        assert!(highlight_file(&src, "sample.rs").is_empty());
    }

    #[test]
    fn oversize_guard_returns_empty_for_too_many_bytes() {
        let src = "a".repeat(DIFF_MAX_BYTES as usize + 1);
        assert!(highlight_file(&src, "sample.rs").is_empty());
    }

    // ── line_spans projection ────────────────────────────────────────────

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>) -> Line {
        Line {
            kind,
            text: String::new(),
            old_no,
            new_no,
        }
    }

    #[test]
    fn del_line_gets_old_side_spans() {
        let old_spans = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::Keyword,
        }]];
        let new_spans: Vec<Vec<Span>> = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::StringLit,
        }]];
        let l = line(LineKind::Del, Some(1), None);
        let spans = line_spans(&l, &old_spans, &new_spans);
        assert_eq!(spans[0].scope, CodeScope::Keyword);
    }

    #[test]
    fn add_line_gets_new_side_spans() {
        let old_spans: Vec<Vec<Span>> = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::Keyword,
        }]];
        let new_spans = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::StringLit,
        }]];
        let l = line(LineKind::Add, None, Some(1));
        let spans = line_spans(&l, &old_spans, &new_spans);
        assert_eq!(spans[0].scope, CodeScope::StringLit);
    }

    #[test]
    fn context_line_gets_new_side_spans() {
        let old_spans = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::Keyword,
        }]];
        let new_spans = vec![vec![Span {
            start: 0,
            len: 3,
            scope: CodeScope::Type,
        }]];
        let l = line(LineKind::Context, Some(1), Some(1));
        let spans = line_spans(&l, &old_spans, &new_spans);
        assert_eq!(spans[0].scope, CodeScope::Type);
    }

    #[test]
    fn out_of_range_line_number_yields_no_spans() {
        let l = line(LineKind::Add, None, Some(99));
        let spans = line_spans(&l, &[], &[]);
        assert!(spans.is_empty());
    }
}
