//! Every fixture's `GroveTerm` output must match the frozen dump in `tests/expected/` (blessed from a since-deleted vt100 oracle). Do not edit these tests to pass — fix the model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]

mod common;

use common::{
    apply_inverse, assert_expected, describe_cell_mismatch, load_all, normalize_cell_text,
    selection_probes, serialize_dump, CellDump, Probes, ScreenDump, CHUNK_SIZES, RESIZE_SCRIPT,
};
use grove_terminal::GroveTerm;

/// Build a `GroveTerm`, feed it the fixture in one blob.
fn grove(bytes: &[u8], rows: u16, cols: u16) -> GroveTerm {
    let mut t = GroveTerm::new(rows, cols);
    t.process(bytes);
    t
}

/// Mirrors what the deleted `oracle::dump` produced (blank-cell normalization, INVERSE swap) so frozen files stay comparable.
fn dump(t: &GroveTerm) -> ScreenDump {
    let snap = t.snapshot();
    let cells = snap
        .cells
        .iter()
        .map(|c| {
            let (fg, bg) = apply_inverse(c.fg, c.bg, c.inverse);
            CellDump {
                text: normalize_cell_text(&c.c.to_string()),
                fg,
                bg,
                bold: c.bold,
            }
        })
        .collect();
    let (crow, ccol, hidden) = t.cursor();
    ScreenDump {
        rows: snap.rows,
        cols: snap.cols,
        cells,
        cursor: (crow, ccol),
        cursor_hidden: hidden,
        title: t.title(),
        bell_count: t.bell_count(),
        display_offset: t.display_offset(),
        app_cursor: t.app_cursor(),
    }
}

#[test]
fn golden_chunking_invariance() {
    // Catches stateful parser bugs at chunk edges: same bytes, split differently, must match.
    for f in load_all() {
        let whole = dump(&grove(&f.bytes, f.rows, f.cols));
        for &size in CHUNK_SIZES {
            let mut t = GroveTerm::new(f.rows, f.cols);
            for chunk in f.bytes.chunks(size) {
                t.process(chunk);
            }
            let got = dump(&t);
            if whole.cells != got.cells {
                panic!(
                    "chunk size {size}: {}",
                    describe_cell_mismatch(&f.label, &whole, &got)
                );
            }
            assert_eq!(
                (whole.cursor, whole.title.clone(), whole.bell_count),
                (got.cursor, got.title.clone(), got.bell_count),
                "fixture `{}`: non-cell state differs at chunk size {size}",
                f.label
            );
        }
    }
}

/// `GROVE_TERM_BLESS=1` still works but, with the oracle leg gone, would overwrite the independent expectation with the model's own output — a diff here is a regression, not a file to regenerate.
#[test]
fn golden_dumps_match_the_frozen_files() {
    for f in load_all() {
        let mut t = grove(&f.bytes, f.rows, f.cols);
        let got = dump(&t);

        let mut probes = Probes::default();
        for n in [1usize, 5, 20, 60] {
            probes.tails.push((n, t.tail_contents(n)));
        }
        for (a, b) in selection_probes(f.rows) {
            let g = t.selection_text(a, b);
            probes.selections.push(((a, b), g));
        }
        assert_expected(
            &format!("{}__base", f.label),
            &serialize_dump(&got, &probes),
            &[
                "Blessed from the vt100 oracle (Plan 10 Task 4) while it was",
                "still in the tree. Do NOT re-bless from GroveTerm.",
            ],
        );

        for &size in CHUNK_SIZES {
            let mut ct = GroveTerm::new(f.rows, f.cols);
            for chunk in f.bytes.chunks(size) {
                ct.process(chunk);
            }
            let cgot = dump(&ct);
            assert_expected(
                &format!("{}__chunk{size}", f.label),
                &serialize_dump(&cgot, &Probes::default()),
                &[
                    "Blessed from the vt100 oracle (Plan 10 Task 4).",
                    "Chunk-boundary invariance: identical to the __base screen.",
                ],
            );
        }

        if !f.alt_screen {
            continue;
        }
        t = grove(&f.bytes, f.rows, f.cols);
        for &(rows, cols) in RESIZE_SCRIPT {
            t.resize(rows, cols);
            let rgot = dump(&t);
            assert_expected(
                &format!("{}__resize{rows}x{cols}", f.label),
                &serialize_dump(&rgot, &Probes::default()),
                &[
                    "Blessed from the vt100 oracle (Plan 10 Task 4).",
                    "Cumulative walk of common::RESIZE_SCRIPT on an alt-screen",
                    "fixture, where both parsers agree that resize never reflows.",
                ],
            );
        }
    }
}

/// A frozen file with no case, or a case with no file, silently lost its assertion.
#[test]
fn every_frozen_file_has_a_case_and_every_case_has_a_file() {
    use std::collections::BTreeSet;

    // Meaningful only against a settled tree; blessing writes files concurrently.
    if common::blessing() {
        return;
    }
    let cases: BTreeSet<String> = common::expected_cases().into_iter().collect();
    let dir = common::expected_dir();
    let files: BTreeSet<String> = fs_err::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dump"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();

    let orphan_files: Vec<&String> = files.difference(&cases).collect();
    let missing_files: Vec<&String> = cases.difference(&files).collect();
    assert!(
        orphan_files.is_empty(),
        "expected/ holds {} file(s) no case produces: {orphan_files:?}",
        orphan_files.len()
    );
    assert!(
        missing_files.is_empty(),
        "{} case(s) have no frozen file: {missing_files:?}",
        missing_files.len()
    );
}
