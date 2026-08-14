//! `GroveTerm` — the headless terminal model: an `alacritty_terminal::Term` exposing Grove's *token space* only (no theme colors, no gpui types, no executor).
//! Behavioral parity with the in-tree `vt100` parser is enforced by `tests/golden.rs`, which feeds recorded PTY streams to both and compares cell by cell.

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

/// Matches the deleted iced app's vt100 parser, so the frozen golden dumps retain the same history.
pub const SCROLLING_HISTORY: usize = 5000;

/// Crate-local rather than a re-export of `vt100`'s `MouseProtocolMode`, so the vt100 dependency can be deleted later without touching callers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    None,
    /// `?1000`.
    Normal,
    /// `?1002`.
    Button,
    /// `?1003`.
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    #[default]
    Default,
    /// `?1006`.
    Sgr,
    /// `?1005`.
    Utf8,
}

/// Local rather than `alacritty_terminal::term::test::TermSize`: that lives in a module named `test`, which production code shouldn't reach into.
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
    /// Must never reset — callers diff against their last-seen value.
    bells: usize,
    title: Option<String>,
    /// Bytes the emulator wants written back to the PTY (Device Attributes, cursor-position reports, …), drained by `GroveTerm::take_responses`.
    responses: Vec<u8>,
}

/// alacritty reports the window title through the event channel, so this listener is the only source for it.
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
            Event::PtyWrite(text) => s.responses.extend_from_slice(text.as_bytes()),
            // ColorRequest (needs the gpui theme layer, not here) and ClipboardLoad (would leak the clipboard to any asking program) stay unanswered.
            _ => {}
        }
    }
}

pub struct GroveTerm {
    term: FairMutex<Term<GroveListener>>,
    processor: Processor<StdSyncHandler>,
    listener: GroveListener,
    /// Bumped on damage; a cheap "did anything change" signal instead of diffing snapshots.
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

    /// Chunk boundaries are irrelevant: `Processor` carries escape-sequence state across calls.
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

    pub fn damage_generation(&self) -> u64 {
        self.damage_gen
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Italic, underline, dim and strikethrough are deliberately dropped — Grove never drew them; don't "fix" this without changing the spec.
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
                    c: cell_char(cell),
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

    /// `(row, col, hidden)` in viewport coordinates.
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

    /// OSC 0/1/2 window title, trimmed; `None` when empty.
    pub fn title(&self) -> Option<String> {
        let s = self.listener.state.lock().ok()?;
        let t = s.title.as_ref()?.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    pub fn bell_count(&self) -> usize {
        self.listener.state.lock().map_or(0, |s| s.bells)
    }

    /// Protocol replies, NOT user input — the caller must write them straight to the PTY.
    pub fn take_responses(&mut self) -> Vec<u8> {
        self.listener
            .state
            .lock()
            .map_or_else(|_| Vec::new(), |mut s| std::mem::take(&mut s.responses))
    }

    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

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

    /// SGR wins over UTF-8 when both are set, matching how terminals resolve the overlap.
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

    pub fn history_size(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    /// Clamped to the configured scrollback.
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

    /// Last `n` rows of the *live* screen, for the activity classifier. Scroll offset is temporarily zeroed so a scrolled-back user can't feed it stale markers.
    pub fn tail_contents(&mut self, n: usize) -> String {
        let mut term = self.term.lock();
        let orig = term.grid().display_offset();
        if orig != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        let rows = term.grid().screen_lines();
        // A 2n-row window, not the whole grid — avoids materializing hundreds of rows on every activity tick.
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

    /// Clamps to >=1, no-ops when unchanged, snaps scroll to live screen first.
    /// Note: this **reflows** the primary screen — unsuppressable without patching alacritty (see `golden_resize.rs`).
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

    /// Text between two endpoints in scrollback-stable absolute coordinates (`(abs_row, col)`, larger `abs_row` is older).
    /// Grove's selection is a plain cell rectangle, not alacritty's semantic `Selection`, so this walks the grid directly.
    /// Takes `&mut self`: reading off-screen rows is a viewport-moving operation on the oracle side.
    pub fn selection_text(&mut self, p1: (usize, usize), p2: (usize, usize)) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let h = grid.screen_lines();
        if h == 0 {
            return None;
        }
        let cols = grid.columns();
        let offset = grid.display_offset();
        // Viewport row for an absolute row; may fall outside [0, h-1] when scrolled off screen.
        let vr = |a: usize| -> isize { (h as isize - 1) - (a as isize - offset as isize) };
        let (r1, r2) = (vr(p1.0), vr(p2.0));
        let (top, bot) = if (r1, p1.1) <= (r2, p2.1) {
            ((r1, p1.1), (r2, p2.1))
        } else {
            ((r2, p2.1), (r1, p1.1))
        };

        if top.0 >= 0 && bot.0 < h as isize {
            // Fully visible: one multi-row read.
            let sc = top.1.min(cols);
            let ec = bot.1.saturating_add(1).min(cols);
            let raw = contents_between(&term, top.0 as usize, sc, bot.0 as usize, ec);
            return clean_selection(raw);
        }

        // Off-screen: walk absolute rows, older first; smaller column starts on a row tie.
        let (a_top, c_top, a_bot, c_bot) =
            if (p1.0, std::cmp::Reverse(p1.1)) >= (p2.0, std::cmp::Reverse(p2.1)) {
                (p1.0, p1.1, p2.0, p2.1)
            } else {
                (p2.0, p2.1, p1.0, p1.1)
            };
        let history = grid.history_size();
        let mut lines: Vec<String> = Vec::new();
        for a in (a_bot..=a_top).rev() {
            // Rows older than the retained history are skipped, matching vt100's clamp-then-skip behavior.
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

/// Wide chars live in their lead cell; the spacer cells render blank so the grid stays rectangular, matching vt100's layout.
fn cell_char(cell: &ACell) -> char {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        return ' ';
    }
    cell.c
}

/// The 16 ANSI slots keep their raw index (bright stays 8..=15) — folding onto 0..=7 happens at paint time, downstream.
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
        // DIM_* is alacritty's rendering of DIM over a named color; DIM itself is an attribute Grove drops.
        N::DimBlack => TermColor::Ansi(0),
        N::DimRed => TermColor::Ansi(1),
        N::DimGreen => TermColor::Ansi(2),
        N::DimYellow => TermColor::Ansi(3),
        N::DimBlue => TermColor::Ansi(4),
        N::DimMagenta => TermColor::Ansi(5),
        N::DimCyan => TermColor::Ansi(6),
        N::DimWhite => TermColor::Ansi(7),
        // Foreground/Background/Cursor and variants are the terminal's defaults (vt100's `Color::Default`).
        _ => TermColor::Default,
    }
}

/// Ports vt100's `Row::write_contents` to an alacritty row: emit occupied cells, pad gaps, stop at the last occupied column, skip wide-char spacers.
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

/// Ports vt100's `Screen::contents_between` over viewport rows: soft-wrapped rows join, hard-broken rows get a newline.
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

/// Matches vt100's `Grid::write_contents`/`contents_between` newline rules.
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

fn tail_lines(contents: &str, n: usize) -> String {
    // Right-trimmed: alacritty can't distinguish a written-but-blank cell from an untouched one the way vt100 can, so both parsers normalize trailing spaces away.
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let from = lines.len().saturating_sub(n);
    lines[from..].join("\n")
}

/// Trims trailing whitespace per line, drops trailing blank lines, `None` when nothing is left.
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

    fn modes(bytes: &[u8]) -> (MouseMode, MouseEncoding) {
        let mut t = GroveTerm::new(24, 80);
        t.process(bytes);
        (t.mouse_mode(), t.encoding())
    }

    /// These are the DEC private modes, independently derived — not a snapshot of `GroveTerm`.
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
    fn sgr_torture_fixture_toggles_the_alt_screen_without_enabling_the_mouse() {
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
