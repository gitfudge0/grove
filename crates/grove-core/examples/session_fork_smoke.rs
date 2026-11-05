//! Live fork isolation check; requires disposable GROVE_CONFIG_DIR and working directory.
use grove_core::codex::{self, Action, Connection, Provider, Snapshot, Status};
use std::time::Duration;
fn wait(connection: &Connection, status: Status) -> Result<Snapshot, String> {
    loop {
        let state = connection
            .updates
            .recv_timeout(Duration::from_secs(90))
            .map_err(|e| e.to_string())?;
        if matches!(state.status, Status::Disconnected | Status::Failed) {
            return Err(state.error.unwrap_or_default());
        }
        if state.status == status {
            return Ok(state);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Use a disposable GROVE_CONFIG_DIR".into());
    }
    let cwd = std::env::args()
        .nth(1)
        .ok_or("Supply a disposable working directory")?;
    let provider = if std::env::args().any(|arg| arg == "--claude") {
        Provider::Claude
    } else {
        Provider::Codex
    };
    let saved = serde_json::from_value(serde_json::json!({"key":grove_core::identity::random_id()?,"project":"","cwd":cwd,"label":"Fork smoke","roots":[],"thread_id":null,"items":[],"provider":provider,"native_config_version":1})).map_err(|e|e.to_string())?;
    let source = codex::connect(saved, Vec::new());
    wait(&source, Status::Idle)?;
    source.handle.send(Action::Prompt(
        "Remember the secret word MARIGOLD. Reply only OK. Do not use tools.".into(),
    ))?;
    let first = wait(&source, Status::Done)?;
    let boundary = first.saved.items.len();
    source.handle.send(Action::Prompt(
        "Replace the secret word with BLUEBELL. Reply only OK. Do not use tools.".into(),
    ))?;
    let second = wait(&source, Status::Done)?;
    let mut user_boundary = boundary;
    while second
        .saved
        .items
        .get(user_boundary)
        .is_some_and(|item| item["type"] != "userMessage")
    {
        user_boundary += 1;
    }
    let user_fork = second.saved.fork_at(user_boundary, true)?;
    if !user_fork.draft.contains("BLUEBELL") {
        return Err("User fork lost selected prompt".into());
    }
    let saved = second.saved.fork_at(boundary, false)?;
    if !saved.draft.is_empty() {
        return Err("Response fork copied a later prompt".into());
    }
    let title = saved.title.clone();
    let fork = codex::connect(saved, Vec::new());
    let ready = wait(&fork, Status::Idle)?;
    fork.handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let recovered = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|saved| saved.key == ready.saved.key)
        .ok_or("Fork missing after shutdown")?;
    let fork = codex::connect(recovered, Vec::new());
    let ready = wait(&fork, Status::Idle)?;
    let nested_saved = ready.saved.fork_at(boundary, false)?;
    let nested = codex::connect(nested_saved, Vec::new());
    let nested_ready = wait(&nested, Status::Idle)?;
    if nested_ready.saved.thread_id == ready.saved.thread_id {
        return Err("Nested fork reused identity".into());
    }
    nested.handle.send(Action::Prompt(
        "What is the secret word? Reply only the word. Do not use tools.".into(),
    ))?;
    let nested_done = wait(&nested, Status::Done)?;
    let nested_reply = nested_done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(|item| codex::item_text(item))
        .unwrap_or_default();
    if !nested_reply.contains("MARIGOLD") {
        return Err(format!("Nested fork lost context: {nested_reply}"));
    }
    if ready.saved.thread_id == second.saved.thread_id {
        return Err("Fork reused source identity".into());
    }
    if ready.saved.title != title {
        return Err("Fork title changed before submission".into());
    }
    fork.handle.send(Action::Prompt(
        "What is the secret word? Reply only the word. Do not use tools.".into(),
    ))?;
    let done = wait(&fork, Status::Done)?;
    let reply = done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(|item| codex::item_text(item))
        .unwrap_or_default();
    if !reply.contains("MARIGOLD") || reply.contains("BLUEBELL") {
        return Err(format!("Incorrect fork context: {reply}"));
    }
    if done.saved.fork_origin.is_some() {
        return Err("Fork title hold survived accepted input".into());
    }
    source.handle.send(Action::Prompt(
        "What is the secret word? Reply only the word. Do not use tools.".into(),
    ))?;
    let original = wait(&source, Status::Done)?;
    let original_reply = original
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(|item| codex::item_text(item))
        .unwrap_or_default();
    if !original_reply.contains("BLUEBELL") {
        return Err(format!("Fork changed source context: {original_reply}"));
    }
    for connection in [&source, &fork, &nested] {
        connection
            .handle
            .stop_completion(false)
            .recv_timeout(Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
    }
    println!("PASS {provider:?}: distinct fork identity, selected prefix context, held title, user draft, restart recovery, nested forks, released title hold, unchanged source context");
    Ok(())
}
