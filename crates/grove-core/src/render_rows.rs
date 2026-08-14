//! Pre-baked render rows for the diff viewer, so the view's render path is a pure lookup instead of deriving rows every frame.

use crate::diff::{Line, LineKind, PairedRow, Patch, Run, UnifiedRow};
use crate::highlight::{line_spans, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnifiedRenderRow {
    HunkHeader(String),
    Line { line: Line, spans: Vec<Span> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitCell {
    pub line: Line,
    pub spans: Vec<Span>,
    /// `Some` only for a real Del/Add pair.
    pub runs: Option<Vec<Run>>,
}

/// A half-empty row leaves the shorter side `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitRenderRow {
    HunkHeader(String),
    Lines {
        old: Option<SplitCell>,
        new: Option<SplitCell>,
    },
}

/// The `"@@ "` prefix the view prints before a hunk header's text.
const HUNK_HEADER_PREFIX_CHARS: usize = 3;

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

/// Intraline word runs are computed only for a real Del/Add pair, never a context line or half-empty row.
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

/// The wider side governs, since both halves pan off one shared offset — otherwise the wider side could never reach its own right edge. NaN-safe: non-finite input yields `0`.
pub fn split_pan_extent(left_content_w: f32, right_content_w: f32, half_w: f32) -> f32 {
    let widest = left_content_w.max(right_content_w);
    let extent = widest - half_w;
    if extent.is_finite() && extent > 0.0 {
        extent
    } else {
        0.0
    }
}

/// Called on every read rather than write, so a pan carried across a file switch collapses to what the new layout can show.
pub fn clamp_split_pan(pan_x: f32, left_content_w: f32, right_content_w: f32, half_w: f32) -> f32 {
    let extent = split_pan_extent(left_content_w, right_content_w, half_w);
    if pan_x.is_finite() {
        pan_x.clamp(0.0, extent)
    } else {
        0.0
    }
}

/// `""` if `rows` has no row on that side. Measuring the real painted width needs a `Window`, so it happens in the view (see `views::modals::diff_viewer::split_content_w`), not here.
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

fn unified_row_width(row: &UnifiedRenderRow) -> usize {
    match row {
        UnifiedRenderRow::HunkHeader(h) => h.chars().count() + HUNK_HEADER_PREFIX_CHARS,
        UnifiedRenderRow::Line { line, .. } => line.text.chars().count(),
    }
}

/// Feeds gpui's `uniform_list::with_width_from_item`, which measures exactly one item, so the truly widest row matters.
pub fn widest_unified_row(rows: &[UnifiedRenderRow]) -> usize {
    widest_by(rows, unified_row_width)
}

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
        // Measured in bytes the CJK line would win; in chars, the ASCII line does.
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

    #[test]
    fn pan_extent_is_governed_by_the_wider_side() {
        assert_eq!(split_pan_extent(900.0, 500.0, 400.0), 500.0);
        assert_eq!(split_pan_extent(500.0, 900.0, 400.0), 500.0);
    }

    #[test]
    fn pan_extent_is_zero_when_both_sides_fit() {
        assert_eq!(split_pan_extent(300.0, 200.0, 400.0), 0.0);
        assert_eq!(split_pan_extent(400.0, 400.0, 400.0), 0.0);
    }

    #[test]
    fn pan_extent_never_negative_on_a_degenerate_layout() {
        assert_eq!(split_pan_extent(0.0, 0.0, 0.0), 0.0);
        assert_eq!(split_pan_extent(100.0, 100.0, f32::NAN), 0.0);
        assert_eq!(split_pan_extent(f32::NAN, 0.0, 400.0), 0.0);
    }

    #[test]
    fn clamp_pins_the_pan_inside_the_extent() {
        assert_eq!(clamp_split_pan(-50.0, 900.0, 500.0, 400.0), 0.0);
        assert_eq!(clamp_split_pan(120.0, 900.0, 500.0, 400.0), 120.0);
        assert_eq!(clamp_split_pan(9_000.0, 900.0, 500.0, 400.0), 500.0);
        assert_eq!(clamp_split_pan(f32::NAN, 900.0, 500.0, 400.0), 0.0);
    }

    #[test]
    fn a_stored_pan_collapses_once_content_no_longer_overflows() {
        let stored = 480.0;
        assert_eq!(clamp_split_pan(stored, 900.0, 500.0, 400.0), 480.0);
        assert_eq!(clamp_split_pan(stored, 300.0, 200.0, 400.0), 0.0);
    }
}
