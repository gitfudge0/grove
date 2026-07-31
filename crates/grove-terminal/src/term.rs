//! `GroveTerm` — the headless terminal model.
//!
//! An `alacritty_terminal::Term` behind a `FairMutex`, driven by a single
//! stateful `Processor`, exposing exactly the surface Grove needs in *token
//! space*: no theme colors, no gpui types, no executor.
//!
//! Behavioral parity with the in-tree `vt100` parser is not aspirational — it
//! is enforced by `tests/golden.rs`, which feeds recorded PTY streams to both
//! and compares cell by cell.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell as ACell, Flags, LineLength};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor, StdSyncHandler};

use crate::cell::{Cell, Snapshot};
use crate::color::TermColor;

/// Scrollback depth, matching `crates/grove-core/src/session.rs`'s vt100
/// parser so both sides of the golden harness retain the same history.
pub const SCROLLING_HISTORY: usize = 5000;

/// How the inner app wants mouse events reported.
///
/// Deliberately crate-local rather than a re-export of `vt100`'s
/// `MouseProtocolMode`: grove-terminal owns its vocabulary so the vt100
/// dependency can be deleted in a later phase without touching callers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// No mouse reporting.
    #[default]
    None,
    /// Press/release only (`?1000`).
    Normal,
    /// Press/release plus motion while a button is held (`?1002`).
    Button,
    /// All motion (`?1003`).
    Any,
}

/// How mouse reports are encoded on the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    /// The original X10 byte encoding.
    #[default]
    Default,
    /// SGR (`?1006`).
    Sgr,
    /// UTF-8 (`?1005`).
    Utf8,
}

/// Geometry handed to `Term::new`/`Term::resize`.
///
/// A local type rather than `alacritty_terminal::term::test::TermSize`: that
/// one lives in a module named `test`, and production code should not reach
/// into a dependency's test helpers.
#[derive(Clone, Copy, Debug)]
struct GroveSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for GroveSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Debug, Default)]
struct ListenerState {
    /// Monotonic BEL counter. Callers diff against their last-seen value, so
    /// it must never reset — see `session.rs:930-938`.
    bells: usize,
    title: Option<String>,
}

/// Captures the two `Term` events Grove cares about. alacritty reports the
/// window title through the event channel (vt100 exposes it as screen state
/// instead), so the listener is the only place it can come from.
#[derive(Clone, Debug, Default)]
struct GroveListener {
    state: Arc<Mutex<ListenerState>>,
}

impl EventListener for GroveListener {
    fn send_event(&self, event: Event) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        match event {
            Event::Bell => s.bells += 1,
            Event::Title(t) => s.title = Some(t),
            Event::ResetTitle => s.title = None,
            _ => {}
        }
    }
}

/// The headless terminal model.
pub struct GroveTerm {
    term: FairMutex<Term<GroveListener>>,
    processor: Processor<StdSyncHandler>,
    listener: GroveListener,
    /// Bumped whenever a `process` call reported any damage. Callers use it as
    /// a cheap "did anything change" signal instead of diffing snapshots.
    damage_gen: u64,
    rows: u16,
    cols: u16,
}

impl GroveTerm {
    pub fn new(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let size = GroveSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        };
        let listener = GroveListener::default();
        let config = Config {
            scrolling_history: SCROLLING_HISTORY,
            ..Config::default()
        };
        Self {
            term: FairMutex::new(Term::new(config, &size, listener.clone())),
            processor: Processor::<StdSyncHandler>::new(),
            listener,
            damage_gen: 0,
            rows,
            cols,
        }
    }

    /// Feed a chunk of PTY output. Chunk boundaries are irrelevant: the
    /// `Processor` carries the escape-sequence state across calls
    /// (`golden_chunking_invariance` guards it).
    pub fn process(&mut self, bytes: &[u8]) {
        let mut term = self.term.lock();
        self.processor.advance(&mut *term, bytes);
        let damaged = match term.damage() {
            alacritty_terminal::term::TermDamage::Full => true,
            alacritty_terminal::term::TermDamage::Partial(mut it) => it.next().is_some(),
        };
        term.reset_damage();
        drop(term);
        if damaged {
            self.damage_gen = self.damage_gen.wrapping_add(1);
        }
    }

    /// Monotonic damage counter; changes whenever `process` saw the grid move.
    pub fn damage_generation(&self) -> u64 {
        self.damage_gen
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// The visible grid in token space.
    ///
    /// Italic, underline, dim and strikethrough are deliberately dropped: the
    /// spec's parity decision is that Grove never drew them. Do not "fix" this
    /// without changing the spec.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let grid = term.grid();
        let rows = grid.screen_lines();
        let cols = grid.columns();
        let offset = grid.display_offset() as i32;
        let mut cells = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row = &grid[Line(r as i32 - offset)];
            for c in 0..cols {
                let cell = &row[Column(c)];
                cells.push(Cell {
                    text: cell_text(cell),
                    fg: map_color(cell.fg),
                    bg: map_color(cell.bg),
                    bold: cell.flags.contains(Flags::BOLD),
                    inverse: cell.flags.contains(Flags::INVERSE),
                });
            }
        }
        drop(term);
        Snapshot {
            rows: u16::try_from(rows).unwrap_or(u16::MAX),
            cols: u16::try_from(cols).unwrap_or(u16::MAX),
            cells,
        }
    }

    /// `(row, col, hidden)` in viewport coordinates. Adding the display offset
    /// keeps a scrolled-back view lined up with vt100's `cursor_position`.
    pub fn cursor(&self) -> (u16, u16, bool) {
        let term = self.term.lock();
        let grid = term.grid();
        let point = grid.cursor.point;
        let row = point.line.0 + grid.display_offset() as i32;
        let row = u16::try_from(row.max(0)).unwrap_or(u16::MAX);
        let col = u16::try_from(point.column.0).unwrap_or(u16::MAX);
        let hidden = !term.mode().contains(TermMode::SHOW_CURSOR);
        (row, col, hidden)
    }

    /// The current OSC 0/1/2 window title, trimmed; `None` when empty.
    /// Semantics copied from `session.rs:871-881`.
    pub fn title(&self) -> Option<String> {
        let s = self.listener.state.lock().ok()?;
        let t = s.title.as_ref()?.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// Total BEL count seen on this stream. Monotonic; callers diff it.
    pub fn bell_count(&self) -> usize {
        self.listener.state.lock().map(|s| s.bells).unwrap_or(0)
    }

    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    /// Mouse reporting mode requested by the inner app.
    pub fn mouse_mode(&self) -> MouseMode {
        let term = self.term.lock();
        let mode = *term.mode();
        drop(term);
        if mode.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Any
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Button
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Normal
        } else {
            MouseMode::None
        }
    }

    /// Wire encoding for mouse reports. SGR wins over UTF-8 when both are set,
    /// matching how terminals resolve the overlap.
    pub fn encoding(&self) -> MouseEncoding {
        let term = self.term.lock();
        let mode = *term.mode();
        drop(term);
        if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        }
    }

    /// Rows of scrollback currently scrolled above the live screen.
    pub fn history_size(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    /// Scroll so `n` rows of history sit above the viewport, clamped to the
    /// configured scrollback — mirroring `session.rs:690-705`.
    pub fn scroll_to(&mut self, n: usize) {
        let mut term = self.term.lock();
        let history = term.grid().history_size();
        let target = n.min(history).min(SCROLLING_HISTORY);
        let current = term.grid().display_offset();
        if target == current {
            return;
        }
        let delta = target as i32 - current as i32;
        term.scroll_display(Scroll::Delta(delta));
    }

    /// Last `n` rows of the *live* screen, newline-joined, for
    /// `src/gui/activity.rs`'s classifier.
    ///
    /// The scroll offset is temporarily zeroed so a user scrolled into history
    /// can't feed the classifier stale markers (or hide a fresh prompt at the
    /// bottom). Ported semantically from `session.rs:883-928`.
    pub fn tail_contents(&mut self, n: usize) -> String {
        let mut term = self.term.lock();
        let orig = term.grid().display_offset();
        if orig != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        let rows = term.grid().screen_lines();
        // A 2n-row window rather than the whole grid: materializing hundreds of
        // rows on every activity tick, for every session, is the cost this
        // avoids. Trailing blank rows are trimmed here, so 2n rows normally
        // still leave n real ones; only when they don't do we pay for the
        // full screen.
        let window = n.saturating_mul(2);
        let start = rows.saturating_sub(window);
        let mut out = tail_lines(&rows_to_string(&term, start, rows), n);
        if start > 0 && out.lines().count() < n {
            out = tail_lines(&rows_to_string(&term, 0, rows), n);
        }
        if orig != 0 {
            term.scroll_display(Scroll::Delta(orig as i32));
        }
        out
    }

    /// Resize the grid. Clamps to ≥1, no-ops when unchanged, and snaps the
    /// scroll offset to the live screen first.
    ///
    /// The snap is unconditional: vt100 0.15.2 does not clamp its offset on
    /// resize (`session.rs:957-963`), and snapping on both sides keeps the two
    /// parsers starting from the same state.
    ///
    /// Note this **reflows** the primary screen — `Term::resize` hardcodes
    /// `self.grid.resize(!is_alt, ..)` (`term/mod.rs:677`) with no config knob,
    /// so the spec §3 sentence "reflow-on-resize is suppressed" is not
    /// achievable without patching alacritty. See
    /// `primary_screen_reflow_is_a_known_divergence` in `tests/golden_resize.rs`.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        term.resize(GroveSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        });
    }

    /// Text between two endpoints in scrollback-stable absolute coordinates
    /// (`(abs_row, col)`, larger `abs_row` is older).
    ///
    /// Grove's selection is a plain cell rectangle, not alacritty's semantic
    /// `Selection` (spec §3), so this walks the grid directly. Endpoint
    /// ordering matches `src/gui/pty.rs:374-381`'s `normalize_selection`;
    /// extraction and cleanup match `session.rs:802-869`.
    /// Takes `&mut self` because reading rows that are scrolled off screen
    /// is a viewport-moving operation on the oracle side (`set_scrollback`
    /// then restore), and callers must not assume otherwise.
    pub fn selection_text(&mut self, p1: (usize, usize), p2: (usize, usize)) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let h = grid.screen_lines();
        if h == 0 {
            return None;
        }
        let cols = grid.columns();
        let offset = grid.display_offset();
        // Viewport row for an absolute row at the current offset; may fall
        // outside [0, h-1] when the row is scrolled off screen.
        let vr = |a: usize| -> isize { (h as isize - 1) - (a as isize - offset as isize) };
        let (r1, r2) = (vr(p1.0), vr(p2.0));
        let (top, bot) = if (r1, p1.1) <= (r2, p2.1) {
            ((r1, p1.1), (r2, p2.1))
        } else {
            ((r2, p2.1), (r1, p1.1))
        };

        if top.0 >= 0 && bot.0 < h as isize {
            // Fully visible: one multi-row read, preserving soft-wrap joining.
            let sc = top.1.min(cols);
            let ec = bot.1.saturating_add(1).min(cols);
            let raw = contents_between(&term, top.0 as usize, sc, bot.0 as usize, ec);
            return clean_selection(raw);
        }

        // Off-screen: walk absolute rows, older first. On a row tie the
        // smaller column starts.
        let (a_top, c_top, a_bot, c_bot) =
            if (p1.0, std::cmp::Reverse(p1.1)) >= (p2.0, std::cmp::Reverse(p2.1)) {
                (p1.0, p1.1, p2.0, p2.1)
            } else {
                (p2.0, p2.1, p1.0, p1.1)
            };
        let history = grid.history_size();
        let mut lines: Vec<String> = Vec::new();
        for a in (a_bot..=a_top).rev() {
            // Absolute row `a` is grid line `h - 1 - a`, independent of the
            // display offset. Rows older than the retained history are skipped,
            // matching vt100's clamp-then-skip behavior.
            let line = h as isize - 1 - a as isize;
            if line < -(history as isize) {
                continue;
            }
            let sc = if a == a_top { c_top.min(cols) } else { 0 };
            let ec = if a == a_bot {
                c_bot.saturating_add(1).min(cols)
            } else {
                cols
            };
            let mut s = String::new();
            write_row(&mut s, &grid[Line(line as i32)], sc, ec);
            lines.push(s.trim_end().to_string());
        }
        drop(term);
        let out = lines.join("\n");
        let out = out.trim_end_matches('\n').to_string();
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

// ---------------------------------------------------------------------------
// Cell/text helpers
// ---------------------------------------------------------------------------

/// The visible text of a cell.
///
/// Wide characters live in their lead cell; the trailing `WIDE_CHAR_SPACER`
/// (and the `LEADING_WIDE_CHAR_SPACER` a wide char at the line edge pushes out)
/// render blank so the grid stays rectangular — the same layout vt100 produces.
fn cell_text(cell: &ACell) -> String {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        return " ".to_string();
    }
    let mut s = String::new();
    s.push(cell.c);
    for c in cell.zerowidth().into_iter().flatten() {
        s.push(*c);
    }
    s
}

/// `vte::ansi::Color` → token space.
///
/// The 16 ANSI slots keep their raw index — bright variants stay at 8..=15
/// rather than folding onto 0..=7, because `src/gui/pty.rs`'s `ansi_idx` does
/// that folding at *paint* time and the index has to survive until then.
fn map_color(c: AColor) -> TermColor {
    match c {
        AColor::Named(n) => named_to_token(n),
        AColor::Indexed(i) => TermColor::Ansi(i),
        AColor::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn named_to_token(n: NamedColor) -> TermColor {
    use NamedColor as N;
    match n {
        N::Black => TermColor::Ansi(0),
        N::Red => TermColor::Ansi(1),
        N::Green => TermColor::Ansi(2),
        N::Yellow => TermColor::Ansi(3),
        N::Blue => TermColor::Ansi(4),
        N::Magenta => TermColor::Ansi(5),
        N::Cyan => TermColor::Ansi(6),
        N::White => TermColor::Ansi(7),
        N::BrightBlack => TermColor::Ansi(8),
        N::BrightRed => TermColor::Ansi(9),
        N::BrightGreen => TermColor::Ansi(10),
        N::BrightYellow => TermColor::Ansi(11),
        N::BrightBlue => TermColor::Ansi(12),
        N::BrightMagenta => TermColor::Ansi(13),
        N::BrightCyan => TermColor::Ansi(14),
        N::BrightWhite => TermColor::Ansi(15),
        // The DIM_* slots are alacritty's rendering of the DIM attribute over a
        // named color; the underlying token is the base color, and DIM itself
        // is one of the attributes Grove drops.
        N::DimBlack => TermColor::Ansi(0),
        N::DimRed => TermColor::Ansi(1),
        N::DimGreen => TermColor::Ansi(2),
        N::DimYellow => TermColor::Ansi(3),
        N::DimBlue => TermColor::Ansi(4),
        N::DimMagenta => TermColor::Ansi(5),
        N::DimCyan => TermColor::Ansi(6),
        N::DimWhite => TermColor::Ansi(7),
        // Foreground/Background/Cursor and their bright/dim variants are the
        // terminal's *defaults* — vt100 calls this `Color::Default`.
        _ => TermColor::Default,
    }
}

/// vt100's `Row::write_contents` (`vt100-0.15.2/src/row.rs:98-135`) ported to
/// an alacritty row: emit occupied cells, pad gaps with spaces, stop at the
/// last occupied column, and never emit a wide char's trailing spacer.
fn write_row(
    out: &mut String,
    row: &alacritty_terminal::grid::Row<ACell>,
    start: usize,
    end: usize,
) {
    let end = end.min(row.len());
    let occupied = row.line_length().0.min(end);
    for col in start..occupied {
        let cell = &row[Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        out.push(cell.c);
        for c in cell.zerowidth().into_iter().flatten() {
            out.push(*c);
        }
    }
}

fn row_wrapped(row: &alacritty_terminal::grid::Row<ACell>) -> bool {
    row.last()
        .is_some_and(|c| c.flags.contains(Flags::WRAPLINE))
}

/// vt100's `Screen::contents_between` (`vt100-0.15.2/src/screen.rs:182-231`)
/// over viewport rows: soft-wrapped rows are joined, hard-broken rows get a
/// newline.
fn contents_between(
    term: &Term<GroveListener>,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> String {
    let grid = term.grid();
    let cols = grid.columns();
    let offset = grid.display_offset() as i32;
    let line_of = |r: usize| Line(r as i32 - offset);
    match start_row.cmp(&end_row) {
        std::cmp::Ordering::Less => {
            let mut out = String::new();
            for r in start_row..=end_row {
                let row = &grid[line_of(r)];
                if r == start_row {
                    write_row(&mut out, row, start_col, cols);
                    if !row_wrapped(row) {
                        out.push('\n');
                    }
                } else if r == end_row {
                    write_row(&mut out, row, 0, end_col);
                } else {
                    write_row(&mut out, row, 0, cols);
                    if !row_wrapped(row) {
                        out.push('\n');
                    }
                }
            }
            out
        }
        std::cmp::Ordering::Equal => {
            if start_col < end_col {
                let mut out = String::new();
                write_row(&mut out, &grid[line_of(start_row)], start_col, end_col);
                out
            } else {
                String::new()
            }
        }
        std::cmp::Ordering::Greater => String::new(),
    }
}

/// Viewport rows `[start, end)` rendered as text, matching vt100's
/// `Grid::write_contents`/`contents_between` newline rules.
fn rows_to_string(term: &Term<GroveListener>, start: usize, end: usize) -> String {
    if start + 1 >= end {
        let mut out = String::new();
        if start < end {
            let grid = term.grid();
            let offset = grid.display_offset() as i32;
            write_row(
                &mut out,
                &grid[Line(start as i32 - offset)],
                0,
                grid.columns(),
            );
        }
        return out;
    }
    let cols = term.grid().columns();
    contents_between(term, start, 0, end - 1, cols)
}

/// `session.rs:888-896`'s `tail_lines`.
fn tail_lines(contents: &str, n: usize) -> String {
    // Each line is right-trimmed. vt100 records "written" per cell, so a
    // written-but-blank cell at the end of a row survives its text extraction;
    // alacritty records occupancy per row only (`Row::occ`, private) and cannot
    // tell such a cell from an untouched one. Trailing spaces mean nothing to
    // `src/gui/activity.rs`'s classifier, so both parsers normalize them away —
    // the same shape as the golden harness's blank-cell normalization.
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let from = lines.len().saturating_sub(n);
    lines[from..].join("\n")
}

/// `session.rs:970-990`'s `clean_selection`: trim trailing whitespace per line,
/// drop trailing blank lines, `None` when nothing is left.
fn clean_selection(raw: String) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for line in raw.split('\n') {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    let end = out.trim_end_matches('\n').len();
    out.truncate(end);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// vt100's equivalent of `mouse_mode`/`encoding`, so the mapping is checked
    /// against the parser Grove ships today rather than against my reading of
    /// the DEC private modes.
    fn oracle_modes(bytes: &[u8]) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(bytes);
        (
            p.screen().mouse_protocol_mode(),
            p.screen().mouse_protocol_encoding(),
        )
    }

    fn modes(bytes: &[u8]) -> (MouseMode, MouseEncoding) {
        let mut t = GroveTerm::new(24, 80);
        t.process(bytes);
        (t.mouse_mode(), t.encoding())
    }

    #[test]
    fn mouse_mode_and_encoding_track_the_dec_private_modes() {
        let cases: &[(&[u8], MouseMode, MouseEncoding)] = &[
            (b"", MouseMode::None, MouseEncoding::Default),
            (b"\x1b[?1000h", MouseMode::Normal, MouseEncoding::Default),
            (b"\x1b[?1002h", MouseMode::Button, MouseEncoding::Default),
            (b"\x1b[?1003h", MouseMode::Any, MouseEncoding::Default),
            (
                b"\x1b[?1000h\x1b[?1006h",
                MouseMode::Normal,
                MouseEncoding::Sgr,
            ),
            (
                b"\x1b[?1002h\x1b[?1005h",
                MouseMode::Button,
                MouseEncoding::Utf8,
            ),
            (
                b"\x1b[?1003h\x1b[?1003l",
                MouseMode::None,
                MouseEncoding::Default,
            ),
            (
                b"\x1b[?1006h\x1b[?1006l\x1b[?1000h",
                MouseMode::Normal,
                MouseEncoding::Default,
            ),
        ];
        for (bytes, mode, encoding) in cases {
            assert_eq!(modes(bytes), (*mode, *encoding), "for {bytes:?}");
        }
    }

    #[test]
    fn mouse_mode_and_encoding_agree_with_the_vt100_oracle() {
        let streams: &[&[u8]] = &[
            b"",
            b"\x1b[?1000h",
            b"\x1b[?1002h",
            b"\x1b[?1003h",
            b"\x1b[?1000h\x1b[?1006h",
            b"\x1b[?1002h\x1b[?1005h",
            b"\x1b[?1003h\x1b[?1003l",
            b"\x1b[?1000h\x1b[?1002h\x1b[?1006h",
        ];
        for bytes in streams {
            let (mode, encoding) = modes(bytes);
            let (vt_mode, vt_encoding) = oracle_modes(bytes);
            let want_mode = match vt_mode {
                vt100::MouseProtocolMode::None => MouseMode::None,
                vt100::MouseProtocolMode::Press => MouseMode::Normal,
                vt100::MouseProtocolMode::PressRelease => MouseMode::Normal,
                vt100::MouseProtocolMode::ButtonMotion => MouseMode::Button,
                vt100::MouseProtocolMode::AnyMotion => MouseMode::Any,
            };
            let want_encoding = match vt_encoding {
                vt100::MouseProtocolEncoding::Default => MouseEncoding::Default,
                vt100::MouseProtocolEncoding::Sgr => MouseEncoding::Sgr,
                vt100::MouseProtocolEncoding::Utf8 => MouseEncoding::Utf8,
            };
            assert_eq!(
                (mode, encoding),
                (want_mode, want_encoding),
                "vt100 said {vt_mode:?}/{vt_encoding:?} for {bytes:?}"
            );
        }
    }

    #[test]
    fn sgr_torture_fixture_toggles_the_alt_screen_without_enabling_the_mouse() {
        // The committed `sgr-torture` fixture ends back on the primary screen
        // and never enables mouse reporting; a change to the fixture that
        // silently added mouse modes would be caught here.
        let bytes = fs_err::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sgr-torture.bin"
        ))
        .expect("sgr-torture fixture");
        let mut t = GroveTerm::new(24, 80);
        t.process(&bytes);
        assert_eq!(t.mouse_mode(), MouseMode::None);
        assert_eq!(t.encoding(), MouseEncoding::Default);
    }
}
