//! Opt-in live current-chat queue smoke; use disposable config/cwd with authenticated CLI.
use grove_core::codex::{self, Action, Provider, SavedSession, Snapshot, Status};
use std::time::{Duration, Instant};
fn wait(
    connection: &codex::Connection,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = "no snapshots".to_owned();
    loop {
        let snapshot = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| format!("Timed out waiting for provider: {last}"))?;
        last = diagnostic(&snapshot);
        if matches!(snapshot.status, Status::Failed | Status::Disconnected) {
            return Err(format!("Provider failed: {last}"));
        }
        if !snapshot.approvals.is_empty() {
            return Err("Unexpected approval; smoke never grants approvals".into());
        }
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
    }
}
fn item_text(item: &serde_json::Value) -> String {
    if let Some(text) = item["text"].as_str() {
        return text.to_owned();
    }
    item["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn delivered_both(snapshot: &Snapshot) -> bool {
    let items = &snapshot.saved.items;
    ["FIRST", "SECOND"].iter().all(|reply| {
        let prompt = format!("Reply exactly {reply}. Do not use any tools.");
        items
            .iter()
            .any(|item| item["type"] == "userMessage" && item_text(item) == prompt)
            && items
                .iter()
                .any(|item| item["type"] == "agentMessage" && item_text(item).trim() == *reply)
    }) && !snapshot.status.busy()
        && snapshot.turn_id.is_none()
        && snapshot.queued_prompts.is_empty()
}
fn diagnostic(snapshot: &Snapshot) -> String {
    let users = snapshot
        .saved
        .items
        .iter()
        .filter(|item| item["type"] == "userMessage")
        .count();
    let assistants = snapshot
        .saved
        .items
        .iter()
        .filter(|item| item["type"] == "agentMessage")
        .count();
    format!("status={:?}, active_turn={}, users={users}, assistants={assistants}, queued={}, sending={}, uncertain={}, error_present={}", snapshot.status, snapshot.turn_id.is_some(), snapshot.queued_prompts.len(), snapshot.queue_sending.len(), snapshot.queue_uncertain.len(), snapshot.error.is_some())
}
struct Cleanup(codex::Handle);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(true)
            .recv_timeout(Duration::from_secs(10));
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set disposable GROVE_CONFIG_DIR".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => {
            return Err(
                "Usage: native_project_prompt_smoke codex|claude EMPTY_DISPOSABLE_CWD".into(),
            )
        }
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Use empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let saved: SavedSession = serde_json::from_value(serde_json::json!({"provider":provider,"key":grove_core::identity::random_id()?,"project":"Project prompt smoke","cwd":cwd,"label":"Smoke","roots":[],"thread_id":null,"items":[]})).map_err(|e|e.to_string())?;
    let connection = codex::connect(saved, Vec::new());
    let _cleanup = Cleanup(connection.handle.clone());
    wait(&connection, |s| s.status == Status::Idle)?;
    let draft = "Preserve this unsent draft";
    let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    connection.handle.send(Action::Draft(draft.into()))?;
    connection
        .handle
        .send(Action::DraftImages(images.clone()))?;
    let reference = codex::ReferenceDraft {
        id: "smoke-reference".into(),
        thread_id: None,
        sources: vec![codex::ReferenceSource {
            message_id: "smoke".into(),
            role: "assistant".into(),
        }],
        text: "Preserve quoted context".into(),
    };
    connection
        .handle
        .send(Action::DraftReference(Some(reference.clone())))?;
    let first = connection
        .handle
        .project_prompt("Reply exactly FIRST. Do not use any tools.".into())?;
    wait(&connection, |s| s.status.busy())?;
    let second = connection
        .handle
        .project_prompt("Reply exactly SECOND. Do not use any tools.".into())?;
    let queued = wait(&connection, |s| {
        s.queued_prompts.iter().any(|p| p.id == second) || s.accepted_submission == Some(second)
    })?;
    if queued.saved.draft != draft
        || queued.saved.draft_images != images
        || queued.saved.draft_reference != Some(reference.clone())
    {
        return Err("Composer changed during queue".into());
    }
    let final_state = wait(&connection, delivered_both)?;
    if final_state.saved.draft != draft
        || final_state.saved.draft_images != images
        || final_state.saved.draft_reference != Some(reference)
    {
        return Err("Composer changed after delivery".into());
    }
    println!("{} live project prompts {first}/{second}: two exact user prompts and assistant replies observed, settled turns, busy queue observed, draft/images/reference preserved; no tools/approval expected", provider.label());
    Ok(())
}
