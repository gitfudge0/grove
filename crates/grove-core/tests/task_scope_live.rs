//! Explicit live acceptance for evolving task scopes; never part of the normal suite.
//! Run with `cargo test -p grove-core --test task_scope_live -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use grove_core::codex::{self, ComposerSettings, Provider, SavedSession, Snapshot, Status};
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn draft(provider: Provider, cwd: &str) -> SavedSession {
    serde_json::from_value(serde_json::json!({
        "provider": provider,
        "key": grove_core::identity::random_id().unwrap(),
        "project": "Task scope live QA",
        "cwd": cwd,
        "label": provider.label(),
        "roots": [],
        "thread_id": null,
        "items": [],
        "native_config_version": 1,
        "composer": ComposerSettings::default(),
    }))
    .unwrap()
}

fn await_state(
    connection: &codex::Connection,
    seconds: u64,
    ready: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Timed out waiting for provider state".into());
        }
        let state = match connection
            .updates
            .recv_timeout(remaining.min(Duration::from_secs(10)))
        {
            Ok(state) => state,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => return Err(format!("Provider state unavailable: {error}")),
        };
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if ready(&state) {
            return Ok(state);
        }
        println!("{} status={:?}", state.saved.provider.label(), state.status);
    }
}

// The override is process-global. Keep explicit live acceptance tests isolated.
static LIVE_PROGRESS_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct LiveEnvironment(Option<std::ffi::OsString>);
impl LiveEnvironment {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("GROVE_CONFIG_DIR");
        std::env::set_var("GROVE_CONFIG_DIR", path);
        Self(previous)
    }
}
impl Drop for LiveEnvironment {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("GROVE_CONFIG_DIR", value),
            None => std::env::remove_var("GROVE_CONFIG_DIR"),
        }
    }
}

fn live_task_scope(provider: Provider) {
    let _guard = LIVE_PROGRESS_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let _environment = LiveEnvironment::set(&directory.path().join("isolated-config"));
    let checkout = directory.path().join("checkout");
    fs_err::create_dir(&checkout).unwrap();
    assert!(Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&checkout)
        .status()
        .unwrap()
        .success());
    let cwd = checkout
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let args = provider.agent().launch_args_for_backend(true, true, false);
    let connection = codex::connect(draft(provider, &cwd), args);
    let marker = grove_core::identity::random_id().unwrap();
    let retained_title = format!("Retained scope {marker}");
    let removed_title = format!("Earlier scope {marker}");
    let added_title = format!("Replacement scope {marker}");
    let outcome = (|| -> Result<(), String> {
        await_state(&connection, 60, |state| {
            state.status == Status::Idle
                && (provider == Provider::Claude || state.saved.thread_id.is_some())
        })?;
        let prompt = format!(
            "This is an explicit task-scope acceptance test in a disposable empty repository. Do not read or modify files or use shell commands. Use your structured task tools to publish exactly two tasks with literal titles '{retained_title}' and '{removed_title}'. Mark the first in_progress and the second pending. Codex: use update_plan. Claude: use TaskCreate then TaskUpdate; load the tools with ToolSearch if deferred. Do not mark either task completed. Leave both tasks in that state because the test intentionally waits for another user instruction. Once the structured list is published, reply only 'initial scope ready'."
        );
        let submission = connection.handle.submit(prompt, Vec::new())?;
        let initial = await_state(&connection, 120, |state| {
            state.accepted_submission == Some(submission)
                && matches!(state.status, Status::Done | Status::Idle)
                && state.saved.items.iter().any(|item| {
                    item["type"] == "agentMessage"
                        && codex::item_text(item).contains("initial scope ready")
                })
        })?;
        let initial_scope = grove_core::tasks::scope(&initial.saved.items);
        let initial_tasks = initial_scope
            .current
            .as_ref()
            .ok_or("Initial structured checklist missing")?;
        if initial_tasks.total() != 2 {
            return Err(format!(
                "Expected two initial tasks, found {}",
                initial_tasks.total()
            ));
        }
        let retained = initial_tasks
            .tasks
            .iter()
            .find(|task| task.step == retained_title)
            .ok_or("Initial retained task missing")?
            .clone();
        let removed = initial_tasks
            .tasks
            .iter()
            .find(|task| task.step == removed_title)
            .ok_or("Initial removed task missing")?
            .clone();
        if retained.status != grove_core::tasks::TaskStatus::InProgress
            || removed.status != grove_core::tasks::TaskStatus::Pending
        {
            return Err("Initial tasks did not preserve requested unfinished states".into());
        }
        let initial_user_count = initial
            .saved
            .items
            .iter()
            .filter(|item| item["type"] == "userMessage")
            .count();
        println!(
            "{} initial scope verified: {:?}",
            provider.label(),
            initial_tasks.tasks
        );
        let prompt = format!(
            "Revise the current structured checklist now. Keep '{retained_title}' as the same continuous task, with the exact same title and identity, still in_progress. Remove '{removed_title}' from the current checklist WITHOUT marking it completed; it is no longer in scope. Add a new pending task with exact title '{added_title}'. Codex: replace the list using update_plan with only retained and replacement tasks. Claude: delete the earlier task using TaskUpdate status deleted, retain the existing first task ID, and TaskCreate the replacement. Do not mark any task completed. Do not read, write, or modify files. After publishing the revised list, reply only 'revised scope ready'."
        );
        let submission = connection.handle.submit(prompt, Vec::new())?;
        let revised = await_state(&connection, 120, |state| {
            state.accepted_submission == Some(submission)
                && matches!(state.status, Status::Done | Status::Idle)
                && state.saved.items.iter().any(|item| {
                    item["type"] == "agentMessage"
                        && codex::item_text(item).contains("revised scope ready")
                })
        })?;
        let items: Vec<serde_json::Value> = revised
            .saved
            .items
            .iter()
            .map(|item| item.as_ref().clone())
            .collect();
        let evidence = format!(
            "/tmp/grove-task-scope-live-{}-items.json",
            provider.label().to_lowercase()
        );
        fs_err::write(&evidence, serde_json::to_vec_pretty(&items).unwrap())
            .map_err(|error| error.to_string())?;
        println!(
            "{} provider transcript evidence: {evidence}",
            provider.label()
        );
        let second_prompt = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item["type"] == "userMessage")
            .nth(initial_user_count)
            .map(|(index, _)| index)
            .ok_or("Second provider user message was not recorded")?;
        let awaiting = grove_core::tasks::scope(&items[..=second_prompt]);
        if !awaiting.awaiting_update
            || awaiting.current != initial_scope.current
            || !awaiting.history.is_empty()
        {
            return Err("Second recorded user message did not preserve the existing scope while awaiting update".into());
        }
        let scope = grove_core::tasks::scope(&items);
        if scope.awaiting_update {
            return Err("New checklist remained awaiting an update".into());
        }
        let current = scope
            .current
            .as_ref()
            .ok_or("Revised structured checklist missing")?;
        if current.total() != 2 || current.tasks.iter().any(|task| task.step == removed_title) {
            return Err(format!("Unexpected revised checklist: {:?}", current.tasks));
        }
        let retained_now = current
            .tasks
            .iter()
            .find(|task| task.step == retained_title)
            .ok_or("Revised scope lost the retained task")?;
        if !retained_now.owns_activity(&retained)
            || retained_now.status != grove_core::tasks::TaskStatus::InProgress
        {
            return Err("Retained task lost continuous identity or unfinished status".into());
        }
        if !current.tasks.iter().any(|task| {
            task.step == added_title && task.status == grove_core::tasks::TaskStatus::Pending
        }) {
            return Err("Replacement pending task missing".into());
        }
        let archived: Vec<_> = scope
            .history
            .iter()
            .flat_map(|revision| &revision.tasks)
            .collect();
        if archived.len() != 1
            || archived[0].step != removed_title
            || archived[0].status != grove_core::tasks::TaskStatus::Pending
            || !archived[0].owns_activity(&removed)
        {
            return Err(format!(
                "Removal history did not preserve only the unfinished old task: {archived:?}"
            ));
        }
        let restored: Vec<serde_json::Value> =
            serde_json::from_slice(&serde_json::to_vec(&items).unwrap()).unwrap();
        if grove_core::tasks::scope(&restored) != scope {
            return Err("Task scope changed after transcript JSON reload".into());
        }
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&checkout)
            .output()
            .map_err(|error| error.to_string())?;
        if !status.stdout.is_empty() {
            return Err("Provider modified the disposable checkout".into());
        }
        println!("{} VERIFIED live initial scope → awaiting update → revised scope; retained identity, unfinished removal history, JSON reload", provider.label());
        Ok(())
    })();
    let stopped = connection
        .handle
        .stop_completion(true)
        .recv_timeout(Duration::from_secs(15));
    assert!(
        stopped.is_ok(),
        "{} owned provider must stop: {stopped:?}",
        provider.label()
    );
    assert!(
        outcome.is_ok(),
        "{} live task scope: {outcome:?}",
        provider.label()
    );
}

#[test]
#[ignore = "Launches authenticated Codex CLI and consumes two model turns; explicit acceptance only"]
fn task_scope_live_codex_revisions() {
    live_task_scope(Provider::Codex);
}

#[test]
#[ignore = "Launches authenticated Claude CLI and consumes two model turns; explicit acceptance only"]
fn task_scope_live_claude_revisions() {
    live_task_scope(Provider::Claude);
}
