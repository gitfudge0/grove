//! Explicit production-actor acceptance against installed authenticated CLIs.
//! Never runs in the ordinary suite; every repository and Grove journal is disposable.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use grove_core::codex::{self, Action, Provider, SavedSession, Snapshot, Status};
use std::{
    process::Command,
    time::{Duration, Instant},
};
static ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Environment(Option<std::ffi::OsString>);
impl Environment {
    fn new(path: &std::path::Path) -> Self {
        let old = std::env::var_os("GROVE_CONFIG_DIR");
        std::env::set_var("GROVE_CONFIG_DIR", path);
        Self(old)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        match &self.0 {
            Some(old) => std::env::set_var("GROVE_CONFIG_DIR", old),
            None => std::env::remove_var("GROVE_CONFIG_DIR"),
        }
    }
}
fn wait(
    connection: &codex::Connection,
    ready: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        if Instant::now() >= deadline {
            return Err("Timed out waiting for live rewind state".into());
        }
        let snapshot = match connection.updates.recv_timeout(Duration::from_secs(5)) {
            Ok(s) => s,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => return Err(error.to_string()),
        };
        if let Some(error) = &snapshot.error {
            return Err(format!("{}: {error}", snapshot.saved.provider.label()));
        }
        if ready(&snapshot) {
            return Ok(snapshot);
        }
    }
}
fn live(provider: Provider) {
    let _lock = ENVIRONMENT.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let _environment = Environment::new(&directory.path().join("config"));
    let main = directory.path().join("project");
    let checkout = directory.path().join("worktree");
    fs_err::create_dir(&main).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    assert!(Command::new("git")
        .arg("-C")
        .arg(&main)
        .args(["worktree", "add", "-qb", "rewind-live"])
        .arg(&checkout)
        .status()
        .unwrap()
        .success());
    let file = checkout.join("work");
    fs_err::write(&file, "before first prompt").unwrap();
    let key = grove_core::identity::random_id().unwrap();
    let saved: SavedSession = serde_json::from_value(serde_json::json!({
        "provider":provider,"key":key,"project":"Rewind live QA","cwd":checkout,"label":provider.label(),
        "roots":[],"thread_id":null,"items":[],"native_config_version":1
    })).unwrap();
    let args = provider.agent().launch_args_for_backend(true, false, false);
    let mut connection = codex::connect(saved, args.clone());
    let outcome = (|| -> Result<(), String> {
        wait(&connection, |s| {
            s.status == Status::Idle
                && (provider == Provider::Claude || s.saved.thread_id.is_some())
        })?;
        let prompt = "Reply with exactly REWIND_FIRST. Do not run tools or change any files.";
        let submission = connection.handle.submit(prompt.into(), vec![])?;
        let completed = wait(&connection, |s| {
            s.accepted_submission == Some(submission)
                && matches!(s.status, Status::Idle | Status::Done)
                && s.saved
                    .items
                    .iter()
                    .any(|item| item["type"] == "agentMessage")
        })?;
        let index = completed
            .saved
            .items
            .iter()
            .position(|item| item["type"] == "userMessage")
            .ok_or("No user message")?;
        let message_id = completed.saved.items[index]["id"]
            .as_str()
            .ok_or("Missing message id")?
            .to_owned();
        if completed.rewind_files_unavailable(index).is_some() {
            return Err(format!(
                "File rewind unexpectedly unavailable: {:?}",
                completed.rewind_files_unavailable(index)
            ));
        }
        fs_err::write(&file, "later workspace").map_err(|e| e.to_string())?;
        connection
            .handle
            .send(Action::Draft("independent draft".into()))?;
        connection.handle.send(Action::DraftImages(vec![
            "data:image/png;base64,aGVsbG8=".into()
        ]))?;
        connection.handle.send(Action::RewindMessage {
            index,
            message_id,
            restore_files: false,
        })?;
        let rewound = wait(&connection, |s| {
            s.edited_messages > completed.edited_messages
        })?;
        if rewound.saved.draft != prompt
            || !rewound.saved.items.is_empty()
            || rewound.saved.rewind_drafts.len() != 1
        {
            return Err("Rewind did not restore prompt and preserve draft".into());
        }
        if fs_err::read_to_string(&file).unwrap() != "later workspace" {
            return Err("Keep-files rewind changed files".into());
        }
        println!(
            "{} PASS keep files, restored prompt, preserved independent draft; no new turn",
            provider.label()
        );
        connection
            .handle
            .stop_completion(false)
            .recv_timeout(Duration::from_secs(15))
            .map_err(|e| e.to_string())?;
        let disk: SavedSession = serde_json::from_slice(
            &fs_err::read(codex::recovery_dir().unwrap().join(format!("{key}.json")))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if disk.draft != prompt || disk.rewind_drafts.len() != 1 {
            return Err("Draft handoff was not durable".into());
        }
        connection = codex::connect(disk, args.clone());
        wait(&connection, |s| s.status == Status::Idle)?;
        let corrected = "Reply with exactly REWIND_SECOND. Do not run tools or change any files.";
        let submission = connection.handle.submit(corrected.into(), vec![])?;
        let completed = wait(&connection, |s| {
            s.accepted_submission == Some(submission)
                && matches!(s.status, Status::Idle | Status::Done)
                && s.saved
                    .items
                    .iter()
                    .any(|item| item["type"] == "agentMessage")
        })?;
        let index = completed
            .saved
            .items
            .iter()
            .position(|item| item["type"] == "userMessage")
            .ok_or("No resent user message")?;
        let message_id = completed.saved.items[index]["id"]
            .as_str()
            .ok_or("Missing resent id")?
            .to_owned();
        fs_err::write(&file, "changes after corrected turn").map_err(|e| e.to_string())?;
        connection.handle.send(Action::RewindMessage {
            index,
            message_id,
            restore_files: true,
        })?;
        let rewound = wait(&connection, |s| {
            s.edited_messages > completed.edited_messages
        })?;
        if rewound.saved.draft != corrected || !rewound.saved.items.is_empty() {
            return Err("Repeated rewind did not restore corrected prompt".into());
        }
        if fs_err::read_to_string(&file).unwrap() != "later workspace" {
            return Err("Restore-files rewind used the wrong turn checkpoint".into());
        }
        println!(
            "{} PASS reconnect, explicit resend, repeated rewind and file restoration",
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
        "Owned provider failed to stop: {stopped:?}"
    );
    assert!(
        outcome.is_ok(),
        "{} live rewind: {outcome:?}",
        provider.label()
    );
}
#[test]
#[ignore = "Consumes two live Codex turns; requires installed authenticated CLI"]
fn rewind_live_codex() {
    live(Provider::Codex);
}
#[test]
#[ignore = "Consumes two live Claude turns; requires installed authenticated CLI"]
fn rewind_live_claude() {
    live(Provider::Claude);
}
