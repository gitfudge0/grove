//! One live terminal: a PTY plus the `grove_terminal::GroveTerm` model that
//! parses it.
//!
//! This is **grove-gpui's own session type**, deliberately not
//! `grove_core::session::Session` (Plan 04 Global Constraint 3): grove-core
//! stays vt100-backed and untouched as the golden oracle until Plan 10. What is
//! reused from grove-core is only the genuinely UI-free machinery —
//! `tmux::{available, next_free_n, make_name, new_session,
//! configure_embedded_session, pane_pid, kill_session, SOCKET}`,
//! `session_meta::write`, `agent::Agent::program` — so the tmux command
//! construction cannot drift between the two front ends.
//!
//! Plan 05 replaces the hardcoded single spawn here with a session registry.

// The full readout surface is ported in one go so Tasks 3-5 and Plan 05 are
// mechanical; several accessors have no caller until their consumer lands.
#![allow(dead_code)]

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt as _;
use gpui::{Context, Task};
use grove_core::agent::Agent;
use grove_core::session_meta::{self, SessionMeta};
use grove_core::tmux;
use grove_terminal::{GroveTerm, MouseEncoding, MouseMode, PtyHandle, Snapshot};

use crate::terminal::keys;
use crate::terminal::mouse::{self, AbsCell};
use portable_pty::CommandBuilder;

/// Initial PTY size before the element's first `prepaint` reports real bounds
/// (`crates/grove-core/src/session.rs:53-54`).
const INIT_ROWS: u16 = 24;
const INIT_COLS: u16 = 80;

/// Where the hardcoded Plan 04 session opens. Overridable so the manual
/// checklist can point the terminal at any tree.
const CWD_ENV: &str = "GROVE_GPUI_SESSION_CWD";

/// Which kind of PTY is on the other end. Scroll and copy-mode behavior differ
/// (`session.rs:667-705`): tmux keeps its scrollback in copy-mode on the
/// alternate screen, so grove's own scrollback is empty for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Tmux { name: String },
    Native,
}

pub struct TerminalSession {
    term: GroveTerm,
    /// `None` only when no PTY could be spawned at all — the grid still
    /// renders (empty) rather than taking the window down.
    pty: Option<PtyHandle>,
    backend: Backend,
    rows: u16,
    cols: u16,
    last_damage_gen: u64,
    /// Whether tmux is currently parked in copy-mode because the user scrolled
    /// up (`session.rs:617-622`).
    tmux_copy_mode: bool,
    /// Attention plumbing reads these in Plan 06; they are stamped here so the
    /// semantics match `session.rs:604-605,642` from the start.
    last_input_at: Option<Instant>,
    last_scroll_at: Option<Instant>,
    /// Dropping the `Task` stops the reader, so this field *is* the reader.
    _reader: Task<()>,
}

impl TerminalSession {
    /// Spawn the phase's single session: a tmux-backed login shell when tmux is
    /// available, otherwise a plain PTY so the element still renders on a box
    /// without tmux.
    pub fn spawn(cx: &mut Context<Self>) -> Self {
        let cwd = session_cwd();
        let spawned = match spawn_tmux(&cwd) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("grove-gpui: tmux unavailable ({e}); falling back to a native PTY");
                match spawn_native(&cwd) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::error!("grove-gpui: could not spawn a PTY: {e}");
                        None
                    }
                }
            }
        };
        let (mut pty, backend) = match spawned {
            Some((pty, backend)) => (Some(pty), backend),
            None => (None, Backend::Native),
        };
        let rx = pty.as_mut().and_then(PtyHandle::take_receiver);
        Self {
            term: GroveTerm::new(INIT_ROWS, INIT_COLS),
            pty,
            backend,
            rows: INIT_ROWS,
            cols: INIT_COLS,
            last_damage_gen: 0,
            tmux_copy_mode: false,
            last_input_at: None,
            last_scroll_at: None,
            _reader: Self::spawn_reader(rx, cx),
        }
    }

    /// Bridge the PTY's blocking `std::sync::mpsc` channel onto gpui's
    /// foreground executor — findings §S1 Step 5, and the whole reason
    /// `PtyHandle::take_receiver` exists.
    ///
    /// A dedicated thread *blocks* on `recv()`, so a silent PTY costs **zero
    /// wakeups**: no timer, no poll, nothing on the data path until bytes
    /// actually arrive. Chunks cross to the foreground through a
    /// `futures::channel::mpsc::unbounded`, where they are processed and — only
    /// if the grid actually moved — repainted.
    ///
    /// A plain `std::thread` rather than `background_executor().spawn`: the
    /// loop blocks for the session's whole life, and parking one executor pool
    /// thread per session would starve the pool once Plan 05 makes sessions
    /// plural. The thread ends on EOF (or when the receiver is dropped).
    fn spawn_reader(rx: Option<Receiver<Vec<u8>>>, cx: &mut Context<Self>) -> Task<()> {
        let Some(rx) = rx else {
            return Task::ready(());
        };
        let (tx, mut chunks) = mpsc::unbounded::<Vec<u8>>();
        std::thread::Builder::new()
            .name("grove-gpui-pty-bridge".into())
            .spawn(move || {
                // `Err` means EOF (the PTY closed) or the UI went away.
                while let Ok(chunk) = rx.recv() {
                    if tx.unbounded_send(chunk).is_err() {
                        break;
                    }
                }
            })
            // A failed thread spawn leaves the grid static rather than dead;
            // there is nothing to retry against.
            .map_or_else(
                |e| {
                    tracing::error!("grove-gpui: PTY bridge thread: {e}");
                    Task::ready(())
                },
                |_| {
                    cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                        while let Some(chunk) = chunks.next().await {
                            // Coalesce everything already queued into the same
                            // frame: a burst of output must cost one repaint,
                            // not one per 8 KiB chunk.
                            let mut batch = vec![chunk];
                            while let Ok(more) = chunks.try_recv() {
                                batch.push(more);
                            }
                            if this
                                .update(cx, |this: &mut Self, cx| this.ingest(&batch, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                },
            )
    }

    /// Feed chunks into the model and repaint **only** if the grid actually
    /// moved. `GroveTerm::process` already folds alacritty's `TermDamage` into a
    /// generation counter, so this comparison is the entire damage gate
    /// (findings §S1 Step 5).
    fn ingest(&mut self, chunks: &[Vec<u8>], cx: &mut Context<Self>) {
        for chunk in chunks {
            self.term.process(chunk);
        }
        let generation = self.term.damage_generation();
        if generation != self.last_damage_gen {
            self.last_damage_gen = generation;
            cx.notify();
        }
    }

    // ── input ────────────────────────────────────────────────────────────

    /// Port of `session.rs:604-625`. The order is load-bearing: snapping the
    /// view back to the live screen and leaving tmux copy-mode must both happen
    /// *before* the bytes go out, or the keystroke is eaten as a copy-mode
    /// command instead of reaching the agent.
    pub fn send(&mut self, bytes: &[u8]) {
        self.last_input_at = Some(Instant::now());
        // Typing snaps the view back to the live screen, like a real terminal.
        self.term.scroll_to(0);
        // Tmux renders on the alternate screen, so its scrollback lives in
        // copy-mode rather than grove's own buffer. Leave copy-mode exactly
        // once per scroll-back excursion.
        if self.tmux_copy_mode {
            if let Backend::Tmux { name } = &self.backend {
                tmux::cancel_copy_mode(name);
            }
            self.tmux_copy_mode = false;
        }
        if let Some(pty) = self.pty.as_mut() {
            if let Err(e) = pty.write(bytes) {
                tracing::debug!("grove-gpui: PTY write failed: {e}");
            }
        }
    }

    /// Port of `session.rs:940-967`. Both resizes are required: `GroveTerm`
    /// reflows the model, `PtyHandle::resize` is the `TIOCSWINSZ` that makes
    /// the inner app learn its new size (SIGWINCH).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        // A real size change makes the inner app repaint; discount that burst
        // so Plan 06's attention classifier doesn't read it as agent output.
        self.last_input_at = Some(Instant::now());
        self.rows = rows;
        self.cols = cols;
        // Snap to live before reflowing: a scrolled-back offset is meaningless
        // against the new geometry.
        self.term.scroll_to(0);
        self.term.resize(rows, cols);
        if let Some(pty) = self.pty.as_ref() {
            if let Err(e) = pty.resize(rows, cols) {
                tracing::debug!("grove-gpui: PTY resize failed: {e}");
            }
        }
    }

    // ── scrolling ────────────────────────────────────────────────────────

    /// One wheel notch over the grid at pane-relative, 0-based `(col, row)`.
    /// Port of `session.rs:641-661` — the dispatch is the observable contract:
    /// with no mouse reporting the *view* scrolls, otherwise the notch is
    /// forwarded to the inner app.
    pub fn scroll(&mut self, up: bool, col: u16, row: u16) {
        self.last_scroll_at = Some(Instant::now());
        if self.term.mouse_mode() == MouseMode::None {
            self.scroll_view(up, mouse::SCROLL_STEP);
            return;
        }
        self.send_wheel_notch(up, col, row);
    }

    /// The keyboard chords (Shift+PageUp/Down, Shift+Home/End). Mirrors
    /// [`Self::scroll`]'s dispatch exactly (`session.rs:720-738`), but forwards
    /// at the **viewport center** and through the 200-notch flood cap — without
    /// which the Shift+Home full-scrollback jump would hang the PTY.
    pub fn scroll_lines(&mut self, up: bool, lines: usize) {
        self.last_scroll_at = Some(Instant::now());
        if self.term.mouse_mode() == MouseMode::None {
            self.scroll_view(up, lines);
            return;
        }
        let (col, row) = (self.cols / 2, self.rows / 2);
        for _ in 0..mouse::scroll_notch_count(lines) {
            self.send_wheel_notch(up, col, row);
        }
    }

    /// Page size for Shift+PageUp/PageDown (`session.rs:743-749`).
    pub fn scroll_page_lines(&self) -> usize {
        mouse::scroll_page_lines(self.rows)
    }

    fn send_wheel_notch(&mut self, up: bool, col: u16, row: u16) {
        // Wheel up is cb 64, wheel down cb 65 (`session.rs:666-669`).
        let cb: u32 = if up { 64 } else { 65 };
        let bytes = mouse::encode_mouse(self.term.encoding(), cb, col, row, true);
        self.send(&bytes);
    }

    /// Scroll the terminal's own view, ignoring the inner app's mouse mode
    /// (`session.rs:673-708`).
    fn scroll_view(&mut self, up: bool, lines: usize) {
        match &self.backend {
            Backend::Tmux { name } => {
                // The agent runs on tmux's alternate screen, so this terminal's
                // own scrollback is empty — drive tmux copy-mode instead. The
                // re-render arrives through the reader like any other output.
                let name = name.clone();
                tmux::scroll(&name, up, lines);
                if up {
                    self.tmux_copy_mode = true;
                }
            }
            Backend::Native => {
                let cur = self.term.display_offset();
                let target = if up {
                    (cur + lines).min(mouse::SCROLLBACK_LINES)
                } else {
                    cur.saturating_sub(lines)
                };
                self.term.scroll_to(target);
            }
        }
    }

    // ── click and selection ──────────────────────────────────────────────

    /// A plain (non-dragging) click at pane-relative viewport cell
    /// `(col, row)`. Port of `session.rs:758-794`.
    ///
    /// Forwarded to the inner app when it wants mouse reporting; otherwise
    /// synthesized as Left/Right arrows so the caret follows the click. No-op
    /// while scrolled back (clicking history must never poke the live screen),
    /// while the caret is hidden, and across rows — at a shell prompt Up/Down
    /// mean history recall, not caret movement.
    pub fn click(&mut self, col: u16, row: u16) {
        if self.term.display_offset() != 0 {
            return;
        }
        let (rows, cols) = self.term.size();
        let col = col.min(cols.saturating_sub(1));
        // The row came from geometry captured at press time; a resize mid-
        // gesture can leave it past the live screen, so clamp it like col.
        let row = row.min(rows.saturating_sub(1));

        if self.term.mouse_mode() != MouseMode::None {
            let encoding = self.term.encoding();
            // Left button (cb = 0): press then release at the same cell.
            let press = mouse::encode_mouse(encoding, 0, col, row, true);
            let release = mouse::encode_mouse(encoding, 0, col, row, false);
            self.send(&press);
            self.send(&release);
            return;
        }

        let (cur_row, cur_col, hidden) = self.term.cursor();
        if hidden || row != cur_row {
            return;
        }
        let bytes = keys::arrow_moves(cur_col, col, self.term.app_cursor());
        if !bytes.is_empty() {
            self.send(&bytes);
        }
    }

    /// Selected text between two absolute endpoints. `GroveTerm::selection_text`
    /// already applies `clean_selection`'s trailing-whitespace and blank-line
    /// trimming — verified by grove-terminal's `golden_selection_text_matches`
    /// against the vt100 oracle, so it is **not** re-implemented here.
    pub fn selection_text(&mut self, a: AbsCell, head: AbsCell) -> Option<String> {
        self.term
            .selection_text((a.a_row, a.col), (head.a_row, head.col))
    }

    // ── readout ──────────────────────────────────────────────────────────

    pub fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.term.snapshot()
    }

    /// `(row, col, hidden)` in viewport coordinates, display offset already
    /// folded in by `GroveTerm::cursor`.
    pub fn cursor(&self) -> (u16, u16, bool) {
        self.term.cursor()
    }

    pub fn display_offset(&self) -> usize {
        self.term.display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }

    pub fn app_cursor(&self) -> bool {
        self.term.app_cursor()
    }

    pub fn mouse_mode(&self) -> MouseMode {
        self.term.mouse_mode()
    }

    pub fn encoding(&self) -> MouseEncoding {
        self.term.encoding()
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn input_age(&self) -> Option<Duration> {
        self.last_input_at.map(|t| t.elapsed())
    }

    pub fn scroll_age(&self) -> Option<Duration> {
        self.last_scroll_at.map(|t| t.elapsed())
    }
}

/// The tree the phase's single session opens in.
fn session_cwd() -> String {
    if let Some(dir) = std::env::var(CWD_ENV).ok().filter(|d| !d.is_empty()) {
        return dir;
    }
    std::env::current_dir().map_or_else(|_| "/".to_string(), |p| p.display().to_string())
}

/// Create the persistent tmux session and attach an embedded client to it,
/// reproducing `session.rs:177-236` (create + sidecar) and `:349-384` (attach).
fn spawn_tmux(cwd: &str) -> Result<(PtyHandle, Backend), String> {
    if !tmux::available() {
        return Err("tmux not on PATH".to_string());
    }
    let agent = Agent::Terminal;
    let n = tmux::next_free_n(cwd, agent);
    let name = tmux::make_name(cwd, agent, n);
    tmux::new_session(&name, cwd, INIT_ROWS, INIT_COLS, &agent.program(), &[], &[])
        .map_err(|e| e.to_string())?;
    // Without the sidecar the session can't be rediscovered after a restart —
    // kill it rather than orphan it (`session.rs:213-227`).
    if let Err(e) = session_meta::write(
        &name,
        &SessionMeta {
            wt_path: cwd.to_string(),
            project: String::new(),
            label: "gpui".to_string(),
            agent,
        },
    ) {
        tmux::kill_session(&name);
        return Err(format!("session metadata: {e}"));
    }
    tmux::configure_embedded_session(&name);

    let mut cmd = CommandBuilder::new("tmux");
    cmd.arg("-L");
    cmd.arg(tmux::SOCKET);
    // `-u` forces UTF-8 output to this client even with no UTF-8 locale in the
    // environment. Without it tmux reads the client as non-UTF-8 and downgrades
    // box-drawing to ACS/DEC line-drawing escapes (`session.rs:362-367`).
    cmd.arg("-u");
    cmd.arg("attach-session");
    cmd.arg("-t");
    cmd.arg(format!("={name}"));
    cmd.env("TERM", "xterm-256color");
    cmd.env("LC_ALL", "en_US.UTF-8");

    match grove_terminal::pty::spawn(cmd, INIT_ROWS, INIT_COLS) {
        Ok(pty) => Ok((pty, Backend::Tmux { name })),
        Err(e) => {
            tmux::kill_session(&name);
            Err(e.to_string())
        }
    }
}

/// The escape hatch: a bare login shell on a PTY, so the visual checklist is
/// runnable on a machine without tmux (`session.rs:238-268` in spirit).
fn spawn_native(cwd: &str) -> Result<(PtyHandle, Backend), String> {
    let mut cmd = CommandBuilder::new(grove_core::env_path::login_shell());
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LC_ALL", "en_US.UTF-8");
    grove_terminal::pty::spawn(cmd, INIT_ROWS, INIT_COLS)
        .map(|pty| (pty, Backend::Native))
        .map_err(|e| e.to_string())
}
