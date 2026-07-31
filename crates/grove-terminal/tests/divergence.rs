//! Asserted, documented divergences between `alacritty_terminal` and `vt100`,
//! plus the scrollback-selection coverage the shared golden probes cannot carry.
//!
//! A divergence recorded here is a **deliverable**, not a bug to fix: each one
//! is a place where the two parsers cannot be made to agree without patching a
//! dependency, so it is pinned down by a test that fails loudly if the behavior
//! ever changes.
//!
//! Plan 10 Task 7 Step 2 deleted the vt100 oracle. What vt100 *did* survives as
//! prose in each test's doc comment plus the frozen dumps blessed from it; what
//! remains executable is the `GroveTerm` side, which is the side that can still
//! regress.
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
/// on the alternate screen, where the two parsers agreed (that is what the
/// frozen `__resize*` cases in `tests/golden.rs` cover). The spec sentence is
/// amended by this test rather than implemented.
#[test]
fn primary_screen_reflow_is_a_known_divergence() {
    let f = common::fixture("resize-storm-primary");
    assert!(
        !f.alt_screen,
        "this fixture must be a primary-screen capture"
    );

    let mut t = grove(&f.bytes, f.rows, f.cols);
    // Narrow enough that every recorded line must wrap.
    t.resize(34, 40);
    let got = dump(&t);

    // The specific shape: vt100 truncated each logical line at the new width, so
    // every non-blank row still started a fresh line. alacritty rewraps, so
    // continuation rows carrying the tail of the previous line appear. With the
    // oracle gone only the alacritty half is executable — but that is the half
    // that can regress, and a continuation row is exactly what "reflowed" means.
    let al_rows = common::render_rows(&got);
    assert!(
        al_rows
            .iter()
            .any(|r| r.trim_start().starts_with("mps over the lazy dog")),
        "alacritty did not rewrap the long lines — if it gained a no-reflow \
         knob, adopt it and delete this test: {al_rows:?}"
    );

    // Plan 10 Task 4: freeze the *reflowed* screen too. This is the one dump
    // case blessed from `GroveTerm` rather than from the oracle, because here
    // the oracle is by definition wrong — see the file's own header.
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
    let t = grove(&f.bytes, f.rows, f.cols);

    // vt100 held 0 rows of history here (measured while the oracle was in the
    // tree, Plan 10 Task 7 Step 2); alacritty holds a screen's worth.
    assert!(
        t.history_size() > 0,
        "alacritty stopped retaining ED-2 cleared screens; this divergence is \
         gone and the shared selection probes can cover scrollback again"
    );
}

/// Scrollback-crossing selections, on the one fixture whose history both
/// parsers agreed on (a bare shell streaming 500 lines — no clears, so the
/// `ED 2` divergence above never bites). The expected texts were frozen from
/// the oracle before it was deleted, so this remains a real regression test.
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
