use crate::agent::Agent;
use crate::session_meta;
use std::process::{Command, Stdio};
use thiserror::Error;

/// Everything the tmux layer can fail with. Mirrors `git::GitError` and
/// `storage::StoreError`: callers that only bubble errors upward keep working
/// unchanged (`anyhow` converts any `std::error::Error` via `?`), while a
/// caller that cares can match the specific failure instead of grepping a
/// formatted string.
#[derive(Debug, Error)]
pub enum TmuxError {
    /// An env var name handed to [`new_session`] is not a POSIX shell
    /// identifier. Only values are shell-quoted, so a non-identifier key
    /// could smuggle arbitrary shell into the session's command line.
    #[error("invalid env var name: {0:?}")]
    InvalidEnvKey(String),
    /// A `tmux` subprocess ran but exited non-zero. `cmd` is the tmux
    /// subcommand (e.g. `new-session`).
    #[error("tmux {cmd} failed")]
    Command { cmd: String },
    /// Spawning `tmux` failed outright (not on PATH, fork failure, …).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Shorthand for this module's fallible functions.
pub type Result<T, E = TmuxError> = std::result::Result<T, E>;

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
/// where we only care about the exit code. stderr is redirected to
/// `/dev/null` above, so a failure can only be logged with the exit status,
/// not captured stderr text.
fn run_silent(mut cmd: Command) -> std::io::Result<std::process::ExitStatus> {
    tracing::debug!(
        args = ?cmd.get_args().collect::<Vec<_>>(),
        "running tmux command"
    );
    let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
    if let Ok(s) = &status {
        if !s.success() {
            tracing::warn!(status = ?s, "tmux command failed");
        }
    }
    status
}

/// True if `tmux` is on PATH and runnable.
pub fn available() -> bool {
    tracing::debug!(args = "-V", "running tmux command");
    let status = Command::new("tmux")
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Ok(s) = &status {
        if !s.success() {
            tracing::warn!(status = ?s, "tmux command failed");
        }
    }
    status.map(|s| s.success()).unwrap_or(false)
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

/// True when `key` is a POSIX shell environment-variable name:
/// `[A-Za-z_][A-Za-z0-9_]*`.
fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Create a detached tmux session running `program args...` in `cwd`, with
/// `env` set for that process only (via a shell env-prefix, not a tmux
/// session option — see `new_session`'s doc comment on `cmdline`).
pub fn new_session(
    name: &str,
    cwd: &str,
    rows: u16,
    cols: u16,
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<()> {
    // tmux runs a single-string command via the user's shell (`$SHELL -c
    // <cmdline>`), so a leading `KEY='value' ...` env prefix is understood by
    // any POSIX shell without needing tmux's own (3.2+-only) `-e` flag.
    let mut cmdline = String::new();
    for (k, v) in env {
        // Only the value is shell-quoted below; the key is spliced in raw, so a
        // key that isn't a plain identifier could smuggle in arbitrary shell.
        if !valid_env_key(k) {
            return Err(TmuxError::InvalidEnvKey(k.clone()));
        }
        cmdline.push_str(k);
        cmdline.push('=');
        cmdline.push_str(&sh_quote(v));
        cmdline.push(' ');
    }
    cmdline.push_str(&sh_quote(program));
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
        return Err(TmuxError::Command {
            cmd: "new-session".to_string(),
        });
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

/// Drive an embedded tmux client's scrollback via copy-mode. The attached
/// client renders on the alternate screen, so the real pane history lives in
/// tmux's own buffer and is only reachable through copy-mode — grove's vt100
/// scrollback is empty here. Wheel-up enters mouse copy-mode (`-e`, which
/// auto-exits when scrolled back to the bottom) and walks the view up; wheel-
/// down walks it back down. `lines` is the notch step.
pub fn scroll(name: &str, up: bool, lines: usize) {
    // NB: `copy-mode`/`send-keys` take a *pane* target, where the `=` exact-
    // match prefix used by `exact()` is invalid ("can't find pane"). The plain
    // session name resolves to the session's active pane — the agent — which is
    // what we want for these single-pane embedded sessions.
    if up {
        let mut enter = tmux();
        enter.args(["copy-mode", "-e", "-t", name]);
        let _ = run_silent(enter);
    }
    let cmd = if up { "scroll-up" } else { "scroll-down" };
    let mut c = tmux();
    c.args(["send-keys", "-t", name, "-X", "-N", &lines.to_string(), cmd]);
    let _ = run_silent(c);
}

/// Leave copy-mode so subsequent keystrokes reach the agent again. A no-op if
/// the client is not currently in copy-mode.
pub fn cancel_copy_mode(name: &str) {
    let mut c = tmux();
    c.args(["send-keys", "-t", name, "-X", "cancel"]);
    let _ = run_silent(c);
}

pub fn kill_session(name: &str) {
    let mut c = tmux();
    c.args(["kill-session", "-t", &exact(name)]);
    let _ = run_silent(c);
}

/// PID of the pane's foreground process in tmux session `name` (i.e. the
/// direct child of the tmux server for that pane — typically the agent
/// process's shell, or the agent itself if exec'd directly). Used as the
/// process-tree root for matching a session against `claude agents --json`
/// rows in `claude_agents::Poller::status_for`. Returns `None` on any
/// failure (tmux not running, session gone, unparsable output) — callers
/// treat that as "no live signal" and fall back to other heuristics.
pub fn pane_pid(name: &str) -> Option<u32> {
    tracing::debug!(args = "list-panes -F #{pane_pid}", target = %name, "running tmux command");
    let out = tmux()
        .args(["list-panes", "-t", &exact(name), "-F", "#{pane_pid}"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(status = ?out.status, "tmux command failed");
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
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
    tracing::debug!(
        args = "list-sessions -F #{session_name}",
        "running tmux command"
    );
    let out = tmux()
        .args(["list-sessions", "-F", "#{session_name}"])
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        tracing::warn!(status = ?out.status, "tmux command failed");
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

/// Stable 16-hex-char hash of an arbitrary string. Used to fit a worktree path
/// into a tmux session name (which can't contain `:` or `.`). Full 64 bits so
/// two worktree paths can't realistically collide into the same session name.
pub fn short_hash(s: &str) -> String {
    // FNV-1a 64-bit. Good enough for naming; not cryptographic.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
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
    // Bounded so a wedged tmux server can't spin this forever.
    (0u32..1024)
        .find(|n| !has_session(&make_name(wt_path, agent, *n)))
        .unwrap_or(0)
}

/// Anchor a target so tmux treats it as a session-exact match, not a prefix.
fn exact(name: &str) -> String {
    format!("={}", name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;

    // ── short_hash ───────────────────────────────────────────────────────────

    /// `short_hash` always produces exactly 16 hex characters.
    #[test]
    fn short_hash_is_16_hex_chars() {
        for s in &[
            "",
            "hello",
            "/home/user/project/worktree",
            "a".repeat(200).as_str(),
        ] {
            let h = short_hash(s);
            assert_eq!(
                h.len(),
                16,
                "short_hash({s:?}) must be 16 chars, got {:?}",
                h
            );
            assert!(
                h.chars().all(|c| c.is_ascii_hexdigit()),
                "short_hash({s:?}) must be hex digits, got {h:?}"
            );
        }
    }

    /// `short_hash` is deterministic: the same input always produces the same
    /// output, and different inputs produce different outputs (FNV collisions
    /// are astronomically unlikely for these controlled inputs).
    #[test]
    fn short_hash_deterministic_and_distinct() {
        let a = short_hash("/home/user/project/wt-a");
        let b = short_hash("/home/user/project/wt-b");
        assert_eq!(
            a,
            short_hash("/home/user/project/wt-a"),
            "short_hash must be deterministic"
        );
        assert_ne!(a, b, "different paths must produce different hashes");
    }

    // ── make_name ────────────────────────────────────────────────────────────

    /// `make_name` produces a string that starts with `NAME_PREFIX`, embeds the
    /// hash and agent label, and ends with the disambiguator `n`.
    #[test]
    fn make_name_structure() {
        let path = "/repos/myproject/wt-feat";
        let name = make_name(path, Agent::Claude, 0);
        assert!(
            name.starts_with(NAME_PREFIX),
            "session name must start with NAME_PREFIX, got {name:?}"
        );
        assert!(
            name.contains(short_hash(path).as_str()),
            "session name must contain the path hash"
        );
        assert!(
            name.contains("claude"),
            "session name must contain the agent label"
        );
        assert!(
            name.ends_with("__0"),
            "session name must end with the disambiguator __0"
        );

        // Different `n` → different name.
        let name1 = make_name(path, Agent::Claude, 1);
        assert_ne!(name, name1, "n=0 and n=1 must produce different names");
    }

    // ── sh_quote ─────────────────────────────────────────────────────────────

    /// `sh_quote` wraps the string in single quotes.
    #[test]
    fn sh_quote_wraps_in_single_quotes() {
        let q = sh_quote("hello world");
        assert_eq!(q, "'hello world'");
    }

    /// A string containing a single quote must be escaped as `'\''` so the
    /// shell receives the correct literal character.
    #[test]
    fn sh_quote_escapes_embedded_single_quote() {
        // Input: a'b  → expected: 'a'\''b'
        let q = sh_quote("a'b");
        assert_eq!(
            q, "'a'\\''b'",
            "embedded single quote must use '\\'' escape sequence"
        );
    }

    /// An empty string round-trips as `''`.
    #[test]
    fn sh_quote_empty_string() {
        assert_eq!(sh_quote(""), "''");
    }

    /// Read a single tmux format value for a target on grove's socket.
    fn display(target: &str, fmt: &str) -> String {
        let out = tmux()
            .args(["display-message", "-p", "-t", target, fmt])
            .stderr(Stdio::null())
            .output()
            .expect("tmux display-message");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // Drives the real copy-mode scroll path against a throwaway session on
    // grove's socket. Verifies the wheel actually enters copy-mode and moves
    // the view, and that cancel returns to the live screen — the regression
    // that the `=`-prefixed pane target silently broke. Skipped when tmux is
    // unavailable (e.g. minimal CI).
    #[test]
    fn scroll_drives_copy_mode() {
        if !available() {
            eprintln!("skipping: tmux not on PATH");
            return;
        }
        let name = "grove__selftest__scroll__0";
        // Clean any leftover from a previous aborted run.
        kill_session(name);

        // A 24-row pane printing 200 lines guarantees real scrollback history.
        let mut create = tmux();
        create.args([
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            "80",
            "-y",
            "24",
            "sh",
            "-c",
            "for i in $(seq 1 200); do echo line $i; done; sleep 30",
        ]);
        assert!(
            run_silent(create).expect("spawn").success(),
            "new-session failed"
        );

        // Give the shell a moment to emit its output into the pane history.
        std::thread::sleep(std::time::Duration::from_millis(300));

        assert_eq!(display(name, "#{pane_in_mode}"), "0", "should start live");

        scroll(name, true, 3);
        assert_eq!(
            display(name, "#{pane_in_mode}"),
            "1",
            "wheel-up enters copy-mode"
        );
        let pos: i32 = display(name, "#{scroll_position}").parse().unwrap_or(-1);
        assert!(pos > 0, "scroll_position should advance, got {pos}");

        cancel_copy_mode(name);
        assert_eq!(
            display(name, "#{pane_in_mode}"),
            "0",
            "cancel returns to live"
        );

        kill_session(name);
    }
}
