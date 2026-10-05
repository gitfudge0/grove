use crate::agent::Agent;
use crate::session_meta;
use std::cmp::Reverse;
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
    c.args(["-u", "-L"]);
    // Unit tests exercise real tmux history/copy mode. A live Grove client can
    // resize panes on the production socket, invalidating fixed viewport
    // fixtures and changing the user's paste buffer. Each test process gets
    // its own server and default configuration instead.
    #[cfg(test)]
    c.arg(format!("grove-selftest-{}", std::process::id()))
        .args(["-f", "/dev/null"]);
    #[cfg(not(test))]
    c.arg(SOCKET);
    c.env("LC_ALL", "en_US.UTF-8");
    c.stdin(Stdio::null());
    c
}

fn run_silent(mut cmd: Command) -> std::io::Result<std::process::ExitStatus> {
    // Session command lines can contain exported credentials.
    if cmd.get_args().any(|arg| arg == "new-session") {
        tracing::debug!("running tmux new-session");
    } else {
        tracing::debug!(args = ?cmd.get_args().collect::<Vec<_>>(), "running tmux command");
    }
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

pub(crate) fn valid_env_key(key: &str) -> bool {
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
    let status = if let Some(env) = crate::env_path::session_environment() {
        let mut applied = APPLIED_ENV_KEYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = applied.get_or_insert_with(global_environment_keys);
        // Track even partially applied scripts so a later launch can remove
        // their keys after a command or transport failure.
        keys.extend(env.iter().map(|(key, _)| key.clone()));
        let input = session_script(name, cwd, rows, cols, &cmdline, &env, keys)?;
        run_script(&input)?
    } else {
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
        run_silent(cmd)?
    };
    if !status.success() {
        return Err(TmuxError::Command {
            cmd: "new-session".to_string(),
        });
    }
    configure_embedded_session(name);
    Ok(())
}

// Serialize each complete environment + new-session transaction, preventing
// concurrent launches holding different snapshots from crossing environments.
static APPLIED_ENV_KEYS: Mutex<Option<std::collections::BTreeSet<String>>> = Mutex::new(None);

fn global_environment_keys() -> std::collections::BTreeSet<String> {
    // A first/recreated server inherits the app's startup environment, which
    // may contain exports removed by a subsequent shell refresh.
    let mut keys = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| valid_env_key(key))
        .collect::<std::collections::BTreeSet<_>>();
    let output = tmux()
        .args(["show-environment", "-g"])
        .stderr(Stdio::null())
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            keys.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter_map(|line| {
                        let key = line
                            .split_once('=')
                            .map_or_else(|| line.strip_prefix('-'), |(key, _)| Some(key))?;
                        valid_env_key(key).then(|| key.to_string())
                    }),
            );
        }
    }
    keys
}

/// Quote a tmux configuration token, including control characters and shell
/// expansion characters. Environment contents only travel on private stdin.
fn tmux_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' | '\"' | '$' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '\n' => quoted.push_str("\\012"),
            '\r' => quoted.push_str("\\015"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('\"');
    quoted
}

fn session_script(
    name: &str,
    cwd: &str,
    rows: u16,
    cols: u16,
    cmdline: &str,
    env: &[(String, String)],
    previous_keys: &std::collections::BTreeSet<String>,
) -> Result<String> {
    use std::fmt::Write;
    let mut input = String::new();
    for key in previous_keys {
        if !env.iter().any(|(current, _)| current == key) {
            let _ = writeln!(input, "set-environment -gu {}", tmux_quote(key));
        }
    }
    for (key, value) in env {
        if !valid_env_key(key) {
            return Err(TmuxError::InvalidEnvKey(key.clone()));
        }
        let _ = writeln!(
            input,
            "set-environment -g {} {}",
            tmux_quote(key),
            tmux_quote(value)
        );
    }
    let _ = writeln!(
        input,
        "new-session -d -E -s {} -c {} -x {} -y {} {}",
        tmux_quote(name),
        tmux_quote(cwd),
        cols,
        rows,
        tmux_quote(cmdline)
    );
    Ok(input)
}

fn run_script(input: &str) -> std::io::Result<std::process::ExitStatus> {
    run_script_command(tmux(), input)
}

fn run_script_command(
    mut command: Command,
    input: &str,
) -> std::io::Result<std::process::ExitStatus> {
    use std::io::Write;
    let mut child = command
        .args(["start-server", ";", "source-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("tmux stdin unavailable"))?
        .write_all(input.as_bytes());
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    child.wait()
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
/// Returns tmux's resulting copy-mode scroll position. The query shares the
/// scroll invocation, so drag autoscroll still starts only one process per tick.
pub fn scroll(name: &str, up: bool, lines: usize) -> Option<usize> {
    // copy-mode/send-keys take a pane target — exact()'s `=` prefix is invalid here ("can't find pane").
    let lines = lines.to_string();
    let mut c = tmux();
    if up {
        c.args(["copy-mode", "-e", "-t", name, ";"]);
    }
    let cmd = if up { "scroll-up" } else { "scroll-down" };
    c.args([
        "send-keys",
        "-t",
        name,
        "-X",
        "-N",
        &lines,
        cmd,
        ";",
        "display-message",
        "-p",
        "-t",
        name,
        "#{scroll_position}",
    ]);
    let out = c.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout);
    Some(value.trim().parse().unwrap_or(0))
}

/// Copies a Grove selection from tmux's own history. Tmux-backed sessions are
/// rendered on an alternate screen, so GroveTerm only contains the current
/// copy-mode viewport and cannot read an endpoint after it scrolls off screen.
pub fn selection_text(
    name: &str,
    p1: (usize, usize),
    p2: (usize, usize),
    restore_offset: usize,
) -> Option<String> {
    // Older absolute rows come first. On a row tie, the smaller column is the
    // start of the selection.
    let (start, end) = if (p1.0, Reverse(p1.1)) >= (p2.0, Reverse(p2.1)) {
        (p1, p2)
    } else {
        (p2, p1)
    };

    // tmux's vi selection includes its cursor cell; emacs excludes it.
    // Grove endpoints always identify inclusive cells, matching GroveTerm.
    let mode = tmux()
        .args(["display-message", "-p", "-t", name, "#{mode-keys}"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !mode.status.success() {
        return None;
    }
    let end_col = if String::from_utf8_lossy(&mode.stdout).trim() == "vi" {
        end.1
    } else {
        end.1.saturating_add(1)
    };
    let mut c = tmux();
    c.args(["copy-mode", "-e", "-t", name, ";"]);
    push_copy_cursor(&mut c, name, start.0, start.1);
    c.args(["send-keys", "-t", name, "-X", "begin-selection", ";"]);
    push_copy_cursor(&mut c, name, end.0, end_col);
    c.args(["send-keys", "-t", name, "-X", "copy-selection", ";"]);
    if restore_offset == 0 {
        c.args(["send-keys", "-t", name, "-X", "cancel", ";"]);
    } else {
        c.args([
            "send-keys",
            "-t",
            name,
            "-X",
            "history-bottom",
            ";",
            "send-keys",
            "-N",
            &restore_offset.to_string(),
            "-t",
            name,
            "-X",
            "scroll-up",
            ";",
        ]);
    }
    c.args(["save-buffer", "-"]);
    let out = c.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\n')
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn push_copy_cursor(c: &mut Command, name: &str, a_row: usize, col: usize) {
    c.args(["send-keys", "-t", name, "-X", "history-bottom", ";"]);
    if a_row > 0 {
        c.args([
            "send-keys",
            "-N",
            &a_row.to_string(),
            "-t",
            name,
            "-X",
            "cursor-up",
            ";",
        ]);
    }
    // Reset the horizontal goal on the target row. Doing this on the blank
    // bottom row first lets tmux restore its preferred column during cursor-up,
    // and cursor-right can then wrap onto the wrong row.
    c.args(["send-keys", "-t", name, "-X", "start-of-line", ";"]);
    if col > 0 {
        c.args([
            "send-keys",
            "-N",
            &col.to_string(),
            "-t",
            name,
            "-X",
            "cursor-right",
            ";",
        ]);
    }
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
    /// Active pane title captured during discovery, before any PTY is attached.
    pub pane_title: Option<String>,
    pub wt_path: String,
    pub project: String,
    pub label: String,
    pub agent: Agent,
    pub context_roots: Vec<session_meta::ContextRoot>,
    pub temp_bundle_path: Option<String>,
}

fn discovery_records(
    sidecars: impl IntoIterator<Item = (String, Option<String>, session_meta::SessionMeta)>,
) -> (Vec<DiscoveredSession>, Vec<std::path::PathBuf>) {
    let mut sessions = Vec::new();
    let mut active_bundles = Vec::new();
    for (name, pane_title, meta) in sidecars {
        if let Some(path) = meta.temp_bundle_path.as_ref() {
            active_bundles.push(std::path::PathBuf::from(path));
        }
        if meta.agent == Agent::Terminal && !meta.managed_worktree_terminal {
            continue;
        }
        sessions.push(DiscoveredSession {
            name,
            pane_title,
            wt_path: meta.wt_path,
            project: meta.project,
            label: meta.label,
            agent: meta.agent,
            context_roots: meta.context_roots,
            temp_bundle_path: meta.temp_bundle_path,
        });
    }
    (sessions, active_bundles)
}

fn parse_live_session(line: &str) -> Option<(String, Option<String>)> {
    let (name, title) = line.split_once('\t')?;
    if !name.starts_with(NAME_PREFIX) {
        return None;
    }
    let title = title.trim();
    Some((
        name.to_string(),
        (!title.is_empty()).then(|| title.to_string()),
    ))
}

fn live_grove_sessions() -> Vec<(String, Option<String>)> {
    tracing::debug!(
        args = "list-sessions -F #{session_name}\\t#{pane_title}",
        "running tmux command"
    );
    let out = tmux()
        .args(["list-sessions", "-F", "#{session_name}\t#{pane_title}"])
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        tracing::warn!(status = ?out.status, "tmux command failed");
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_live_session)
        .collect()
}

pub fn live_grove_session_names() -> Vec<String> {
    live_grove_sessions()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Intersects live tmux sessions with sidecar metadata files; sidecars without a live session are pruned.
pub fn list_grove_sessions() -> Vec<DiscoveredSession> {
    let live = live_grove_sessions();
    let names = live
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    session_meta::prune(&names);
    let sidecars = live
        .into_iter()
        .filter_map(|(name, pane_title)| {
            session_meta::read(&name).map(|meta| (name, pane_title, meta))
        })
        .collect::<Vec<_>>();
    let (sessions, active_bundles) = discovery_records(sidecars);
    crate::multi_root::cleanup_orphaned(&active_bundles);
    sessions
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

    fn sidecar(
        agent: Agent,
        managed_worktree_terminal: bool,
        bundle: Option<&str>,
    ) -> session_meta::SessionMeta {
        session_meta::SessionMeta {
            control_id: None,
            wt_path: "/worktree".into(),
            project: "project".into(),
            label: "Terminal 1".into(),
            agent,
            managed_worktree_terminal,
            context_roots: Vec::new(),
            temp_bundle_path: bundle.map(str::to_owned),
        }
    }

    #[test]
    fn legacy_terminal_sidecars_do_not_reattach_or_lose_live_bundles() {
        let (sessions, bundles) = discovery_records([
            (
                "legacy-terminal".into(),
                None,
                sidecar(Agent::Terminal, false, Some("/bundle/legacy")),
            ),
            (
                "managed-terminal".into(),
                Some("Fix build".into()),
                sidecar(Agent::Terminal, true, Some("/bundle/managed")),
            ),
            (
                "legacy-agent".into(),
                None,
                sidecar(Agent::Claude, false, None),
            ),
        ]);
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.name.as_str())
                .collect::<Vec<_>>(),
            vec!["managed-terminal", "legacy-agent"]
        );
        assert_eq!(
            bundles,
            vec![
                std::path::PathBuf::from("/bundle/legacy"),
                std::path::PathBuf::from("/bundle/managed")
            ]
        );
        assert_eq!(sessions[0].pane_title.as_deref(), Some("Fix build"));
        assert_eq!(sessions[1].pane_title, None);
    }

    #[test]
    fn live_session_line_extracts_pane_title_and_ignores_other_sessions() {
        assert_eq!(
            parse_live_session("grove__abc\t  Fix build  "),
            Some(("grove__abc".into(), Some("Fix build".into())))
        );
        assert_eq!(
            parse_live_session("grove__abc\t"),
            Some(("grove__abc".into(), None))
        );
        assert_eq!(parse_live_session("other\tignored"), None);
        assert_eq!(parse_live_session("grove__missing-delimiter"), None);
    }

    #[test]
    fn live_discovery_reads_title_without_attaching_a_client() {
        if !available() {
            eprintln!("skipping: tmux not on PATH");
            return;
        }
        let name = "grove__selftest__title__0";
        kill_session(name);
        let mut create = tmux();
        create.args(["new-session", "-d", "-s", name, "sleep 30"]);
        assert!(run_silent(create).expect("spawn").success());
        let mut title = tmux();
        title.args(["select-pane", "-t", name, "-T", "Restore this title"]);
        assert!(run_silent(title).expect("set pane title").success());

        let discovered = live_grove_sessions();
        assert_eq!(
            discovered
                .iter()
                .find(|(session, _)| session == name)
                .and_then(|(_, title)| title.as_deref()),
            Some("Restore this title")
        );
        kill_session(name);
    }

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
                (checked_at + AVAILABLE_CACHE_TTL)
                    .checked_sub(Duration::from_millis(1))
                    .expect("AVAILABLE_CACHE_TTL is well above 1ms")
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

    #[test]
    fn stdin_environment_transfer_preserves_literal_values() {
        let name = "grove_test_env_stdin";
        let key = "GROVE_TEST_LITERAL_EXPORT";
        let value = "quotes ' \" $HOME $(literal) {braces} \\ backslash\nsecond=line café";
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("synthetic-export");
        let shell_script = format!(
            "printf '%s' \"${key}\" > {}; exec /bin/sleep 10",
            sh_quote(output_path.to_str().unwrap())
        );
        let cmdline = format!("exec /bin/sh -c {}", sh_quote(&shell_script));
        let input = session_script(
            name,
            "/",
            24,
            80,
            &cmdline,
            &[(key.into(), value.into())],
            &std::collections::BTreeSet::default(),
        )
        .unwrap();
        if !available() {
            return;
        }
        assert!(run_script(&input).unwrap().success());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !output_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = fs_err::read_to_string(&output_path);
        kill_session(name);
        assert_eq!(output.unwrap(), value);
    }

    #[test]
    fn refreshed_tmux_environment_removes_exports_only_for_new_sessions() {
        if !available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let old_path = dir.path().join("old-session");
        let new_path = dir.path().join("new-session");
        let key = "GROVE_TEST_REFRESH_REMOVED";
        let old_name = "grove_test_refresh_old";
        let new_name = "grove_test_refresh_new";
        let old_script = format!(
            "/bin/sleep 0.5; printf '%s' \"${key}\" > {}; exec /bin/sleep 10",
            sh_quote(old_path.to_str().unwrap())
        );
        let new_script = format!(
            "printf '%s' \"${{{key}-unset}}\" > {}; exec /bin/sleep 10",
            sh_quote(new_path.to_str().unwrap())
        );
        let old = session_script(
            old_name,
            "/",
            24,
            80,
            &format!("exec /bin/sh -c {}", sh_quote(&old_script)),
            &[(key.into(), "old-value".into())],
            &std::collections::BTreeSet::default(),
        )
        .unwrap();
        assert!(run_script(&old).unwrap().success());
        let new = session_script(
            new_name,
            "/",
            24,
            80,
            &format!("exec /bin/sh -c {}", sh_quote(&new_script)),
            &[],
            &[key.to_string()].into_iter().collect(),
        )
        .unwrap();
        assert!(run_script(&new).unwrap().success());
        let deadline = Instant::now() + Duration::from_secs(3);
        while (!old_path.exists() || !new_path.exists()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let old_value = fs_err::read_to_string(old_path);
        let new_value = fs_err::read_to_string(new_path);
        kill_session(old_name);
        kill_session(new_name);
        assert_eq!(old_value.unwrap(), "old-value");
        assert_eq!(new_value.unwrap(), "unset");
    }

    #[test]
    fn first_and_recreated_tmux_servers_do_not_restore_deleted_startup_exports() {
        if !available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let socket = format!("grove-refresh-recreated-{}", std::process::id());
        let key = "GROVE_TEST_STALE_STARTUP";
        let previous = [key.to_string()].into_iter().collect();
        for attempt in 0..2 {
            let output_path = dir.path().join(format!("child-{attempt}"));
            let script = format!(
                "printf '%s' \"${{{key}-unset}}\" > {}; exec /bin/sleep 10",
                sh_quote(output_path.to_str().unwrap())
            );
            let input = session_script(
                "probe",
                "/",
                24,
                80,
                &format!("exec /bin/sh -c {}", sh_quote(&script)),
                &[],
                &previous,
            )
            .unwrap();
            let mut command = Command::new("tmux");
            command
                .args(["-L", &socket, "-f", "/dev/null"])
                .env(key, "stale-startup");
            assert!(run_script_command(command, &input).unwrap().success());
            let deadline = Instant::now() + Duration::from_secs(2);
            while !output_path.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = fs_err::read_to_string(output_path);
            let _ = Command::new("tmux")
                .args(["-L", &socket, "kill-server"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            assert_eq!(output.unwrap(), "unset");
        }
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

        let offset = scroll(name, true, 3).expect("scroll position");
        assert_eq!(
            display(name, "#{pane_in_mode}"),
            "1",
            "wheel-up enters copy-mode"
        );
        let pos: i32 = display(name, "#{scroll_position}").parse().unwrap_or(-1);
        assert!(pos > 0, "scroll_position should advance, got {pos}");
        assert_eq!(offset, pos as usize);

        cancel_copy_mode(name);
        assert_eq!(
            display(name, "#{pane_in_mode}"),
            "0",
            "cancel returns to live"
        );

        kill_session(name);
    }

    #[test]
    fn selection_text_reads_rows_outside_the_visible_tmux_viewport() {
        if !available() {
            eprintln!("skipping: tmux not on PATH");
            return;
        }
        let name = "grove__selftest__selection__0";
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
            "6",
            "sh",
            "-c",
            "for i in $(seq 1 15); do printf 'line-%02d-abcdefghij\\n' \"$i\"; done; sleep 30",
        ]);
        assert!(
            run_silent(create).expect("spawn").success(),
            "new-session failed"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));

        assert_eq!(
            display(name, "#{pane_height}"),
            "6",
            "selection fixture viewport must stay fixed"
        );

        // Output ends in a newline: absolute row 0 is blank, row 1 is
        // line-15, row 2 is line-14, and row 12 is line-04. Endpoint columns
        // are zero-based and inclusive, exactly as GroveTerm::selection_text.
        let expected = "-abcdefghij\nline-05-abcdefghij\nline-06-abcdefghij\nline-07-abcdefghij\nline-08-abcdefghij\nline-09-abcdefghij\nline-10-abcdefghij\nline-11-abcdefghij\nline-12-abcdefghij\nline-13-abcdefghij\nline-1";
        for mode in ["emacs", "vi"] {
            let mut set_mode = tmux();
            set_mode.args(["set-window-option", "-t", name, "mode-keys", mode]);
            assert!(run_silent(set_mode).expect("set mode").success());
            assert_eq!(
                selection_text(name, (12, 7), (2, 5), 0).as_deref(),
                Some(expected),
                "history selection with {mode} keys"
            );
            let offset = scroll(name, true, 3).expect("scroll position");
            assert_eq!(
                selection_text(name, (2, 5), (12, 7), offset).as_deref(),
                Some(expected),
                "reversed history selection with {mode} keys"
            );
            assert_eq!(
                display(name, "#{scroll_position}"),
                offset.to_string(),
                "copy should restore the drag viewport"
            );
            assert_eq!(
                selection_text(name, (2, 0), (2, 0), 0).as_deref(),
                Some("l")
            );
            assert_eq!(
                selection_text(name, (2, 5), (2, 7), 0).as_deref(),
                Some("14-")
            );
        }

        kill_session(name);
    }
}
