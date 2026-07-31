//! Fixture-corpus sanity. Lives in its own target so it stays green while the
//! golden harness is still red.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]

mod common;

#[test]
fn every_fixture_loads_and_is_nonempty() {
    let all = common::load_all();
    assert!(
        all.len() >= 8,
        "expected at least 8 fixtures, found {}: {:?}",
        all.len(),
        all.iter().map(|f| &f.label).collect::<Vec<_>>()
    );
    for f in &all {
        assert!(!f.bytes.is_empty(), "fixture `{}` is empty", f.label);
        assert!(
            f.rows > 0 && f.cols > 0,
            "fixture `{}` has no geometry",
            f.label
        );
        assert!(
            f.bytes.len() <= 2 * 1024 * 1024,
            "fixture `{}` exceeds the 2 MB cap ({} bytes)",
            f.label,
            f.bytes.len()
        );
    }
    for want in [
        "claude-tmux",
        "codex-tmux",
        "tmux-bare",
        "vim",
        "resize-storm",
        "resize-storm-primary",
        "sgr-torture",
        "activity-snippets",
    ] {
        assert!(
            all.iter().any(|f| f.label == want),
            "required fixture `{want}` is missing"
        );
    }
}

#[test]
fn the_oracle_does_not_panic_on_any_fixture() {
    // The oracle is the reference; a panic here invalidates every downstream
    // comparison, so it is checked separately from the model.
    for f in common::load_all() {
        let mut p = common::oracle::parser(&f.bytes, f.rows, f.cols);
        let d = common::oracle::dump(&p);
        assert_eq!(d.cells.len(), d.rows as usize * d.cols as usize);
        for n in [1usize, 5, 20, 60] {
            let _ = common::oracle::tail_contents(&mut p, n);
        }
        for (a, b) in common::selection_probes(f.rows) {
            let _ = common::oracle::selection_text(&mut p, a, b);
        }
        common::oracle::resize(&mut p, 20, 60);
        common::oracle::resize(&mut p, 40, 140);
        let _ = common::oracle::dump(&p);
    }
}
