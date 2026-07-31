//! Spike S3 — two-scope rem zoom.
//!
//! Throwaway measurement code. Run with `cargo run -p spike-zoom`.
//! Everything interesting is eprintln'd with an `S3:` prefix.
//!
//! Chrome (fake sidebar) is styled with `rems()` units and scales via
//! `Window::set_rem_size`. The terminal element's cell metrics are scaled
//! by multiplying the base CELL_W/CELL_H/FONT_PT by the same zoom factor —
//! this mirrors src/gui/metrics.rs's `cell_w: CELL_W * zoom` treatment
//! (copied from spike-term, S1's terminal element).

use std::borrow::Cow;
use std::io::{Read, Write};
use std::sync::Arc;

use alacritty_terminal::event::{Event as AlacTermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as AlacConfig, Term, TermDamage};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use gpui::{
    div, fill, point, prelude::*, px, relative, rems, size, App, AssetSource, Bounds, Context,
    ElementId, Entity, FocusHandle, Focusable, GlobalElementId, Hsla, KeyDownEvent, LayoutId,
    Pixels, Point, SharedString, Style, TextAlign, TextRun, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;

// ---------------------------------------------------------------- constants (base, zoom=1.0)

const BASE_FONT_PT: f32 = 12.5;
const BASE_CELL_W: f32 = 7.5;
const BASE_CELL_H: f32 = 17.0;
const BASE_REM_PX: f32 = 16.0; // gpui's default Window::rem_size()
const FONT_FAMILY: &str = "BlexMono Nerd Font Mono";
const SCROLLBACK: usize = 5000;

// ---------------------------------------------------------------- oracle constants
// Copied verbatim from src/gui/metrics.rs (the iced app). Do NOT import —
// spikes share by copy, not by crate dep.
const SIDEBAR_MIN_W: f32 = 220.0;
const APPBAR_H: f32 = 44.0;
const STATUS_H: f32 = 26.0;
const SESSBAR_H: f32 = 36.0;
const SIDEBAR_DIVIDER_W: f32 = 6.0;
const PTY_PAD_W: f32 = 36.0;
const PTY_PAD_H: f32 = 28.0;
const ORACLE_CELL_W: f32 = 7.5;
const ORACLE_CELL_H: f32 = 17.0;

/// Verbatim port of `compute_pty_dims` from src/gui/metrics.rs:265-295.
/// iced applies `zoom` as its application scale factor, which shrinks the
/// *logical* viewport: `logical_w = win_w / zoom`.
fn oracle_compute_pty_dims(win_w: f32, win_h: f32, zoom: f32, sidebar_w: f32) -> (u16, u16) {
    let zoom = zoom.max(0.1);
    let logical_w = win_w / zoom;
    let logical_h = win_h / zoom;
    let visible_w = sidebar_w + SIDEBAR_DIVIDER_W;
    let visible_h = APPBAR_H + STATUS_H;
    let usable_w = logical_w - (visible_w + PTY_PAD_W);
    let usable_h = logical_h - (visible_h + SESSBAR_H + PTY_PAD_H);
    let cols = (usable_w / ORACLE_CELL_W).max(10.0) as u16;
    let rows = (usable_h / ORACLE_CELL_H).max(4.0) as u16;
    (rows, cols)
}

/// The gpui-side formula: instead of shrinking the logical viewport, gpui's
/// rem-based zoom *grows* every chrome/cell dimension in physical pixels
/// (rem_size = BASE_REM_PX * zoom, and cell metrics are multiplied by zoom
/// directly). The window's physical size stays fixed. Algebraically this is
/// the same relation as the oracle's divide-by-zoom: since every term in the
/// oracle's `usable_w` numerator and the `CELL_W` denominator both get
/// multiplied by `zoom` here, dividing cancels zoom out identically:
///
///   oracle: cols = (win_w/zoom - chrome) / CELL_W
///   gpui:   cols = (win_w - chrome*zoom) / (CELL_W*zoom)
///         = (win_w/zoom - chrome) / CELL_W        [divide num & denom by zoom]
///
/// so the two formulas MUST agree exactly (mod float rounding / the `as u16`
/// truncation, which happens at different points in the arithmetic).
fn gpui_compute_pty_dims(win_w: f32, win_h: f32, zoom: f32, sidebar_w: f32) -> (u16, u16) {
    let zoom = zoom.max(0.1);
    let cell_w_z = ORACLE_CELL_W * zoom;
    let cell_h_z = ORACLE_CELL_H * zoom;
    let visible_w = (sidebar_w + SIDEBAR_DIVIDER_W) * zoom;
    let visible_h = (APPBAR_H + STATUS_H) * zoom;
    let usable_w = win_w - (visible_w + PTY_PAD_W * zoom);
    let usable_h = win_h - (visible_h + (SESSBAR_H + PTY_PAD_H) * zoom);
    let cols = (usable_w / cell_w_z).max(10.0) as u16;
    let rows = (usable_h / cell_h_z).max(4.0) as u16;
    (rows, cols)
}

fn oracle_comparison() {
    eprintln!("S3: ---- oracle comparison (logical window sizes, sidebar={SIDEBAR_MIN_W}) ----");
    for (win_w, win_h) in [(1280.0f32, 800.0f32)] {
        for zoom in [1.0f32, 1.4, 2.0] {
            let oracle = oracle_compute_pty_dims(win_w, win_h, zoom, SIDEBAR_MIN_W);
            let gpui_d = gpui_compute_pty_dims(win_w, win_h, zoom, SIDEBAR_MIN_W);
            let pass = oracle == gpui_d;
            eprintln!(
                "S3: win={win_w}x{win_h} zoom={zoom} oracle(rows,cols)={oracle:?} gpui(rows,cols)={gpui_d:?} {}",
                if pass { "PASS" } else { "FAIL" }
            );
        }
    }
}

fn rgb8(c: (u8, u8, u8)) -> Hsla {
    gpui::rgb(((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32).into()
}

const T_BG: (u8, u8, u8) = (0x1a, 0x1b, 0x26);
const T_FG: (u8, u8, u8) = (0xc0, 0xca, 0xf5);
const T_BLUE: (u8, u8, u8) = (0x7a, 0xa2, 0xf7);

// ---------------------------------------------------------------- assets

struct Assets {
    root: std::path::PathBuf,
}

impl Assets {
    fn new() -> Self {
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

// Zoom levels to iterate for the automated step-through: 0.6 -> 2.0 by 0.1.
fn zoom_steps() -> Vec<f32> {
    let mut v = Vec::new();
    let mut z = 0.6f32;
    while z <= 2.0001 {
        v.push((z * 100.0).round() / 100.0);
        z += 0.1;
    }
    v
}

// The four "MANUAL: user to verify" crispness checkpoints.
const CRISP_CHECKPOINTS: [f32; 4] = [0.6, 1.0, 1.37, 2.0];

// ---------------------------------------------------------------- app state

struct Spike {
    term: Arc<FairMutex<Term<Listener>>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    focus: FocusHandle,
    size: TermSize,
    zoom: f32,
    step_idx: usize,
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
                pixel_width: (size.cols as f32 * BASE_CELL_W) as u16,
                pixel_height: (size.lines as f32 * BASE_CELL_H) as u16,
            })
            .expect("openpty");
        let mut cmd = portable_pty::CommandBuilder::new("bash");
        cmd.env("TERM", "xterm-256color");
        let _child = pair.slave.spawn_command(cmd).expect("spawn bash");
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");

        let (ev_tx, _ev_rx) = futures::channel::mpsc::unbounded();
        let cfg = AlacConfig {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(cfg, &size, Listener(ev_tx))));

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
                        .update(cx, |_this: &mut Spike, cx| {
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        });

        Self {
            term,
            writer,
            master: pair.master,
            focus: cx.focus_handle(),
            size,
            zoom: 1.0,
            step_idx: 0,
            _tasks: vec![pump],
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
        let cw = BASE_CELL_W * self.zoom;
        let ch = BASE_CELL_H * self.zoom;
        let _ = self.master.resize(portable_pty::PtySize {
            rows: lines as u16,
            cols: cols as u16,
            pixel_width: (cols as f32 * cw) as u16,
            pixel_height: (lines as f32 * ch) as u16,
        });
        self.term.lock().resize(self.size);
    }

    /// Step zoom by `delta`, clamped to [0.6, 2.0], and log the effective
    /// rem size + resulting cell size.
    fn step_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let new_zoom = (self.zoom + delta).clamp(0.6, 2.0);
        self.set_zoom(new_zoom, window, cx);
    }

    fn set_zoom(&mut self, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom = zoom;
        let rem = BASE_REM_PX * zoom;
        window.set_rem_size(px(rem));
        let cw = BASE_CELL_W * zoom;
        let ch = BASE_CELL_H * zoom;
        eprintln!(
            "S3: zoom={:.2} rem_size={:.3}px cell=({:.3},{:.3})px font_pt={:.3}",
            zoom,
            rem,
            cw,
            ch,
            BASE_FONT_PT * zoom
        );
        if CRISP_CHECKPOINTS.iter().any(|c| (c - zoom).abs() < 0.001) {
            eprintln!("S3: MANUAL: user to verify crispness at zoom={zoom:.2}");
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        // Chrome-level zoom keys, gated on ctrl+shift so they never collide
        // with anything sent to the pty.
        if m.control && m.shift {
            match k.key.as_str() {
                "=" | "+" => return self.step_zoom(0.1, window, cx),
                "-" => return self.step_zoom(-0.1, window, cx),
                "1" => return self.set_zoom(CRISP_CHECKPOINTS[0], window, cx),
                "2" => return self.set_zoom(CRISP_CHECKPOINTS[1], window, cx),
                "3" => return self.set_zoom(CRISP_CHECKPOINTS[2], window, cx),
                "4" => return self.set_zoom(CRISP_CHECKPOINTS[3], window, cx),
                "n" => {
                    // Advance through the full 0.6->2.0 step table (automated demo).
                    let steps = zoom_steps();
                    self.step_idx = (self.step_idx + 1).min(steps.len() - 1);
                    return self.set_zoom(steps[self.step_idx], window, cx);
                }
                _ => {}
            }
        }
        let text = k.key_char.clone().unwrap_or_else(|| k.key.clone());
        if !text.is_empty() && !m.platform && text.chars().count() == 1 {
            self.write(text.as_bytes());
            cx.notify();
        } else if k.key == "enter" {
            self.write(b"\r");
            cx.notify();
        }
    }
}

// ---------------------------------------------------------------- terminal element

struct RunPiece {
    line: gpui::ShapedLine,
    origin: Point<Pixels>,
}

struct PrepaintState {
    quads: Vec<gpui::PaintQuad>,
    runs: Vec<RunPiece>,
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
        let zoom = self.state.read(cx).zoom;
        let cell_w = BASE_CELL_W * zoom;
        let cell_h = BASE_CELL_H * zoom;
        let font_pt = BASE_FONT_PT * zoom;

        let want_cols = (f32::from(bounds.size.width) / cell_w).floor().max(1.0) as usize;
        let want_lines = (f32::from(bounds.size.height) / cell_h).floor().max(1.0) as usize;
        self.state
            .update(cx, |s, _| s.resize(want_cols, want_lines));

        let term = self.state.read_with(cx, |s, _| s.term.clone());
        let term = term.lock();
        let grid = term.grid();
        let rows = grid.screen_lines();
        let cols = grid.columns();

        let regular = gpui::font(FONT_FAMILY);
        let bold = gpui::font(FONT_FAMILY).bold();

        let mut cells: Vec<Vec<(char, Hsla, bool)>> =
            vec![vec![(' ', rgb8(T_FG), false); cols]; rows];
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
            cells[l as usize][col] = (c.cell.c, rgb8(T_FG), bolded);
        }

        let mut runs = Vec::new();
        for (r, row) in cells.iter().enumerate() {
            let y = bounds.origin.y + px(r as f32 * cell_h);
            let mut c0 = 0usize;
            while c0 < cols {
                if row[c0].0 == ' ' {
                    c0 += 1;
                    continue;
                }
                let (fg, bold_f) = (row[c0].1, row[c0].2);
                let mut text = String::new();
                let mut c1 = c0;
                while c1 < cols && row[c1].0 != ' ' && row[c1].2 == bold_f {
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
                    px(font_pt),
                    &[run],
                    None,
                );
                runs.push(RunPiece {
                    line: shaped,
                    origin: point(bounds.origin.x + px(c0 as f32 * cell_w), y),
                });
                c0 = c1;
            }
        }

        PrepaintState {
            quads: vec![fill(bounds, rgb8(T_BG))],
            runs,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _rl: &mut (),
        pre: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cell_h = BASE_CELL_H * self.state.read(cx).zoom;
        for q in pre.quads.drain(..) {
            window.paint_quad(q);
        }
        for r in pre.runs.iter() {
            let _ = r
                .line
                .paint(r.origin, px(cell_h), TextAlign::Left, None, window, cx);
        }
    }
}

// ---------------------------------------------------------------- render

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Fake chrome: a sidebar styled entirely in rems, so it scales with
        // Window::set_rem_size the same way the real app's iced sidebar
        // scales with the iced app-level `ui_zoom` scale factor.
        let sidebar = div()
            .w(rems(SIDEBAR_MIN_W / BASE_REM_PX))
            .h_full()
            .bg(rgb8(T_BLUE))
            .p(rems(0.5))
            .text_color(rgb8(T_BG))
            .child(SharedString::from(format!("zoom {:.2}", self.zoom)));

        div()
            .track_focus(&self.focus)
            .key_context("ZoomSpike")
            .size_full()
            .flex()
            .bg(rgb8(T_BG))
            .text_color(rgb8(T_FG))
            .on_key_down(cx.listener(Self::on_key))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .child(TerminalElement {
                        state: cx.entity(),
                    }),
            )
    }
}

// ---------------------------------------------------------------- main

fn main() {
    oracle_comparison();

    let assets = Assets::new();
    eprintln!("S3: asset root = {}", assets.root.display());

    application().with_assets(Assets::new()).run(|cx: &mut App| {
        let a = Assets::new();
        let mut fonts = Vec::new();
        for f in [
            "fonts/BlexMonoNerdFontMono-Regular.ttf",
            "fonts/BlexMonoNerdFontMono-Bold.ttf",
        ] {
            match a.load(f) {
                Ok(Some(b)) => fonts.push(b),
                other => eprintln!("S3: FONT MISSING {f}: {:?}", other.is_err()),
            }
        }
        let n = fonts.len();
        match cx.text_system().add_fonts(fonts) {
            Ok(()) => eprintln!("S3: add_fonts OK ({n} files)"),
            Err(e) => eprintln!("S3: add_fonts FAILED: {e}"),
        }

        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_window, cx| cx.new(|cx| Spike::new(cx)),
            )
            .unwrap();
        window
            .update(cx, |view, window, cx| {
                let fh = view.focus.clone();
                window.focus(&fh, cx);
                // Log the full 0.6 -> 2.0 step table once at startup, driving
                // the rem size and cell metrics through every step so the
                // per-step log (Step 1's requirement) is captured even
                // without manual key presses.
                for z in zoom_steps() {
                    view.set_zoom(z, window, cx);
                }
                view.set_zoom(1.0, window, cx);
            })
            .unwrap();
        cx.activate(true);
    });
}
