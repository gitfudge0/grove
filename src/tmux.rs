use crate::agent::Agent;
use crate::session_meta;
use anyhow::{anyhow, Result};
use std::process::{Command, Stdio};

/// Private socket name. Keeps grove's tmux server isolated from the user's.
pub const SOCKET: &str = "grove";

/// Prefix on every tmux session grove owns. Used to filter discovery.
pub const NAME_PREFIX: &str = "grove__";

fn tmux() -> Command {
    let mut c = Command::new("tmux");
    // `-u` forces UTF-8 handling even when the environment carries no UTF-8
    // locale, and `LC_ALL` gives the tmux server itself one. Grove launches
    // from a macOS .app bundle that inherits no `LANG`/`LC_*` from the shell;
    // without this, tmux treats clients as non-UTF-8 and downgrades Unicode
    // box-drawing to ACS line-drawing escapes (rendered as literal `q`/`x`).
    c.args(["-u", "-L", SOCKET]);
    c.env("LC_ALL", "en_US.UTF-8");
    // Closed stdin is always safe; stdout/stderr are configured per-call so
    // `.output()` callers can still capture, while `.status()` callers route
    // both to /dev/null so tmux warnings do not leak into the parent process.
    c.stdin(Stdio::null());
    c
}

/// Run `cmd` to completion with stdout/stderr silenced. Use for tmux calls
/// where we only care about the exit code.
fn run_silent(mut cmd: Command) -> std::io::Result<std::process::ExitStatus> {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status()
}

/// True if `tmux` is on PATH and runnable.
pub fn available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn has_session(name: &str) -> bool {
    let mut c = tmux();
    c.args(["has-session", "-t", &exact(name)]);
    run_silent(c).map(|s| s.success()).unwrap_or(false)
}

/// Single-quote shell-escape so tmux's parser hands the command through intact.
fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Create a detached tmux session running `program args...` in `cwd`.
pub fn new_session(
    name: &str,
    cwd: &str,
    rows: u16,
    cols: u16,
    program: &str,
    args: &[String],
) -> Result<()> {
    let mut cmdline = sh_quote(program);
    for a in args {
        cmdline.push(' ');
        cmdline.push_str(&sh_quote(a));
    }
    let mut cmd = tmux();
    cmd.args([
        "new-session",
        "-d",
        "-s",
        name,
        "-c",
        cwd,
        "-x",
        &cols.to_string(),
        "-y",
        &rows.to_string(),
        &cmdline,
    ]);
    let status = run_silent(cmd)?;
    if !status.success() {
        return Err(anyhow!("tmux new-session failed"));
    }
    configure_embedded_session(name);
    Ok(())
}

/// Configure a grove-owned tmux session so its attached client behaves like a
/// transparent embedded terminal.
pub fn configure_embedded_session(name: &str) {
    // Hide the status bar and disable the prefix so the embedded view is just
    // the agent's screen, with no tmux keybindings stealing input.
    for (key, value) in [
        ("status", "off"),
        ("prefix", "None"),
        ("mouse", "off"),
        // The GUI reads session context from OSC window-title updates. Native
        // sessions expose those directly; tmux needs to be told to forward the
        // active pane title to the attached client.
        ("set-titles", "on"),
        ("set-titles-string", "#{pane_title}"),
    ] {
        let mut c = tmux();
        c.args(["set-option", "-t", name, key, value]);
        let _ = run_silent(c);
    }

    // Let agent CLIs update the tmux pane title from their own OSC title
    // sequences. This is a window option, not a session option.
    let mut c = tmux();
    c.args(["set-window-option", "-t", name, "allow-rename", "on"]);
    let _ = run_silent(c);
}

pub fn kill_session(name: &str) {
    let mut c = tmux();
    c.args(["kill-session", "-t", &exact(name)]);
    let _ = run_silent(c);
}

#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub name: String,
    pub wt_path: String,
    pub project: String,
    pub label: String,
    pub agent: Agent,
}

/// Names of all grove-owned tmux sessions currently on the server.
pub fn live_grove_session_names() -> Vec<String> {
    let out = tmux()
        .args(["list-sessions", "-F", "#{session_name}"])
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|n| n.starts_with(NAME_PREFIX))
        .map(|n| n.to_string())
        .collect()
}

/// Discover prior grove sessions by intersecting live tmux sessions with
/// sidecar metadata files. Sidecars without a live tmux session are pruned.
pub fn list_grove_sessions() -> Vec<DiscoveredSession> {
    let live = live_grove_session_names();
    session_meta::prune(&live);
    live.into_iter()
        .filter_map(|name| {
            let meta = session_meta::read(&name)?;
            Some(DiscoveredSession {
                name,
                wt_path: meta.wt_path,
                project: meta.project,
                label: meta.label,
                agent: meta.agent,
            })
        })
        .collect()
}

/// Stable 8-hex-char hash of an arbitrary string. Used to fit a worktree path
/// into a tmux session name (which can't contain `:` or `.`).
pub fn short_hash(s: &str) -> String {
    // FNV-1a 64-bit, truncated. Good enough for naming; not cryptographic.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

/// Build a unique grove session name. `n` disambiguates multiple sessions
/// against the same (wt, agent).
pub fn make_name(wt_path: &str, agent: Agent, n: u32) -> String {
    format!(
        "{}{}__{}__{}",
        NAME_PREFIX,
        short_hash(wt_path),
        agent.label(),
        n
    )
}

/// Pick the smallest `n` such that `make_name(...)` isn't taken yet.
pub fn next_free_n(wt_path: &str, agent: Agent) -> u32 {
    (0u32..)
        .find(|n| !has_session(&make_name(wt_path, agent, *n)))
        .unwrap_or(0)
}

/// Anchor a target so tmux treats it as a session-exact match, not a prefix.
fn exact(name: &str) -> String {
    format!("={}", name)
}
