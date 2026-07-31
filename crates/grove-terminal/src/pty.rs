//! PTY spawning and the blocking reader thread.
//!
//! Deliberately executor-free: chunks land in a `std::sync::mpsc::Receiver`,
//! not a future. Picking an async runtime is the UI layer's call — the gpui
//! shell bridges this receiver into `cx.spawn`, and nothing here forces that
//! choice on other callers.

use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Bytes read per `read()` call on the PTY master.
const READ_CHUNK: usize = 8192;

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// A spawned child on its own PTY.
///
/// Dropping the handle drops the master, which closes the PTY and ends the
/// reader thread; the child is not killed for you — call [`PtyHandle::kill`].
pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
}

impl PtyHandle {
    /// Spawn `cmd` on a fresh PTY of the given size and start reading it.
    pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> std::io::Result<Self> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system().openpty(size).map_err(io_err)?;
        let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        // The slave fd must not outlive the spawn, or the reader never sees EOF
        // when the child exits.
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
                        // EOF, or the child went away.
                        Ok(0) | Err(_) => break,
                        // A closed receiver means the handle was dropped.
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
        })
    }

    /// Send input to the child.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Tell the child the window changed size (raises `SIGWINCH`).
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

    /// The channel carrying output chunks. Drain it from the caller's own
    /// event loop; a disconnected channel means the PTY reached EOF.
    pub fn receiver(&self) -> &Receiver<Vec<u8>> {
        &self.rx
    }

    /// Take every chunk currently queued, in order. Returns `None` once the
    /// PTY has closed *and* the queue is drained.
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

    /// Whether the child has exited, without blocking.
    pub fn try_wait(&mut self) -> std::io::Result<bool> {
        self.child.try_wait().map(|s| s.is_some())
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
}

/// Convenience wrapper mirroring the spec's `spawn(cmd, rows, cols)`.
pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> std::io::Result<PtyHandle> {
    PtyHandle::spawn(cmd, rows, cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::time::{Duration, Instant};

    /// End-to-end smoke test: a real child on a real PTY, its output arriving
    /// through the channel, feeding a real `GroveTerm`.
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
}
