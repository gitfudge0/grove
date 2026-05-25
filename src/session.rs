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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

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

/// An agent process running inside an embedded pseudo-terminal.
pub struct Session {
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
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    rows: u16,
    cols: u16,
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
        let _ = session_meta::write(
            &tmux_name,
            &SessionMeta {
                wt_path: wt_path.clone(),
                project: project.clone(),
                label: label.clone(),
                agent,
            },
        );

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
        let mut cmd = CommandBuilder::new("tmux");
        cmd.arg("-L");
        cmd.arg(tmux::SOCKET);
        cmd.arg("attach-session");
        cmd.arg("-t");
        cmd.arg(format!("={}", tmux_name));
        cmd.env("TERM", "xterm-256color");

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

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 5000)));
        let dirty = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(SessionStatus::Running));
        let child = Arc::new(Mutex::new(child));

        {
            let parser = parser.clone();
            let dirty = dirty.clone();
            let status = status.clone();
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
            label,
            project,
            wt_path,
            branch,
            agent,
            backend,
            parser,
            dirty,
            status,
            writer,
            master: pair.master,
            child,
            rows,
            cols,
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
            SessionBackend::Native => {
                if let Ok(mut c) = self.child.lock() {
                    let _ = c.kill();
                }
            }
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
        // Typing snaps the view back to the live screen, like a real terminal.
        if let Ok(mut p) = self.parser.lock() {
            if p.screen().scrollback() != 0 {
                p.set_scrollback(0);
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Handle a mouse-wheel notch over the agent pane. `col`/`row` are
    /// pane-relative and 0-based. If the inner app requested mouse reporting
    /// the event is forwarded to it; otherwise we scroll grove's own
    /// scrollback buffer.
    pub fn scroll(&mut self, up: bool, col: u16, row: u16) {
        let mut p = match self.parser.lock() {
            Ok(p) => p,
            Err(_) => return,
        };

        if p.screen().mouse_protocol_mode() == MouseProtocolMode::None {
            // App doesn't want the mouse — drive our own scrollback view.
            let cur = p.screen().scrollback();
            let next = if up {
                cur + SCROLL_STEP
            } else {
                cur.saturating_sub(SCROLL_STEP)
            };
            if next != cur {
                p.set_scrollback(next);
                self.dirty.store(true, Ordering::Relaxed);
            }
            return;
        }

        let encoding = p.screen().mouse_protocol_encoding();
        drop(p);

        // Forward a wheel event the way the inner app expects to receive it.
        let cb: u32 = if up { 64 } else { 65 };
        self.send(&encode_mouse(encoding, cb, col, row, true));
    }

    /// Extract the text logically between two pane-relative cells (inclusive
    /// of the cell under `end`). Returns `None` if the selection is empty.
    pub fn selection_text(&self, start: (u16, u16), end: (u16, u16)) -> Option<String> {
        let p = self.parser.lock().ok()?;
        let screen = p.screen();
        let (_, cols) = screen.size();
        let end_col = end.0.saturating_add(1).min(cols);
        let raw = screen.contents_between(start.1, start.0, end.1, end_col);
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

    /// True if the inner app has asked to receive mouse events.
    pub fn wants_mouse(&self) -> bool {
        self.parser
            .lock()
            .map(|p| p.screen().mouse_protocol_mode() != MouseProtocolMode::None)
            .unwrap_or(false)
    }

    /// Forward a left-button press+release at a pane-relative cell to the inner
    /// app, encoded the way it requested. Used for bare clicks (no drag) so
    /// agents with clickable UI still work while drag-to-select is always on.
    pub fn forward_click(&mut self, col: u16, row: u16) {
        let encoding = match self.parser.lock() {
            Ok(p) => p.screen().mouse_protocol_encoding(),
            Err(_) => return,
        };
        let mut bytes = encode_mouse(encoding, 0, col, row, true);
        bytes.extend(encode_mouse(encoding, 0, col, row, false));
        let _ = self.writer.write_all(&bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.set_size(rows, cols);
        }
        self.dirty.store(true, Ordering::Relaxed);
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
        // Default / Utf8: classic X10 packet, one printable byte each.
        _ => {
            let enc = |v: u32| -> u8 { (32 + v).min(255) as u8 };
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

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }
}
