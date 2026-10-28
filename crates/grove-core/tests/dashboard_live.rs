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
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("isolated-config");
    std::env::set_var("GROVE_CONFIG_DIR", &config);
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
    std::env::remove_var("GROVE_CONFIG_DIR");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
