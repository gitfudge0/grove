//! Codex subprocess launch, protocol reader and shutdown.

use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
};

pub(super) struct Process {
    pub(super) activity: crate::activity_mcp::Server,
    pub(super) visuals: crate::visuals_mcp::Server,
    pub(super) child: Child,
    pub(super) input: ChildStdin,
    pub(super) lines: mpsc::Receiver<Result<Value, String>>,
    pub(super) capture: crate::provider_diagnostics::Capture,
    pub(super) ownership: Option<crate::provider_ownership::ThreadOwnership>,
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub(super) fn spawn_profile(
    args: &[String],
    cwd: &str,
    side_chat: bool,
    ownership: Option<crate::provider_ownership::ThreadOwnership>,
) -> Result<Process, String> {
    let activity = crate::activity_mcp::Server::start()?;
    let visuals = crate::visuals_mcp::Server::start()?;
    let mut argv = Vec::new();
    if side_chat {
        argv.push("--no-daemon".into());
    }
    argv.extend(["app-server".into(), "--stdio".into()]);
    argv.extend_from_slice(args);
    let (program, argv) = crate::agent::Agent::Codex.session_invocation(&argv);
    let mut cmd = Command::new(program);
    cmd.args(argv)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = crate::env_path::session_environment() {
        cmd.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    cmd.env(
        "GROVE_CONFIG_DIR",
        crate::storage::config_dir().map_err(|e| e.to_string())?,
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        format!("Cannot launch Codex. Install the Codex CLI and sign in with `codex login`: {e}")
    })?;
    let input = child.stdin.take().ok_or("Codex stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("Codex stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Codex stderr unavailable")?;
    // Keep a bounded, session-private diagnostic tail for an actionable exit error.
    // Never write provider output to global logs.
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let diagnostic_writer = diagnostics.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(mut tail) = diagnostic_writer.lock() {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 4096 {
                    let mut at = tail.len() - 4096;
                    while !tail.is_char_boundary(at) {
                        at += 1;
                    }
                    tail.drain(..at);
                }
            }
        }
    });
    let capture = crate::provider_diagnostics::Capture::from_env("codex");
    let incoming_capture = capture.clone();
    let (send, lines) = mpsc::channel();
    let actor_thread = std::thread::current();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let value = line.map_err(|e| e.to_string()).and_then(|line| {
                serde_json::from_str(&line)
                    .map_err(|e| format!("Invalid Codex protocol message: {e}"))
            });
            if let Ok(value) = &value {
                incoming_capture.record(false, value);
            }
            if send.send(value).is_err() {
                return;
            }
            actor_thread.unpark();
        }
        let tail = diagnostics
            .lock()
            .map(|tail| tail.trim().to_owned())
            .unwrap_or_default();
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("\n{tail}")
        };
        let _ = send.send(Err(format!(
            "Codex exited. Check CLI authentication/configuration, then reconnect.{detail}"
        )));
        actor_thread.unpark();
    });
    Ok(Process {
        activity,
        visuals,
        child,
        input,
        lines,
        capture,
        ownership,
    })
}
