//! The dual-parser golden harness: every fixture is fed to the vt100 oracle
//! *and* to `GroveTerm`, and the two must agree.
//!
//! These tests are written before the implementation and are expected to fail
//! red until `GroveTerm` exists. Do not edit them to make them pass — fix the
//! model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]

mod common;

use common::{
    apply_inverse, assert_expected, describe_cell_mismatch, load_all, normalize_cell_text, oracle,
    selection_probes, serialize_dump, CellDump, Probes, ScreenDump, CHUNK_SIZES, RESIZE_SCRIPT,
};
use grove_terminal::GroveTerm;

/// Build a `GroveTerm`, feed it the fixture in one blob.
fn grove(bytes: &[u8], rows: u16, cols: u16) -> GroveTerm {
    let mut t = GroveTerm::new(rows, cols);
    t.process(bytes);
    t
}

/// The model side of the comparison. Mirrors `oracle::dump` exactly, including
/// the shared blank-cell normalization and the shared INVERSE swap.
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
fn golden_cells_match() {
    for f in load_all() {
        let want = oracle::dump_bytes(&f.bytes, f.rows, f.cols);
        let got = dump(&grove(&f.bytes, f.rows, f.cols));
        assert_eq!(
            (want.rows, want.cols),
            (got.rows, got.cols),
            "fixture `{}`: geometry differs",
            f.label
        );
        if want.cells != got.cells {
            panic!("{}", describe_cell_mismatch(&f.label, &want, &got));
        }
    }
}

// 2 --------------------------------------------------------------------------

#[test]
fn golden_cursor_matches() {
    for f in load_all() {
        let want = oracle::dump_bytes(&f.bytes, f.rows, f.cols);
        let got = dump(&grove(&f.bytes, f.rows, f.cols));
        assert_eq!(
            (want.cursor, want.cursor_hidden),
            (got.cursor, got.cursor_hidden),
            "fixture `{}`: cursor differs",
            f.label
        );
        assert_eq!(
            want.app_cursor, got.app_cursor,
            "fixture `{}`: application-cursor mode differs",
            f.label
        );
        assert_eq!(
            want.display_offset, got.display_offset,
            "fixture `{}`: display offset differs",
            f.label
        );
    }
}

// 3 --------------------------------------------------------------------------

#[test]
fn golden_title_matches() {
    for f in load_all() {
        let want = oracle::dump_bytes(&f.bytes, f.rows, f.cols);
        let got = dump(&grove(&f.bytes, f.rows, f.cols));
        assert_eq!(
            want.title, got.title,
            "fixture `{}`: title differs",
            f.label
        );
    }
}

// 4 --------------------------------------------------------------------------

#[test]
fn golden_bell_count_matches() {
    for f in load_all() {
        let want = oracle::dump_bytes(&f.bytes, f.rows, f.cols);
        let got = dump(&grove(&f.bytes, f.rows, f.cols));
        assert_eq!(
            want.bell_count, got.bell_count,
            "fixture `{}`: bell count differs",
            f.label
        );
    }
}

// 5 --------------------------------------------------------------------------

#[test]
fn golden_tail_contents_matches() {
    for f in load_all() {
        let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
        let mut t = grove(&f.bytes, f.rows, f.cols);
        for n in [1usize, 5, 20, 60] {
            let want = oracle::tail_contents(&mut p, n);
            let got = t.tail_contents(n);
            assert_eq!(
                want, got,
                "fixture `{}`: tail_contents({n}) differs\n--- oracle ---\n{want}\n--- grove ---\n{got}",
                f.label
            );
        }
    }
}

// 6 --------------------------------------------------------------------------

#[test]
fn golden_after_resize_matches() {
    // Alt-screen fixtures only: the primary screen reflows on resize in
    // alacritty and not in vt100, an unfixable divergence documented and
    // asserted by `primary_screen_reflow_is_a_known_divergence`.
    for f in load_all().into_iter().filter(|f| f.alt_screen) {
        let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
        let mut t = grove(&f.bytes, f.rows, f.cols);
        for &(rows, cols) in RESIZE_SCRIPT {
            oracle::resize(&mut p, rows, cols);
            t.resize(rows, cols);
            let want = oracle::dump(&p);
            let got = dump(&t);
            assert_eq!(
                (want.rows, want.cols),
                (got.rows, got.cols),
                "fixture `{}`: geometry differs after resize to {rows}x{cols}",
                f.label
            );
            if want.cells != got.cells {
                panic!(
                    "after resize to {rows}x{cols}: {}",
                    describe_cell_mismatch(&f.label, &want, &got)
                );
            }
        }
    }
}

// 7 --------------------------------------------------------------------------

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

// 8 --------------------------------------------------------------------------

#[test]
fn golden_selection_text_matches() {
    for f in load_all() {
        let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
        let mut t = grove(&f.bytes, f.rows, f.cols);
        for (a, b) in selection_probes(f.rows) {
            let want = oracle::selection_text(&mut p, a, b);
            let got = t.selection_text(a, b);
            assert_eq!(
                want, got,
                "fixture `{}`: selection_text({a:?}, {b:?}) differs",
                f.label
            );
        }
    }
}

// 9 — the freeze (Plan 10 Task 4) -------------------------------------------

/// **Three-way agreement: model vs oracle vs frozen file.**
///
/// This is the test that makes the freeze trustworthy. While vt100 is still in
/// the tree every case is asserted on all three legs at once, so a frozen file
/// cannot be wrong without this failing. Plan 10 Task 7 Step 2 deletes only the
/// **oracle** leg, leaving model-vs-file — a real regression test, because the
/// file was blessed from an independent parser rather than from the model
/// itself. See `tests/common/mod.rs`'s module doc for the alternatives that
/// were rejected.
///
/// Re-bless with, and only with, the oracle still present:
/// ```text
/// GROVE_TERM_BLESS=1 cargo test -p grove-terminal --test golden
/// ```
#[test]
fn golden_dumps_match_the_frozen_files() {
    for f in load_all() {
        // ── base: the whole stream in one blob, plus the probe outputs ──
        let want = oracle::dump_bytes(&f.bytes, f.rows, f.cols);
        let mut t = grove(&f.bytes, f.rows, f.cols);
        let got = dump(&t);
        assert_eq!(want.cells, got.cells, "fixture `{}`: oracle leg", f.label);

        let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
        let mut probes = Probes::default();
        for n in [1usize, 5, 20, 60] {
            let o = oracle::tail_contents(&mut p, n);
            let g = t.tail_contents(n);
            assert_eq!(o, g, "fixture `{}`: tail_contents({n}) oracle leg", f.label);
            probes.tails.push((n, g));
        }
        for (a, b) in selection_probes(f.rows) {
            let o = oracle::selection_text(&mut p, a, b);
            let g = t.selection_text(a, b);
            assert_eq!(
                o, g,
                "fixture `{}`: selection_text({a:?},{b:?}) oracle leg",
                f.label
            );
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
            assert_eq!(
                want.cells, cgot.cells,
                "fixture `{}`: oracle leg at chunk size {size}",
                f.label
            );
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
        let mut rp = oracle::parser(&f.bytes, f.rows, f.cols);
        t = grove(&f.bytes, f.rows, f.cols);
        for &(rows, cols) in RESIZE_SCRIPT {
            oracle::resize(&mut rp, rows, cols);
            t.resize(rows, cols);
            let rwant = oracle::dump(&rp);
            let rgot = dump(&t);
            if rwant.cells != rgot.cells {
                panic!(
                    "after resize to {rows}x{cols}: {}",
                    describe_cell_mismatch(&f.label, &rwant, &rgot)
                );
            }
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

// 10 — the drift guard -------------------------------------------------------

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
