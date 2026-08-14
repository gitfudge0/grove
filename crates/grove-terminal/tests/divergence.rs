//! Asserted, documented divergences between `alacritty_terminal` and `vt100`; each is a deliverable, not a bug to fix, pinned down so a behavior change fails loudly.
//! The vt100 oracle was deleted; what vt100 did survives as doc-comment prose and blessed dumps, and only the `GroveTerm` side remains executable.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]

mod common;

use common::{apply_inverse, normalize_cell_text, CellDump, ScreenDump};
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

/// Known divergence: `Term::resize` hardcodes primary-screen rewrap on the alternate-screen-never path (`alacritty_terminal/src/term/mod.rs:677`); vt100 never rewraps either. Benign since Grove runs agents on the alternate screen.
#[test]
fn primary_screen_reflow_is_a_known_divergence() {
    let f = common::fixture("resize-storm-primary");
    assert!(
        !f.alt_screen,
        "this fixture must be a primary-screen capture"
    );

    let mut t = grove(&f.bytes, f.rows, f.cols);
    t.resize(34, 40);
    let got = dump(&t);

    // vt100 truncated at the new width; alacritty rewraps, so a continuation row is exactly what "reflowed" means.
    let al_rows = common::render_rows(&got);
    assert!(
        al_rows
            .iter()
            .any(|r| r.trim_start().starts_with("mps over the lazy dog")),
        "alacritty did not rewrap the long lines — if it gained a no-reflow \
         knob, adopt it and delete this test: {al_rows:?}"
    );

    // The one dump case blessed from `GroveTerm` rather than the oracle, since here the oracle is by definition wrong.
    common::assert_expected(
        common::DIVERGENCE_REFLOW_CASE,
        &common::serialize_dump(&got, &common::Probes::default()),
        &[
            "BLESSED FROM GroveTerm (alacritty), NOT from the vt100 oracle.",
            "",
            "Master plan row 02: `Term::resize` hardcodes",
            "`self.grid.resize(!is_alt, ..)`",
            "(alacritty_terminal/src/term/mod.rs:677), so the primary screen",
            "ALWAYS rewraps and the alternate screen NEVER does. vt100 rewraps",
            "neither. Plan 02 pinned the difference down rather than patching",
            "alacritty; spec §3's \"reflow-on-resize is suppressed\" is amended",
            "by `primary_screen_reflow_is_a_known_divergence`, not implemented.",
            "",
            "This file therefore records what alacritty does, on purpose. A",
            "diff here means alacritty's reflow changed.",
        ],
    );
}

/// Known divergence: `\x1b[2J` scrolls the erased screen into alacritty's history; vt100 drops it — so scrollback selections can't be compared on a clears-built fixture.
#[test]
fn ed2_scrollback_retention_is_a_known_divergence() {
    let f = common::fixture("activity-snippets");
    let t = grove(&f.bytes, f.rows, f.cols);

    assert!(
        t.history_size() > 0,
        "alacritty stopped retaining ED-2 cleared screens; this divergence is \
         gone and the shared selection probes can cover scrollback again"
    );
}

/// Uses the one fixture whose history both parsers agreed on (no clears, so the `ED 2` divergence above never bites); expected texts were frozen from the oracle before it was deleted.
#[test]
fn selection_into_scrollback_matches_where_history_agrees() {
    let f = common::fixture("resize-storm-primary");
    let mut t = grove(&f.bytes, f.rows, f.cols);
    assert!(
        t.history_size() > 40,
        "fixture has too little scrollback to probe"
    );

    let mut probes: Vec<common::SelectionProbe> = Vec::new();
    for (a, b) in common::scrollback_selection_probes(f.rows) {
        probes.push(((a, b), t.selection_text(a, b)));
    }
    common::assert_expected(
        common::SCROLLBACK_SELECTION_CASE,
        &common::serialize_selection_probes(&probes),
        &[
            "Blessed from the vt100 oracle (Plan 10 Task 7 Step 2) while it was",
            "still in the tree. Do NOT re-bless from GroveTerm.",
        ],
    );
}
