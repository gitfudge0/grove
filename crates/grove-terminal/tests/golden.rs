//! The golden harness: every fixture is fed to `GroveTerm` and the result must
//! match the dump frozen in `tests/expected/`.
//!
//! It was originally a *dual-parser* harness — every fixture went to the vt100
//! oracle as well, and the two had to agree. Plan 10 Task 7 Step 2 deleted the
//! oracle leg. What survives is model-vs-frozen-file, which is still a genuine
//! regression test because the files were blessed from an independent parser
//! (see `tests/common/mod.rs`'s module doc). The per-property comparisons that
//! used to live here (cells, cursor, title, bell count, tail contents, resize,
//! selection text) were oracle comparisons end to end; every one of those
//! properties is serialized into the frozen dumps, so they are still asserted —
//! by `golden_dumps_match_the_frozen_files`, once instead of twice.
//!
//! Do not edit these tests to make them pass — fix the model.
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

/// The model side of the comparison. Mirrors what the deleted `oracle::dump`
/// produced, including the shared blank-cell normalization and INVERSE swap —
/// which is why the frozen files stay comparable.
fn dump(t: &GroveTerm) -> ScreenDump {
    let snap = t.snapshot();
    let cells = snap
        .cells
        .iter()
        .map(|c| {
            let (fg, bg) = apply_inverse(c.fg, c.bg, c.inverse);
            CellDump {
                text: normalize_cell_text(&c.text),
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

// 1 --------------------------------------------------------------------------

#[test]
fn golden_chunking_invariance() {
    // A stateful parser bug at chunk edges would show up here and nowhere else:
    // the same bytes, split differently, must land on the same screen.
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

// 2 — the freeze (Plan 10 Task 4) -------------------------------------------

/// **Model vs frozen file.**
///
/// While vt100 was still in the tree every case was asserted on all three legs
/// at once, so a frozen file could not be wrong without this failing. Plan 10
/// Task 7 Step 2 deleted the **oracle** leg, leaving model-vs-file — a real
/// regression test, because the file was blessed from an independent parser
/// rather than from the model itself. See `tests/common/mod.rs`'s module doc for
/// the alternatives that were rejected.
///
/// **These files can no longer be legitimately re-blessed.** `GROVE_TERM_BLESS=1`
/// still works, but with the oracle gone it would overwrite the independent
/// expectation with whatever the model currently does — turning a parity test
/// into a change detector. A diff here is a regression to investigate, not a
/// file to regenerate.
#[test]
fn golden_dumps_match_the_frozen_files() {
    for f in load_all() {
        // ── base: the whole stream in one blob, plus the probe outputs ──
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

        // ── chunking: the same stream split at each boundary ──
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

        // ── resize: alt-screen fixtures only ──
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

// 3 — the drift guard -------------------------------------------------------

/// A frozen file with no case, or a case with no file, is a fixture that has
/// silently lost its assertion. Both directions fail here.
#[test]
fn every_frozen_file_has_a_case_and_every_case_has_a_file() {
    use std::collections::BTreeSet;

    // Under `GROVE_TERM_BLESS=1` the files are being written by a sibling test
    // in the same binary; the guard would race it. It is meaningful only
    // against a settled tree.
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
