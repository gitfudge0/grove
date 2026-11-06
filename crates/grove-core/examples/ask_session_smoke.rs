//! Live normal-conversation Ask smoke with isolated Grove recovery data.
//! Run with an existing, empty temporary GROVE_CONFIG_DIR:
//! cargo run -p grove-core --example ask_session_smoke -- codex|claude [PR-URL]
//! GROVE_ASK_SMOKE_MODEL optionally overrides only this fixture session.
//! GROVE_ASK_SMOKE_CUSTOM=1 verifies a persisted custom prompt and native Markdown content.
use grove_core::{
    change_description::{self, Scope},
    change_questions::{self, Answer},
    change_result_store::{PendingRequest, Source, Store},
    codex::{self, Action, Connection, Handle, Provider, SavedSession, Status},
    conversation_result,
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
    request: PendingRequest,
) -> Result<Answer, String> {
    let prompt = request.prompt.clone();
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
            return Err("Ask unexpectedly requested a tool approval or user response".into());
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
            let text = conversation_result::completed_text(&state, &request, 0, false)
                .ok_or("Completed turn does not own the Ask request")??;
            return change_questions::from_conversation(
                request.snapshot.clone(),
                &request.input,
                &text,
            );
        }
    }
}

fn main() -> Result<(), String> {
    let provider = match std::env::args().nth(1).as_deref() {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: ask_session_smoke codex|claude [PR-URL]".into()),
    };
    let config = isolated_config()?;
    let custom = std::env::var_os("GROVE_ASK_SMOKE_CUSTOM").is_some();
    const CUSTOM_HEADING: &str = "## Grove custom rendering smoke";
    if custom {
        let mut settings = grove_core::storage::Store::default();
        settings.prompt_overrides.insert(
            grove_core::prompts::PromptKind::AskChanges.key().into(),
            format!("Answer the supplied question. In the answer field include the exact heading {CUSTOM_HEADING}, followed by a Markdown table with the exact header | Before | After | and one row explaining the behavior change. Keep the required citations, examined_files and limitations accurate."),
        );
        grove_core::storage::save(&settings).map_err(|error| error.to_string())?;
    }
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
    let question =
        "How does the new guard treat an already-seen delivery?\nExplain the available evidence.";
    let body = change_questions::conversation_prompt(&snapshot, question)?;
    let source = Source {
        checkout: snapshot.checkout.clone(),
        scope: snapshot.scope.clone(),
    };
    let store_root = config.join("result-store");
    let store = Store::<Answer>::new(store_root.clone());
    let key = grove_core::identity::random_id()?;
    store.bind(&key, &source)?;
    let generation = grove_core::identity::random_id()?;
    let display = format!("/ask local {question}");
    let prompt = conversation_result::compose_request("ask", &display, &generation, &body);
    let request = PendingRequest {
        snapshot: snapshot.clone(),
        input: question.into(),
        prompt,
        generation: generation.clone(),
    };
    store.save_pending(&key, &request)?;
    if store.pending_for_session(&key)?.as_ref() != Some(&request) {
        return Err("Exact Ask question/prompt were not persisted".into());
    }
    let mut saved = SavedSession::empty(
        key.clone(),
        provider,
        "Session smoke fixture".into(),
        path.to_string_lossy().into_owned(),
    );
    if let Ok(model) = std::env::var("GROVE_ASK_SMOKE_MODEL") {
        if model.trim().is_empty() {
            return Err("GROVE_ASK_SMOKE_MODEL must name an available model".into());
        }
        saved.composer.model = Some(model);
        saved.composer.effort = Some("low".into());
    }
    let args = provider.agent().launch_args_for_backend(true, false, false);
    let connection = codex::connect(saved, args);
    let mut owned = OwnedSession(Some(connection.handle.clone()));
    let generated = generate_in_conversation(&connection, provider, request);
    let shutdown = owned.stop();
    let answer = generated?;
    shutdown?;
    if custom
        && (!answer.answer.contains(CUSTOM_HEADING)
            || !answer.answer.contains("| Before | After |")
            || !answer.markdown().contains(CUSTOM_HEADING))
    {
        return Err(
            "Custom prompt did not produce the requested Markdown heading and table".into(),
        );
    }
    if answer.question != question || answer.citations.is_empty() {
        return Err("Ask did not retain the exact question or cite the captured patch".into());
    }
    let expected_path = store.result_path(&source)?;
    let saved_path = store.save_generation(&key, &answer, &generation)?;
    let restarted = Store::new(store_root);
    if expected_path != saved_path
        || restarted.result_path(&source)? != saved_path
        || restarted.source_for_session(&key)? != Some(source.clone().normalized()?)
        || restarted.load(&source)? != Some(answer.clone())
        || restarted.pending_for_session(&key)?.is_some()
        || !change_description::snapshot_is_current(path, &answer.snapshot)?
    {
        return Err("Canonical answer storage, reload, or freshness failed".into());
    }
    if git(path, &["status", "--porcelain=v1"])? != before
        || git(path, &["rev-parse", "HEAD"])? != head
        || git(path, &["ls-files", "--stage"])? != index
    {
        return Err("Normal Ask session changed the temporary repository".into());
    }
    if custom {
        println!("PASS {provider:?}: persisted custom prompt produced the requested native Markdown heading and table");
    }
    println!(
        "PASS {provider:?}: normal Ask conversation completed; {} files examined with validated citations; exact question/prompt persisted; canonical answer reloaded; pending cleared; snapshot current; HEAD/index/working state unchanged; owned session stopped",
        answer.examined_files.len()
    );
    Ok(())
}
