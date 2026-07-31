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
