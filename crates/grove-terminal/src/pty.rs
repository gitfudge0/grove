//! PTY spawning and the blocking reader thread; deliberately executor-free.

use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Bytes read per `read()` call on the PTY master.
const READ_CHUNK: usize = 8192;

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// Dropping the handle kills the child and reaps it off-thread, then drops the master to close the PTY.
pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    rx: Receiver<Vec<u8>>,
    rx_taken: bool,
    exit_code: Option<u32>,
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
            child: Some(child),
            rx,
            rx_taken: false,
            exit_code: None,
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
        self.child.as_ref().and_then(|child| child.process_id())
    }

    pub fn exit_code(&mut self) -> std::io::Result<Option<u32>> {
        if self.exit_code.is_none() {
            if let Some(child) = self.child.as_mut() {
                self.exit_code = child.try_wait()?.map(|status| status.exit_code());
            }
        }
        Ok(self.exit_code)
    }

    pub fn try_wait(&mut self) -> std::io::Result<bool> {
        self.exit_code().map(|code| code.is_some())
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.as_mut().map_or(Ok(()), |child| child.kill())
    }
}

/// `portable_pty`'s writer drop writes `b"\n\x04"` into the PTY — kill the child first or those bytes land as keystrokes (the "phantom row" bug).
impl Drop for PtyHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            kill_and_reap(child);
        }
    }
}

fn kill_and_reap(mut child: Box<dyn Child + Send + Sync>) {
    let _ = child.kill();
    // Reaping can block indefinitely even after kill returns. Do not hold the
    // UI thread while the operating system reports exit.
    let _ = std::thread::Builder::new()
        .name("grove-pty-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
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
    fn reaping_a_stalled_child_does_not_block_the_caller() {
        use portable_pty::{ChildKiller, ExitStatus};
        use std::sync::{mpsc, Arc, Mutex};

        #[derive(Debug, Clone)]
        struct StalledChild {
            events: mpsc::Sender<&'static str>,
            release: Arc<Mutex<mpsc::Receiver<()>>>,
        }
        impl ChildKiller for StalledChild {
            fn kill(&mut self) -> std::io::Result<()> {
                self.events.send("kill").unwrap();
                Ok(())
            }
            fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
                Box::new(self.clone())
            }
        }
        impl Child for StalledChild {
            fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
                Ok(None)
            }
            fn wait(&mut self) -> std::io::Result<ExitStatus> {
                self.events.send("wait").unwrap();
                self.release.lock().unwrap().recv().unwrap();
                Ok(ExitStatus::with_exit_code(0))
            }
            fn process_id(&self) -> Option<u32> {
                None
            }
            #[cfg(windows)]
            fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
                None
            }
        }
        let (events, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (done, returned) = mpsc::channel();
        std::thread::spawn(move || {
            kill_and_reap(Box::new(StalledChild {
                events,
                release: Arc::new(Mutex::new(gate)),
            }));
            done.send(()).unwrap();
        });
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            "kill"
        );
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            "wait"
        );
        let caller_returned = returned.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        assert!(caller_returned.is_ok(), "reaping blocked the caller");
    }

    #[test]
    #[cfg(unix)]
    fn reports_nonzero_child_exit_code_without_inferring_success_from_eof() {
        let mut command = CommandBuilder::new("sh");
        command.arg("-c");
        command.arg("exit 7");
        let mut child = spawn(command, 24, 80).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(code) = child.exit_code().unwrap() {
                assert_eq!(code, 7);
                assert_eq!(child.exit_code().unwrap(), Some(7));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[cfg(unix)]
    fn eof_does_not_imply_a_successful_exit() {
        let mut command = CommandBuilder::new("sh");
        command.arg("-c");
        command.arg("exec 0<&- 1>&- 2>&-; sleep 0.1; exit 7");
        let mut child = spawn(command, 24, 80).unwrap();
        let receiver = child.take_receiver().unwrap();
        while receiver.recv_timeout(Duration::from_secs(5)).is_ok() {}
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(code) = child.exit_code().unwrap() {
                assert_eq!(code, 7);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut command = CommandBuilder::new("sh");
        command.arg("-c");
        command.arg("exit 0");
        let mut child = spawn(command, 24, 80).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(code) = child.exit_code().unwrap() {
                assert_eq!(code, 0);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

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
