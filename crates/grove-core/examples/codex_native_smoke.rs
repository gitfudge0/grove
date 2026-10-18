//! Explicit live smoke for Grove's production Rust Codex transport.
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
    let extra_root = std::env::args().skip(2).find(|arg| arg != "--image");
    if extra_root
        .as_ref()
        .is_some_and(|path| !std::path::Path::new(path).is_dir())
    {
        return Err("Extra root must be an existing disposable directory".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::control::random_id()?;
    let saved = SavedSession {
        provider: grove_core::codex::Provider::Codex,
        composer: grove_core::codex::ComposerSettings::default(),
        context: None,
        key: key.clone(),
        project: "Codex smoke".into(),
        cwd,
        label: "Smoke".into(),
        title: None,
        roots: extra_root
            .iter()
            .map(|path| grove_core::session_meta::ContextRoot {
                project: "Extra smoke".into(),
                wt_path: path.clone(),
            })
            .collect(),
        thread_id: None,
        items: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        config_args: if extra_root.is_some() {
            vec!["-c".into(), "sandbox_mode=\"workspace-write\"".into()]
        } else {
            Vec::new()
        },
        task_id: None,
    };
    let connection = codex::connect(saved, Vec::new(), None);
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
    connection.handle.send(Action::Prompt("Remember the word MARIGOLD for this conversation. Run exactly `pwd` once in the working directory, then reply only MARIGOLD. Do not modify files or access other directories.".into()))?;
    let done = wait(&connection, |s| s.status == Status::Done)?;
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
    let resumed = codex::connect(restored, Vec::new(), None);
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
