use crate::agent::Agent;
use crate::session_meta::{self, SessionMeta};
use crate::tmux;
use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Initial PTY size; the event loop resizes it to the real pane on first draw.
const INIT_ROWS: u16 = 24;
const INIT_COLS: u16 = 80;
/// Lines moved per wheel notch when scrolling grove's own scrollback buffer.
const SCROLL_STEP: usize = 3;
/// Max scrollback lines retained by the vt100 parser per session.
const SCROLLBACK_LINES: usize = 5000;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum SessionStatus {
    Running,
    Exited(Option<i32>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionBackend {
    Native,
    Tmux { name: String },
}

/// Process-wide session id source. Never reused, unlike Arc pointer
/// addresses, so map keys derived from `Session::id` can't collide when a
/// session is killed and another spawned at the same allocation.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(0);

/// An agent process running inside an embedded pseudo-terminal.
pub struct Session {
    /// Unique, never-reused id (see `NEXT_SESSION_ID`).
    pub id: u64,
    pub label: String,
    pub project: String,
    #[allow(dead_code)]
    pub wt_path: String,
    pub branch: String,
    pub agent: Agent,
    pub backend: SessionBackend,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub dirty: Arc<AtomicBool>,
    pub status: Arc<Mutex<SessionStatus>>,
    pub last_output_at: Arc<Mutex<Instant>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    rows: u16,
    cols: u16,
    /// Tmux-only: whether the attached client is currently in copy-mode (i.e.
    /// the user has scrolled back). Tracked so we only spawn a `cancel` tmux
    /// call once when typing resumes, rather than on every keystroke.
    tmux_copy_mode: bool,
    /// When the user last scrolled this session. Scrolling redraws the PTY
    /// (tmux copy-mode, forwarded mouse events), which looks like fresh agent
    /// output to the activity classifier — this timestamp lets it discount
    /// output that immediately follows a scroll.
    last_scroll_at: Option<Instant>,
    /// When the user last typed into or resized this session. The inner app's
    /// keystroke echo and SIGWINCH repaint flow back through the PTY reader,
    /// which looks like fresh agent output to the activity classifier — this
    /// timestamp lets it discount output that immediately follows the user's
    /// own interaction.
    last_input_at: Option<Instant>,
}

impl Session {
    pub fn spawn(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: &[String],
        cwd: &str,
        use_tmux: bool,
    ) -> Result<Self> {
        if use_tmux {
            Self::spawn_tmux(label, project, wt_path, agent, args, cwd)
        } else {
            Self::spawn_native(label, project, wt_path, agent, args, cwd)
        }
    }

    fn spawn_tmux(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: &[String],
        cwd: &str,
    ) -> Result<Self> {
        let rows = INIT_ROWS;
        let cols = INIT_COLS;

        // Create the persistent tmux session, then attach a client to it via
        // our embedded PTY. The agent process lives inside tmux, not as a
        // direct child of grove, so it survives grove restarts.
        let n = tmux::next_free_n(&wt_path, agent);
        let tmux_name = tmux::make_name(&wt_path, agent, n);
        tmux::new_session(&tmux_name, cwd, rows, cols, &agent.program(), args)?;
        // Without the sidecar the session can't be rediscovered after a grove
        // restart — kill the tmux session rather than orphan it.
        if let Err(e) = session_meta::write(
            &tmux_name,
            &SessionMeta {
                wt_path: wt_path.clone(),
                project: project.clone(),
                label: label.clone(),
                agent,
            },
        ) {
            tmux::kill_session(&tmux_name);
            return Err(e.context("failed to write session metadata"));
        }

        Self::attach_tmux(label, project, wt_path, agent, tmux_name, rows, cols)
    }

    fn spawn_native(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: &[String],
        cwd: &str,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(agent.program());
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        // Ensure the agent emits UTF-8 even when grove is launched from a macOS
        // .app bundle (which inherits no UTF-8 locale from the shell).
        cmd.env("LC_ALL", "en_US.UTF-8");

        Self::launch_pty(
            label,
            project,
            wt_path,
            agent,
            SessionBackend::Native,
            cmd,
            INIT_ROWS,
            INIT_COLS,
        )
    }

    /// Spawn a one-off shell session running a lifecycle script (`setup` /
    /// `run` / `teardown`). Always native — these are ephemeral and not worth
    /// persisting through tmux. The shell exits when the script finishes (the
    /// session then shows `Exited`); long-lived scripts like dev servers keep
    /// the session running. Carries `Agent::Terminal` so it reuses terminal
    /// rendering/handling; the `label` distinguishes the lifecycle stage.
    pub fn spawn_script(
        label: String,
        project: String,
        wt_path: String,
        script: &str,
        cwd: &str,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(crate::env_path::login_shell());
        cmd.arg("-lc");
        cmd.arg(script);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("LC_ALL", "en_US.UTF-8");

        Self::launch_pty(
            label,
            project,
            wt_path,
            Agent::Terminal,
            SessionBackend::Native,
            cmd,
            INIT_ROWS,
            INIT_COLS,
        )
    }

    /// Re-attach to an existing tmux session previously created by grove.
    pub fn attach_existing(d: tmux::DiscoveredSession) -> Result<Self> {
        Self::attach_tmux(
            d.label, d.project, d.wt_path, d.agent, d.name, INIT_ROWS, INIT_COLS,
        )
    }

    fn attach_tmux(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        tmux_name: String,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        tmux::configure_embedded_session(&tmux_name);

        let mut cmd = CommandBuilder::new("tmux");
        cmd.arg("-L");
        cmd.arg(tmux::SOCKET);
        // `-u` forces tmux to write UTF-8 to this client even when the
        // environment has no UTF-8 locale. Grove launches from a macOS .app
        // bundle, which inherits no `LANG`/`LC_*` from the shell, so without
        // this tmux thinks the client is non-UTF-8 (client_utf8=0) and
        // downgrades box-drawing glyphs to ACS/DEC line-drawing escapes —
        // which surface in the vt100 renderer as literal `q`/`x` characters.
        cmd.arg("-u");
        cmd.arg("attach-session");
        cmd.arg("-t");
        cmd.arg(format!("={}", tmux_name));
        cmd.env("TERM", "xterm-256color");
        cmd.env("LC_ALL", "en_US.UTF-8");

        Self::launch_pty(
            label,
            project,
            wt_path,
            agent,
            SessionBackend::Tmux { name: tmux_name },
            cmd,
            rows,
            cols,
        )
    }

    fn launch_pty(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        backend: SessionBackend,
        cmd: CommandBuilder,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(cmd)?;
        // Slave is held by the child; drop our handle so EOF propagates on exit.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_LINES)));
        let dirty = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(SessionStatus::Running));
        let last_output_at = Arc::new(Mutex::new(Instant::now()));
        let child = Arc::new(Mutex::new(child));

        {
            let parser = parser.clone();
            let dirty = dirty.clone();
            let status = status.clone();
            let last_output_at = last_output_at.clone();
            let child = child.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut p) = parser.lock() {
                                p.process(&buf[..n]);
                            }
                            if let Ok(mut t) = last_output_at.lock() {
                                *t = Instant::now();
                            }
                            dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
                let code = child
                    .lock()
                    .ok()
                    .and_then(|mut c| c.wait().ok())
                    .map(|s| s.exit_code() as i32);
                if let Ok(mut s) = status.lock() {
                    *s = SessionStatus::Exited(code);
                }
                dirty.store(true, Ordering::Relaxed);
            });
        }

        let branch = crate::git::current_branch(&wt_path);
        Ok(Session {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            label,
            project,
            wt_path,
            branch,
            agent,
            backend,
            parser,
            dirty,
            status,
            last_output_at,
            writer,
            master: pair.master,
            child,
            rows,
            cols,
            tmux_copy_mode: false,
            last_scroll_at: None,
            last_input_at: None,
        })
    }

    pub fn tmux_name(&self) -> Option<&str> {
        match &self.backend {
            SessionBackend::Tmux { name } => Some(name.as_str()),
            SessionBackend::Native => None,
        }
    }

    /// Explicit user kill. Tmux sessions destroy their persistent backing
    /// session; native sessions kill the direct child process.
    pub fn kill(&mut self) {
        match &self.backend {
            SessionBackend::Tmux { name } => {
                tmux::kill_session(name);
                session_meta::delete(name);
            }
            SessionBackend::Native => Self::kill_native(&self.child),
        }
    }

    /// Kill a native session's process tree. portable-pty makes the child a
    /// session leader (`setsid`), so its pid is also its process-group id;
    /// signalling the whole group on unix reaps any foreground job the shell
    /// launched (vim, ssh, top, …) instead of orphaning it. Falls back to
    /// killing just the child elsewhere.
    fn kill_native(child: &Arc<Mutex<Box<dyn Child + Send + Sync>>>) {
        if let Ok(mut c) = child.lock() {
            #[cfg(unix)]
            if let Some(pid) = c.process_id() {
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            let _ = c.kill();
        }
    }

    pub fn status(&self) -> SessionStatus {
        self.status
            .lock()
            .map(|s| *s)
            .unwrap_or(SessionStatus::Running)
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.status() == SessionStatus::Running
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.last_input_at = Some(Instant::now());
        // Typing snaps the view back to the live screen, like a real terminal.
        if let Ok(mut p) = self.parser.lock() {
            if p.screen().scrollback() != 0 {
                p.set_scrollback(0);
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
        // Tmux renders on the alternate screen, so its scrollback lives in
        // copy-mode rather than grove's vt100 buffer. If the user had scrolled
        // back, leave copy-mode first so these keystrokes reach the agent
        // instead of being eaten as copy-mode commands.
        if self.tmux_copy_mode {
            if let SessionBackend::Tmux { name } = &self.backend {
                tmux::cancel_copy_mode(name);
            }
            self.tmux_copy_mode = false;
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Handle a mouse-wheel notch over the agent pane. `col`/`row` are
    /// pane-relative and 0-based. If the inner app requested mouse reporting
    /// the event is forwarded to it; otherwise we scroll grove's own
    /// scrollback buffer.
    /// Seconds since the user last scrolled this session, if ever.
    pub fn scroll_age(&self) -> Option<std::time::Duration> {
        self.last_scroll_at.map(|t| t.elapsed())
    }

    /// Seconds since the user last typed into or resized this session, if ever.
    pub fn input_age(&self) -> Option<std::time::Duration> {
        self.last_input_at.map(|t| t.elapsed())
    }

    pub fn scroll(&mut self, up: bool, col: u16, row: u16) {
        self.last_scroll_at = Some(Instant::now());

        let mut p = match self.parser.lock() {
            Ok(p) => p,
            Err(_) => return,
        };

        if p.screen().mouse_protocol_mode() == MouseProtocolMode::None {
            // Inner app doesn't handle the mouse — scroll the terminal view.
            match &self.backend {
                SessionBackend::Tmux { name } => {
                    // The agent runs on the alternate screen inside tmux, so
                    // grove's vt100 scrollback is empty. Drive tmux copy-mode
                    // instead; the re-render arrives through the reader thread.
                    let name = name.clone();
                    drop(p);
                    tmux::scroll(&name, up, SCROLL_STEP);
                    if up {
                        self.tmux_copy_mode = true;
                    }
                }
                SessionBackend::Native => {
                    // Cap at the configured scrollback size. vt100 0.15.2's
                    // `set_scrollback` clamps to the actually-filled scrollback
                    // internally, so reading `scrollback()` back gives the
                    // effective offset (and avoids the `rows_len - offset`
                    // underflow in `visible_rows` when the buffer isn't full).
                    let cur = p.screen().scrollback();
                    let target = if up {
                        (cur + SCROLL_STEP).min(SCROLLBACK_LINES)
                    } else {
                        cur.saturating_sub(SCROLL_STEP)
                    };
                    if target != cur {
                        p.set_scrollback(target);
                        if p.screen().scrollback() != cur {
                            self.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            return;
        }

        let encoding = p.screen().mouse_protocol_encoding();
        drop(p);

        // Inner app has mouse reporting — forward the wheel event to it.
        // For the tmux backend, writing to the PTY reaches the agent through
        // tmux's input forwarding (unaffected by the `mouse off` session option,
        // which only suppresses tmux's own terminal mouse interception).
        let cb: u32 = if up { 64 } else { 65 };
        self.send(&encode_mouse(encoding, cb, col, row, true));
    }

    /// Extract the selected text between two endpoints given in scrollback-
    /// stable absolute coordinates (`(a_row, col)`, where larger `a_row` is
    /// older — see `gui::state::AbsCell`). The selection may extend beyond the
    /// currently-visible viewport: when both endpoints are on screen we read it
    /// in one pass (preserving soft-wrapped-line joining); otherwise we walk the
    /// scrollback row by row, restoring the original offset afterwards.
    pub fn selection_text_abs(&self, p1: (usize, usize), p2: (usize, usize)) -> Option<String> {
        let mut parser = self.parser.lock().ok()?;
        let (h, cols) = parser.screen().size();
        let h = h as usize;
        if h == 0 {
            return None;
        }
        let s = parser.screen().scrollback();
        // Viewport row for an absolute row at the current offset (may be
        // outside `[0, h-1]` when the row is scrolled off screen).
        let vr = |a: usize| -> isize { (h as isize - 1) - (a as isize - s as isize) };
        let (r1, r2) = (vr(p1.0), vr(p2.0));
        // Order endpoints by (viewport row, col) for the fast path.
        let (top, bot) = if (r1, p1.1) <= (r2, p2.1) {
            ((r1, p1.1), (r2, p2.1))
        } else {
            ((r2, p2.1), (r1, p1.1))
        };

        if top.0 >= 0 && bot.0 <= h as isize - 1 {
            // Fully visible — single multi-row read, as before.
            let sc = top.1 as u16;
            let ec = (bot.1 as u16).saturating_add(1).min(cols);
            let raw = parser
                .screen()
                .contents_between(top.0 as u16, sc, bot.0 as u16, ec);
            return clean_selection(raw);
        }

        // Off-screen: walk the scrollback. Order endpoints by absolute row
        // (older/top first); on a row tie the smaller column starts.
        let (a_top, c_top, a_bot, c_bot) =
            if (p1.0, std::cmp::Reverse(p1.1)) >= (p2.0, std::cmp::Reverse(p2.1)) {
                (p1.0, p1.1, p2.0, p2.1)
            } else {
                (p2.0, p2.1, p1.0, p1.1)
            };
        let orig = s;
        let mut lines: Vec<String> = Vec::new();
        for a in (a_bot..=a_top).rev() {
            // Bring absolute row `a` to the bottom visible row (vr = h-1).
            // `set_scrollback` clamps to the filled buffer; if `a` is older
            // than the oldest retained line it lands `delta` rows above bottom.
            parser.set_scrollback(a);
            let actual = parser.screen().scrollback();
            let delta = a.saturating_sub(actual);
            if delta < h {
                let vrow = (h - 1 - delta) as u16;
                let sc = if a == a_top { c_top as u16 } else { 0 };
                let ec = if a == a_bot {
                    (c_bot as u16).saturating_add(1).min(cols)
                } else {
                    cols
                };
                let raw = parser.screen().contents_between(vrow, sc, vrow, ec);
                lines.push(raw.trim_end().to_string());
            }
        }
        parser.set_scrollback(orig);
        drop(parser);
        let out = lines.join("\n");
        let out = out.trim_end_matches('\n').to_string();
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// The current OSC 0/1/2 window title emitted by the inner app, if any.
    /// vt100 already tracks this from the PTY byte stream; we just read it.
    pub fn current_title(&self) -> Option<String> {
        let p = self.parser.lock().ok()?;
        let t = p.screen().title().trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// Last `n` rows of the visible screen, newline-joined, for the activity
    /// classifier. Always reads the *live* grid: the scrollback offset is
    /// temporarily zeroed so a user scrolled into history doesn't make the
    /// classifier see stale markers (or miss a fresh prompt at the bottom).
    pub fn tail_contents(&self, n: usize) -> String {
        let Ok(mut p) = self.parser.lock() else {
            return String::new();
        };
        let orig = p.screen().scrollback();
        if orig != 0 {
            p.set_scrollback(0);
        }
        let contents = p.screen().contents();
        if orig != 0 {
            p.set_scrollback(orig);
        }
        let lines: Vec<&str> = contents.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    }

    /// Total BEL (0x07) count vt100 has seen on this session's stream.
    /// Monotonic; the caller diffs against its last-seen value. Using vt100's
    /// counter (not a raw byte scan) means OSC terminators don't false-ring.
    pub fn bell_count(&self) -> usize {
        self.parser
            .lock()
            .map(|p| p.screen().audible_bell_count())
            .unwrap_or(0)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        // A real size change makes the inner app repaint (SIGWINCH); discount
        // that redraw burst so it isn't read as the agent producing output.
        self.last_input_at = Some(Instant::now());
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            // vt100 0.15.2 doesn't clamp the scrollback offset on resize, so
            // shrinking below the current offset overflows `rows_len -
            // scrollback_offset` in `visible_rows`. Snap to live first.
            if p.screen().scrollback() != 0 {
                p.set_scrollback(0);
            }
            p.set_size(rows, cols);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }
}

/// Normalize raw `contents_between` output for the clipboard: trim trailing
/// whitespace on each line and drop trailing blank lines. Returns `None` when
/// the result is empty.
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

/// Encode a single mouse report (`cb` = button/wheel code) at a pane-relative
/// 0-based cell, in whichever protocol the inner app negotiated. `press` picks
/// the press vs. release form (SGR final byte `M`/`m`; X10 release uses button
/// code 3).
fn encode_mouse(
    encoding: MouseProtocolEncoding,
    cb: u32,
    col: u16,
    row: u16,
    press: bool,
) -> Vec<u8> {
    match encoding {
        MouseProtocolEncoding::Sgr => format!(
            "\x1b[<{};{};{}{}",
            cb,
            col + 1,
            row + 1,
            if press { 'M' } else { 'm' }
        )
        .into_bytes(),
        // Default / Utf8: classic X10 packet, one printable byte each. The
        // protocol tops out at coordinate 223 (byte 255); past that the
        // position can't be represented, so emit nothing rather than a wrong
        // position.
        _ => {
            if col >= 223 || row >= 223 {
                return vec![];
            }
            let enc = |v: u32| -> u8 { (32 + v) as u8 };
            let button = if press { cb } else { 3 };
            vec![
                0x1b,
                b'[',
                b'M',
                enc(button),
                enc(col as u32 + 1),
                enc(row as u32 + 1),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt100::MouseProtocolEncoding;

    // ── clean_selection ──────────────────────────────────────────────────────

    /// Trailing whitespace on each line is stripped.
    #[test]
    fn clean_selection_strips_trailing_whitespace() {
        let result = clean_selection("hello   \nworld  ".into());
        assert_eq!(result, Some("hello\nworld".into()));
    }

    /// Trailing blank lines are removed.
    #[test]
    fn clean_selection_removes_trailing_blank_lines() {
        let result = clean_selection("line1\nline2\n\n\n".into());
        assert_eq!(result, Some("line1\nline2".into()));
    }

    /// A string that is only whitespace/newlines returns `None`.
    #[test]
    fn clean_selection_all_whitespace_returns_none() {
        assert_eq!(clean_selection("   \n  \n".into()), None);
        assert_eq!(clean_selection("".into()), None);
    }

    /// Interior blank lines are preserved; only trailing ones are dropped.
    #[test]
    fn clean_selection_preserves_interior_blank_lines() {
        let result = clean_selection("a\n\nb\n".into());
        assert_eq!(result, Some("a\n\nb".into()));
    }

    // ── encode_mouse ─────────────────────────────────────────────────────────

    /// SGR encoding uses the `\x1b[<cb;col+1;rowM` format.
    #[test]
    fn encode_mouse_sgr_format() {
        // Wheel-up (cb=64) at col=0, row=0, press=true → "\x1b[<64;1;1M"
        let bytes = encode_mouse(MouseProtocolEncoding::Sgr, 64, 0, 0, true);
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }

    /// SGR release uses lowercase `m` as the final byte.
    #[test]
    fn encode_mouse_sgr_release_uses_lowercase_m() {
        let bytes = encode_mouse(MouseProtocolEncoding::Sgr, 64, 5, 3, false);
        let s = std::str::from_utf8(&bytes).expect("valid utf8");
        assert!(s.ends_with('m'), "SGR release must end with 'm', got {s:?}");
    }

    /// X10 (default) encoding: col or row >= 223 returns an empty vec because
    /// the coordinate can't fit in one printable byte.
    #[test]
    fn encode_mouse_x10_large_coord_returns_empty() {
        let bytes = encode_mouse(MouseProtocolEncoding::default(), 64, 223, 0, true);
        assert!(
            bytes.is_empty(),
            "X10 must return empty vec when col >= 223"
        );
        let bytes = encode_mouse(MouseProtocolEncoding::default(), 64, 0, 223, true);
        assert!(
            bytes.is_empty(),
            "X10 must return empty vec when row >= 223"
        );
    }

    /// X10 encoding for small coordinates produces the classic 6-byte packet:
    /// ESC [ M <button+32> <col+1+32> <row+1+32>.
    #[test]
    fn encode_mouse_x10_small_coords_correct_packet() {
        // cb=64 (wheel-up), col=0, row=0 → button byte = 64+32=96, col byte = 0+1+32=33, row byte = same
        let bytes = encode_mouse(MouseProtocolEncoding::default(), 64, 0, 0, true);
        assert_eq!(bytes.len(), 6, "X10 packet must be 6 bytes");
        assert_eq!(&bytes[..3], b"\x1b[M", "must start with ESC [ M");
        assert_eq!(bytes[3], 96, "button byte: cb + 32");
        assert_eq!(bytes[4], 33, "col byte: col + 1 + 32");
        assert_eq!(bytes[5], 33, "row byte: row + 1 + 32");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        match &self.backend {
            // Native: reap the whole process group (see `kill_native`).
            SessionBackend::Native => Self::kill_native(&self.child),
            // Tmux: only our attach client is a child of grove; the backing
            // tmux session is meant to survive, so just drop the client.
            SessionBackend::Tmux { .. } => {
                if let Ok(mut c) = self.child.lock() {
                    let _ = c.kill();
                }
            }
        }
    }
}
