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
    apply_inverse, describe_cell_mismatch, load_all, normalize_cell_text, oracle, selection_probes,
    CellDump, ScreenDump, CHUNK_SIZES, RESIZE_SCRIPT,
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
