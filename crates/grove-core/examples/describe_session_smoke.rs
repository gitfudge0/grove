//! Live normal-conversation description smoke with isolated Grove recovery data.
//! Run with an existing, empty temporary GROVE_CONFIG_DIR:
//! cargo run -p grove-core --example describe_session_smoke -- codex|claude [PR-URL]
//! GROVE_DESCRIPTION_SMOKE_MODEL optionally overrides only this fixture session.
use grove_core::{
    change_description::{self, Scope},
    change_description_store::{Source, Store},
    codex::{self, Action, Connection, Handle, Provider, SavedSession, Status},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("Temporary fixture Git operation failed".into());
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn isolated_config() -> Result<PathBuf, String> {
    let path = std::env::var_os("GROVE_CONFIG_DIR")
        .map(PathBuf::from)
        .ok_or("Set GROVE_CONFIG_DIR to an existing, empty temporary directory")?;
    if !path.is_absolute() || !path.is_dir() {
        return Err("GROVE_CONFIG_DIR must be an existing absolute temporary directory".into());
    }
    if std::fs::read_dir(&path)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("Refusing to use nonempty Grove configuration for a live smoke".into());
    }
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

struct OwnedSession(Option<Handle>);
impl OwnedSession {
    fn stop(&mut self) -> Result<(), String> {
        if let Some(handle) = self.0.take() {
            handle
                .stop_completion(false)
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| "Owned provider session did not acknowledge shutdown".to_string())?;
        }
        Ok(())
    }
}
impl Drop for OwnedSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn generate_in_conversation(
    connection: &Connection,
    provider: Provider,
    prompt: String,
    snapshot: change_description::Snapshot,
) -> Result<change_description::Description, String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut sent = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Normal provider description session timed out".into());
        }
        let state = match connection
            .updates
            .recv_timeout(remaining.min(Duration::from_secs(1)))
        {
            Ok(state) => state,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Normal provider session disconnected".into());
            }
        };
        if let Some(error) = state.error {
            return Err(format!("Normal provider session failed: {error}"));
        }
        if !state.approvals.is_empty()
            || !state.questions.is_empty()
            || !state.elicitations.is_empty()
        {
            return Err(
                "Description unexpectedly requested a tool approval or user response".into(),
            );
        }
        if !sent
            && state.status == Status::Idle
            && (provider == Provider::Claude || state.saved.thread_id.is_some())
        {
            connection.handle.send(Action::Prompt(prompt.clone()))?;
            sent = true;
            eprintln!("Sent captured snapshot through normal {provider:?} conversation");
            continue;
        }
        if matches!(
            state.status,
            Status::Failed | Status::Disconnected | Status::Interrupted
        ) {
            return Err(format!(
                "Normal provider session ended as {:?}",
                state.status
            ));
        }
        if sent && state.status == Status::Done && state.turn_id.is_none() {
            let period = state
                .saved
                .work_periods
                .last()
                .filter(|period| period.end_item.is_some())
                .ok_or("Provider completion has no completed work period")?;
            let items: Vec<_> = state
                .saved
                .items
                .iter()
                .skip(period.start_item)
                .take(period.end_item.unwrap_or(period.start_item) - period.start_item)
                .collect();
            if !items.iter().any(|item| {
                item["type"] == "userMessage" && codex::item_text(item).contains(&prompt)
            }) {
                return Err("Completed provider turn does not own the captured prompt".into());
            }
            let response = items
                .iter()
                .rev()
                .find(|item| item["type"] == "agentMessage")
                .map(|item| codex::item_text(item))
                .ok_or("Completed provider turn has no assistant result")?;
            return change_description::from_conversation(snapshot, &response);
        }
    }
}

fn main() -> Result<(), String> {
    let provider = match std::env::args().nth(1).as_deref() {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: describe_session_smoke codex|claude [PR-URL]".into()),
    };
    let config = isolated_config()?;
    let fixture = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = fixture.path();
    git(path, &["init", "-b", "main"])?;
    git(path, &["config", "user.name", "Grove Session Smoke"])?;
    git(path, &["config", "user.email", "grove-smoke@example.test"])?;
    std::fs::write(
        path.join("handler.ts"),
        "export function shouldProcess(seen: boolean) { return true; }\n",
    )
    .map_err(|error| error.to_string())?;
    git(path, &["add", "."])?;
    git(path, &["commit", "-m", "Fixture"])?;
    std::fs::write(
        path.join("handler.ts"),
        "export function shouldProcess(seen: boolean) { return !seen; }\n",
    )
    .map_err(|error| error.to_string())?;
    let before = git(path, &["status", "--porcelain=v1"])?;
    let head = git(path, &["rev-parse", "HEAD"])?;
    let index = git(path, &["ls-files", "--stage"])?;
    let scope = std::env::args()
        .nth(2)
        .map_or(Scope::Local, |url| Scope::PullRequest { url });
    let snapshot = change_description::prepare(path, &scope)?;
    let prompt = change_description::conversation_prompt(&snapshot)?;
    let source = Source {
        checkout: snapshot.checkout.clone(),
        scope: snapshot.scope.clone(),
    };
    let store_root = config.join("result-store");
    let store = Store::new(store_root.clone());
    let key = grove_core::identity::random_id()?;
    store.bind(&key, &source)?;
    store.save_pending(&key, &snapshot)?;
    let generation = store
        .pending_generation(&key)?
        .ok_or("Missing generation identity")?;
    let prompt = format!("{prompt}\n\nGrove request identity: {generation}. This identifies this generation attempt; do not include it in the result.");
    let mut saved = SavedSession::empty(
        key.clone(),
        provider,
        "Session smoke fixture".into(),
        path.to_string_lossy().into_owned(),
    );
    if let Ok(model) = std::env::var("GROVE_DESCRIPTION_SMOKE_MODEL") {
        if model.trim().is_empty() {
            return Err("GROVE_DESCRIPTION_SMOKE_MODEL must name an available model".into());
        }
        saved.composer.model = Some(model);
        saved.composer.effort = Some("low".into());
    }
    let args = provider.agent().launch_args_for_backend(true, false, false);
    let connection = codex::connect(saved, args);
    let mut owned = OwnedSession(Some(connection.handle.clone()));
    let generated = generate_in_conversation(&connection, provider, prompt, snapshot);
    let shutdown = owned.stop();
    let description = generated?;
    shutdown?;
    let expected_path = store.result_path(&source)?;
    let saved_path = store.save_generation(&key, &description, &generation)?;
    let restarted = Store::new(store_root);
    if expected_path != saved_path
        || restarted.result_path(&source)? != saved_path
        || restarted.source_for_session(&key)? != Some(source.clone().normalized()?)
        || restarted.load(&source)? != Some(description.clone())
        || restarted.pending_for_session(&key)?.is_some()
        || !change_description::is_current(path, &description)?
    {
        return Err("Canonical description storage, reload, or freshness failed".into());
    }
    if git(path, &["status", "--porcelain=v1"])? != before
        || git(path, &["rev-parse", "HEAD"])? != head
        || git(path, &["ls-files", "--stage"])? != index
    {
        return Err("Normal description session changed the temporary repository".into());
    }
    println!(
        "PASS {provider:?}: normal conversation completed; {} files described; canonical result reloaded; pending capture cleared; snapshot current; HEAD, index and working state unchanged; owned session stopped",
        description.files.len()
    );
    Ok(())
}
