//! PTY spawning and the blocking reader thread; deliberately executor-free.

use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Bytes read per `read()` call on the PTY master.
const READ_CHUNK: usize = 8192;

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// Dropping the handle kills and reaps the child, then drops the master, which closes the PTY and ends the reader thread.
pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    rx_taken: bool,
}

impl PtyHandle {
    pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> std::io::Result<Self> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system().openpty(size).map_err(io_err)?;
        let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        // The slave fd must not outlive the spawn, or the reader never sees EOF when the child exits.
        drop(pair.slave);

        let writer = pair.master.take_writer().map_err(io_err)?;
        let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
        let (tx, rx) = channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("grove-pty-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
            rx,
            rx_taken: false,
        })
    }

    /// By value, so a caller can block on it from its own thread; `receiver()`/`drain()` then report closed. `None` after the first call.
    pub fn take_receiver(&mut self) -> Option<Receiver<Vec<u8>>> {
        if self.rx_taken {
            return None;
        }
        self.rx_taken = true;
        // The paired sender is dropped immediately, so the replacement channel reads as disconnected.
        let (_dead_tx, dead_rx) = channel::<Vec<u8>>();
        Some(std::mem::replace(&mut self.rx, dead_rx))
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Raises `SIGWINCH`.
    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)
    }

    /// A disconnected channel means the PTY reached EOF.
    pub fn receiver(&self) -> &Receiver<Vec<u8>> {
        &self.rx
    }

    /// `None` once the PTY has closed and the queue is drained.
    pub fn drain(&self) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(chunk) => out.push(chunk),
                Err(TryRecvError::Empty) => return Some(out),
                Err(TryRecvError::Disconnected) => {
                    return if out.is_empty() { None } else { Some(out) };
                }
            }
        }
    }

    /// Still reported after the child is reaped; check [`PtyHandle::try_wait`] for liveness.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn try_wait(&mut self) -> std::io::Result<bool> {
        self.child.try_wait().map(|s| s.is_some())
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
}

/// `portable_pty`'s writer drop writes `b"\n\x04"` into the PTY — kill the child first or those bytes land as keystrokes (the "phantom row" bug).
impl Drop for PtyHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> std::io::Result<PtyHandle> {
    PtyHandle::spawn(cmd, rows, cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn child_pid_is_reported_while_the_child_lives() {
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("30");
        let mut handle = spawn(cmd, 24, 80).expect("spawn");
        let pid = handle.child_pid().expect("a live child has a pid");
        assert!(pid > 0);
        assert_eq!(handle.child_pid(), Some(pid), "the id is stable");
        handle.kill().expect("kill");
    }

    #[test]
    fn dropping_the_handle_kills_the_child() {
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("30");
        let handle = spawn(cmd, 24, 80).expect("spawn");
        let pid = handle.child_pid().expect("a live child has a pid");

        drop(handle);

        let is_alive = || {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .expect("run kill -0")
                .success()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && is_alive() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!is_alive(), "child {pid} survived dropping its PtyHandle");
    }

    #[test]
    fn spawn_reads_child_output_into_the_model() {
        let mut cmd = CommandBuilder::new("echo");
        cmd.arg("hello-from-pty");
        cmd.env("TERM", "xterm-256color");
        let mut handle = spawn(cmd, 24, 80).expect("spawn");

        let mut term = crate::GroveTerm::new(24, 80);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_eof = false;
        while Instant::now() < deadline && !saw_eof {
            match handle.drain() {
                Some(chunks) => {
                    if chunks.is_empty() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    for c in chunks {
                        term.process(&c);
                    }
                }
                None => saw_eof = true,
            }
        }

        assert!(saw_eof, "PTY never reached EOF");
        assert!(
            term.tail_contents(5).contains("hello-from-pty"),
            "child output never reached the model: {:?}",
            term.tail_contents(5)
        );
        let _ = handle.kill();
    }

    #[test]
    fn take_receiver_hands_the_channel_over_exactly_once() {
        let mut cmd = CommandBuilder::new("echo");
        cmd.arg("taken-channel");
        cmd.env("TERM", "xterm-256color");
        let mut handle = spawn(cmd, 24, 80).expect("spawn");

        let rx = handle
            .take_receiver()
            .expect("first take yields the channel");
        assert!(
            handle.take_receiver().is_none(),
            "the channel must only be handed over once"
        );
        assert!(
            handle.drain().is_none(),
            "a handle whose channel was taken reports itself closed"
        );

        let mut term = crate::GroveTerm::new(24, 80);
        while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(10)) {
            term.process(&chunk);
        }
        assert!(
            term.tail_contents(5).contains("taken-channel"),
            "child output never reached the model: {:?}",
            term.tail_contents(5)
        );
        let _ = handle.kill();
    }
}
