use crate::agent::Agent;
use crate::session_meta;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TmuxError {
    /// Only values are shell-quoted, so a non-identifier key could smuggle arbitrary shell in.
    #[error("invalid env var name: {0:?}")]
    InvalidEnvKey(String),
    #[error("tmux {cmd} failed")]
    Command { cmd: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = TmuxError> = std::result::Result<T, E>;

pub const SOCKET: &str = "grove";
pub const NAME_PREFIX: &str = "grove__";

fn tmux() -> Command {
    let mut c = Command::new("tmux");
    // Grove launches from a macOS .app bundle with no LANG/LC_* — without -u/LC_ALL, tmux downgrades Unicode box-drawing to literal q/x.
    c.args(["-u", "-L", SOCKET]);
    c.env("LC_ALL", "en_US.UTF-8");
    c.stdin(Stdio::null());
    c
}

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

/// Short enough to pick up a fresh tmux install; long enough that a per-frame render path doesn't fork `tmux -V` 15-20 times a second.
const AVAILABLE_CACHE_TTL: Duration = Duration::from_secs(5);

static AVAILABLE_CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

fn cache_is_fresh(checked_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(checked_at) < AVAILABLE_CACHE_TTL
}

pub fn available() -> bool {
    let now = Instant::now();
    {
        let cache = AVAILABLE_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((checked_at, result)) = *cache {
            if cache_is_fresh(checked_at, now) {
                return result;
            }
        }
    }

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
    let result = status.is_ok_and(|s| s.success());

    let mut cache = AVAILABLE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache = Some((now, result));
    result
}

pub fn has_session(name: &str) -> bool {
    let mut c = tmux();
    c.args(["has-session", "-t", &exact(name)]);
    run_silent(c).is_ok_and(|s| s.success())
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

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `env` is set via a shell env-prefix on the command line, not a tmux session option.
pub fn new_session(
    name: &str,
    cwd: &str,
    rows: u16,
    cols: u16,
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<()> {
    // Leading KEY='value' env prefix understood by any POSIX shell, avoiding tmux's 3.2+-only -e flag.
    let mut cmdline = String::new();
    for (k, v) in env {
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

pub fn configure_embedded_session(name: &str) {
    for (key, value) in [
        ("status", "off"),
        ("prefix", "None"),
        ("mouse", "off"),
        ("set-titles", "on"),
        ("set-titles-string", "#{pane_title}"),
    ] {
        let mut c = tmux();
        c.args(["set-option", "-t", name, key, value]);
        let _ = run_silent(c);
    }

    let mut c = tmux();
    c.args(["set-window-option", "-t", name, "allow-rename", "on"]);
    let _ = run_silent(c);

    // tmux loads the user's own ~/.tmux.conf, which can enable extended-keys/xterm-keys — Grove's input layer can never answer those capability queries, so force both off globally.
    let mut c = tmux();
    c.args(["set-option", "-g", "extended-keys", "off"]);
    let _ = run_silent(c);
    let mut c = tmux();
    c.args(["set-window-option", "-g", "xterm-keys", "off"]);
    let _ = run_silent(c);

    // Suppresses Claude Code's "tmux focus-events off" notice; NOT a fix for phantom input rows (disproven by direct experiment).
    let mut c = tmux();
    c.args(["set-option", "-s", "focus-events", "on"]);
    let _ = run_silent(c);
}

/// The attached client renders on the alternate screen, so real pane history lives in tmux's own buffer, reachable only through copy-mode.
pub fn scroll(name: &str, up: bool, lines: usize) {
    // copy-mode/send-keys take a pane target — exact()'s `=` prefix is invalid here ("can't find pane").
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

/// Process-tree root used to match a session against `claude agents --json` rows; `None` on any failure means "no live signal".
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
        .map(std::string::ToString::to_string)
        .collect()
}

/// Intersects live tmux sessions with sidecar metadata files; sidecars without a live session are pruned.
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

/// Fits a worktree path into a tmux session name, which can't contain `:` or `.`.
pub fn short_hash(s: &str) -> String {
    // FNV-1a 64-bit; not cryptographic.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// `n` disambiguates multiple sessions against the same (wt, agent).
pub fn make_name(wt_path: &str, agent: Agent, n: u32) -> String {
    format!(
        "{}{}__{}__{}",
        NAME_PREFIX,
        short_hash(wt_path),
        agent.label(),
        n
    )
}

pub fn next_free_n(wt_path: &str, agent: Agent) -> u32 {
    // Bounded so a wedged tmux server can't spin this forever.
    (0u32..1024)
        .find(|n| !has_session(&make_name(wt_path, agent, *n)))
        .unwrap_or(0)
}

/// Anchor a target so tmux treats it as a session-exact match, not a prefix.
fn exact(name: &str) -> String {
    format!("={name}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::agent::Agent;

    #[test]
    fn cache_is_fresh_respects_ttl_boundary() {
        let checked_at = Instant::now();
        assert!(
            cache_is_fresh(checked_at, checked_at),
            "an entry checked just now must be fresh"
        );
        assert!(
            cache_is_fresh(
                checked_at,
                checked_at + AVAILABLE_CACHE_TTL - Duration::from_millis(1)
            ),
            "an entry just under the TTL must still be fresh"
        );
        assert!(
            !cache_is_fresh(checked_at, checked_at + AVAILABLE_CACHE_TTL),
            "an entry exactly at the TTL must be stale"
        );
        assert!(
            !cache_is_fresh(
                checked_at,
                checked_at + AVAILABLE_CACHE_TTL + Duration::from_secs(1)
            ),
            "an entry past the TTL must be stale"
        );
    }

    #[test]
    fn short_hash_is_16_hex_chars() {
        for s in &[
            "",
            "hello",
            "/home/user/project/worktree",
            "a".repeat(200).as_str(),
        ] {
            let h = short_hash(s);
            assert_eq!(h.len(), 16, "short_hash({s:?}) must be 16 chars, got {h:?}");
            assert!(
                h.chars().all(|c| c.is_ascii_hexdigit()),
                "short_hash({s:?}) must be hex digits, got {h:?}"
            );
        }
    }

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

        let name1 = make_name(path, Agent::Claude, 1);
        assert_ne!(name, name1, "n=0 and n=1 must produce different names");
    }

    #[test]
    fn sh_quote_wraps_in_single_quotes() {
        let q = sh_quote("hello world");
        assert_eq!(q, "'hello world'");
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quote() {
        let q = sh_quote("a'b");
        assert_eq!(
            q, "'a'\\''b'",
            "embedded single quote must use '\\'' escape sequence"
        );
    }

    #[test]
    fn sh_quote_empty_string() {
        assert_eq!(sh_quote(""), "''");
    }

    fn display(target: &str, fmt: &str) -> String {
        let out = tmux()
            .args(["display-message", "-p", "-t", target, fmt])
            .stderr(Stdio::null())
            .output()
            .expect("tmux display-message");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // Verifies the regression where the `=`-prefixed pane target silently broke wheel scroll. Skipped when tmux is unavailable.
    #[test]
    fn scroll_drives_copy_mode() {
        if !available() {
            eprintln!("skipping: tmux not on PATH");
            return;
        }
        let name = "grove__selftest__scroll__0";
        kill_session(name);

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
