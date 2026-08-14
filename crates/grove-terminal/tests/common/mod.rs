//! Shared test scaffolding: the fixture corpus loader, the neutral `ScreenDump` comparison value, and the frozen expected-dump serializer.
//! Dumps are frozen from the vt100 oracle before its Plan 10 Phase C deletion, so parity stays a real regression test rather than a self-snapshot re-blessed from `GroveTerm`.
//! vt100 0.15.2 panics on deep scrollback (`grid.rs:125`'s `visible_rows` underflows without a guard); the oracle clamped the offset before every read — preserved here as the only record, since the oracle itself is deleted.
//! Every item here is used by at least one integration test target, but not all, so `dead_code` is allowed module-wide.
#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::format_push_string,
    clippy::manual_assert
)]

use std::path::{Path, PathBuf};

/// A recorded PTY byte stream plus the geometry it was recorded at.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub label: String,
    pub bytes: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    /// Primary-screen fixtures reflow on resize instead; see the known-divergence test.
    pub alt_screen: bool,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Fixtures are raw bytes, never text, so escape sequences survive round-tripping.
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

/// Confined to the visible screen; scrollback-crossing spans are [`scrollback_selection_probes`] instead — see `ed2_scrollback_retention_is_a_known_divergence` in `tests/divergence.rs`.
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

pub const RESIZE_SCRIPT: &[(u16, u16)] = &[(20, 60), (40, 140), (34, 120), (10, 200)];

pub const CHUNK_SIZES: &[usize] = &[1, 7, 64, 4096];

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

/// vt100 reports a blank cell as `""`, alacritty as `' '` — the same visual cell; normalize once so the difference can't masquerade as a mismatch.
pub fn normalize_cell_text(s: &str) -> String {
    if s.is_empty() {
        " ".to_string()
    } else {
        s.to_string()
    }
}

/// Applied identically by both sides via this one helper, so the semantics cannot drift apart.
pub fn apply_inverse(fg: TermColor, bg: TermColor, inverse: bool) -> (TermColor, TermColor) {
    if inverse {
        (bg, fg)
    } else {
        (fg, bg)
    }
}

pub fn render_rows(d: &ScreenDump) -> Vec<String> {
    (0..d.rows as usize)
        .map(|r| {
            (0..d.cols as usize)
                .map(|c| d.cells[r * d.cols as usize + c].text.as_str())
                .collect::<String>()
        })
        .collect()
}

pub fn first_cell_diff(a: &ScreenDump, b: &ScreenDump) -> Option<usize> {
    a.cells.iter().zip(b.cells.iter()).position(|(x, y)| x != y)
}

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

/// `(endpoints, selection text)`; `None` when the span yields nothing.
pub type SelectionProbe = (((usize, usize), (usize, usize)), Option<String>);

/// Empty for the chunk/resize cases, which only re-assert the screen.
#[derive(Debug, Default)]
pub struct Probes {
    pub tails: Vec<(usize, String)>,
    pub selections: Vec<SelectionProbe>,
}

/// Both the freeze/compare harness and the drift guard read this, so a case can never exist without a file or vice versa. See `__base`/`__chunk<size>`/`__resize<rows>x<cols>`/`__DIVERGENCE` naming below.
pub fn expected_cases() -> Vec<String> {
    let mut out = Vec::new();
    for f in load_all() {
        out.push(format!("{}__base", f.label));
        for size in CHUNK_SIZES {
            out.push(format!("{}__chunk{size}", f.label));
        }
        if f.alt_screen {
            for (rows, cols) in RESIZE_SCRIPT {
                out.push(format!("{}__resize{rows}x{cols}", f.label));
            }
        }
    }
    out.push(DIVERGENCE_REFLOW_CASE.to_string());
    out.push(SCROLLBACK_SELECTION_CASE.to_string());
    out.sort();
    out
}

/// The one asserted divergence with a screen dump, blessed from `GroveTerm` because the oracle is by definition wrong here.
pub const DIVERGENCE_REFLOW_CASE: &str = "resize-storm-primary__reflow34x40__DIVERGENCE";

/// Frozen from the oracle so `selection_into_scrollback_matches_where_history_agrees` survives the oracle's deletion as a real regression test.
pub const SCROLLBACK_SELECTION_CASE: &str = "resize-storm-primary__scrollback_selection";

/// Spans that start in scrollback and end on the visible screen, including one with reversed endpoints.
pub fn scrollback_selection_probes(rows: u16) -> Vec<((usize, usize), (usize, usize))> {
    let r = rows as usize;
    vec![
        ((r + 1, 0), (r - 1, 20)),
        ((r + 5, 0), (r + 1, 30)),
        ((r + 12, 4), (r + 9, 60)),
        ((r + 3, 30), (r + 7, 2)),
    ]
}

/// For cases that freeze selection text without a screen (frozen already by `__base`).
pub fn serialize_selection_probes(probes: &[SelectionProbe]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for ((a, b), text) in probes {
        let _ = writeln!(
            s,
            "selection_text(({},{}),({},{})) \"{}\"",
            a.0,
            a.1,
            b.0,
            b.1,
            text.as_deref().map_or("<none>".to_string(), escape)
        );
    }
    s
}

pub fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expected")
}

/// Gated so a stale expectation can never be papered over by an accidental re-run.
pub fn blessing() -> bool {
    std::env::var("GROVE_TERM_BLESS").as_deref() == Ok("1")
}

/// One line per cell run (consecutive cells sharing fg/bg/bold collapse), keeping a 34×120 screen reviewable:
///
/// ```text
/// r{row} c{col} "{text}" fg={TermColor:?} bg={TermColor:?} bold={bool}
/// ```
///
/// Followed by a trailer block of non-cell state and probe outputs; text is escaped so it stays diffable.
pub fn serialize_dump(d: &ScreenDump, probes: &Probes) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "geometry {}x{}", d.rows, d.cols);
    let cols = d.cols as usize;
    for row in 0..d.rows as usize {
        let mut col = 0usize;
        while col < cols {
            let head = &d.cells[row * cols + col];
            let mut end = col + 1;
            let mut text = head.text.clone();
            while end < cols {
                let next = &d.cells[row * cols + end];
                if next.fg != head.fg || next.bg != head.bg || next.bold != head.bold {
                    break;
                }
                text.push_str(&next.text);
                end += 1;
            }
            let _ = writeln!(
                s,
                "r{row} c{col} \"{}\" fg={:?} bg={:?} bold={}",
                escape(&text),
                head.fg,
                head.bg,
                head.bold
            );
            col = end;
        }
    }
    let _ = writeln!(s, "-- trailer --");
    let _ = writeln!(
        s,
        "cursor ({},{},visible={})",
        d.cursor.0, d.cursor.1, !d.cursor_hidden
    );
    let _ = writeln!(
        s,
        "title {}",
        d.title.as_deref().map_or("<none>".to_string(), escape)
    );
    let _ = writeln!(s, "bell_count {}", d.bell_count);
    let _ = writeln!(s, "display_offset {}", d.display_offset);
    let _ = writeln!(s, "app_cursor {}", d.app_cursor);
    for (n, tail) in &probes.tails {
        let _ = writeln!(s, "tail_contents({n}) \"{}\"", escape(tail));
    }
    for ((a, b), text) in &probes.selections {
        let _ = writeln!(
            s,
            "selection_text(({},{}),({},{})) \"{}\"",
            a.0,
            a.1,
            b.0,
            b.1,
            text.as_deref().map_or("<none>".to_string(), escape)
        );
    }
    s
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// `header` is prepended as `# `-lines when blessing — how a `__DIVERGENCE` file records which side it came from and why.
pub fn assert_expected(case: &str, text: &str, header: &[&str]) {
    let path = expected_dir().join(format!("{case}.dump"));
    let mut body = String::new();
    for line in header {
        body.push_str("# ");
        body.push_str(line);
        body.push('\n');
    }
    body.push_str(text);

    if blessing() {
        fs_err::create_dir_all(expected_dir()).unwrap();
        fs_err::write(&path, body.as_bytes()).unwrap();
        return;
    }

    let want = fs_err::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing expected dump {}: {e}\nthese files were blessed from the \
             vt100 oracle, which no longer exists — re-blessing with \
             GROVE_TERM_BLESS=1 would replace an independent expectation with \
             the model's own output",
            path.display()
        )
    });
    if want == body {
        return;
    }
    // Names the first differing line, not a 4000-line wall of text.
    let (mut wl, mut gl) = (want.lines(), body.lines());
    let mut n = 0usize;
    loop {
        n += 1;
        match (wl.next(), gl.next()) {
            (None, None) => break,
            (a, b) if a == b => {}
            (a, b) => panic!(
                "{}: line {n} differs\n  expected: {:?}\n  actual  : {:?}",
                path.display(),
                a,
                b
            ),
        }
    }
    panic!("{}: content differs but no line differs?", path.display());
}
