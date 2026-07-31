//! Shared test scaffolding: the fixture corpus loader and the neutral
//! `ScreenDump` comparison value both parsers must produce.
//!
//! Every item here is used by at least one integration test target, but not by
//! all of them, so `dead_code` is allowed module-wide.
#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::format_push_string,
    clippy::manual_assert
)]

use std::path::{Path, PathBuf};

pub mod oracle;

/// A recorded PTY byte stream plus the geometry it was recorded at.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub label: String,
    pub bytes: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    /// True when the stream is expected to spend its life on the alternate
    /// screen (tmux/vim). Primary-screen fixtures reflow on resize; see the
    /// known-divergence test.
    pub alt_screen: bool,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Load every `*.bin` in `tests/fixtures` together with its `*.meta.json`
/// sidecar. Fixtures are raw bytes — never text — so escape sequences survive
/// round-tripping.
pub fn load_all() -> Vec<Fixture> {
    let dir = fixtures_dir();
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = fs_err::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    entries.sort();

    for bin in entries {
        let meta_path = bin.with_extension("meta.json");
        let bytes = fs_err::read(&bin).unwrap_or_else(|e| panic!("read {}: {e}", bin.display()));
        let meta_raw = fs_err::read_to_string(&meta_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", meta_path.display()));
        let meta: serde_json::Value = serde_json::from_str(&meta_raw)
            .unwrap_or_else(|e| panic!("parse {}: {e}", meta_path.display()));
        let label = meta["label"]
            .as_str()
            .unwrap_or_else(|| panic!("{} has no label", meta_path.display()))
            .to_string();
        let rows = u16::try_from(meta["rows"].as_u64().unwrap_or(0)).unwrap_or(0);
        let cols = u16::try_from(meta["cols"].as_u64().unwrap_or(0)).unwrap_or(0);
        let alt_screen = meta["alt_screen"].as_bool().unwrap_or(false);
        out.push(Fixture {
            label,
            bytes,
            rows,
            cols,
            alt_screen,
        });
    }
    out
}

pub fn fixture(label: &str) -> Fixture {
    load_all()
        .into_iter()
        .find(|f| f.label == label)
        .unwrap_or_else(|| panic!("no fixture labelled {label}"))
}

/// Shared selection probe rectangles, in scrollback-absolute coordinates
/// (larger row = older), covering: same-row, multi-row, reversed endpoints, a
/// full-screen span, and a mid-screen multi-row span.
///
/// Deliberately confined to the visible screen. Selections that reach into
/// scrollback are covered separately, on a fixture whose history the two
/// parsers agree on — see `ed2_scrollback_retention_is_a_known_divergence` in
/// `tests/divergence.rs` for why a shared deep-scrollback probe cannot exist.
pub fn selection_probes(rows: u16) -> Vec<((usize, usize), (usize, usize))> {
    let r = rows as usize;
    vec![
        ((0, 0), (0, 10)),
        ((3, 2), (0, 40)),
        ((0, 40), (3, 2)),
        ((r.saturating_sub(1), 0), (0, 5)),
        ((r / 2, 0), (r / 4, 30)),
    ]
}

/// The resize script `golden_after_resize_matches` replays.
pub const RESIZE_SCRIPT: &[(u16, u16)] = &[(20, 60), (40, 140), (34, 120), (10, 200)];

/// Chunk boundaries `golden_chunking_invariance` feeds a fixture at.
pub const CHUNK_SIZES: &[usize] = &[1, 7, 64, 4096];

// ---------------------------------------------------------------------------
// The neutral comparison value
// ---------------------------------------------------------------------------

use grove_terminal::TermColor;

#[derive(Debug, PartialEq, Eq)]
pub struct CellDump {
    pub text: String,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ScreenDump {
    pub rows: u16,
    pub cols: u16,
    /// Row-major, `rows * cols` entries.
    pub cells: Vec<CellDump>,
    pub cursor: (u16, u16),
    pub cursor_hidden: bool,
    pub title: Option<String>,
    pub bell_count: usize,
    pub display_offset: usize,
    pub app_cursor: bool,
}

/// vt100 reports a blank cell as `""` while alacritty reports `' '`. Neither is
/// wrong; they are the same visual cell. Normalize once, here, so both dumps
/// speak the same language and the difference can never masquerade as a real
/// mismatch.
pub fn normalize_cell_text(s: &str) -> String {
    if s.is_empty() {
        " ".to_string()
    } else {
        s.to_string()
    }
}

/// INVERSE is recorded by swapping fg/bg — applied identically by both sides
/// via this one helper, so the semantics cannot drift apart.
pub fn apply_inverse(fg: TermColor, bg: TermColor, inverse: bool) -> (TermColor, TermColor) {
    if inverse {
        (bg, fg)
    } else {
        (fg, bg)
    }
}

/// Render a dump's cells as plain text rows, for readable mismatch reports.
pub fn render_rows(d: &ScreenDump) -> Vec<String> {
    (0..d.rows as usize)
        .map(|r| {
            (0..d.cols as usize)
                .map(|c| d.cells[r * d.cols as usize + c].text.as_str())
                .collect::<String>()
        })
        .collect()
}

/// First differing cell index between two dumps of the same geometry.
pub fn first_cell_diff(a: &ScreenDump, b: &ScreenDump) -> Option<usize> {
    a.cells.iter().zip(b.cells.iter()).position(|(x, y)| x != y)
}

/// A readable report of the first cell-level difference: coordinates, both
/// cells, and a ±3-row text rendering of each side.
pub fn describe_cell_mismatch(label: &str, a: &ScreenDump, b: &ScreenDump) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "fixture `{label}`: screen dumps differ");
    if a.rows != b.rows || a.cols != b.cols {
        let _ = writeln!(
            s,
            "  geometry: oracle {}x{} vs grove {}x{}",
            a.rows, a.cols, b.rows, b.cols
        );
        return s;
    }
    let Some(idx) = first_cell_diff(a, b) else {
        let _ = writeln!(s, "  cells equal but lengths differ?");
        return s;
    };
    let (row, col) = (idx / a.cols as usize, idx % a.cols as usize);
    let _ = writeln!(s, "  first mismatch at (row {row}, col {col})");
    let _ = writeln!(s, "    oracle: {:?}", a.cells[idx]);
    let _ = writeln!(s, "    grove : {:?}", b.cells[idx]);
    let ar = render_rows(a);
    let br = render_rows(b);
    let lo = row.saturating_sub(3);
    let hi = (row + 4).min(ar.len());
    let _ = writeln!(s, "  oracle rows {lo}..{hi}:");
    for (i, line) in ar[lo..hi].iter().enumerate() {
        let _ = writeln!(s, "    {:>4} |{}|", lo + i, line);
    }
    let _ = writeln!(s, "  grove rows {lo}..{hi}:");
    for (i, line) in br[lo..hi].iter().enumerate() {
        let _ = writeln!(s, "    {:>4} |{}|", lo + i, line);
    }
    s
}
