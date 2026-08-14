//! Attention-signal detection: hooks append a state word to a per-session file. Failures are silent by design — no signal, not a crash.

use crate::agent::Agent;
use crate::error::Result;
use fs_err as fs;
use std::path::{Path, PathBuf};

/// Env var carrying a session's state-file path to the spawned agent process.
pub const STATE_FILE_ENV: &str = "GROVE_STATE_FILE";

/// `None` (missing/empty/unrecognized last line) means "no signal" — callers fall back to the running/idle baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionState {
    Working,
    NeedsYou,
    Done,
}

#[derive(Clone, Debug)]
pub struct AttentionFiles {
    pub state_file: PathBuf,
    /// `None` for Codex, which needs no extra file.
    pub settings_file: Option<PathBuf>,
}

/// Routed through `storage::config_dir()` so legacy migration and `GROVE_CONFIG_DIR` both apply.
fn attention_dir() -> Result<PathBuf> {
    let dir = crate::storage::config_dir()?.join("attention");
    fs::create_dir_all(&dir)?;
    restrict_dir(&dir);
    Ok(dir)
}

/// Best-effort 0700 on a grove-owned directory. No-op off unix.
pub(crate) fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::debug!(path = %dir.display(), error = %e, "attention: chmod dir failed");
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Errs on the side of "alive" (keep the file) when it can't tell.
fn pid_is_live(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // pid 0/negative and anything overflowing pid_t can't name a real process — dead by definition.
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // signal 0 checks existence without signalling; EPERM (a live process we don't own) also means "alive".
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Garbage-collects previous runs' files, keyed by the pid prefix in each filename (`{run}-{session_id}.state`).
pub fn cleanup_stale_files() {
    if let Ok(dir) = attention_dir() {
        clear_dir(&dir);
    }
}

fn clear_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if !is_stale_name(&name.to_string_lossy()) {
            continue;
        }
        let _ = fs::remove_file(path);
    }
}

/// Anything that doesn't parse as `{pid}-…` is left alone rather than risk deleting a live session's file.
fn is_stale_name(name: &str) -> bool {
    let Some((pid, _)) = name.split_once('-') else {
        return false;
    };
    match pid.parse::<u32>() {
        Ok(pid) => !pid_is_live(pid),
        Err(_) => false,
    }
}

/// `None` for OpenCode, Terminal, and (v1) Windows, or if the settings file can't be written.
pub fn prepare(agent: Agent, session_id: u64) -> Option<(Vec<String>, AttentionFiles)> {
    #[cfg(windows)]
    {
        let _ = (agent, session_id);
        None
    }
    #[cfg(not(windows))]
    {
        match agent {
            Agent::Claude => {
                let state_file = match state_file_path(session_id) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(error = %e, "attention: state file path failed");
                        return None;
                    }
                };
                let settings_file = match write_claude_settings(session_id, &state_file) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(error = %e, "attention: write claude settings failed");
                        return None;
                    }
                };
                Some((
                    vec!["--settings".into(), settings_file.display().to_string()],
                    AttentionFiles {
                        state_file,
                        settings_file: Some(settings_file),
                    },
                ))
            }
            Agent::Codex => {
                let state_file = match state_file_path(session_id) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(error = %e, "attention: state file path failed");
                        return None;
                    }
                };
                Some((
                    vec!["-c".into(), codex_notify_arg()],
                    AttentionFiles {
                        state_file,
                        settings_file: None,
                    },
                ))
            }
            Agent::OpenCode | Agent::Terminal => None,
        }
    }
}

fn state_file_path(session_id: u64) -> Result<PathBuf> {
    let run = std::process::id();
    let path = attention_dir()?.join(format!("{run}-{session_id}.state"));
    // Pre-create 0600 so the agent's `>>` append inherits our mode, not its umask.
    create_private(&path);
    Ok(path)
}

/// Best-effort create-if-absent with 0600 on unix. No-op off unix.
fn create_private(path: &Path) {
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        if let Err(e) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
        {
            tracing::debug!(path = %path.display(), error = %e, "attention: create private file failed");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Best-effort 0600 on an existing file. No-op off unix.
fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::debug!(path = %path.display(), error = %e, "attention: chmod file failed");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn settings_file_path(session_id: u64) -> Result<PathBuf> {
    let run = std::process::id();
    Ok(attention_dir()?.join(format!("{run}-{session_id}.claude-settings.json")))
}

/// A failed append is swallowed by shell redirection semantics, never surfaced to the agent CLI.
fn append_command(word: &str) -> String {
    format!("echo {word} >> \"${STATE_FILE_ENV}\"")
}

/// `--settings` merges with the user's own settings rather than replacing them.
pub fn claude_settings_json() -> String {
    let hook = |command: String| {
        serde_json::json!([{
            "hooks": [{ "type": "command", "command": command }]
        }])
    };
    let doc = serde_json::json!({
        "hooks": {
            "Notification": hook(append_command("needs-you")),
            "Stop": hook(append_command("done")),
            "UserPromptSubmit": hook(append_command("working")),
        }
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

#[cfg(not(windows))]
fn write_claude_settings(session_id: u64, _state_file: &Path) -> Result<PathBuf> {
    let path = settings_file_path(session_id)?;
    let json = claude_settings_json();
    // 0600 from creation so the file is never briefly world-readable before a chmod.
    crate::storage::write_atomic_private(&path, json.as_bytes())?;
    restrict_file(&path);
    Ok(path)
}

/// Codex appends a JSON payload as one more argument, ignored by our one-liner.
#[cfg(not(windows))]
fn codex_notify_arg() -> String {
    let cmd = append_command("done");
    let value = serde_json::json!(["sh", "-c", cmd]);
    format!("notify={value}")
}

/// Last line wins, so a burst of hook fires resolves to the most recent one.
pub fn parse_state(raw: &str) -> Option<AttentionState> {
    raw.lines().rev().find_map(|line| match line.trim() {
        "working" => Some(AttentionState::Working),
        "needs-you" => Some(AttentionState::NeedsYou),
        "done" => Some(AttentionState::Done),
        _ => None,
    })
}

/// Keyed on (mtime, len) so the steady state costs one `metadata()` call instead of a read+parse.
type StateCache =
    std::collections::HashMap<PathBuf, (std::time::SystemTime, u64, Option<AttentionState>)>;
static STATE_CACHE: std::sync::Mutex<Option<StateCache>> = std::sync::Mutex::new(None);

pub fn read_state(path: &Path) -> Option<AttentionState> {
    use std::io::{Read, Seek, SeekFrom};

    // Files grow unbounded (append-only); only the last line matters, so read a bounded tail.
    const TAIL_BYTES: u64 = 4096;

    let mut file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    let stamp = meta.modified().ok();

    // The cache is pure derived data, so recover from a poisoned lock rather than propagate.
    let mut guard = STATE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cache = guard.get_or_insert_with(Default::default);
    if let Some(stamp) = stamp {
        if let Some((cached_stamp, cached_len, value)) = cache.get(path) {
            if *cached_stamp == stamp && *cached_len == len {
                return *value;
            }
        }
    }

    if len > TAIL_BYTES {
        file.seek(SeekFrom::Start(len - TAIL_BYTES)).ok()?;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES.min(len) as usize);
    file.read_to_end(&mut buf).ok()?;
    let value = parse_state(&String::from_utf8_lossy(&buf));
    if let Some(stamp) = stamp {
        cache.insert(path.to_path_buf(), (stamp, len, value));
    }
    value
}

/// Truncates rather than deletes, so hooks can keep appending to the same path.
pub fn acknowledge(path: &Path) {
    // This truncate races the agent's `>>` append — benign today (worst case one missed signal); see history for the seq-numbered fix if that changes.
    if let Err(e) = fs::write(path, b"") {
        tracing::debug!(path = %path.display(), error = %e, "attention: acknowledge write failed");
    }
}

pub fn cleanup(files: &AttentionFiles) {
    // Drop the cache entry too, so the map doesn't grow for the process's life.
    if let Ok(mut guard) = STATE_CACHE.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.remove(&files.state_file);
        }
    }
    if let Err(e) = fs::remove_file(&files.state_file) {
        tracing::debug!(path = %files.state_file.display(), error = %e, "attention: cleanup remove state file failed");
    }
    if let Some(settings) = &files.settings_file {
        if let Err(e) = fs::remove_file(settings) {
            tracing::debug!(path = %settings.display(), error = %e, "attention: cleanup remove settings file failed");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn parse_state_recognizes_each_word() {
        assert_eq!(parse_state("working"), Some(AttentionState::Working));
        assert_eq!(parse_state("needs-you"), Some(AttentionState::NeedsYou));
        assert_eq!(parse_state("done"), Some(AttentionState::Done));
    }

    #[test]
    fn parse_state_last_line_wins() {
        assert_eq!(
            parse_state("working\nneeds-you\ndone"),
            Some(AttentionState::Done)
        );
        assert_eq!(parse_state("done\nworking"), Some(AttentionState::Working));
    }

    #[test]
    fn parse_state_trailing_blank_lines_ignored() {
        assert_eq!(
            parse_state("needs-you\n\n\n"),
            Some(AttentionState::NeedsYou)
        );
    }

    #[test]
    fn parse_state_empty_or_garbage_is_none() {
        assert_eq!(parse_state(""), None);
        assert_eq!(parse_state("\n\n"), None);
        assert_eq!(parse_state("some garbage line"), None);
    }

    #[test]
    fn parse_state_skips_unrecognized_trailing_line() {
        assert_eq!(parse_state("working\n???"), Some(AttentionState::Working));
    }

    fn tmp_state_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "grove_test_attention_{}_{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create test dir");
        dir.join("session.state")
    }

    #[test]
    fn read_state_missing_file_is_none() {
        let path = tmp_state_file("missing");
        assert_eq!(read_state(&path), None);
    }

    #[test]
    fn read_state_reads_last_appended_word() {
        let path = tmp_state_file("read");
        fs::write(&path, "working\nneeds-you\n").unwrap();
        assert_eq!(read_state(&path), Some(AttentionState::NeedsYou));
    }

    #[test]
    fn acknowledge_truncates_file_to_no_signal() {
        let path = tmp_state_file("ack");
        fs::write(&path, "needs-you\n").unwrap();
        assert_eq!(read_state(&path), Some(AttentionState::NeedsYou));
        acknowledge(&path);
        assert_eq!(read_state(&path), None);
        assert!(path.exists());
    }

    #[test]
    fn clear_dir_removes_dead_pid_files_but_keeps_the_dir() {
        let dir = tmp_state_file("cleanup").parent().unwrap().to_path_buf();
        fs::write(dir.join("4294967295-0.state"), "needs-you\n").unwrap();
        fs::write(dir.join("4294967295-1.claude-settings.json"), "{}").unwrap();
        clear_dir(&dir);
        assert!(dir.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn clear_dir_keeps_files_of_live_pids_and_unattributable_names() {
        let dir = tmp_state_file("cleanup_live")
            .parent()
            .unwrap()
            .to_path_buf();
        let live = dir.join(format!("{}-0.state", std::process::id()));
        let odd = dir.join("not-a-pid.state");
        fs::write(&live, "working\n").unwrap();
        fs::write(&odd, "working\n").unwrap();
        clear_dir(&dir);
        assert!(
            live.exists(),
            "a live pid's state file must survive cleanup"
        );
        assert!(odd.exists());
    }

    #[test]
    fn read_state_reads_only_the_tail_of_a_huge_file() {
        let path = tmp_state_file("tail");
        let mut raw = "working\n".repeat(20_000);
        raw.push_str("needs-you\n");
        fs::write(&path, &raw).unwrap();
        assert_eq!(read_state(&path), Some(AttentionState::NeedsYou));
    }

    #[test]
    fn clear_dir_missing_dir_is_a_silent_noop() {
        let dir = std::env::temp_dir().join("grove_test_attention_missing_dir_xyz");
        let _ = fs::remove_dir_all(&dir);
        clear_dir(&dir);
    }

    #[test]
    fn claude_settings_json_is_valid_json() {
        let json = claude_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(parsed.get("hooks").is_some());
    }

    #[test]
    fn claude_settings_json_declares_all_three_hook_events() {
        let json = claude_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let hooks = &parsed["hooks"];
        for event in ["Notification", "Stop", "UserPromptSubmit"] {
            assert!(
                hooks.get(event).is_some(),
                "missing hook event {event:?} in {json}"
            );
        }
    }

    #[test]
    fn claude_settings_json_hook_shape_matches_schema() {
        let json = claude_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &parsed["hooks"]["Notification"][0]["hooks"][0];
        assert_eq!(entry["type"], "command");
        let command = entry["command"].as_str().expect("command is a string");
        assert!(command.contains("needs-you"));
        assert!(command.contains(STATE_FILE_ENV));
    }

    #[test]
    fn claude_settings_json_maps_events_to_correct_words() {
        let json = claude_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cmd = |event: &str| {
            parsed["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(cmd("Notification").contains("needs-you"));
        assert!(cmd("Stop").contains("done"));
        assert!(cmd("UserPromptSubmit").contains("working"));
    }

    #[test]
    fn append_command_references_env_var_not_literal_path() {
        let cmd = append_command("done");
        assert_eq!(cmd, "echo done >> \"$GROVE_STATE_FILE\"");
    }

    #[cfg(not(windows))]
    #[test]
    fn prepare_claude_appends_settings_flag_and_returns_files() {
        let (args, files) = prepare(Agent::Claude, 999_001).expect("claude should prepare");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "--settings");
        let pid = std::process::id().to_string();
        assert!(args[1].contains(&pid));
        assert!(args[1].ends_with("999001.claude-settings.json"));
        assert!(files.settings_file.is_some());
        assert!(files.state_file.to_string_lossy().contains(&pid));
        assert!(files.state_file.ends_with(format!("{pid}-999001.state")));
        let contents = fs::read_to_string(files.settings_file.as_ref().unwrap()).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&contents).is_ok());
        cleanup(&files);
    }

    #[cfg(not(windows))]
    #[test]
    fn prepare_codex_appends_notify_flag() {
        let (args, files) = prepare(Agent::Codex, 999_002).expect("codex should prepare");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("notify="));
        assert!(args[1].contains("done"));
        assert!(files.settings_file.is_none());
        cleanup(&files);
    }

    #[test]
    fn prepare_opencode_and_terminal_return_none() {
        assert!(prepare(Agent::OpenCode, 999_003).is_none());
        assert!(prepare(Agent::Terminal, 999_004).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn prepare_is_disabled_on_windows() {
        assert!(prepare(Agent::Claude, 999_005).is_none());
        assert!(prepare(Agent::Codex, 999_006).is_none());
    }

    #[test]
    fn state_file_paths_contain_pid_prefix_and_differ_by_session_id() {
        let pid = std::process::id().to_string();
        let p1 = state_file_path(1).unwrap();
        let p2 = state_file_path(2).unwrap();
        let name1 = p1.file_name().unwrap().to_string_lossy();
        let name2 = p2.file_name().unwrap().to_string_lossy();
        assert_eq!(name1.as_ref(), format!("{pid}-1.state"));
        assert_eq!(name2.as_ref(), format!("{pid}-2.state"));
        assert_ne!(p1, p2);
    }
}
