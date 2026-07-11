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
use anyhow::{Context, Result};
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

fn attention_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir")?;
    let dir = base.join("grove").join("attention");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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
                let state_file = state_file_path(session_id).ok()?;
                let settings_file = write_claude_settings(session_id, &state_file).ok()?;
                Some((
                    vec!["--settings".into(), settings_file.display().to_string()],
                    AttentionFiles {
                        state_file,
                        settings_file: Some(settings_file),
                    },
                ))
            }
            Agent::Codex => {
                let state_file = state_file_path(session_id).ok()?;
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
    Ok(attention_dir()?.join(format!("{session_id}.state")))
}

fn settings_file_path(session_id: u64) -> Result<PathBuf> {
    Ok(attention_dir()?.join(format!("{session_id}.claude-settings.json")))
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
    crate::storage::write_atomic(&path, json.as_bytes())?;
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

/// Read and parse a session's state file. `None` — missing file, unreadable,
/// or unparseable — is treated identically to "no signal" (see module docs).
pub fn read_state(path: &Path) -> Option<AttentionState> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse_state(&raw)
}

/// Clear a session's recorded signal back to baseline. Called when the user
/// focuses/views the session — mirrors `gui::activity::Tracker::acknowledge`.
/// Truncates (rather than deletes) so the hooks can keep appending to the
/// same path for the life of the session. Best-effort: a failed truncate is
/// silently ignored, same as every other write in this module.
pub fn acknowledge(path: &Path) {
    let _ = std::fs::write(path, b"");
}

/// Remove a session's attention files when it closes. Best-effort per the
/// design doc's cleanup note.
pub fn cleanup(files: &AttentionFiles) {
    let _ = std::fs::remove_file(&files.state_file);
    if let Some(settings) = &files.settings_file {
        let _ = std::fs::remove_file(settings);
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
        std::fs::create_dir_all(&dir).expect("create test dir");
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
        std::fs::write(&path, "working\nneeds-you\n").unwrap();
        assert_eq!(read_state(&path), Some(AttentionState::NeedsYou));
    }

    #[test]
    fn acknowledge_truncates_file_to_no_signal() {
        let path = tmp_state_file("ack");
        std::fs::write(&path, "needs-you\n").unwrap();
        assert_eq!(read_state(&path), Some(AttentionState::NeedsYou));
        acknowledge(&path);
        assert_eq!(read_state(&path), None);
        // The file itself must still exist (truncated, not removed) so hooks
        // can keep appending to the same path.
        assert!(path.exists());
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
        assert!(args[1].ends_with("999001.claude-settings.json"));
        assert!(files.settings_file.is_some());
        assert!(files.state_file.ends_with("999001.state"));
        // The settings file must actually exist on disk with valid JSON.
        let contents = std::fs::read_to_string(files.settings_file.as_ref().unwrap()).unwrap();
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
}
