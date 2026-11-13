//! Opt-in authenticated generation; uses no checkout context or persistent conversation.
use grove_core::{
    codex::{Provider, SavedSession, Snapshot, Status},
    commit_agent::CommitControl,
    next_reply::Request,
};
use serde_json::json;
fn main() -> Result<(), String> {
    let provider = match std::env::args().nth(1).as_deref() {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: next_reply_smoke codex|claude".into()),
    };
    let config = tempfile::tempdir().map_err(|e| e.to_string())?;
    std::env::set_var(grove_core::storage::CONFIG_DIR_ENV, config.path());
    let saved: SavedSession = serde_json::from_value(json!({"provider":provider,"key":"next-reply-smoke","project":"Smoke","cwd":config.path(),"label":"Smoke","roots":[],"thread_id":null,"items":[
        {"id":"u","type":"userMessage","content":[{"type":"inputText","text":"Fix the draft preservation bug and show me the patch before adding tests."}]},
        {"id":"a","type":"agentMessage","text":"The patch is ready. Shall I add regression tests for draft preservation?"}
    ]})).map_err(|e|e.to_string())?;
    let mut state = Snapshot::new(saved);
    state.status = Status::Done;
    let request = Request::capture(&state).ok_or("Missing response context")?;
    let result = request.generate(&CommitControl::default())?;
    if result.is_empty() {
        return Err("Provider returned no suggestion for the fixture".into());
    }
    println!(
        "{} next-reply structured generation passed ({} suggestions, lengths: {:?})",
        provider.label(),
        result.len(),
        result.iter().map(|s| s.chars().count()).collect::<Vec<_>>()
    );
    Ok(())
}
