//! Pre-baked render rows for the diff viewer: the diff arithmetic of
//! [`crate::diff`] composed once with the syntax spans of
//! [`crate::highlight`] and the intraline word runs of
//! [`crate::diff::word_runs`], so the view's render path is a pure lookup.
//!
//! Like the rest of this crate, nothing here touches a UI framework or the
//! filesystem — a [`Patch`] plus its two span tables go in, a flat row
//! vector comes out. The view used to derive these every frame; building
//! them once per patch load is the whole point of this module.

use crate::diff::{Line, LineKind, PairedRow, Patch, Run, UnifiedRow};
use crate::highlight::{line_spans, Span};

/// One unified-mode row, ready to render: a hunk separator, or a line with
/// its syntax spans already attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnifiedRenderRow {
    HunkHeader(String),
    Line { line: Line, spans: Vec<Span> },
}

/// One side of a split-mode row: the line, its syntax spans, and its
/// intraline word runs when the row is a real Del/Add pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitCell {
    pub line: Line,
    pub spans: Vec<Span>,
    /// Intraline word-diff runs; `Some` only for a real Del/Add pair.
    pub runs: Option<Vec<Run>>,
}

/// One split-mode row: a hunk separator, or up to one [`SplitCell`] per
/// side (a half-empty row leaves the shorter side `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitRenderRow {
    HunkHeader(String),
    Lines {
        old: Option<SplitCell>,
        new: Option<SplitCell>,
    },
}

/// The `"@@ "` prefix the view prints before a hunk header's own text, so a
/// header's rendered width is its text plus these three chars.
const HUNK_HEADER_PREFIX_CHARS: usize = 3;

/// Bake every unified row of `patch` once, attaching
/// [`crate::highlight::line_spans`] to each line row. Order and count match
/// [`Patch::unified_rows`] exactly.
pub fn unified_render_rows(
    patch: &Patch,
    old_spans: &[Vec<Span>],
    new_spans: &[Vec<Span>],
) -> Vec<UnifiedRenderRow> {
    patch
        .unified_rows()
        .into_iter()
        .map(|row| match row {
            UnifiedRow::HunkHeader(h) => UnifiedRenderRow::HunkHeader(h),
            UnifiedRow::Line(line) => {
                let spans = line_spans(&line, old_spans, new_spans);
                UnifiedRenderRow::Line { line, spans }
            }
        })
        .collect()
}

/// Bake every split row of `patch` once. Order and count match
/// [`Patch::paired_rows`] exactly. Intraline word runs are computed only for
/// a real Del/Add pair — never for a context line (which pairs with itself)
/// nor for a half-empty row — matching what the view drew before these rows
/// were pre-baked.
pub fn split_render_rows(
    patch: &Patch,
    old_spans: &[Vec<Span>],
    new_spans: &[Vec<Span>],
) -> Vec<SplitRenderRow> {
    patch
        .paired_rows()
        .into_iter()
        .map(|row| match row {
            PairedRow::HunkHeader(h) => SplitRenderRow::HunkHeader(h),
            PairedRow::Lines { old, new } => {
                let runs = match (&old, &new) {
                    (Some(o), Some(n)) if o.kind == LineKind::Del && n.kind == LineKind::Add => {
                        Some(crate::diff::word_runs(&o.text, &n.text))
                    }
                    _ => None,
                };
                let (old_runs, new_runs) = match runs {
                    Some((o, n)) => (Some(o), Some(n)),
                    None => (None, None),
                };
                let cell = |line: Option<Line>, runs: Option<Vec<Run>>| {
                    line.map(|line| {
                        let spans = line_spans(&line, old_spans, new_spans);
                        SplitCell { line, spans, runs }
                    })
                };
                SplitRenderRow::Lines {
                    old: cell(old, old_runs),
                    new: cell(new, new_runs),
                }
            }
        })
        .collect()
}

/// The widest line of text on one side (`is_old` selects old/new) across
/// every [`SplitRenderRow::Lines`] row — `""` if `rows` has no such row on
/// that side. The split body renders as a `uniform_list` of paired rows with
/// a *shared* horizontal scroll (both columns move together), so the two
/// columns need a stable, definite width to stay aligned across every row
/// rather than each row sizing its own cells to its own content. The view
/// measures this text's real painted width (it needs a `Window`, so it can't
/// happen in this UI-framework-free crate) and uses that as the column's
/// fixed width — see `views::modals::diff_viewer::split_body`.
pub fn widest_split_side_text(rows: &[SplitRenderRow], is_old: bool) -> &str {
    rows.iter()
        .filter_map(|row| match row {
            SplitRenderRow::HunkHeader(_) => None,
            SplitRenderRow::Lines { old, new } => {
                let cell = if is_old { old } else { new };
                cell.as_ref().map(|c| c.line.text.as_str())
            }
        })
        .max_by_key(|text| text.chars().count())
        .unwrap_or("")
}

/// Rendered width of one row's text in chars — the measure both
/// `widest_*` helpers rank on.
fn unified_row_width(row: &UnifiedRenderRow) -> usize {
    match row {
        UnifiedRenderRow::HunkHeader(h) => h.chars().count() + HUNK_HEADER_PREFIX_CHARS,
        UnifiedRenderRow::Line { line, .. } => line.text.chars().count(),
    }
}

/// Index of the widest unified row, measured in `chars().count()` — `0` for
/// an empty slice. The caller feeds this to gpui's
/// `uniform_list::with_width_from_item`, which measures exactly one item to
/// decide the horizontal scroll extent, so the *truly* widest row matters.
pub fn widest_unified_row(rows: &[UnifiedRenderRow]) -> usize {
    widest_by(rows, unified_row_width)
}

/// Shared "index of the max, first one wins" scan for `widest_unified_row`;
/// `0` on an empty slice.
fn widest_by<T>(rows: &[T], width: impl Fn(&T) -> usize) -> usize {
    rows.iter()
        .enumerate()
        .max_by_key(|(ix, row)| (width(row), std::cmp::Reverse(*ix)))
        .map_or(0, |(ix, _)| ix)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::diff::Hunk;

    fn line(kind: LineKind, text: &str, old_no: Option<u32>, new_no: Option<u32>) -> Line {
        Line {
            kind,
            text: text.to_string(),
            old_no,
            new_no,
        }
    }

    fn text_patch(lines: Vec<Line>) -> Patch {
        Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: "fn main".to_string(),
                lines,
            }],
            no_newline_at_eof: false,
        }
    }

    #[test]
    fn unified_rows_preserve_order_and_attach_spans() {
        let patch = text_patch(vec![
            line(LineKind::Context, "ctx", Some(1), Some(1)),
            line(LineKind::Del, "old", Some(2), None),
            line(LineKind::Add, "new", None, Some(2)),
        ]);
        let old_spans = vec![vec![], vec![]];
        let new_spans = vec![vec![], vec![]];
        let baked = unified_render_rows(&patch, &old_spans, &new_spans);
        let raw = patch.unified_rows();
        assert_eq!(baked.len(), raw.len());
        for (baked, raw) in baked.iter().zip(&raw) {
            match (baked, raw) {
                (UnifiedRenderRow::HunkHeader(a), UnifiedRow::HunkHeader(b)) => assert_eq!(a, b),
                (UnifiedRenderRow::Line { line, spans }, UnifiedRow::Line(raw_line)) => {
                    assert_eq!(line, raw_line);
                    assert_eq!(*spans, line_spans(raw_line, &old_spans, &new_spans));
                }
                _ => panic!("row kinds diverged"),
            }
        }
    }

    #[test]
    fn split_rows_preserve_order_and_count() {
        let patch = text_patch(vec![
            line(LineKind::Context, "ctx", Some(1), Some(1)),
            line(LineKind::Del, "let a = 1;", Some(2), None),
            line(LineKind::Add, "let a = 2;", None, Some(2)),
        ]);
        let baked = split_render_rows(&patch, &[], &[]);
        let raw = patch.paired_rows();
        assert_eq!(baked.len(), raw.len());
        for (baked, raw) in baked.iter().zip(&raw) {
            match (baked, raw) {
                (SplitRenderRow::HunkHeader(a), PairedRow::HunkHeader(b)) => assert_eq!(a, b),
                (
                    SplitRenderRow::Lines { old, new },
                    PairedRow::Lines {
                        old: raw_old,
                        new: raw_new,
                    },
                ) => {
                    assert_eq!(old.as_ref().map(|c| &c.line), raw_old.as_ref());
                    assert_eq!(new.as_ref().map(|c| &c.line), raw_new.as_ref());
                }
                _ => panic!("row kinds diverged"),
            }
        }
    }

    /// The `runs` on both cells of row `ix`, for the pair/lone assertions.
    fn runs_at(rows: &[SplitRenderRow], ix: usize) -> (bool, bool) {
        let SplitRenderRow::Lines { old, new } = &rows[ix] else {
            panic!("row {ix} is not a Lines row");
        };
        (
            old.as_ref().is_some_and(|c| c.runs.is_some()),
            new.as_ref().is_some_and(|c| c.runs.is_some()),
        )
    }

    #[test]
    fn runs_are_some_only_for_a_del_add_pair() {
        let patch = text_patch(vec![
            line(LineKind::Del, "let a = 1;", Some(1), None),
            line(LineKind::Add, "let a = 2;", None, Some(1)),
        ]);
        let rows = split_render_rows(&patch, &[], &[]);
        assert_eq!(runs_at(&rows, 1), (true, true));
    }

    #[test]
    fn runs_are_none_for_context_and_lone_del_and_lone_add() {
        let ctx = text_patch(vec![line(LineKind::Context, "ctx", Some(1), Some(1))]);
        assert_eq!(
            runs_at(&split_render_rows(&ctx, &[], &[]), 1),
            (false, false)
        );

        let lone_del = text_patch(vec![line(LineKind::Del, "gone", Some(1), None)]);
        assert_eq!(
            runs_at(&split_render_rows(&lone_del, &[], &[]), 1),
            (false, false)
        );

        let lone_add = text_patch(vec![line(LineKind::Add, "fresh", None, Some(1))]);
        assert_eq!(
            runs_at(&split_render_rows(&lone_add, &[], &[]), 1),
            (false, false)
        );
    }

    #[test]
    fn widest_unified_picks_the_longest_line() {
        let patch = text_patch(vec![
            line(LineKind::Context, "short", Some(1), Some(1)),
            line(LineKind::Add, "a much longer line of code", None, Some(2)),
            line(LineKind::Context, "mid", Some(2), Some(3)),
        ]);
        let rows = unified_render_rows(&patch, &[], &[]);
        assert_eq!(widest_unified_row(&rows), 2);
    }

    #[test]
    fn widest_unified_can_be_the_hunk_header() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: "a very long function signature here".to_string(),
                lines: vec![line(LineKind::Context, "x", Some(1), Some(1))],
            }],
            no_newline_at_eof: false,
        };
        let rows = unified_render_rows(&patch, &[], &[]);
        assert_eq!(widest_unified_row(&rows), 0);
    }

    #[test]
    fn widest_is_zero_on_empty_slices() {
        assert_eq!(widest_unified_row(&[]), 0);
    }

    #[test]
    fn widest_is_measured_in_chars_not_bytes() {
        // "日本語のコメントです" is 10 chars / 30 bytes; the ASCII line is
        // 20 chars / 20 bytes. Measured in bytes the CJK line would win.
        let patch = text_patch(vec![
            line(LineKind::Context, "日本語のコメントです", Some(1), Some(1)),
            line(LineKind::Context, "abcdefghijklmnopqrst", Some(2), Some(2)),
        ]);
        let rows = unified_render_rows(&patch, &[], &[]);
        assert_eq!(widest_unified_row(&rows), 2);
    }

    #[test]
    fn widest_split_side_text_picks_the_longest_line_per_side() {
        let patch = text_patch(vec![
            line(LineKind::Del, "short old", Some(1), None),
            line(
                LineKind::Add,
                "a considerably wider new line",
                None,
                Some(1),
            ),
            line(LineKind::Del, "a much longer old line here", Some(2), None),
            line(LineKind::Add, "short new", None, Some(2)),
        ]);
        let rows = split_render_rows(&patch, &[], &[]);
        assert_eq!(
            widest_split_side_text(&rows, true),
            "a much longer old line here"
        );
        assert_eq!(
            widest_split_side_text(&rows, false),
            "a considerably wider new line"
        );
    }

    #[test]
    fn widest_split_side_text_empty_when_no_rows_on_that_side() {
        assert_eq!(widest_split_side_text(&[], true), "");
        assert_eq!(widest_split_side_text(&[], false), "");
    }

    #[test]
    fn binary_and_too_large_yield_no_rows() {
        for patch in [
            Patch::Binary,
            Patch::TooLarge {
                added: 9,
                removed: 9,
            },
        ] {
            assert!(unified_render_rows(&patch, &[], &[]).is_empty());
            assert!(split_render_rows(&patch, &[], &[]).is_empty());
        }
    }
}
