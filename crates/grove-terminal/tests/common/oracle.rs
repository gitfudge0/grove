//! The vt100 oracle: the in-tree parser the iced app ships with, wrapped so it
//! produces the same neutral `ScreenDump` the model must produce.
//!
//! This side is the reference. If it panics, everything downstream is
//! meaningless — treat an oracle panic as a bug in this file, never as a
//! finding about the model.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{apply_inverse, normalize_cell_text, CellDump, ScreenDump};
use grove_terminal::TermColor;

pub const SCROLLBACK: usize = 5000;

/// `src/gui/pty.rs:382-390`'s `vt_color_opt` with the theme lookup removed —
/// that removal is the whole point of token space.
fn map_color(c: vt100::Color) -> TermColor {
    match c {
        vt100::Color::Default => TermColor::Default,
        vt100::Color::Idx(i) => TermColor::Ansi(i),
        vt100::Color::Rgb(r, g, b) => TermColor::Rgb(r, g, b),
    }
}

/// vt100 0.15.2's `Grid::visible_rows` computes `rows_len - scrollback_offset`
/// without a guard, so any offset larger than the screen height panics with
/// "attempt to subtract with overflow" (`vt100-0.15.2/src/grid.rs:125`).
/// `set_scrollback` only clamps to the *scrollback* length, which is far
/// larger. The iced app never drives an offset past a screen height, so the bug
/// stays latent there; the oracle must clamp explicitly or deep-scrollback
/// probes crash the reference implementation.
fn set_scrollback_clamped(p: &mut vt100::Parser, n: usize) {
    let (rows, _) = p.screen().size();
    p.set_scrollback(n.min(rows as usize));
}

pub fn parser(bytes: &[u8], rows: u16, cols: u16) -> vt100::Parser {
    let mut p = vt100::Parser::new(rows, cols, SCROLLBACK);
    p.process(bytes);
    p
}

pub fn dump(p: &vt100::Parser) -> ScreenDump {
    let (rows, cols) = p.screen().size();
    let mut cells = Vec::with_capacity(rows as usize * cols as usize);
    for row in 0..rows {
        for col in 0..cols {
            let (text, fg, bg, bold) = match p.screen().cell(row, col) {
                Some(c) => {
                    let (fg, bg) =
                        apply_inverse(map_color(c.fgcolor()), map_color(c.bgcolor()), c.inverse());
                    (normalize_cell_text(&c.contents()), fg, bg, c.bold())
                }
                None => (
                    " ".to_string(),
                    TermColor::Default,
                    TermColor::Default,
                    false,
                ),
            };
            cells.push(CellDump { text, fg, bg, bold });
        }
    }
    let cursor = p.screen().cursor_position();
    let title = {
        let t = p.screen().title().trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    ScreenDump {
        rows,
        cols,
        cells,
        cursor,
        cursor_hidden: p.screen().hide_cursor(),
        title,
        bell_count: p.screen().audible_bell_count(),
        display_offset: p.screen().scrollback(),
        app_cursor: p.screen().application_cursor(),
    }
}

pub fn dump_bytes(bytes: &[u8], rows: u16, cols: u16) -> ScreenDump {
    dump(&parser(bytes, rows, cols))
}

/// `crates/grove-core/src/session.rs:940-967`'s `resize`, minus the PTY master.
pub fn resize(p: &mut vt100::Parser, rows: u16, cols: u16) {
    let rows = rows.max(1);
    let cols = cols.max(1);
    if (rows, cols) == p.screen().size() {
        return;
    }
    if p.screen().scrollback() != 0 {
        p.set_scrollback(0);
    }
    p.set_size(rows, cols);
}

/// `crates/grove-core/src/session.rs:887-928`'s `tail_contents`.
pub fn tail_contents(p: &mut vt100::Parser, n: usize) -> String {
    fn tail_lines(contents: &str, n: usize) -> String {
        // Trailing-space normalization, applied identically on both sides (see
        // `GroveTerm::tail_contents`): vt100 tracks "was this cell written"
        // per cell, so a written-but-blank cell at the end of a row survives
        // extraction; alacritty tracks occupancy per row only and cannot
        // distinguish it from an untouched cell. The difference is invisible
        // to the activity classifier, so both sides right-trim.
        let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        let from = lines.len().saturating_sub(n);
        lines[from..].join("\n")
    }

    let orig = p.screen().scrollback();
    if orig != 0 {
        p.set_scrollback(0);
    }
    let (rows, cols) = p.screen().size();
    let window = u16::try_from(n.saturating_mul(2)).unwrap_or(u16::MAX);
    let start = rows.saturating_sub(window);
    let mut out = tail_lines(
        &p.screen()
            .contents_between(start, 0, rows.saturating_sub(1), cols),
        n,
    );
    if start > 0 && out.lines().count() < n {
        out = tail_lines(&p.screen().contents(), n);
    }
    if orig != 0 {
        p.set_scrollback(orig);
    }
    out
}

/// `crates/grove-core/src/session.rs:802-869`'s `selection_text_abs`.
pub fn selection_text(
    p: &mut vt100::Parser,
    p1: (usize, usize),
    p2: (usize, usize),
) -> Option<String> {
    let (h, cols) = p.screen().size();
    let h = h as usize;
    if h == 0 {
        return None;
    }
    let s = p.screen().scrollback();
    let vr = |a: usize| -> isize { (h as isize - 1) - (a as isize - s as isize) };
    let (r1, r2) = (vr(p1.0), vr(p2.0));
    let (top, bot) = if (r1, p1.1) <= (r2, p2.1) {
        ((r1, p1.1), (r2, p2.1))
    } else {
        ((r2, p2.1), (r1, p1.1))
    };

    if top.0 >= 0 && bot.0 < h as isize {
        let sc = top.1 as u16;
        let ec = (bot.1 as u16).saturating_add(1).min(cols);
        let raw = p
            .screen()
            .contents_between(top.0 as u16, sc, bot.0 as u16, ec);
        return clean_selection(raw);
    }

    let (a_top, c_top, a_bot, c_bot) =
        if (p1.0, std::cmp::Reverse(p1.1)) >= (p2.0, std::cmp::Reverse(p2.1)) {
            (p1.0, p1.1, p2.0, p2.1)
        } else {
            (p2.0, p2.1, p1.0, p1.1)
        };
    let orig = s;
    let mut lines: Vec<String> = Vec::new();
    for a in (a_bot..=a_top).rev() {
        set_scrollback_clamped(p, a);
        let actual = p.screen().scrollback();
        let delta = a.saturating_sub(actual);
        if delta < h {
            let vrow = (h - 1 - delta) as u16;
            let sc = if a == a_top { c_top as u16 } else { 0 };
            let ec = if a == a_bot {
                (c_bot as u16).saturating_add(1).min(cols)
            } else {
                cols
            };
            let raw = p.screen().contents_between(vrow, sc, vrow, ec);
            lines.push(raw.trim_end().to_string());
        }
    }
    set_scrollback_clamped(p, orig);
    let out = lines.join("\n");
    let out = out.trim_end_matches('\n').to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// `crates/grove-core/src/session.rs:973-987`'s `clean_selection`.
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
