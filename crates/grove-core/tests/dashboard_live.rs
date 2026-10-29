//! Explicit live provider acceptance. Never runs in the normal suite.
//! Run with `cargo test -p grove-core --test dashboard_live -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use grove_core::codex::{self, ComposerSettings, Provider, SavedSession, Snapshot, Status};
use std::{
    collections::HashMap,
    process::Command,
    time::{Duration, Instant},
};

fn draft(provider: Provider, cwd: &str) -> SavedSession {
    SavedSession {
        claude_subagents: HashMap::default(),
        rich_plans: grove_core::rich_plan::PlanStore::default(),
        claude_resume: None,
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        provider,
        key: grove_core::identity::random_id().unwrap(),
        project: "Dashboard live QA".into(),
        cwd: cwd.into(),
        label: provider.label().into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: None,
        items: Vec::new(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        composer: ComposerSettings::default(),
        context: None,
    }
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

#[test]
#[ignore = "Launches installed authenticated Codex and Claude CLIs; consumes a live model turn"]
fn dashboard_live_provider_catalog_and_selected_checkout() {
    let _guard = LIVE_PROGRESS_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("isolated-config");
    let _environment = LiveEnvironment::set(&config);
    let mut failures = Vec::new();
    for provider in [Provider::Codex, Provider::Claude] {
        let checkout = directory.path().join(provider.label().replace(' ', "-"));
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
        let saved = draft(provider, &cwd);
        let args = provider.agent().launch_args_for_backend(true, true, false);
        println!("{} live checkout={cwd}", provider.label());
        match codex::discover_composer(saved.clone(), args.clone()) {
            Ok(catalog) => {
                println!(
                    "{} catalog models={} skills={} permission_modes={}",
                    provider.label(),
                    catalog.models.len(),
                    catalog.skills.len(),
                    catalog.permission_modes.len()
                );
                if catalog.models.is_empty() {
                    failures.push(format!("{} returned no model catalog", provider.label()));
                }
                if config.join("codex-sessions").exists() && provider == Provider::Codex {
                    failures.push("Discovery persisted a Grove conversation".into());
                }
            }
            Err(error) => failures.push(format!("{} catalog: {error}", provider.label())),
        }
        let connection = codex::connect(saved, args);
        let outcome = (|| -> Result<(), String> {
            await_state(&connection, 75, |state| {
                state.status == Status::Idle
                    && (provider == Provider::Claude || state.saved.thread_id.is_some())
            })?;
            let submission = connection.handle.submit("Run pwd, then return only the exact current working directory. Do not change files.".into(), Vec::new())?;
            let completed = await_state(&connection, 120, |state| {
                state.accepted_submission == Some(submission)
                    && matches!(state.status, Status::Done | Status::Idle)
                    && state
                        .saved
                        .items
                        .iter()
                        .any(|item| item["type"] == "agentMessage")
            })?;
            let response = completed
                .saved
                .items
                .iter()
                .filter(|item| item["type"] == "agentMessage")
                .map(codex::item_text)
                .collect::<Vec<_>>()
                .join("\n");
            if completed.saved.cwd != cwd || !response.contains(&cwd) {
                return Err(format!(
                    "Checkout mismatch: saved={} response={response:?}",
                    completed.saved.cwd
                ));
            }
            println!(
                "{} VERIFIED cwd={cwd} response={response:?}",
                provider.label()
            );
            let status = Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(&checkout)
                .output()
                .map_err(|error| error.to_string())?;
            if !status.stdout.is_empty() {
                return Err("Provider changed checkout files".into());
            }
            Ok(())
        })();
        connection
            .handle
            .stop_completion(true)
            .recv_timeout(Duration::from_secs(15))
            .expect("Owned provider process must stop");
        if let Err(error) = outcome {
            failures.push(format!("{} conversation: {error}", provider.label()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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

fn live_dashboard_progress(provider: Provider) {
    let _guard = LIVE_PROGRESS_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("isolated-config");
    let _environment = LiveEnvironment::set(&config);
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
    let marker = format!(
        "dashboard-progress-{}",
        grove_core::identity::random_id().unwrap()
    );
    let outcome = (|| -> Result<(), String> {
        await_state(&connection, 75, |state| {
            state.status == Status::Idle
                && (provider == Provider::Claude || state.saved.thread_id.is_some())
        })?;
        let prompt = format!("This is an explicit live progress acceptance test in a disposable empty repository. Do not create or change any files. Publish a structured task list using your task tools (Codex update_plan; Claude TodoWrite or TaskCreate/TaskUpdate). Use exactly two task titles: 'Inspect checkout {marker}' and 'Confirm result {marker}'. First mark Inspect checkout in progress and Confirm result pending. Emit a short commentary containing '{marker} observed activity', then run pwd using your shell tool. Update Inspect checkout completed and Confirm result in progress. Run printf 'progress confirmed\\n' using your shell tool. Finally mark both tasks completed and answer '{marker} complete'. Do not skip the intermediate structured task updates.");
        let submission = connection.handle.submit(prompt, Vec::new())?;
        let deadline = Instant::now() + Duration::from_mins(3);
        let expected = [
            format!("Inspect checkout {marker}"),
            format!("Confirm result {marker}"),
        ];
        let mut last_titles = Vec::new();
        let mut saw_plan = false;
        let mut saw_in_progress = false;
        let mut saw_activity = false;
        let mut completed = false;
        let mut terminal = false;
        while Instant::now() < deadline {
            let state = match connection.updates.recv_timeout(Duration::from_secs(5)) {
                Ok(state) => state,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => return Err(error.to_string()),
            };
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            completed = false;
            if let Some(list) = grove_core::tasks::current(&state.saved.items) {
                last_titles = list.tasks.iter().map(|task| task.step.clone()).collect();
                if list.total() == 2
                    && expected
                        .iter()
                        .all(|title| list.tasks.iter().any(|task| &task.step == title))
                {
                    saw_plan = true;
                    saw_in_progress |= list
                        .tasks
                        .iter()
                        .any(|task| task.status == grove_core::tasks::TaskStatus::InProgress);
                    completed = list.total() == 2 && list.done() == 2;
                    println!(
                        "{} PROGRESS status={:?} done={}/{} current={:?}",
                        provider.label(),
                        state.status,
                        list.done(),
                        list.total(),
                        list.now_label()
                    );
                }
            }
            saw_activity |= state.saved.items.iter().any(|item| {
                item["type"] == "commandExecution"
                    || (item["type"] == "agentMessage" && codex::item_text(item).contains(&marker))
            });
            if state.accepted_submission == Some(submission)
                && matches!(state.status, Status::Done | Status::Idle)
                && completed
            {
                terminal = true;
                break;
            }
        }
        if !saw_plan || !saw_in_progress || !saw_activity || !completed || !terminal {
            return Err(format!("structured progress incomplete: plan={saw_plan} intermediate={saw_in_progress} activity={saw_activity} final_complete={completed} terminal={terminal} observed_titles={last_titles:?}"));
        }
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&checkout)
            .output()
            .map_err(|error| error.to_string())?;
        if !status.stdout.is_empty() {
            return Err("Provider changed checkout files".into());
        }
        println!(
            "{} VERIFIED live structured intermediate plan, session activity and final completion",
            provider.label()
        );
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
        "{} live dashboard progress: {outcome:?}",
        provider.label()
    );
}

#[test]
#[ignore = "Launches authenticated Codex CLI and consumes a model turn; explicit live acceptance only"]
fn dashboard_live_codex_structured_progress() {
    live_dashboard_progress(Provider::Codex);
}
#[test]
#[ignore = "Launches authenticated Claude CLI and consumes a model turn; explicit live acceptance only"]
fn dashboard_live_claude_structured_progress() {
    live_dashboard_progress(Provider::Claude);
}
