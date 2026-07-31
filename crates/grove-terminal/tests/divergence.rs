//! Asserted, documented divergences between `alacritty_terminal` and `vt100`,
//! plus the scrollback-selection coverage the shared golden probes cannot carry.
//!
//! A divergence recorded here is a **deliverable**, not a bug to fix: each one
//! is a place where the two parsers cannot be made to agree without patching a
//! dependency, so it is pinned down by a test that fails loudly if the behavior
//! ever changes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]

mod common;

use common::{apply_inverse, normalize_cell_text, oracle, CellDump, ScreenDump};
use grove_terminal::GroveTerm;

fn grove(bytes: &[u8], rows: u16, cols: u16) -> GroveTerm {
    let mut t = GroveTerm::new(rows, cols);
    t.process(bytes);
    t
}

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

/// **Known divergence: the primary screen reflows on resize.**
///
/// The gpui spec §3 says "reflow-on-resize is suppressed". There is no config
/// knob for it: `Term::resize` hardcodes `self.grid.resize(!is_alt, ..)`
/// (`alacritty_terminal/src/term/mod.rs:677`), so the primary screen *always*
/// rewraps and the alternate screen *never* does. vt100 never rewraps either
/// screen. This plan does not patch alacritty; it pins the difference down.
///
/// Practically this is benign — Grove's terminals run agents inside tmux, i.e.
/// on the alternate screen, where the two parsers agree (that is what
/// `golden_after_resize_matches` covers). The spec sentence is amended by this
/// test rather than implemented.
#[test]
fn primary_screen_reflow_is_a_known_divergence() {
    let f = common::fixture("resize-storm-primary");
    assert!(
        !f.alt_screen,
        "this fixture must be a primary-screen capture"
    );

    let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
    let mut t = grove(&f.bytes, f.rows, f.cols);
    // Narrow enough that every recorded line must wrap.
    oracle::resize(&mut p, 34, 40);
    t.resize(34, 40);

    let want = oracle::dump(&p);
    let got = dump(&t);
    assert_ne!(
        want.cells, got.cells,
        "the primary-screen reflow divergence has disappeared — if alacritty \
         gained a no-reflow knob, adopt it and delete this test"
    );

    // The specific shape: vt100 truncates each logical line at the new width,
    // so every non-blank row still starts a fresh line. alacritty rewraps, so
    // continuation rows carrying the tail of the previous line appear.
    let vt_rows = common::render_rows(&want);
    let al_rows = common::render_rows(&got);
    assert!(
        vt_rows
            .iter()
            .filter(|r| !r.trim().is_empty())
            .all(|r| r.starts_with("primary line")),
        "vt100 unexpectedly produced a continuation row: {vt_rows:?}"
    );
    assert!(
        al_rows
            .iter()
            .any(|r| r.trim_start().starts_with("mps over the lazy dog")),
        "alacritty did not rewrap the long lines: {al_rows:?}"
    );
}

/// **Known divergence: alacritty keeps an `ED 2`-cleared screen in scrollback.**
///
/// `\x1b[2J` scrolls the erased screen into history in alacritty; vt100 drops
/// it. After the `activity-snippets` stream (nine screens, each preceded by a
/// clear) alacritty holds a screen's worth of scrollback and vt100 holds none.
///
/// Consequence: a selection whose rows live in scrollback cannot be compared
/// across the two parsers on a fixture built out of clears — which is why
/// `common::selection_probes` stays inside the visible screen and
/// `selection_into_scrollback_matches_where_history_agrees` covers the
/// scrollback path on a fixture whose history the parsers do agree on.
#[test]
fn ed2_scrollback_retention_is_a_known_divergence() {
    let f = common::fixture("activity-snippets");
    let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
    p.set_scrollback(usize::MAX);
    let vt_history = p.screen().scrollback();
    let t = grove(&f.bytes, f.rows, f.cols);

    assert_eq!(
        vt_history, 0,
        "vt100 started retaining ED-2 cleared screens; re-evaluate the shared \
         selection probes"
    );
    assert!(
        t.history_size() > 0,
        "alacritty stopped retaining ED-2 cleared screens; this divergence is \
         gone and the shared selection probes can cover scrollback again"
    );
}

/// Scrollback-crossing selections, on the one fixture whose history both
/// parsers agree on (a bare shell streaming 500 lines — no clears, so the
/// `ED 2` divergence above never bites).
#[test]
fn selection_into_scrollback_matches_where_history_agrees() {
    let f = common::fixture("resize-storm-primary");
    let mut p = oracle::parser(&f.bytes, f.rows, f.cols);
    let mut t = grove(&f.bytes, f.rows, f.cols);
    p.set_scrollback(usize::MAX);
    let vt_history = p.screen().scrollback();
    p.set_scrollback(0);
    assert_eq!(
        vt_history,
        t.history_size(),
        "the parsers no longer agree on this fixture's history size"
    );
    assert!(
        vt_history > 40,
        "fixture has too little scrollback to probe"
    );

    let r = f.rows as usize;
    for (a, b) in [
        ((r + 1, 0), (r - 1, 20)),
        ((r + 5, 0), (r + 1, 30)),
        ((r + 12, 4), (r + 9, 60)),
        ((r + 3, 30), (r + 7, 2)),
    ] {
        let want = oracle::selection_text(&mut p, a, b);
        let got = t.selection_text(a, b);
        assert_eq!(want, got, "selection_text({a:?}, {b:?}) differs");
    }
}
