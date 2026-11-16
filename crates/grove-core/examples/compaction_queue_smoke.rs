//! Explicit live compaction/queue check using disposable GROVE_CONFIG_DIR and cwd.
//! Usage: compaction_queue_smoke codex|claude EMPTY_CWD
use grove_core::codex::{self, Action, Connection, SavedSession, Snapshot, Status, SubmissionMode};
use serde_json::json;
use std::time::{Duration, Instant};

struct StopOnDrop(grove_core::provider::Handle);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(true)
            .recv_timeout(Duration::from_secs(5));
    }
}
fn wait(
    connection: &Connection,
    label: &str,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        let state = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| format!("Timed out waiting for {label}"))?;
        if matches!(state.status, Status::Failed | Status::Disconnected) {
            return Err(format!(
                "Provider failed while waiting for {label}: {}",
                state.error.as_deref().unwrap_or("unknown")
            ));
        }
        if !state.approvals.is_empty() || !state.questions.is_empty() {
            return Err(format!(
                "Unexpected provider interaction while waiting for {label}"
            ));
        }
        if predicate(&state) {
            return Ok(state);
        }
    }
}
fn main() -> Result<(), String> {
    let provider = std::env::args().nth(1).ok_or("Supply codex or claude")?;
    if !matches!(provider.as_str(), "codex" | "claude") {
        return Err("Supply codex or claude".into());
    }
    let cwd = std::env::args()
        .nth(2)
        .ok_or("Supply an empty disposable cwd")?;
    let config =
        std::env::var("GROVE_CONFIG_DIR").map_err(|_| "Set disposable GROVE_CONFIG_DIR")?;
    if !std::path::Path::new(&config).is_absolute()
        || !std::path::Path::new(&cwd).is_absolute()
        || fs_err::read_dir(&cwd)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("Require absolute disposable config and empty cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let saved: SavedSession = serde_json::from_value(json!({
        "provider":provider,"key":grove_core::identity::random_id()?,"project":"Compaction queue smoke",
        "cwd":cwd,"label":"Smoke","roots":[],"thread_id":null,"items":[],"draft":"",
        "native_config_version":1,"activity_reporting":false
    })).map_err(|e| e.to_string())?;
    let connection = codex::connect(saved, vec![]);
    let _cleanup = StopOnDrop(connection.handle.clone());
    wait(&connection, "initialization", |s| s.status == Status::Idle)?;
    for interrupted in [false, true] {
        if interrupted {
            connection.handle.submit("Print the integers 1 through 10000, one per line. Do not use tools or modify files.".into(), vec![])?;
            wait(&connection, "active turn", |s| {
                s.status == Status::Working && s.turn_id.is_some()
            })?;
            connection.handle.send(Action::Interrupt)?;
            wait(&connection, "interruption", |s| {
                s.status == Status::Interrupted && s.turn_id.is_none()
            })?;
            // Codex Stop settles observed background work after its idle grace period.
            std::thread::sleep(Duration::from_secs(1));
        }
        let seed = connection.handle.submit(format!("Keep this disposable compaction fixture in context. Do not use tools or modify files. Reply only SEED_OK.\n{}", "A compaction fixture preserves queued input ordering.\n".repeat(1200)), vec![])?;
        wait(&connection, "seed completion", |s| {
            s.status == Status::Done && s.submission_accepted(seed)
        })?;
        connection.handle.send(Action::Compact)?;
        wait(&connection, "running compaction", |s| s.compacting)?;
        let marker = if interrupted {
            "COMPACT_QUEUE_AFTER_STOP"
        } else {
            "COMPACT_QUEUE_CLEAN"
        };
        let text = format!("Reply only {marker}. Do not use tools or modify files.");
        let queued = connection
            .handle
            .submit_with_mode(text, vec![], SubmissionMode::Queue)?;
        let done = wait(&connection, "automatic queued completion", |s| {
            s.status == Status::Done
                && !s.compacting
                && s.submission_item_id(queued).is_some()
                && !s.queued_prompts.iter().any(|p| p.id == queued)
        })?;
        let echoes = done
            .saved
            .items
            .iter()
            .filter(|item| item["type"] == "userMessage" && codex::item_text(item).contains(marker))
            .count();
        if echoes != 1 {
            return Err(format!("Expected one queued input echo, found {echoes}"));
        }
        let input = done
            .submission_item_id(queued)
            .ok_or("Missing queued receipt")?;
        let index = done
            .saved
            .items
            .iter()
            .position(|item| item["id"] == input)
            .ok_or("Missing input item")?;
        if !done
            .saved
            .items
            .iter()
            .skip(index + 1)
            .any(|item| item["type"] == "agentMessage" && codex::item_text(item).contains(marker))
        {
            return Err("Missing response to automatically dispatched queued message".into());
        }
        println!("PASS {provider} compaction queue: interrupted={interrupted}, exactly one input and matching response");
    }
    Ok(())
}
