//! Whole-file syntax highlighting: text in, `Vec<Span>` per line out, each span carrying a [`CodeScope`] rather than a colour.

use std::sync::OnceLock;

use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::diff::{DIFF_MAX_BYTES, DIFF_MAX_LINES};

/// The seven `CODE_*` theme targets, plus `Plain` for anything no rule below claims.
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

/// `start`/`len` are char offsets, not byte offsets, so slicing multibyte text by these bounds never lands mid-character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub len: usize,
    pub scope: CodeScope,
}

/// Parsed once and reused — `load_defaults_newlines` compiles a sizeable dump, too costly to pay per file.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Checked most-specific-first so a nested rule wins over its container.
fn map_scope(scope: Scope) -> Option<CodeScope> {
    let name = scope.build_string();
    if name.starts_with("keyword.operator") || name.starts_with("punctuation") {
        // Checked before the general `keyword.*` rule below since `keyword.operator` also starts with `keyword.`.
        return Some(CodeScope::Punct);
    }
    if name.starts_with("keyword") || name.starts_with("storage") {
        return Some(CodeScope::Keyword);
    }
    if name.starts_with("string") {
        return Some(CodeScope::StringLit);
    }
    if name.starts_with("constant.numeric") {
        return Some(CodeScope::Number);
    }
    if name.starts_with("constant") {
        return Some(CodeScope::Keyword);
    }
    if name.starts_with("comment") {
        return Some(CodeScope::Comment);
    }
    if name.starts_with("entity.name.type")
        || name.starts_with("entity.name.class")
        || name.starts_with("entity.other.inherited-class")
        || name.starts_with("support.type")
        || name.starts_with("support.class")
    {
        return Some(CodeScope::Type);
    }
    if name.starts_with("entity.name.function") || name.starts_with("support.function") {
        return Some(CodeScope::Func);
    }
    None
}

/// Innermost scope that [`map_scope`] recognises, falling back to [`CodeScope::Plain`].
fn scope_for_stack(stack: &ScopeStack) -> CodeScope {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|&s| map_scope(s))
        .unwrap_or(CodeScope::Plain)
}

/// Coalesces adjacent same-scope regions into one [`Span`] each; `visible_len` excludes the line's trailing `\n`/`\r\n`.
fn spans_for_line(
    line: &str,
    visible_len: usize,
    ops: &[(usize, syntect::parsing::ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut char_pos = 0usize;
    for (text, op) in ScopeRegionIterator::new(ops, line) {
        // The op must be applied before reading the stack for this region, not after.
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

/// An unrecognised extension highlights everything as Plain; oversize files return empty, matching the diff viewer's guard.
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

/// `Context` lines read from `new_spans` — arbitrary but consistent, since both sides highlight identical text. Out-of-range indices yield no spans rather than panicking.
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
        // No bundled TypeScript definition; JavaScript stands in as the web/TS-family sample.
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
        }
    }

    /// Asserts spans have no adjacent same-scope pair and cover the line exactly (no gaps/overlaps).
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
        let src = "this whole line is one comment scope end to end\n";
        let lines = highlight_file(&format!("// {src}"), "sample.rs");
        assert_eq!(lines.len(), 1);
        // `Punct` for the `//` marker, `Comment` for everything after.
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
