//! Explicit live smoke for Grove's production Rust Codex transport.
//! Use a disposable empty cwd and GROVE_CONFIG_DIR; authenticates using the installed CLI.
use grove_core::codex::{self, Action, Connection, SavedSession, Snapshot, Status};
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
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let state = connection
            .updates
            .recv_timeout(remaining)
            .map_err(|e| e.to_string())?;
        if state.status == Status::Disconnected || state.status == Status::Failed {
            return Err(state.error.unwrap_or_else(|| state.status.label().into()));
        }
        if predicate(&state) {
            return Ok(state);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set GROVE_CONFIG_DIR to a disposable directory first".into());
    }
    let cwd = std::env::args()
        .nth(1)
        .ok_or("Supply an empty disposable working directory")?;
    if !std::path::Path::new(&cwd).is_dir() {
        return Err("Smoke directory does not exist".into());
    }
    let image_test = std::env::args().any(|arg| arg == "--image");
    let extra_root = std::env::args().skip(2).find(|arg| arg != "--image");
    if extra_root
        .as_ref()
        .is_some_and(|path| !std::path::Path::new(path).is_dir())
    {
        return Err("Extra root must be an existing disposable directory".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let saved = SavedSession {
        claude_subagents: std::collections::HashMap::default(),
        session_review: Default::default(),
        rich_plans: grove_core::rich_plan::PlanStore::default(),
        side_chats: Default::default(),
        fork_origin: None,
        claude_resume: None,
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        rewind_drafts: Vec::new(),
        provider: grove_core::codex::Provider::Codex,
        composer: grove_core::codex::ComposerSettings::default(),
        context: None,
        context_details: None,
        key: key.clone(),
        project: "Codex smoke".into(),
        cwd,
        label: "Smoke".into(),
        title: None,
        title_generation: None,
        roots: extra_root
            .iter()
            .map(|path| grove_core::session_meta::ContextRoot {
                project: "Extra smoke".into(),
                wt_path: path.clone(),
            })
            .collect(),
        thread_id: None,
        items: Vec::new().into(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        native_config_version: 1,
        activity_reporting: false,
        config_args: if extra_root.is_some() {
            vec!["-c".into(), "sandbox_mode=\"workspace-write\"".into()]
        } else {
            Vec::new()
        },
    };
    let connection = codex::connect(saved, Vec::new());
    let _connection_cleanup = StopOnDrop(connection.handle.clone());
    let ready = wait(&connection, |s| s.status == Status::Idle)?;
    println!(
        "PASS initialized thread {}",
        ready.saved.thread_id.as_deref().unwrap_or_default()
    );
    if let Some(extra) = &extra_root {
        let sandbox = ready.sandbox.as_ref().ok_or("Missing effective sandbox")?;
        if sandbox["type"] != "workspaceWrite"
            || !sandbox["writableRoots"]
                .as_array()
                .is_some_and(|roots| roots.iter().any(|root| root.as_str() == Some(extra)))
        {
            return Err(format!(
                "Additional root missing from effective sandbox: {sandbox}"
            ));
        }
        println!("PASS additional writable root in effective workspace-write policy");
    }
    // The same native thread under a different app-session identity must refuse
    // a second writer while the original process remains usable.
    let mut duplicate = ready.saved.clone();
    duplicate.key = grove_core::identity::random_id()?;
    duplicate.fork_origin = Some(grove_core::provider::ForkOrigin {
        source_thread: None,
        last_turn: None,
        users: 0,
    });
    let contested = codex::connect(duplicate, Vec::new());
    let refusal = wait(&contested, |s| s.status == Status::Idle)
        .err()
        .ok_or("Duplicate native thread unexpectedly opened")?;
    contested
        .handle
        .stop_completion(true)
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    if !refusal.contains("already open") {
        return Err(format!("Unexpected ownership refusal: {refusal}"));
    }
    println!("PASS duplicate native thread refused without replacing its identity");
    connection.handle.send(Action::Prompt("Remember the word MARIGOLD for this conversation. Run exactly `pwd` once in the working directory, then reply only MARIGOLD. Do not modify files or access other directories.".into()))?;
    let done = wait(&connection, |s| s.status == Status::Done)?;
    let reply = done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(|item| codex::item_text(item))
        .unwrap_or_default();
    if !reply.contains("MARIGOLD") {
        return Err("Missing streamed agent response".into());
    }
    if !done.saved.items.iter().any(|item| {
        item["type"] == "commandExecution"
            && item["aggregatedOutput"]
                .as_str()
                .is_some_and(|output| output.contains(&done.saved.cwd))
    }) {
        return Err("Missing real command execution and working-directory output".into());
    }
    println!("PASS real streamed turn");
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let recovery = codex::load_sessions()?;
    let restored = recovery
        .sessions
        .into_iter()
        .find(|s| s.key == key)
        .ok_or("Missing recovery record")?;
    if restored.thread_id != done.saved.thread_id {
        return Err("Persisted thread identity changed".into());
    }
    let resumed = codex::connect(restored, Vec::new());
    let _resumed_cleanup = StopOnDrop(resumed.handle.clone());
    wait(&resumed, |s| s.status == Status::Idle)?;
    resumed.handle.send(Action::Prompt("What word did I ask you to remember? Reply only that word. Do not use tools or modify files.".into()))?;
    let done = wait(&resumed, |s| s.status == Status::Done)?;
    let last = done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(|item| codex::item_text(item))
        .unwrap_or_default();
    if !last.contains("MARIGOLD") {
        return Err(format!("Resumed thread did not remember context: {last}"));
    }
    println!("PASS persisted resume and remembered context");
    if image_test {
        resumed.handle.send(Action::PromptWithImages {
            text: "Identify the dominant color of this attached image. Reply only the color name. Do not use tools or modify files.".into(),
            images: vec!["data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP4z8BAEiJN9aiGUQ1DSgMAkPn/Afnh+ngAAAAASUVORK5CYII=".into()],
        })?;
        let image_done = wait(&resumed, |s| s.status == Status::Done)?;
        let reply = image_done
            .saved
            .items
            .iter()
            .rev()
            .find(|item| item["type"] == "agentMessage")
            .map(|item| codex::item_text(item))
            .unwrap_or_default();
        if !reply.to_lowercase().contains("red") {
            return Err(format!("Image attachment was not understood: {reply}"));
        }
        println!("PASS actual provider image input and color recognition");
    }
    resumed.handle.send(Action::Prompt("Run exactly `sleep 20` in the working directory, then reply done. Do not write files or access any other directory.".into()))?;
    let started = wait(&resumed, |s| s.turn_id.is_some())?;
    if started.turn_id.is_none() {
        return Err("No active turn to interrupt".into());
    }
    resumed.handle.send(Action::Interrupt)?;
    wait(&resumed, |s| s.status == Status::Interrupted)?;
    println!("PASS real interrupt");
    resumed
        .handle
        .stop_completion(true)
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    if codex::load_sessions()?
        .sessions
        .iter()
        .any(|s| s.key == key)
    {
        return Err("Closed session recovery record survived".into());
    }
    println!("PASS process stop and recovery cleanup");
    Ok(())
}
