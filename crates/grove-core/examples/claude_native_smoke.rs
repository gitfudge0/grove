//! Explicit live smoke for Grove's production Rust Claude Code transport.
//! Use a disposable empty cwd and GROVE_CONFIG_DIR; authenticates using the installed CLI.
use grove_core::codex::{self, Action, Connection, SavedSession, Snapshot, Status};
use std::time::{Duration, Instant};
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
    let startup_queue = std::env::args().any(|arg| arg == "--startup-queue");
    let extra_root = std::env::args()
        .skip(2)
        .find(|arg| arg != "--image" && arg != "--startup-queue");
    if extra_root
        .as_ref()
        .is_some_and(|path| !std::path::Path::new(path).is_dir())
    {
        return Err("Extra root must be an existing disposable directory".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let saved = SavedSession {
        rich_plans: Default::default(),
        claude_resume: None,
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        provider: grove_core::codex::Provider::Claude,
        composer: grove_core::codex::ComposerSettings::default(),
        context: None,
        key: key.clone(),
        project: "Claude smoke".into(),
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
        items: Vec::new(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        config_args: Vec::new(),
    };
    let connection = codex::connect(saved, Vec::new());
    let ready = if startup_queue {
        // Submit immediately, before waiting for any initialize snapshot. This
        // exercises the production actor and installed CLI, without a UI fixture.
        connection.handle.submit_with_mode(
            "Reply only STARTUP_FIRST. Do not use tools.".into(),
            vec![],
            codex::SubmissionMode::Queue,
        )?;
        connection.handle.submit_with_mode(
            "Reply only STARTUP_SECOND. Do not use tools.".into(),
            vec![],
            codex::SubmissionMode::Queue,
        )?;
        let ready = wait(&connection, |s| {
            s.status == Status::Done && s.accepted_prompts >= 2
        })?;
        let requests = ready
            .saved
            .items
            .iter()
            .filter(|item| item["type"] == "userMessage")
            .map(codex::item_text)
            .collect::<Vec<_>>();
        if requests.len() != 2
            || !requests[0].contains("STARTUP_FIRST")
            || !requests[1].contains("STARTUP_SECOND")
            || !ready.queued_prompts.is_empty()
        {
            return Err("Startup queue was lost, duplicated, reordered, or not drained".into());
        }
        println!("PASS production startup queue: two inputs delivered exactly once in FIFO order");
        ready
    } else {
        wait(&connection, |s| s.status == Status::Idle)?
    };
    println!(
        "PASS initialized thread {}",
        ready.saved.thread_id.as_deref().unwrap_or_default()
    );
    connection.handle.send(Action::Prompt("Remember the word MARIGOLD for this conversation. Run exactly `pwd` once in the working directory, then reply only MARIGOLD. Do not modify files or access other directories.".into()))?;
    let done = wait(&connection, |s| s.status == Status::Done)?;
    let mut assistant_blocks = std::collections::HashSet::new();
    for item in done
        .saved
        .items
        .iter()
        .filter(|item| item["type"] == "agentMessage")
    {
        let id = item["id"].as_str().unwrap_or_default();
        let message = id.rsplit_once('-').map_or(id, |(message, _)| message);
        if !assistant_blocks.insert((message.to_owned(), codex::item_text(item))) {
            return Err("Duplicate assistant block after streaming/final reconciliation".into());
        }
    }
    let reply = done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(codex::item_text)
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
    wait(&resumed, |s| s.status == Status::Idle)?;
    resumed.handle.send(Action::Prompt("What word did I ask you to remember? Reply only that word. Do not use tools or modify files.".into()))?;
    let done = wait(&resumed, |s| s.status == Status::Done)?;
    let last = done
        .saved
        .items
        .iter()
        .rev()
        .find(|item| item["type"] == "agentMessage")
        .map(codex::item_text)
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
            .map(codex::item_text)
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
