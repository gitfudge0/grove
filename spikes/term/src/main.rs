//! Spike S1 — gpui TerminalElement end-to-end.
//!
//! Throwaway measurement code. Run with `cargo run -p spike-term`.
//! Everything interesting is eprintln'd with an `S1:` prefix.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event as AlacTermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as AlacConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, NamedColor, Processor, Rgb as AnsiRgb, StdSyncHandler,
};

use gpui::{
    div, fill, hsla, point, prelude::*, px, relative, size, App, AssetSource, Bounds, Context,
    ElementId, Entity, FocusHandle, Focusable, GlobalElementId, Hsla, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, SharedString, Style, TextAlign, TextRun, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;

// ---------------------------------------------------------------- constants

const FONT_PT: f32 = 12.5;
const CELL_W: f32 = 7.5;
const CELL_H: f32 = 17.0;
const BLINK_MS: u64 = 533;
const SCROLL_PX_PER_LINE: f32 = 17.0;
const FONT_FAMILY: &str = "BlexMono Nerd Font Mono";
const SCROLLBACK: usize = 5000;

// TokyoNight defaults, verbatim from crates/grove-core/src/theme.rs TOKYONIGHT.
const T_BG: (u8, u8, u8) = (0x1a, 0x1b, 0x26);
const T_FG: (u8, u8, u8) = (0xc0, 0xca, 0xf5);
const T_COMMENT: (u8, u8, u8) = (0x56, 0x5f, 0x89);
const T_BLUE: (u8, u8, u8) = (0x7a, 0xa2, 0xf7);
const T_CYAN: (u8, u8, u8) = (0x7d, 0xcf, 0xff);
const T_MAGENTA: (u8, u8, u8) = (0xbb, 0x9a, 0xf7);
const T_GREEN: (u8, u8, u8) = (0x9e, 0xce, 0x6a);
const T_YELLOW: (u8, u8, u8) = (0xe0, 0xaf, 0x68);
const T_RED: (u8, u8, u8) = (0xf7, 0x76, 0x8e);

fn rgb8(c: (u8, u8, u8)) -> Hsla {
    gpui::rgb(((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32).into()
}

/// `bg_strip_of` from src/gui/palette.rs: dark themes mix bg 32% toward black.
fn bg_strip() -> Hsla {
    let m = |x: u8| ((x as f32) * (1.0 - 0.32)) as u8;
    rgb8((m(T_BG.0), m(T_BG.1), m(T_BG.2)))
}

/// Port of `ansi_idx` (src/gui/pty.rs:390-421) against the TokyoNight defaults.
fn ansi_idx(i: u8) -> Hsla {
    match i {
        0 => bg_strip(),
        1 | 9 => rgb8(T_RED),
        2 | 10 => rgb8(T_GREEN),
        3 | 11 => rgb8(T_YELLOW),
        4 | 12 => rgb8(T_BLUE),
        5 | 13 => rgb8(T_MAGENTA),
        6 | 14 => rgb8(T_CYAN),
        7 | 15 => rgb8(T_FG),
        8 => rgb8(T_COMMENT),
        16..=231 => {
            let n = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
            rgb8((v(n / 36), v((n % 36) / 6), v(n % 6)))
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            rgb8((v, v, v))
        }
    }
}

fn named_idx(n: NamedColor) -> Option<u8> {
    use NamedColor::*;
    Some(match n {
        Black | DimBlack => 0,
        Red | DimRed => 1,
        Green | DimGreen => 2,
        Yellow | DimYellow => 3,
        Blue | DimBlue => 4,
        Magenta | DimMagenta => 5,
        Cyan | DimCyan => 6,
        White | DimWhite => 7,
        BrightBlack => 8,
        BrightRed => 9,
        BrightGreen => 10,
        BrightYellow => 11,
        BrightBlue => 12,
        BrightMagenta => 13,
        BrightCyan => 14,
        BrightWhite => 15,
        Foreground | BrightForeground | DimForeground | Cursor | Background => return None,
    })
}

fn conv_fg(c: AnsiColor) -> Hsla {
    match c {
        AnsiColor::Named(n) => match named_idx(n) {
            Some(i) => ansi_idx(i),
            None => rgb8(T_FG),
        },
        AnsiColor::Indexed(i) => ansi_idx(i),
        AnsiColor::Spec(AnsiRgb { r, g, b }) => rgb8((r, g, b)),
    }
}

fn conv_bg(c: AnsiColor) -> Option<Hsla> {
    match c {
        AnsiColor::Named(NamedColor::Background) => None,
        AnsiColor::Named(n) => named_idx(n).map(ansi_idx),
        AnsiColor::Indexed(i) => Some(ansi_idx(i)),
        AnsiColor::Spec(AnsiRgb { r, g, b }) => Some(rgb8((r, g, b))),
    }
}

// ---------------------------------------------------------------- assets

struct Assets {
    root: std::path::PathBuf,
}

impl Assets {
    fn new() -> Self {
        // spikes/term -> repo root
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join("assets");
        Self { root }
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let full = self.root.join(path);
        match std::fs::read(&full) {
            Ok(b) => Ok(Some(Cow::Owned(b))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.root.join(path)) {
            for e in rd.flatten() {
                out.push(SharedString::from(
                    e.file_name().to_string_lossy().to_string(),
                ));
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- terminal

#[derive(Clone, Copy, Debug)]
struct TermSize {
    cols: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct Listener(futures::channel::mpsc::UnboundedSender<AlacTermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: AlacTermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

// ---------------------------------------------------------------- reflow probe

/// Step 2 (reflow half). Runs headless before the window opens.
///
/// Writes long wrapped lines into a Term, then shrinks the column count and
/// dumps the rows, for both the primary and the alternate screen.
fn reflow_probe() {
    for alt in [false, true] {
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let sz = TermSize {
            cols: 40,
            lines: 6,
        };
        let cfg = AlacConfig {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term = Term::new(cfg, &sz, Listener(tx));
        let mut proc = Processor::<StdSyncHandler>::new();
        if alt {
            proc.advance(&mut term, b"\x1b[?1049h"); // DECSET 1049 -> alt screen
        }
        let long = (0..3u8)
            .map(|n| String::from((b'A' + n) as char).repeat(60))
            .collect::<Vec<_>>()
            .join("\r\n");
        proc.advance(&mut term, long.as_bytes());

        let before = dump(&term);
        let occupied_before = term.grid().total_lines();
        term.resize(TermSize {
            cols: 20,
            lines: 6,
        });
        let after = dump(&term);
        let occupied_after = term.grid().total_lines();

        eprintln!(
            "S1: reflow probe alt={alt} ALT_SCREEN={}",
            term.mode().contains(TermMode::ALT_SCREEN)
        );
        eprintln!("S1:   before(40 cols): {before:?}");
        eprintln!("S1:   after (20 cols): {after:?}");
        // If content REWRAPPED, the 60-char logical lines now need 3 rows each
        // instead of 2, so the occupied row count grows and the viewport scrolls
        // (the "A" block falls out of view). If reflow is suppressed, each row is
        // truncated in place and the same logical rows stay put.
        eprintln!(
            "S1:   occupied rows {occupied_before} -> {occupied_after}; \
             first visible row still 'A' = {}",
            after[0].starts_with('A')
        );
        eprintln!(
            "S1:   REFLOWED = {}",
            occupied_after > occupied_before || !after[0].starts_with('A')
        );
    }
}

fn dump<T: EventListener>(term: &Term<T>) -> Vec<String> {
    let grid = term.grid();
    let mut rows = vec![String::new(); grid.screen_lines()];
    for cell in grid.display_iter() {
        let l = cell.point.line.0;
        if l >= 0 && (l as usize) < rows.len() {
            rows[l as usize].push(cell.c);
        }
    }
    rows
}

// ---------------------------------------------------------------- app state

struct Spike {
    term: Arc<FairMutex<Term<Listener>>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    focus: FocusHandle,
    blink_on: bool,
    scroll_accum: f32,
    size: TermSize,
    notifies: u64,
    dragging: bool,
    _tasks: Vec<gpui::Task<()>>,
}

impl Focusable for Spike {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Spike {
    fn new(cx: &mut Context<Self>) -> Self {
        let size = TermSize {
            cols: 120,
            lines: 34,
        };
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(portable_pty::PtySize {
                rows: size.lines as u16,
                cols: size.cols as u16,
                pixel_width: (size.cols as f32 * CELL_W) as u16,
                pixel_height: (size.lines as f32 * CELL_H) as u16,
            })
            .expect("openpty");
        let mut cmd = portable_pty::CommandBuilder::new("tmux");
        cmd.args(["new-session", "-A", "-s", "grove-spike"]);
        cmd.env("TERM", "xterm-256color");
        let _child = pair.slave.spawn_command(cmd).expect("spawn tmux");
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");

        let (ev_tx, _ev_rx) = futures::channel::mpsc::unbounded();
        let cfg = AlacConfig {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(cfg, &size, Listener(ev_tx))));

        // PTY reader thread -> unbounded channel -> foreground task.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.unbounded_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let term_for_task = term.clone();
        let pump = cx.spawn(async move |this: gpui::WeakEntity<Spike>, cx| {
            use futures::StreamExt as _;
            let mut proc = Processor::<StdSyncHandler>::new();
            while let Some(chunk) = rx.next().await {
                let changed = {
                    let mut t = term_for_task.lock();
                    proc.advance(&mut *t, &chunk);
                    let dirty = match t.damage() {
                        TermDamage::Full => true,
                        TermDamage::Partial(mut it) => it.next().is_some(),
                    };
                    t.reset_damage();
                    dirty
                };
                if changed
                    && this
                        .update(cx, |this: &mut Spike, cx| {
                            this.notifies += 1;
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        });

        // Cursor blink at 533ms. SPIKE_NO_BLINK=1 isolates blink cost from the
        // damage-driven repaint cost when measuring idle CPU.
        let no_blink = std::env::var("SPIKE_NO_BLINK").is_ok();
        let blink = cx.spawn(async move |this: gpui::WeakEntity<Spike>, cx| loop {
            if no_blink {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(BLINK_MS))
                .await;
            if this
                .update(cx, |this: &mut Spike, cx| {
                    this.blink_on = !this.blink_on;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        });

        Self {
            term,
            writer,
            master: pair.master,
            focus: cx.focus_handle(),
            blink_on: true,
            scroll_accum: 0.0,
            size,
            notifies: 0,
            dragging: false,
            _tasks: vec![pump, blink],
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    fn resize(&mut self, cols: usize, lines: usize) {
        if cols == self.size.cols && lines == self.size.lines {
            return;
        }
        self.size = TermSize { cols, lines };
        let _ = self.master.resize(portable_pty::PtySize {
            rows: lines as u16,
            cols: cols as u16,
            pixel_width: (cols as f32 * CELL_W) as u16,
            pixel_height: (lines as f32 * CELL_H) as u16,
        });
        self.term.lock().resize(self.size);
    }

    // ------------------------------------------------------------ input

    /// Keystroke -> PTY bytes. Table shape cribbed from src/gui/keys.rs.
    fn key_bytes(ev: &KeyDownEvent) -> Option<Vec<u8>> {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        let app = |s: &str| Some(s.as_bytes().to_vec());
        // CSI-modifier parameter: 1 + (shift=1, alt=2, ctrl=4)
        let modp = 1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.control as u8);
        let csi = |fin: char| {
            if modp > 1 {
                Some(format!("\x1b[1;{modp}{fin}").into_bytes())
            } else {
                Some(format!("\x1b[{fin}").into_bytes())
            }
        };
        match k.key.as_str() {
            "enter" => return app("\r"),
            "tab" => return app(if m.shift { "\x1b[Z" } else { "\t" }),
            "backspace" => return app("\x7f"),
            "escape" => return app("\x1b"),
            "delete" => return app("\x1b[3~"),
            "home" => return app("\x1b[H"),
            "end" => return app("\x1b[F"),
            "pageup" => return app("\x1b[5~"),
            "pagedown" => return app("\x1b[6~"),
            "up" => return csi('A'),
            "down" => return csi('B'),
            "right" => return csi('C'),
            "left" => return csi('D'),
            "space" if m.control => return app("\x00"),
            _ => {}
        }
        if m.control && k.key.len() == 1 {
            let c = k.key.as_bytes()[0].to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(vec![c - b'a' + 1]);
            }
            return match c {
                b'[' => app("\x1b"),
                b'\\' => app("\x1c"),
                b']' => app("\x1d"),
                _ => None,
            };
        }
        let text = k.key_char.clone().unwrap_or_else(|| k.key.clone());
        if text.is_empty() || m.platform || text.chars().count() > 1 {
            return None;
        }
        if m.alt {
            let mut v = vec![0x1b];
            v.extend_from_slice(text.as_bytes());
            return Some(v);
        }
        Some(text.into_bytes())
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = Self::key_bytes(ev) {
            self.write(&b);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let lines: i32 = match ev.delta {
            ScrollDelta::Pixels(p) => {
                eprintln!("S1: ScrollDelta::Pixels(x={:?}, y={:?})", p.x, p.y);
                self.scroll_accum += f32::from(p.y);
                let n = (self.scroll_accum / SCROLL_PX_PER_LINE).trunc();
                self.scroll_accum -= n * SCROLL_PX_PER_LINE;
                n as i32
            }
            ScrollDelta::Lines(p) => {
                eprintln!("S1: ScrollDelta::Lines(x={}, y={})", p.x, p.y);
                self.scroll_accum = 0.0;
                p.y as i32
            }
        };
        if lines == 0 {
            return;
        }
        let alt = self.term.lock().mode().contains(TermMode::ALT_SCREEN);
        if alt {
            let seq: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let mut out = Vec::new();
            for _ in 0..lines.abs() {
                out.extend_from_slice(seq);
            }
            self.write(&out);
        } else {
            self.term
                .lock()
                .scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
        }
        cx.notify();
    }

    fn cell_at(&self, pos: Point<Pixels>) -> (usize, usize) {
        let col = (f32::from(pos.x) / CELL_W).max(0.0) as usize;
        let row = (f32::from(pos.y) / CELL_H).max(0.0) as usize;
        (
            col.min(self.size.cols.saturating_sub(1)),
            row.min(self.size.lines.saturating_sub(1)),
        )
    }

    /// SGR (1006) mouse report.
    fn sgr(&mut self, btn: u8, col: usize, row: usize, press: bool) {
        if !self.term.lock().mode().contains(TermMode::SGR_MOUSE) {
            return;
        }
        let s = format!(
            "\x1b[<{};{};{}{}",
            btn,
            col + 1,
            row + 1,
            if press { 'M' } else { 'm' }
        );
        self.write(s.as_bytes());
    }
}

// ---------------------------------------------------------------- element

struct RunPiece {
    line: gpui::ShapedLine,
    origin: Point<Pixels>,
}

struct PrepaintState {
    quads: Vec<gpui::PaintQuad>,
    runs: Vec<RunPiece>,
    cursor: Option<gpui::PaintQuad>,
}

struct TerminalElement {
    state: Entity<Spike>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _rl: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        // Keep the PTY sized to the element.
        let want_cols = (f32::from(bounds.size.width) / CELL_W).floor().max(1.0) as usize;
        let want_lines = (f32::from(bounds.size.height) / CELL_H).floor().max(1.0) as usize;
        self.state
            .update(cx, |s, _| s.resize(want_cols, want_lines));

        let (blink_on, term) = self.state.read_with(cx, |s, _| (s.blink_on, s.term.clone()));
        let term = term.lock();
        let grid = term.grid();
        let rows = grid.screen_lines();
        let cols = grid.columns();

        let regular = gpui::font(FONT_FAMILY);
        let bold = gpui::font(FONT_FAMILY).bold();

        // Gather cells row-major: (char, fg, bg, bold).
        let mut cells: Vec<Vec<(char, Hsla, Option<Hsla>, bool)>> =
            vec![vec![(' ', rgb8(T_FG), None, false); cols]; rows];
        for c in grid.display_iter() {
            let l = c.point.line.0;
            if l < 0 || l as usize >= rows {
                continue;
            }
            let col = c.point.column.0;
            if col >= cols {
                continue;
            }
            let flags = c.cell.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let bolded = flags.contains(Flags::BOLD) || flags.contains(Flags::BOLD_ITALIC);
            let mut fg = conv_fg(c.cell.fg);
            let mut bg = conv_bg(c.cell.bg);
            if flags.contains(Flags::INVERSE) {
                let old = bg.unwrap_or(rgb8(T_BG));
                bg = Some(fg);
                fg = old;
            }
            cells[l as usize][col] = (c.cell.c, fg, bg, bolded);
        }

        let mut quads = Vec::new();
        let mut runs = Vec::new();

        for (r, row) in cells.iter().enumerate() {
            let y = bounds.origin.y + px(r as f32 * CELL_H);

            // Merged background quads: coalesce adjacent equal bg.
            let mut c0 = 0usize;
            while c0 < cols {
                let bg = row[c0].2;
                let mut c1 = c0 + 1;
                while c1 < cols && row[c1].2 == bg {
                    c1 += 1;
                }
                if let Some(bg) = bg {
                    quads.push(fill(
                        Bounds::new(
                            point(bounds.origin.x + px(c0 as f32 * CELL_W), y),
                            size(px((c1 - c0) as f32 * CELL_W), px(CELL_H)),
                        ),
                        bg,
                    ));
                }
                c0 = c1;
            }

            // Text runs: coalesce adjacent cells with equal (fg, bold), skipping blanks.
            // Each run is painted at its own fixed CELL_W column origin, so a
            // width mismatch inside a run cannot drift into the next run.
            let mut c0 = 0usize;
            while c0 < cols {
                if row[c0].0 == ' ' {
                    c0 += 1;
                    continue;
                }
                let (fg, bold_f) = (row[c0].1, row[c0].3);
                let mut text = String::new();
                let mut c1 = c0;
                while c1 < cols && row[c1].0 != ' ' && row[c1].1 == fg && row[c1].3 == bold_f {
                    text.push(row[c1].0);
                    c1 += 1;
                }
                let run = TextRun {
                    len: text.len(),
                    font: if bold_f { bold.clone() } else { regular.clone() },
                    color: fg,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text),
                    px(FONT_PT),
                    &[run],
                    None,
                );
                runs.push(RunPiece {
                    line: shaped,
                    origin: point(bounds.origin.x + px(c0 as f32 * CELL_W), y),
                });
                c0 = c1;
            }
        }

        // Block cursor.
        let cursor = if blink_on && term.mode().contains(TermMode::SHOW_CURSOR) {
            let cp = grid.cursor.point;
            let vis = cp.line.0 + grid.display_offset() as i32;
            if vis >= 0 && (vis as usize) < rows {
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x + px(cp.column.0 as f32 * CELL_W),
                            bounds.origin.y + px(vis as f32 * CELL_H),
                        ),
                        size(px(CELL_W), px(CELL_H)),
                    ),
                    rgb8(T_FG),
                ))
            } else {
                None
            }
        } else {
            None
        };

        PrepaintState {
            quads,
            runs,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _rl: &mut (),
        pre: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_quad(fill(bounds, rgb8(T_BG)));
        for q in pre.quads.drain(..) {
            window.paint_quad(q);
        }
        for r in pre.runs.iter() {
            let _ = r
                .line
                .paint(r.origin, px(CELL_H), TextAlign::Left, None, window, cx);
        }
        if let Some(c) = pre.cursor.take() {
            window.paint_quad(c);
        }
    }
}

// ---------------------------------------------------------------- render

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .bg(rgb8(T_BG))
            .text_color(rgb8(T_FG))
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    let (c, r) = this.cell_at(ev.position);
                    this.dragging = true;
                    this.sgr(0, c, r, true);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, _cx| {
                if this.dragging {
                    let (c, r) = this.cell_at(ev.position);
                    this.sgr(32, c, r, true);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, _cx| {
                    let (c, r) = this.cell_at(ev.position);
                    this.dragging = false;
                    this.sgr(0, c, r, false);
                }),
            )
            .child(TerminalElement {
                state: cx.entity(),
            })
    }
}

// ---------------------------------------------------------------- main

fn measure(window: &mut Window, family: &str) {
    let mk = |s: &str| TextRun {
        len: s.len(),
        font: gpui::font(family),
        color: hsla(0., 0., 1., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    for pt in [11.0f32, 11.5, 12.0, 12.5, 13.0, 13.5, 14.0] {
        let line = window
            .text_system()
            .shape_line(SharedString::from("M"), px(pt), &[mk("M")], None);
        eprintln!("S1: advance \"M\" @ {pt}pt = {:?}", line.width());
    }
    let line = window.text_system().shape_line(
        SharedString::from("MMMMMMMMMM"),
        px(FONT_PT),
        &[mk("MMMMMMMMMM")],
        None,
    );
    eprintln!(
        "S1: 10x M @ {FONT_PT}pt = {:?} (per cell {})",
        line.width(),
        f32::from(line.width()) / 10.0
    );
    for s in ["漢字", "\u{f07b}\u{e0b0}", "→│", "█▀"] {
        let line = window
            .text_system()
            .shape_line(SharedString::from(s), px(FONT_PT), &[mk(s)], None);
        eprintln!(
            "S1: shape {s:?} width={:?} expected_cells_x_7.5={}",
            line.width(),
            f32::from(line.width()) / CELL_W
        );
    }
    eprintln!("S1: window.line_height() = {:?}", window.line_height());
    eprintln!("S1: window.rem_size() = {:?}", window.rem_size());
}

fn main() {
    reflow_probe();

    let assets = Assets::new();
    eprintln!("S1: asset root = {}", assets.root.display());

    application().with_assets(Assets::new()).run(|cx: &mut App| {
        let a = Assets::new();
        let mut fonts = Vec::new();
        for f in [
            "fonts/BlexMonoNerdFontMono-Regular.ttf",
            "fonts/BlexMonoNerdFontMono-Bold.ttf",
        ] {
            match a.load(f) {
                Ok(Some(b)) => fonts.push(b),
                other => eprintln!("S1: FONT MISSING {f}: {:?}", other.is_err()),
            }
        }
        let n = fonts.len();
        match cx.text_system().add_fonts(fonts) {
            Ok(()) => eprintln!("S1: add_fonts OK ({n} files)"),
            Err(e) => eprintln!("S1: add_fonts FAILED: {e}"),
        }
        eprintln!(
            "S1: all_font_names contains {FONT_FAMILY:?} = {}",
            cx.text_system()
                .all_font_names()
                .iter()
                .any(|n| n == FONT_FAMILY)
        );

        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    measure(window, FONT_FAMILY);
                    cx.new(|cx| Spike::new(cx))
                },
            )
            .unwrap();
        window
            .update(cx, |view, window, cx| {
                let fh = view.focus.clone();
                window.focus(&fh, cx);
            })
            .unwrap();
        cx.activate(true);
    });
}
