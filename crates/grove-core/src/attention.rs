//! Attention-signal detection for Claude and Codex sessions.
//!
//! Grove owns the spawn command line for every agent session, so it can wire
//! up hook/notify configuration with zero user setup: each session gets a
//! small per-session state file, whose path is passed to the agent process
//! via the `GROVE_STATE_FILE` env var. Claude's generated `--settings` file
//! declares `Notification`/`Stop`/`UserPromptSubmit` hooks that each append a
//! state word to that file; Codex's `-c notify=[...]` override does the same
//! on the agent-turn-complete event (Codex exposes no approval-prompt event,
//! so it only ever reaches `working`/`done`).
//!
//! The UI's existing activity tick (see `gui::update::refresh_activity`)
//! reads these files each pass and prefers their deterministic signal over
//! the screen-scraping heuristics in `gui::activity` whenever one is present.
//!
//! Failures here are silent by design (see the design doc): a state file
//! that can't be written or read just means "no signal", never a crash or a
//! visible error in the agent CLI.
//!
//! Windows: hook/notify injection is gated off for v1 (see `prepare`) rather
//! than risk a shell one-liner that doesn't work under `pwsh`/`cmd.exe`.
//! Windows sessions simply keep the existing running/idle baseline.

use crate::agent::Agent;
use crate::error::Result;
use fs_err as fs;
use std::path::{Path, PathBuf};

/// Env var carrying a session's state-file path to the spawned agent process.
pub const STATE_FILE_ENV: &str = "GROVE_STATE_FILE";

/// Deterministic attention state parsed from a session's state file. `None`
/// (missing file, empty file, or an unrecognized last line) means "no
/// signal" — callers fall back to the existing running/idle baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionState {
    Working,
    NeedsYou,
    Done,
}

/// Files backing one session's attention signal, plumbed onto `Session` so
/// they can be polled on the UI tick and cleaned up when the session closes.
#[derive(Clone, Debug)]
pub struct AttentionFiles {
    pub state_file: PathBuf,
    /// Claude only: the generated `--settings` file passed on the command
    /// line. `None` for Codex, which needs no extra file.
    pub settings_file: Option<PathBuf>,
}

/// `<storage::config_dir()>/attention`. Routed through `storage::config_dir()`
/// rather than deriving `dirs::config_dir().join("grove")` here, so the legacy
/// directory migration and the `GROVE_CONFIG_DIR` override both apply.
fn attention_dir() -> Result<PathBuf> {
    let dir = crate::storage::config_dir()?.join("attention");
    fs::create_dir_all(&dir)?;
    // The state files carry agent activity signals and the generated settings
    // files are read by the agent CLI — keep both out of other users' reach.
    restrict_dir(&dir);
    Ok(dir)
}

/// Best-effort 0700 on a grove-owned directory. No-op off unix. Shared with
/// `git`'s worktree directories, which carry the same "only this user" rule.
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

/// True when a process with this pid is running. Used to decide whether a
/// leftover attention file belongs to a live Grove or is genuinely stale.
/// Errs on the side of "alive" (keep the file) when it can't tell.
fn pid_is_live(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // kill(2) gives pid 0 and negative pids special meanings ("this
        // process group" / "every process"), and a pid that overflows pid_t
        // would wrap into that range — none of those can name a real single
        // process, so they are dead by definition, not a question for kill.
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // signal 0 performs the permission/existence checks without signalling.
        // EPERM (a live process we don't own) also means "alive".
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Delete every file under the attention dir. Call once on startup, before
/// any session is spawned.
///
/// This is purely garbage collection of previous runs' files. Cross-run
/// collisions are prevented by the pid prefix in the filenames
/// (`{run}-{session_id}.state`): `NEXT_SESSION_ID` resets to 0 each run,
/// but each run's pid is unique, so two runs with the same session id still
/// produce distinct paths. Surviving tmux agents from prior runs will
/// recreate their old state files via `>>` appends, but those recreated
/// files are orphans — nobody in the current run reads them — and they are
/// collected on the next startup. Best-effort: a failure here just means
/// leftover files linger, never a startup error.
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

/// True when a filename's `{pid}-` prefix names a process that is no longer
/// running. Anything that doesn't parse as `{pid}-…` is left alone: another
/// Grove instance (or a surviving tmux agent still appending) owns files we
/// can't attribute, and deleting a live session's state file silently drops
/// its attention signal for the rest of its life.
fn is_stale_name(name: &str) -> bool {
    let Some((pid, _)) = name.split_once('-') else {
        return false;
    };
    match pid.parse::<u32>() {
        Ok(pid) => !pid_is_live(pid),
        Err(_) => false,
    }
}

/// Prepare hook/notify injection for a freshly spawned session, if this
/// agent/platform combination supports it. Returns `None` for OpenCode,
/// Terminal, and (v1) Windows — those sessions keep the existing
/// running/idle baseline with no extra args or env.
///
/// On success, returns the extra CLI args to append to the agent's launch
/// args and the file paths to remember on the `Session`. Best-effort:
/// returns `None` rather than failing the whole spawn if the settings file
/// can't be written.
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
    // Pre-create it 0600 so the agent's `>>` append inherits our mode rather
    // than whatever the agent's umask would have produced.
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

/// Shell one-liner that appends `word` to the state file named by
/// `GROVE_STATE_FILE`. A failed append (missing env var, unwritable path,
/// etc.) is swallowed by shell redirection semantics rather than surfacing
/// as an error the agent CLI would see.
fn append_command(word: &str) -> String {
    format!("echo {word} >> \"${STATE_FILE_ENV}\"")
}

/// Build the JSON contents of a generated Claude `--settings` file. `--settings`
/// merges with the user's own settings rather than replacing them, so this
/// never touches anything the user configured themselves.
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
    // 0600 from creation, so the file is never briefly world-readable between
    // the rename and a post-hoc chmod. `restrict_file` still runs to tighten an
    // already-existing file left over from a previous, looser write.
    crate::storage::write_atomic_private(&path, json.as_bytes())?;
    restrict_file(&path);
    Ok(path)
}

/// Codex `-c notify=[...]` value: a shell + `-c` + our append-"done" one-liner,
/// matching the `program + args` array Codex's `notify` config expects. Codex
/// invokes this on the agent-turn-complete event, appending a JSON payload as
/// one more argument (ignored by the one-liner, which only reads `$0`-style
/// positional args it never references).
#[cfg(not(windows))]
fn codex_notify_arg() -> String {
    let cmd = append_command("done");
    let value = serde_json::json!(["sh", "-c", cmd]);
    format!("notify={value}")
}

/// Parse the last non-empty, recognized line of raw state-file contents.
/// Last line wins so a burst of hook fires (e.g. `working` immediately
/// followed by `done`) resolves to the most recent one. Unrecognized lines
/// are skipped rather than treated as an error.
pub fn parse_state(raw: &str) -> Option<AttentionState> {
    raw.lines().rev().find_map(|line| match line.trim() {
        "working" => Some(AttentionState::Working),
        "needs-you" => Some(AttentionState::NeedsYou),
        "done" => Some(AttentionState::Done),
        _ => None,
    })
}

/// Last (mtime, len) seen per state file and the value parsed from it. The UI
/// polls every session's state file several times a second, but the files only
/// change when a hook fires — keying on the metadata we already have to stat
/// turns the steady state into one `metadata()` call instead of a 4 KiB read
/// plus a parse.
type StateCache =
    std::collections::HashMap<PathBuf, (std::time::SystemTime, u64, Option<AttentionState>)>;
static STATE_CACHE: std::sync::Mutex<Option<StateCache>> = std::sync::Mutex::new(None);

/// Read and parse a session's state file. `None` — missing file, unreadable,
/// or unparseable — is treated identically to "no signal" (see module docs).
pub fn read_state(path: &Path) -> Option<AttentionState> {
    use std::io::{Read, Seek, SeekFrom};

    // These files are append-only for the life of a session and only ever
    // truncated on acknowledge, so a long-running agent's file grows without
    // bound. Only the last line matters — read a bounded tail rather than the
    // whole file on every UI tick. A partial first line in the window is
    // harmless: `parse_state` skips unrecognized lines.
    const TAIL_BYTES: u64 = 4096;

    let mut file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    let stamp = meta.modified().ok();

    // A poisoned lock here would mean a panic inside this tiny critical
    // section; the cache is pure derived data, so recover rather than
    // propagate.
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

/// Clear a session's recorded signal back to baseline. Called when the user
/// focuses/views the session — mirrors `gui::activity::Tracker::acknowledge`.
/// Truncates (rather than deletes) so the hooks can keep appending to the
/// same path for the life of the session. Best-effort: a failed truncate is
/// silently ignored, same as every other write in this module.
pub fn acknowledge(path: &Path) {
    // ponytail: this truncate races the agent's `>>` append — a hook that fires
    // between our read and this write has its word discarded, and a pid that is
    // reused across runs can have a surviving tmux agent appending to a path a
    // new session now owns. Both are benign today (worst case one missed signal
    // until the next hook fires) and the file-per-session + last-line-wins
    // protocol is deliberately dumb. If signals are ever observed genuinely
    // dropping, the fix is a seq-numbered O_APPEND protocol — each hook writes
    // `{seq} {word}` and acknowledge records the last seq it saw instead of
    // truncating — not a lock or a rewrite of the hook wiring.
    if let Err(e) = fs::write(path, b"") {
        tracing::debug!(path = %path.display(), error = %e, "attention: acknowledge write failed");
    }
}

/// Remove a session's attention files when it closes. Best-effort per the
/// design doc's cleanup note.
pub fn cleanup(files: &AttentionFiles) {
    // Drop the cache entry too, so the map doesn't grow for the life of the
    // process as sessions come and go.
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
    use super::*;

    // ── parse_state ──────────────────────────────────────────────────────────

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
        // A stray/corrupted line after a real word must not blank the result.
        assert_eq!(parse_state("working\n???"), Some(AttentionState::Working));
    }

    // ── read_state / acknowledge (real filesystem, tmp dir) ─────────────────

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
        // The file itself must still exist (truncated, not removed) so hooks
        // can keep appending to the same path.
        assert!(path.exists());
    }

    // ── clear_dir (startup cleanup) ──────────────────────────────────────────

    #[test]
    fn clear_dir_removes_dead_pid_files_but_keeps_the_dir() {
        let dir = tmp_state_file("cleanup").parent().unwrap().to_path_buf();
        // u32::MAX is above every platform's pid_max, so it is never live.
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
        clear_dir(&dir); // must not panic
    }

    // ── claude_settings_json ─────────────────────────────────────────────────

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

    // ── append_command ───────────────────────────────────────────────────────

    #[test]
    fn append_command_references_env_var_not_literal_path() {
        // The command must reference $GROVE_STATE_FILE rather than a baked-in
        // path, so the same generated settings file works across sessions
        // (and the path never needs shell-escaping).
        let cmd = append_command("done");
        assert_eq!(cmd, "echo done >> \"$GROVE_STATE_FILE\"");
    }

    // ── prepare (non-Windows) ─────────────────────────────────────────────────

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
        // The settings file must actually exist on disk with valid JSON.
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

    // ── path format ──────────────────────────────────────────────────────────

    #[test]
    fn state_file_paths_contain_pid_prefix_and_differ_by_session_id() {
        let pid = std::process::id().to_string();
        let p1 = state_file_path(1).unwrap();
        let p2 = state_file_path(2).unwrap();
        let name1 = p1.file_name().unwrap().to_string_lossy();
        let name2 = p2.file_name().unwrap().to_string_lossy();
        // Each name must be "{pid}-{session_id}.state".
        assert_eq!(name1.as_ref(), format!("{pid}-1.state"));
        assert_eq!(name2.as_ref(), format!("{pid}-2.state"));
        // Different session ids must produce different paths.
        assert_ne!(p1, p2);
    }
}
