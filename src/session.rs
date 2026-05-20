use crate::agent::Agent;
use crate::session_meta::{self, SessionMeta};
use crate::tmux;
use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};
use std::io::{Read, Write};

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

/// An agent process running inside an embedded pseudo-terminal.
pub struct Session {
    pub label: String,
    pub project: String,
    #[allow(dead_code)]
    pub wt_path: String,
    pub agent: Agent,
    /// Backing tmux session name. The agent process runs inside this session;
    /// grove embeds a `tmux attach-client` to it.
    pub tmux_name: String,
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

        Self::attach(label, project, wt_path, agent, tmux_name, rows, cols)
    }

    /// Re-attach to an existing tmux session previously created by grove.
    pub fn attach_existing(d: tmux::DiscoveredSession) -> Result<Self> {
        Self::attach(
            d.label,
            d.project,
            d.wt_path,
            d.agent,
            d.name,
            INIT_ROWS,
            INIT_COLS,
        )
    }

    fn attach(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        tmux_name: String,
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

        let mut cmd = CommandBuilder::new("tmux");
        cmd.arg("-L");
        cmd.arg(tmux::SOCKET);
        cmd.arg("attach-session");
        cmd.arg("-t");
        cmd.arg(format!("={}", tmux_name));
        cmd.env("TERM", "xterm-256color");

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

        Ok(Session {
            label,
            project,
            wt_path,
            agent,
            tmux_name,
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

    /// Destroy the backing tmux session. Use this when the user explicitly
    /// kills a session — otherwise we only detach (the server keeps it alive).
    pub fn kill_persistent(&mut self) {
        tmux::kill_session(&self.tmux_name);
        session_meta::delete(&self.tmux_name);
    }

    pub fn status(&self) -> SessionStatus {
        self.status.lock().map(|s| *s).unwrap_or(SessionStatus::Running)
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
        let bytes = match encoding {
            MouseProtocolEncoding::Sgr => {
                format!("\x1b[<{};{};{}M", cb, col + 1, row + 1).into_bytes()
            }
            // Default / Utf8: classic X10 packet, one printable byte each.
            _ => {
                let enc = |v: u32| -> u8 { (32 + v).min(255) as u8 };
                vec![
                    0x1b,
                    b'[',
                    b'M',
                    enc(cb),
                    enc(col as u32 + 1),
                    enc(row as u32 + 1),
                ]
            }
        };
        self.send(&bytes);
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

    #[allow(dead_code)]
    pub fn kill(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
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
