//! Grove-owned Claude Code sessions using the CLI's streaming SDK transport.
//! The subprocess owns credentials, tools, permissions, and conversation persistence.
use crate::codex::{
    self, Action, Connection, EditCheckpoint, Handle, PendingEdit, QuestionRequest, QueuedPrompt,
    SavedSession, Snapshot, Status, SubagentStatus, SubmissionMode,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Only a bounded set of native CLI configuration options is accepted. Transport,
/// persistence, cwd and lifecycle flags belong to Grove, never to user arguments.
pub fn launch_options(args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let mut options = Vec::new();
    let mut prompt = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        match flag {
            "--model"
            | "--effort"
            | "--permission-mode"
            | "--add-dir"
            | "--append-system-prompt"
            | "--system-prompt"
            | "--settings"
            | "--setting-sources"
            | "--mcp-config"
            | "--allowedTools"
            | "--allowed-tools"
            | "--disallowedTools"
            | "--disallowed-tools"
            | "--tools"
            | "--agent"
            | "--agents"
            | "--plugin-dir"
            | "--max-budget-usd" => {
                let value = if let Some(value) = inline {
                    value.to_owned()
                } else {
                    i += 1;
                    args.get(i)
                        .filter(|v| !v.starts_with('-'))
                        .cloned()
                        .ok_or_else(|| format!("Claude Code {flag} needs a value"))?
                };
                options.extend([flag.to_owned(), value]);
            }
            "--dangerously-skip-permissions"
            | "--allow-dangerously-skip-permissions"
            | "--strict-mcp-config"
            | "--disable-slash-commands"
            | "--chrome"
            | "--no-chrome"
                if inline.is_none() =>
            {
                options.push(arg.clone());
            }
            "--" => {
                i += 1;
                if i < args.len() && prompt.is_none() {
                    prompt = Some(args[i..].join(" "));
                } else if i < args.len() {
                    return Err("Multiple Claude Code launch prompts are unsupported".into());
                }
                break;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("Unsupported native Claude Code option: {arg}"));
            }
            _ => {
                if prompt.replace(arg.clone()).is_some() {
                    return Err("Multiple Claude Code launch prompts are unsupported".into());
                }
            }
        }
        i += 1;
    }
    Ok((options, prompt))
}

pub fn connect(saved: SavedSession, args: Vec<String>) -> Connection {
    let (handle, input) = Handle::channel();
    let (updates, output) = crate::latest::channel();
    let pid = handle.1.clone();
    let stopping = handle.2.clone();
    std::thread::spawn(move || {
        Actor::new(saved, args, input, updates, pid, stopping).run();
    });
    Connection {
        handle,
        updates: output,
    }
}

struct Process {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<Result<Value, String>>,
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn launch_command(args: &[String], saved: &SavedSession) -> Result<Command, String> {
    // Combine rather than repeat this flag: the CLI keeps only its last value.
    let mut args = args.to_vec();
    let mut guidance = String::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--append-system-prompt" && index + 1 < args.len() {
            args.remove(index);
            guidance.push_str(&args.remove(index));
            guidance.push_str("\n\n");
        } else if let Some(value) = args[index].strip_prefix("--append-system-prompt=") {
            guidance.push_str(value);
            guidance.push_str("\n\n");
            args.remove(index);
        } else {
            index += 1;
        }
    }
    guidance.push_str(crate::agent_guidance::WORKFLOW_GUIDANCE);
    guidance.push_str("\n\n");
    guidance.push_str(crate::tasks::CLAUDE_GUIDANCE);
    guidance.push_str("\n\n");
    guidance.push_str(crate::reports::REPORT_GUIDANCE);
    guidance.push_str("\n\n");
    guidance.push_str(crate::rich_plan::GUIDANCE);
    args.extend(["--append-system-prompt".into(), guidance]);
    let argv = vec![
        "--print".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
        "--replay-user-messages".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];
    let (program, argv) = crate::agent::Agent::Claude.session_invocation(&argv);
    let mut command = Command::new(program);
    command
        .args(argv)
        .args(args)
        .current_dir(&saved.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = crate::env_path::session_environment() {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    command.env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
    command.env("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1");
    // The native question panel renders the extended text and number schema.
    command.env("CLAUDE_CODE_QUESTION_EXTENDED", "1");
    command.env(
        "GROVE_CONFIG_DIR",
        crate::storage::config_dir().map_err(|e| e.to_string())?,
    );
    let pending_resume = saved.pending_edit.as_ref().map(|edit| codex::ClaudeResume {
        original_session: saved.claude_resume.as_ref().map_or_else(
            || saved.thread_id.clone(),
            |resume| resume.original_session.clone(),
        ),
        assistant_uuid: saved
            .items
            .get(edit.target)
            .and_then(|item| item["claudePreviousAssistantUuid"].as_str())
            .map(str::to_owned),
        human_uuid: edit.turn_id.clone(),
        fork_id: edit.claude_fork_id.clone().unwrap_or_default(),
    });
    if let Some(resume) = pending_resume.as_ref().or(saved.claude_resume.as_ref()) {
        command.arg("--session-id").arg(&resume.fork_id);
        if let Some(anchor) = &resume.assistant_uuid {
            command
                .arg("--resume")
                .arg(
                    resume
                        .original_session
                        .as_deref()
                        .ok_or("Missing original Claude session")?,
                )
                .arg("--resume-session-at")
                .arg(anchor)
                .arg("--resume-drops-turn")
                .arg(&resume.human_uuid)
                .arg("--fork-session");
        }
    } else if let Some(id) = &saved.thread_id {
        command.arg("--resume").arg(id);
    }
    if let Some(model) = &saved.composer.model {
        command.arg("--model").arg(model);
    }
    if let Some(effort) = &saved.composer.effort {
        command.arg("--effort").arg(effort);
    }
    if let Some(access) = &saved.composer.access {
        command
            .arg("--permission-mode")
            .arg(permission_mode(access)?);
    }
    for root in &saved.roots {
        if root.wt_path != saved.cwd {
            command.arg("--add-dir").arg(&root.wt_path);
        }
    }
    Ok(command)
}
fn spawn(args: &[String], saved: &SavedSession) -> Result<Process, String> {
    let mut command = launch_command(args, saved)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| format!("Cannot launch Claude Code. Install the Claude Code CLI and sign in with `claude auth login`: {e}"))?;
    let input = child.stdin.take().ok_or("Claude Code stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Claude Code stdout unavailable")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Claude Code stderr unavailable")?;
    let tail = Arc::new(Mutex::new(String::new()));
    let diagnostic = tail.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Ok(mut tail) = diagnostic.lock() {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 8192 {
                    let mut at = tail.len() - 8192;
                    while !tail.is_char_boundary(at) {
                        at += 1;
                    }
                    tail.drain(..at);
                }
            }
        }
    });
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let parsed = line.map_err(|e| e.to_string()).and_then(|line| {
                serde_json::from_str(&line).map_err(|e| format!("Invalid Claude Code stream: {e}"))
            });
            if send.send(parsed).is_err() {
                return;
            }
        }
        let detail = tail.lock().map(|v| v.trim().to_owned()).unwrap_or_default();
        let _ = send.send(Err(if detail.is_empty() {
            "Claude Code disconnected".into()
        } else {
            format!("Claude Code disconnected: {detail}")
        }));
    });
    Ok(Process {
        child,
        input,
        lines,
    })
}
fn permission_mode(access: &str) -> Result<&str, String> {
    match access {
        "read-only" | "plan" => Ok("plan"),
        "workspace-write" | "acceptEdits" => Ok("acceptEdits"),
        "full-access" | "danger-full-access" | "bypassPermissions" => Ok("bypassPermissions"),
        "default" | "manual" | "dontAsk" | "auto" => Ok(access),
        _ => Err(format!("Unsupported Claude Code permission mode: {access}")),
    }
}
/// Claude Code's own compaction command, submitted for the host `/compact`.
const COMPACT_COMMAND: &str = "/compact";
fn nonce() -> String {
    let hex = format!(
        "{:032x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..]
    )
}

fn user_content(text: &str, images: &[String]) -> Vec<Value> {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    for image in images {
        if let Some((media, data)) = image
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
        {
            content.push(
                json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}}),
            );
        }
    }
    content
}
struct Actor {
    state: Snapshot,
    args: Vec<String>,
    input: mpsc::Receiver<Action>,
    updates: crate::latest::Sender<Snapshot>,
    pid: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
    process: Option<Process>,
    initialized: bool,
    started: Instant,
    first_output: bool,
    reconfigure: bool,
    reset_configuration: bool,
    configuring: bool,
    tracks_session_state: bool,
    pending_completion: Option<Status>,
    session_idle: bool,
    initial: Option<String>,
    deferred: Option<QueuedPrompt>,
    in_flight: Option<QueuedPrompt>,
    input_uuid: Option<String>,
    inline_uuid: Option<String>,
    deferred_inline: bool,
    last_assistant_uuid: Option<String>,
    redirect_pending: bool,
    transcript: crate::claude_transcript::ClaudeTranscriptState,
    /// A host `/compact` is in flight; its echo is represented by the divider, not a bubble.
    compact_pending: bool,
    /// Model of the latest primary assistant message, used to pick its context window.
    context_model: Option<String>,
    queue_ready: bool,
    interrupting: bool,
    control_id: u64,
    pending: HashMap<String, String>,
    blocks: HashMap<u64, String>,
    subagent_tasks: HashMap<String, String>,
    message_id: String,
    title_generator: crate::session_title::Generator,
    last_persist: Instant,
    dirty: bool,
}
impl Actor {
    fn new(
        mut saved: SavedSession,
        args: Vec<String>,
        input: mpsc::Receiver<Action>,
        updates: crate::latest::Sender<Snapshot>,
        pid: Arc<AtomicU32>,
        stopping: Arc<AtomicBool>,
    ) -> Self {
        let args = if args.is_empty() {
            saved.config_args.clone()
        } else {
            args
        };
        saved.config_args = launch_options(&args).map_or_else(|_| args.clone(), |v| v.0);
        let mut transcript = crate::claude_transcript::ClaudeTranscriptState::default();
        transcript.restore(&mut saved.items);
        let last_assistant_uuid = saved
            .items
            .iter()
            .rev()
            .find_map(|item| item["claudeAssistantUuid"].as_str().map(str::to_owned));
        let mut state = Snapshot::new(saved);
        state.edit_supported = true;
        Self {
            state,
            args,
            input,
            updates,
            pid,
            stopping,
            process: None,
            initialized: false,
            started: Instant::now(),
            first_output: false,
            reconfigure: false,
            reset_configuration: false,
            configuring: false,
            tracks_session_state: false,
            pending_completion: None,
            session_idle: true,
            initial: None,
            deferred: None,
            in_flight: None,
            input_uuid: None,
            inline_uuid: None,
            deferred_inline: false,
            last_assistant_uuid,
            redirect_pending: false,
            transcript,
            compact_pending: false,
            context_model: None,
            queue_ready: true,
            interrupting: false,
            control_id: 0,
            pending: HashMap::new(),
            blocks: HashMap::new(),
            subagent_tasks: HashMap::new(),
            message_id: String::new(),
            title_generator: crate::session_title::Generator::default(),
            last_persist: Instant::now(),
            dirty: false,
        }
    }
    fn run(mut self) {
        if let Err(error) = self.start() {
            self.fail(error);
        }
        self.save();
        self.publish();
        loop {
            match self.input.recv_timeout(Duration::from_millis(20)) {
                Ok(Action::Stop { forget, done }) => {
                    self.process.take();
                    self.pid.store(0, Ordering::SeqCst);
                    if forget {
                        if !self.state.saved.key.is_empty()
                            && self.state.saved.key.len() < 128
                            && self
                                .state
                                .saved
                                .key
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        {
                            if let Ok(dir) = codex::recovery_dir() {
                                let _ = fs_err::remove_file(
                                    dir.join(format!("{}.json", self.state.saved.key)),
                                );
                            }
                        }
                    } else {
                        self.save();
                    }
                    let _ = done.send(());
                    break;
                }
                Ok(action) => {
                    if let Err(error) = self.action(action) {
                        self.state.error = Some(error);
                    }
                    self.save();
                    self.publish();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // stop_completion kills the process immediately; do not recreate it or
            // write recovery while its Stop action is still in flight.
            if self.stopping.load(Ordering::SeqCst) {
                self.title_generator.cancel();
                continue;
            }
            // The UI consumes complete state, so intermediate protocol deltas
            // need not each clone the accumulated conversation. A bounded batch
            // also lets actions and publication run during continuous output.
            if self.receive_batch() {
                self.publish();
            }
            if self
                .title_generator
                .tick(&mut self.state.saved, self.state.status, false)
            {
                self.dirty = true;
                self.publish();
            }
            if self.dirty && self.last_persist.elapsed() >= Duration::from_millis(250) {
                self.save();
            }
            if self.process.is_some()
                && !self.initialized
                && self.started.elapsed() > Duration::from_mins(1)
            {
                self.fail("Claude Code initialization timed out".into());
                self.publish();
            }
            if self.drain_queue() {
                self.save();
                self.publish();
            }
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
    }
    fn drain_queue(&mut self) -> bool {
        if self.initialized
            && self.state.queue_uncertain.is_empty()
            && !self.state.status.busy()
            && (self.queue_ready || self.deferred.is_some() || self.initial.is_some())
        {
            let next = self
                .deferred
                .take()
                .or_else(|| {
                    self.initial.take().map(|text| QueuedPrompt {
                        id: 0,
                        text,
                        images: Vec::new(),
                    })
                })
                .or_else(|| {
                    if !self.queue_ready
                        || self.state.queued_prompts.is_empty()
                        || self.state.queued_prompts.first().is_some_and(|p| {
                            self.state.queue_edit == Some(p.id)
                                || self.state.queue_sending.contains(&p.id)
                        })
                    {
                        None
                    } else {
                        self.state.queued_prompts.first().cloned()
                    }
                });
            if let Some(prompt) = next {
                if let Err(error) = self.submit(prompt.id, prompt.text, prompt.images) {
                    self.state.error = Some(error);
                    self.queue_ready = false;
                }
                return true;
            }
        }
        false
    }
    fn save(&mut self) {
        self.last_persist = Instant::now();
        if let Err(error) = codex::persist(&self.state.saved) {
            self.state.error = Some(error);
        } else {
            self.dirty = false;
        }
    }
    fn receive_batch(&mut self) -> bool {
        let mut received = false;
        for _ in 0..256 {
            let message = self.process.as_ref().and_then(|p| p.lines.try_recv().ok());
            let Some(message) = message else { break };
            match message {
                Ok(value) => {
                    if !self.first_output {
                        self.first_output = true;
                        tracing::info!(
                            stage = "first_protocol_output",
                            elapsed_ms = self.started.elapsed().as_millis() as u64,
                            "Claude Code startup"
                        );
                    }
                    if let Err(error) = self.message(value) {
                        self.fail(error);
                    }
                }
                Err(error) => self.fail(error),
            }
            self.dirty = true;
            received = true;
        }
        received
    }
    fn publish(&self) {
        let mut snapshot = self.state.clone();
        snapshot.can_redirect = self.initialized
            && self.process.is_some()
            && self.state.status.busy()
            && self.state.turn_id.is_some()
            && !self.state.compacting
            && self.in_flight.is_none()
            && self.pending_completion.is_none()
            && !self.configuring
            && !self.interrupting
            && !self.redirect_pending
            && self.state.saved.pending_edit.is_none();
        let _ = self.updates.send(snapshot);
    }
    fn fail(&mut self, error: String) {
        if !self.initialized {
            // Deliberately omit provider payloads, prompts, paths and raw errors.
            tracing::warn!(
                stage = "initialization_failed",
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                first_output = self.first_output,
                "Claude Code startup"
            );
        }
        self.redirect_pending = false;
        self.deferred_inline = false;
        self.state.finish_work();
        self.state.mark_queue_uncertain();
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        self.initialized = false;
        self.queue_ready = false;
        self.state.status = Status::Failed;
        self.state.turn_id = None;
        self.state.compacting = false;
        self.compact_pending = false;
        self.state.approvals.clear();
        self.state.questions.clear();
        self.state.elicitations.clear();
        self.state.error = Some(error);
        if let Some(prompt) = self.in_flight.take() {
            self.state.rejected_submission = Some(prompt.id);
        }
        self.input_uuid = None;
        self.inline_uuid = None;
        if let Some(prompt) = self.deferred.take() {
            self.state.rejected_submission = Some(prompt.id);
        }
    }
    fn write(&mut self, value: Value) -> Result<(), String> {
        let process = self
            .process
            .as_mut()
            .ok_or("Claude Code is disconnected; reconnect to continue")?;
        serde_json::to_writer(&mut process.input, &value).map_err(|e| e.to_string())?;
        process
            .input
            .write_all(b"\n")
            .and_then(|()| process.input.flush())
            .map_err(|e| format!("Cannot send to Claude Code: {e}"))
    }
    fn control(&mut self, request: Value) -> Result<(), String> {
        self.control_id += 1;
        let id = format!("grove-{}", self.control_id);
        self.pending.insert(
            id.clone(),
            request["subtype"].as_str().unwrap_or_default().into(),
        );
        self.write(json!({"type":"control_request","request_id":id,"request":request}))
    }
    fn start(&mut self) -> Result<(), String> {
        self.started = Instant::now();
        self.first_output = false;
        self.initialized = false;
        tracing::info!(stage = "launch_started", "Claude Code startup");
        self.redirect_pending = false;
        if self.in_flight.is_some() {
            self.state.mark_queue_uncertain();
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        if self.stopping.load(Ordering::SeqCst) {
            return Err("Claude Code session is stopping".into());
        }
        // Forking never mutates the original provider conversation. On uncertain
        // startup, restore the current workspace and retain the original history.
        if self.state.saved.pending_edit.is_some() && self.process.is_none() {
            self.recover_edit()?;
        }
        let (args, initial) = launch_options(&self.args)?;
        if self.state.saved.thread_id.is_none() && self.initial.is_none() && self.deferred.is_none()
        {
            self.initial = initial;
        }
        let process = spawn(&args, &self.state.saved)?;
        tracing::info!(
            stage = "process_spawned",
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "Claude Code startup"
        );
        self.pid.store(process.child.id(), Ordering::SeqCst);
        self.process = Some(process);
        // Close the publication race with Handle::stop_completion.
        if self.stopping.load(Ordering::SeqCst) {
            self.process.take();
            self.pid.store(0, Ordering::SeqCst);
            return Err("Claude Code session is stopping".into());
        }
        if let Some(prompt) = self.in_flight.take() {
            self.state.rejected_submission = Some(prompt.id);
        }
        self.input_uuid = None;
        self.inline_uuid = None;
        self.compact_pending = false;
        self.state.compacting = false;
        self.initialized = false;
        self.reconfigure = false;
        self.reset_configuration = false;
        self.configuring = false;
        self.tracks_session_state = false;
        self.pending_completion = None;
        self.pending.clear();
        self.blocks.clear();
        self.state.status = Status::Starting;
        self.state.error = None;
        self.state.approvals.clear();
        self.state.questions.clear();
        self.state.elicitations.clear();
        self.state.turn_id = None;
        self.control(json!({"subtype":"initialize","hooks":null}))?;
        tracing::info!(
            stage = "initialize_sent",
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "Claude Code startup"
        );
        Ok(())
    }
    fn configure_live(&mut self) -> Result<(), String> {
        self.configuring = true;
        self.reconfigure = false;
        self.state.status = Status::Starting;
        if let Some(model) = self.state.model.clone() {
            self.control(json!({"subtype":"set_model","model":model}))?;
        }
        if let Some(access) = self.state.access.clone() {
            self.control(
                json!({"subtype":"set_permission_mode","mode":permission_mode(&access)?}),
            )?;
        }
        self.control(
            json!({"subtype":"apply_flag_settings","settings":{"effortLevel":self.state.effort}}),
        )?;
        Ok(())
    }
    fn capture_edit_checkpoint(&mut self) {
        let mut roots = vec![std::path::PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| std::path::PathBuf::from(&root.wt_path)),
        );
        if let Ok(files) = crate::checkpoint::repository_roots(&roots)
            .and_then(|roots| crate::checkpoint::capture(&roots))
        {
            self.state.saved.edit_checkpoints.push(EditCheckpoint {
                start_item: self.state.saved.items.len(),
                turn_id: None,
                files,
            });
        }
    }
    fn redirect(&mut self, id: u64, text: String, images: Vec<String>) -> Result<(), String> {
        codex::validate_prompt(&text, &images)?;
        if !self.initialized
            || !self.state.status.busy()
            || self.state.turn_id.is_none()
            || self.state.compacting
            || self.in_flight.is_some()
            || self.pending_completion.is_some()
            || self.configuring
            || self.interrupting
            || self.redirect_pending
        {
            return Err("Wait for an active Claude Code turn before steering.".into());
        }
        let uuid = nonce();
        let queued = self.state.queued_prompts.iter().any(|p| p.id == id);
        if queued {
            self.state.queue_sending.push(id);
            self.state.queue_steering.push(id);
        }
        // Priority next is Claude Code's native steer, like Codex's same-turn steer:
        // the message joins the running turn at its next tool boundary, or runs
        // straight after the turn when no tool boundary remains. Never "now",
        // which aborts the running tools or stream.
        if let Err(error) = self.write(json!({"type":"user","uuid":uuid,"session_id":self.state.saved.thread_id,"priority":"next","origin":{"kind":"human"},"message":{"role":"user","content":user_content(&text,&images)},"parent_tool_use_id":null})) {
            self.state.mark_queue_uncertain();
            self.queue_ready = false;
            return Err(error);
        }
        self.input_uuid = Some(uuid);
        self.in_flight = Some(QueuedPrompt { id, text, images });
        self.redirect_pending = true;
        self.pending_completion = None;
        self.state.error = None;
        Ok(())
    }
    fn edit_message(
        &mut self,
        index: usize,
        message_id: String,
        composer_draft: Option<(String, Vec<String>)>,
    ) -> Result<(), String> {
        let checkpoint = self.state.editable_message(index)?.clone();
        if self.process.is_none()
            || !self.pending.is_empty()
            || self.in_flight.is_some()
            || !self.initialized
            || (self.tracks_session_state && !self.session_idle)
        {
            return Err("Wait for Claude Code to finish reconnecting before editing.".into());
        }
        let item = &self.state.saved.items[index];
        if item["id"] != message_id {
            return Err("This message changed; select Edit again.".into());
        }
        let text = codex::item_text(item);
        let images = item["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "image")
            .filter_map(|part| part["url"].as_str().map(str::to_owned))
            .collect();
        let mut roots = vec![std::path::PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| std::path::PathBuf::from(&root.wt_path)),
        );
        let recovery = crate::checkpoint::capture(&crate::checkpoint::repository_roots(&roots)?)?;
        self.state.saved.pending_edit = Some(PendingEdit {
            target: checkpoint.start_item,
            turn_id: checkpoint.turn_id.ok_or("Missing human turn identity")?,
            text,
            images,
            recovery,
            composer_draft,
            claude_fork_id: Some(nonce()),
        });
        // Persist the complete original transcript and recovery snapshot before
        // touching files. Fork failure cannot destroy the original conversation.
        codex::persist(&self.state.saved)?;
        if let Err(error) = checkpoint.files.restore() {
            self.recover_edit()?;
            return Err(error);
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        let args = launch_options(&self.args)?.0;
        let process = match spawn(&args, &self.state.saved) {
            Ok(process) => process,
            Err(error) => {
                self.recover_edit()?;
                return Err(error);
            }
        };
        self.pid.store(process.child.id(), Ordering::SeqCst);
        self.process = Some(process);
        self.pending.clear();
        self.blocks.clear();
        self.message_id.clear();
        self.started = Instant::now();
        self.redirect_pending = false;
        self.initialized = false;
        self.configuring = false;
        self.tracks_session_state = false;
        self.pending_completion = None;
        self.session_idle = true;
        self.state.status = Status::Starting;
        self.control(json!({"subtype":"initialize","hooks":null}))?;
        Ok(())
    }
    fn recover_edit(&mut self) -> Result<(), String> {
        if let Some(edit) = &self.state.saved.pending_edit {
            edit.recovery.restore()?;
        }
        let mut saved = self.state.saved.clone();
        saved.pending_edit = None;
        codex::persist(&saved)?;
        self.state.saved = saved;
        Ok(())
    }
    fn finish_edit(&mut self) -> Result<(), String> {
        let Some(edit) = self.state.saved.pending_edit.clone() else {
            return Ok(());
        };
        let mut saved = self.state.saved.clone();
        saved.claude_resume = Some(codex::ClaudeResume {
            original_session: saved.claude_resume.as_ref().map_or_else(
                || saved.thread_id.clone(),
                |resume| resume.original_session.clone(),
            ),
            assistant_uuid: saved
                .items
                .get(edit.target)
                .and_then(|item| item["claudePreviousAssistantUuid"].as_str())
                .map(str::to_owned),
            human_uuid: edit.turn_id.clone(),
            fork_id: edit
                .claude_fork_id
                .clone()
                .ok_or("Missing Claude fork identity")?,
        });
        saved.thread_id.clone_from(&edit.claude_fork_id);
        saved.items.truncate(edit.target);
        saved.rich_plans.retain_messages(&saved.items);
        saved
            .edit_checkpoints
            .retain(|checkpoint| checkpoint.start_item < edit.target);
        saved
            .work_periods
            .retain(|period| period.start_item < edit.target);
        let (text, images) = edit.composer_draft.unwrap_or((edit.text, edit.images));
        saved.draft = text;
        saved.draft_images = images;
        saved.pending_edit = None;
        saved.context = None;
        codex::persist(&saved)?;
        self.state.saved = saved;
        self.transcript.restore(&mut self.state.saved.items);
        self.last_assistant_uuid = self
            .state
            .saved
            .items
            .iter()
            .rev()
            .find_map(|item| item["claudeAssistantUuid"].as_str().map(str::to_owned));
        self.state.edited_messages += 1;
        self.state.context_tokens = None;
        self.state.turn_id = None;
        Ok(())
    }
    fn submit(&mut self, id: u64, text: String, images: Vec<String>) -> Result<(), String> {
        let result = self.submit_inner(id, text, images);
        if result.is_err() {
            self.deferred_inline = false;
            self.state.mark_queue_uncertain();
            self.state.rejected_submission = Some(id);
        }
        result
    }
    fn submit_inner(&mut self, id: u64, text: String, images: Vec<String>) -> Result<(), String> {
        codex::validate_prompt(&text, &images)?;
        if !self.initialized || self.state.status.busy() {
            return Err(
                "Claude Code is busy; queue this message to send it after the current turn".into(),
            );
        }
        if self.state.queued_prompts.iter().any(|p| p.id == id)
            && !self.state.queue_sending.contains(&id)
        {
            self.state.queue_sending.push(id);
        }
        if self.reconfigure {
            self.deferred = Some(QueuedPrompt { id, text, images });
            if self.reset_configuration {
                self.start()?;
            } else {
                self.configure_live()?;
            }
            return Ok(());
        }
        self.capture_edit_checkpoint();
        let content = user_content(&text, &images);
        let uuid = nonce();
        self.write(json!({"type":"user","uuid":uuid,"session_id":self.state.saved.thread_id.as_deref().unwrap_or(""),"message":{"role":"user","content":content},"parent_tool_use_id":null}))?;
        if let Some(checkpoint) = self
            .state
            .saved
            .edit_checkpoints
            .last_mut()
            .filter(|checkpoint| checkpoint.turn_id.is_none())
        {
            checkpoint.turn_id = Some(uuid.clone());
        }
        self.input_uuid = Some(uuid.clone());
        if std::mem::take(&mut self.deferred_inline) {
            self.inline_uuid = Some(uuid.clone());
        }
        self.in_flight = Some(QueuedPrompt { id, text, images });
        self.state.begin_work();
        self.state.status = Status::Working;
        self.session_idle = false;
        self.state.turn_id = Some(uuid);
        self.state.error = None;
        self.interrupting = false;
        self.blocks.clear();
        self.message_id.clear();
        Ok(())
    }
    fn acknowledge_input(&mut self, uuid: &Value) {
        if uuid.as_str() != self.input_uuid.as_deref() || self.input_uuid.is_none() {
            return;
        }
        let inline = self.inline_uuid.as_deref() == self.input_uuid.as_deref();
        self.input_uuid = None;
        self.inline_uuid = None;
        // An acknowledged steer has joined the turn; another one may follow.
        self.redirect_pending = false;
        self.state.saved.claude_resume = None;
        let Some(prompt) = self.in_flight.take() else {
            return;
        };
        if std::mem::take(&mut self.compact_pending) && prompt.text == COMPACT_COMMAND {
            self.state.accepted_prompts += 1;
            self.state.accepted_submission = Some(prompt.id);
            self.state.rejected_submission = None;
            return;
        }
        let mut parts = vec![json!({"type":"text","text":prompt.text})];
        parts.extend(
            prompt
                .images
                .iter()
                .map(|url| json!({"type":"image","url":url})),
        );
        self.upsert(json!({"id":uuid,"type":"userMessage","content":parts,"claudeHumanUuid":uuid,"claudePreviousAssistantUuid":self.last_assistant_uuid}));
        if self.state.saved.title.is_none() {
            self.state.saved.title = Some(prompt.text.chars().take(80).collect());
        }
        if !inline && !self.state.queued_prompts.iter().any(|p| p.id == prompt.id) {
            self.state
                .saved
                .clear_submitted_draft(&prompt.text, &prompt.images);
        }
        self.state
            .queued_prompts
            .retain(|queued| queued.id != prompt.id);
        self.state.queue_sending.retain(|id| *id != prompt.id);
        self.state.queue_steering.retain(|id| *id != prompt.id);
        self.state.queue_uncertain.retain(|id| *id != prompt.id);
        self.state.saved.rich_plans.confirm_delivery(prompt.id);
        self.state.accepted_prompts += 1;
        self.state.accepted_submission = Some(prompt.id);
        self.state.rejected_submission = None;
    }
    fn clear_conversation(
        &mut self,
        persist_saved: impl FnOnce(&SavedSession) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.state.status.busy()
            || self.state.turn_id.is_some()
            || self.state.compacting
            || self.compact_pending
            || self.in_flight.is_some()
            || self.deferred.is_some()
            || self.configuring
            || self.interrupting
            || self.pending_completion.is_some()
            || (self.tracks_session_state && !self.session_idle)
        {
            return Err(codex::CLEAR_BUSY.into());
        }
        if self.process.is_none() {
            return Err(codex::CLEAR_DISCONNECTED.into());
        }
        let args = launch_options(&self.args)?.0;
        let saved = self.state.saved.fresh_conversation();
        persist_saved(&saved).map_err(|e| format!("Could not clear conversation: {e}"))?;
        // Dropping the process also drops its event receiver, fencing late events.
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        self.initial = None;
        self.args = args;
        self.pending.clear();
        self.blocks.clear();
        self.message_id.clear();
        self.input_uuid = None;
        self.inline_uuid = None;
        self.context_model = None;
        self.queue_ready = true;
        let cleared = self.state.cleared_conversations + 1;
        self.state = Snapshot::new(saved);
        self.state.edit_supported = true;
        self.last_assistant_uuid = None;
        self.deferred_inline = false;
        self.redirect_pending = false;
        self.transcript.restore(&mut self.state.saved.items);
        self.state.cleared_conversations = cleared;
        Ok(())
    }
    fn send_plan_response(
        &mut self,
        plan_id: &str,
        revision_id: &str,
        persist_saved: impl Fn(&SavedSession) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.process.is_none() || !self.initialized {
            return Err("Reconnect before sending plan feedback".into());
        }
        if let Some((id, text)) = self
            .state
            .saved
            .rich_plans
            .prepare_delivery(plan_id, revision_id)?
        {
            if let Err(error) = persist_saved(&self.state.saved) {
                self.state.saved.rich_plans.fail_delivery(id, &error);
                return Err(error);
            }
            self.state.queued_prompts.push(QueuedPrompt {
                id,
                text,
                images: Vec::new(),
            });
            if !self.state.status.busy() {
                self.queue_ready = true;
            }
        }
        Ok(())
    }
    fn action(&mut self, action: Action) -> Result<(), String> {
        if self.state.saved.pending_edit.is_some()
            && !matches!(
                action,
                Action::Reconnect | Action::Stop { .. } | Action::Draft(_) | Action::DraftImages(_)
            )
        {
            return Err("An interrupted edit needs recovery. Reconnect before continuing.".into());
        }
        match action {
            Action::UpdatePlanReview {
                plan_id,
                revision_id,
                review,
            } => {
                self.state
                    .saved
                    .rich_plans
                    .update_review(&plan_id, &revision_id, review)?;
                codex::persist(&self.state.saved)?;
            }
            Action::RetryPlanImport { message_id, source } => {
                crate::rich_plan::retry_import(&mut self.state.saved, &message_id, &source)?;
                codex::persist(&self.state.saved)?;
            }
            Action::SendPlanResponse {
                plan_id,
                revision_id,
            } => self.send_plan_response(&plan_id, &revision_id, codex::persist)?,
            Action::EditMessage { index, message_id } => {
                self.edit_message(index, message_id, None)?;
            }
            Action::EditMessageInline { index, message_id } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.edit_message(index, message_id, Some(draft))?;
            }
            Action::PromptInline { text, images } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.submit(0, text, images)?;
                self.inline_uuid.clone_from(&self.input_uuid);
                self.deferred_inline = self.deferred.is_some();
                self.state.saved.draft = draft.0;
                self.state.saved.draft_images = draft.1;
            }
            Action::ClearConversation => {
                self.clear_conversation(codex::persist)?;
                if let Err(error) = self.start() {
                    self.fail(error);
                }
            }
            Action::Prompt(text) => self.submit(0, text, Vec::new())?,
            Action::Compact => {
                if self.process.is_none() {
                    return Err(codex::COMPACT_DISCONNECTED.into());
                }
                if self.state.status.busy() || self.in_flight.is_some() {
                    return Err(codex::COMPACT_BUSY.into());
                }
                if !self.initialized {
                    return Err(codex::COMPACT_DISCONNECTED.into());
                }
                if self.state.saved.thread_id.is_none()
                    || !codex::has_conversation(&self.state.saved.items)
                {
                    return Err(codex::NOTHING_TO_COMPACT.into());
                }
                // Claude Code's own slash command, through the ordinary prompt path.
                self.submit(0, COMPACT_COMMAND.into(), Vec::new())?;
                self.compact_pending = true;
            }
            Action::PromptWithImages { text, images } => self.submit(0, text, images)?,
            Action::Submit { id, text, images } => self.submit(id, text, images)?,
            Action::SubmitWithMode {
                id,
                text,
                images,
                mode,
            } => match mode {
                SubmissionMode::Send => self.submit(id, text, images)?,
                SubmissionMode::Steer => {
                    if let Err(error) = self.redirect(id, text, images) {
                        self.state.rejected_submission = Some(id);
                        return Err(error);
                    }
                }
                SubmissionMode::Queue => {
                    if let Err(error) = codex::validate_prompt(&text, &images) {
                        self.state.rejected_submission = Some(id);
                        return Err(error);
                    }
                    self.state.saved.clear_submitted_draft(&text, &images);
                    self.state
                        .queued_prompts
                        .push(QueuedPrompt { id, text, images });
                    self.state.accepted_submission = Some(id);
                }
            },
            Action::BeginQueuedEdit(id) => self.state.begin_queue_edit(id)?,
            Action::SaveQueuedEdit { id, text, images } => {
                self.state.save_queue_edit(id, text, images)?;
            }
            Action::CancelQueuedEdit(id) => {
                if self.state.queue_edit == Some(id) {
                    self.state.queue_edit = None;
                }
            }
            Action::SteerQueued(id) => {
                if !self.state.queue_editable(id) || self.state.queue_edit == Some(id) {
                    return Err("This queued input cannot be steered right now".into());
                }
                let prompt = self
                    .state
                    .queued_prompts
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
                    .ok_or("Queued input unavailable")?;
                self.redirect(id, prompt.text, prompt.images)?;
            }
            Action::RemoveQueued(id) => self.state.remove_queued(id)?,
            Action::DismissUncertainQueued(id) => {
                self.state.dismiss_uncertain_queued(id)?;
                if self.deferred.as_ref().is_some_and(|p| p.id == id) {
                    self.deferred = None;
                }
                self.queue_ready = false;
            }
            Action::ResumeQueue => {
                if !self.state.queue_uncertain.is_empty() {
                    return Err(
                        "Inspect and dismiss unconfirmed deliveries before resuming queued inputs"
                            .into(),
                    );
                }
                if self.state.queued_prompts.is_empty() {
                    return Err("There are no queued prompts to send.".into());
                }
                if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting
                {
                    return Err("Wait for the current turn before resuming queued inputs".into());
                }
                if self.process.is_none() {
                    return Err("Reconnect before resuming queued inputs".into());
                }
                self.queue_ready = true;
            }
            Action::Draft(text) => {
                if text.len() > 512 * 1024 {
                    return Err("Draft text limit: 512 KiB".into());
                }
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((draft, _)) = &mut edit.composer_draft {
                        draft.clone_from(&text);
                    }
                }
                self.state.saved.draft = text;
            }
            Action::DraftImages(images) => {
                codex::validate_images(&images)?;
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((_, draft_images)) = &mut edit.composer_draft {
                        draft_images.clone_from(&images);
                    }
                }
                self.state.saved.draft_images = images;
            }
            Action::Metadata { project, roots } => {
                self.state.saved.project = project;
                if self.state.saved.roots != roots {
                    self.reconfigure = true;
                    self.reset_configuration = true;
                }
                self.state.saved.roots = roots;
            }
            Action::Configure {
                model,
                effort,
                access,
            } => {
                if let Some(effort) = &effort {
                    if !["low", "medium", "high", "xhigh", "max"].contains(&effort.as_str()) {
                        return Err(format!("Unsupported Claude Code effort: {effort}"));
                    }
                }
                if let Some(access) = &access {
                    permission_mode(access)?;
                }
                if let Some(model) = model {
                    if self.state.model.as_deref() != Some(model.as_str()) {
                        self.state.effort = None;
                        self.state.saved.composer.effort = None;
                    }
                    self.state.model = Some(model.clone());
                    self.state.saved.composer.model = Some(model);
                }
                if let Some(effort) = effort {
                    self.state.effort = Some(effort.clone());
                    self.state.saved.composer.effort = Some(effort);
                }
                if let Some(access) = access {
                    self.state.access = Some(access.clone());
                    self.state.saved.composer.access = Some(access);
                }
                self.state.saved.composer.reset_to_configured = false;
                self.reconfigure = true;
            }

            Action::ResetConfiguration => {
                self.reset_configuration = true;
                self.state.saved.composer = codex::ComposerSettings::default();
                self.state.model = None;
                self.state.effort = None;
                self.state.access = None;
                self.reconfigure = true;
            }
            Action::Interrupt => {
                if !self.state.status.busy() {
                    return Err("No active Claude Code turn to interrupt".into());
                }
                self.queue_ready = false;
                self.interrupting = true;
                self.control(json!({"subtype":"interrupt"}))?;
                self.state.questions.clear();
                self.state.elicitations.clear();
                self.state.approvals.clear();
            }
            Action::AnswerQuestions(id, answers) => {
                let at = self
                    .state
                    .questions
                    .iter()
                    .position(|q| q.id == id)
                    .ok_or("This question request has expired")?;
                let request = &self.state.questions[at];
                let answered = answers.is_some();
                let response = if let Some(answers) = answers {
                    request.validate(&answers)?;
                    request.claude_response(answers)
                } else {
                    json!({"behavior":"deny","message":"The user cancelled these questions."})
                };
                self.respond(id.clone(), response)?;
                if answered {
                    let ack = self.state.question_answer_ack(id);
                    self.state.question_answer_acks.push(ack);
                }
                self.state.questions.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::Approval(id, accept) => {
                return self.action(Action::RespondApproval {
                    id,
                    choice: if accept { "allow_once" } else { "deny" }.into(),
                    input: None,
                    feedback: None,
                });
            }
            Action::RespondApproval {
                id,
                choice,
                input,
                feedback,
            } => {
                let at = self
                    .state
                    .approvals
                    .iter()
                    .position(|a| a["id"] == id)
                    .ok_or("This approval has expired")?;
                let response = crate::controls::approval_response(
                    &self.state.approvals[at],
                    &choice,
                    input,
                    feedback,
                )?;
                self.respond(id, response)?;
                self.state.approvals.remove(at);
                if choice == "cancel" {
                    self.interrupting = true;
                    self.queue_ready = false;
                    self.state.approvals.clear();
                    self.state.questions.clear();
                    self.state.elicitations.clear();
                }
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::RespondElicitation {
                id,
                action,
                content,
            } => {
                let at = self
                    .state
                    .elicitations
                    .iter()
                    .position(|r| r.id == id)
                    .ok_or("This request has expired")?;
                if !matches!(action.as_str(), "accept" | "decline" | "cancel") {
                    return Err("Invalid elicitation action".into());
                }
                if action == "accept" {
                    self.state.elicitations[at]
                        .validate_content(content.as_ref().unwrap_or(&Value::Null))?;
                }
                let content = if action == "accept" {
                    match &self.state.elicitations[at].mode {
                        crate::elicitation::ElicitationMode::Form(form) => Some(
                            form.validate(
                                content
                                    .as_ref()
                                    .and_then(Value::as_object)
                                    .ok_or("Form response must be an object")?,
                            )
                            .map_err(|_| "Invalid form content")?,
                        ),
                        crate::elicitation::ElicitationMode::Url { .. } => None,
                    }
                } else {
                    None
                };
                let response = if action == "accept" && content.is_some() {
                    json!({"action":action,"content":content})
                } else {
                    json!({"action":action})
                };
                self.respond(id, response)?;
                self.state.elicitations.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    if self.state.turn_id.is_some() {
                        Status::Working
                    } else {
                        Status::Idle
                    }
                } else {
                    Status::Waiting
                };
            }
            Action::Reconnect => {
                self.queue_ready = false;
                self.start()?;
            }
            Action::Stop { .. } => unreachable!("Stop handled by run loop"),
        }
        Ok(())
    }
    fn respond(&mut self, id: Value, response: Value) -> Result<(), String> {
        self.write(json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}))
    }
    fn upsert(&mut self, mut item: Value) {
        let previous = self
            .state
            .saved
            .items
            .iter()
            .find(|old| old["id"] == item["id"]);
        codex::stamp_message_received(&mut item, previous);
        if let Some(existing) = self
            .state
            .saved
            .items
            .iter_mut()
            .find(|old| old["id"] == item["id"])
        {
            *existing = item;
        } else {
            self.state.saved.items.push(item);
        }
    }
    fn register_subagent(&mut self, block: &Value) {
        if !matches!(block["name"].as_str(), Some("Agent" | "Task")) {
            return;
        }
        let Some(id) = block["id"].as_str() else {
            return;
        };
        let input = &block["input"];
        let name = input["name"]
            .as_str()
            .or_else(|| input["subagent_type"].as_str());
        self.state.upsert_subagent(
            id,
            name,
            input["description"].as_str(),
            SubagentStatus::Working,
        );
        self.state.set_subagent_metadata(
            id,
            input["subagent_type"].as_str(),
            input["description"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .or_else(|| input["prompt"].as_str()),
        );
    }
    fn subagent_message(&mut self, parent: &str, value: &Value) {
        // Child frames must never reach the primary transcript, usage, or turn lifecycle.
        self.state
            .upsert_subagent(parent, None, None, SubagentStatus::Unknown);
        self.subagent_system_message(value);
        match value["type"].as_str() {
            Some("assistant") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for block in blocks {
                        self.subagent_block(parent, block);
                    }
                }
            }
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_start" {
                    self.subagent_block(parent, &event["content_block"]);
                }
            }
            Some("user") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for block in blocks {
                        self.subagent_tool_result(block, value.get("tool_use_result"));
                    }
                }
            }
            Some("result") => self.state.upsert_subagent(
                parent,
                None,
                None,
                if value["is_error"] == true {
                    SubagentStatus::Failed
                } else {
                    SubagentStatus::Completed
                },
            ),
            _ => {}
        }
    }
    fn subagent_block(&mut self, parent: &str, block: &Value) {
        match block["type"].as_str() {
            Some("tool_use") => {
                self.register_subagent(block);
                if let Some(tool) = block["name"].as_str() {
                    self.state.upsert_subagent(
                        parent,
                        None,
                        Some(&format!("Using {tool}")),
                        SubagentStatus::Unknown,
                    );
                }
            }
            Some("text") => {
                if let Some(text) = block["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    self.state
                        .upsert_subagent(parent, None, Some(text), SubagentStatus::Unknown);
                }
            }
            Some("thinking") => {
                self.state
                    .upsert_subagent(parent, None, Some("Thinking"), SubagentStatus::Unknown);
            }
            _ => {}
        }
    }
    fn subagent_tool_result(&mut self, block: &Value, structured: Option<&Value>) {
        if structured
            .is_some_and(|result| result["isAsync"] == true || result["status"] == "async_launched")
        {
            return;
        }
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        if !self
            .state
            .saved
            .work_periods
            .iter()
            .any(|period| period.subagents.iter().any(|agent| agent.id == id))
        {
            return;
        }
        self.state.upsert_subagent(
            id,
            None,
            None,
            if block["is_error"] == true {
                SubagentStatus::Failed
            } else {
                SubagentStatus::Completed
            },
        );
    }
    fn subagent_system_message(&mut self, value: &Value) {
        if value["type"] != "system" {
            return;
        }
        let task = value["task_id"].as_str();
        if let (Some(task), Some(tool)) = (task, value["tool_use_id"].as_str()) {
            self.subagent_tasks.insert(task.to_owned(), tool.to_owned());
        }
        let Some(id) = value["tool_use_id"]
            .as_str()
            .or_else(|| task.and_then(|task| self.subagent_tasks.get(task).map(String::as_str)))
            .or(task)
            .map(str::to_owned)
        else {
            return;
        };
        let known = self
            .state
            .saved
            .work_periods
            .iter()
            .any(|period| period.subagents.iter().any(|agent| agent.id == id));
        if !known
            && value["task_type"] != "local_agent"
            && value["subagent_type"].as_str().is_none()
        {
            return;
        }
        let status = match value["subtype"].as_str() {
            Some("task_started") => SubagentStatus::Working,
            Some("task_progress") => SubagentStatus::Unknown,
            Some("task_notification") => match value["status"].as_str() {
                Some("completed") => SubagentStatus::Completed,
                Some("failed") => SubagentStatus::Failed,
                _ => SubagentStatus::Unknown,
            },
            _ => return,
        };
        let activity = value["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| value["description"].as_str());
        self.state
            .upsert_subagent(&id, value["subagent_type"].as_str(), activity, status);
        self.state.set_subagent_metadata(
            &id,
            value["subagent_type"].as_str(),
            value["description"].as_str(),
        );
        if value["subtype"] == "task_notification" && status == SubagentStatus::Unknown {
            // A stopped child is no longer working; activity-only updates use
            // Unknown as a sentinel, so explicit lifecycle updates set it here.
            if let Some(agent) = self
                .state
                .saved
                .work_periods
                .iter_mut()
                .rev()
                .flat_map(|period| period.subagents.iter_mut())
                .find(|agent| agent.id == id)
            {
                agent.status = SubagentStatus::Unknown;
            }
        }
    }
    fn message(&mut self, value: Value) -> Result<(), String> {
        if let Some(parent) = value["parent_tool_use_id"].as_str() {
            self.subagent_message(parent, &value);
            return Ok(());
        }
        if let Some(session) = value["session_id"].as_str().filter(|id| !id.is_empty()) {
            if self.state.saved.pending_edit.is_some() {
                // A fork's new identity is committed together with its truncated
                // transcript only after initialize succeeds.
            } else if let Some(primary) = &self.state.saved.thread_id {
                if primary != session {
                    return Ok(());
                }
            } else {
                self.state.saved.thread_id = Some(session.into());
            }
        }
        self.subagent_system_message(&value);
        match value["type"].as_str().unwrap_or_default() {
            "control_response" => {
                let response = &value["response"];
                let pending = response["request_id"]
                    .as_str()
                    .and_then(|id| self.pending.remove(id));
                if pending.is_none() {
                    return Ok(());
                }
                if pending.as_deref() == Some("get_context_usage") {
                    // Optional measurement: an error never fails the session.
                    if response["subtype"] == "success" {
                        let data = &response["response"];
                        self.state.record_context(
                            data["totalTokens"].as_u64(),
                            data["maxTokens"].as_u64(),
                        );
                    }
                } else if response["subtype"] == "error" {
                    return Err(response["error"]
                        .as_str()
                        .unwrap_or("Claude Code rejected a control request")
                        .into());
                }
                if self.configuring && self.pending.is_empty() {
                    self.configuring = false;
                    self.state.status = Status::Idle;
                    self.state.resolved_model = self.state.model.clone();
                    self.state.resolved_access = self.state.access.clone();
                    self.state.resolved_effort = self.state.effort.clone();
                }
                if pending.as_deref() == Some("initialize") {
                    self.finish_edit()?;
                    self.initialized = true;
                    tracing::info!(
                        stage = "initialized",
                        elapsed_ms = self.started.elapsed().as_millis() as u64,
                        "Claude Code startup"
                    );
                    self.state.status = Status::Idle;
                    let data = &response["response"];
                    self.tracks_session_state = data.get("session_state").is_some();
                    self.session_idle = data["session_state"] == "idle";
                    self.state.resolved_access =
                        data["current_permission_mode"].as_str().map(str::to_owned);
                    self.state.resolved_model = data["models"]
                        .as_array()
                        .and_then(|models| models.iter().find(|m| m["value"] == "default"))
                        .and_then(|m| m["resolvedModel"].as_str())
                        .map(str::to_owned);

                    if let Some(models) = data["models"].as_array() {
                        self.state.models=models.iter().map(|m| json!({"model":m["value"],"resolvedModel":m["resolvedModel"],"displayName":m["displayName"],"description":m["description"],"supportedReasoningEfforts":m["supportedEffortLevels"].as_array().map(|values| values.iter().map(|v| json!({"reasoningEffort":v})).collect::<Vec<_>>()).unwrap_or_default()})).collect();
                    }
                    if let Some(commands) = data["commands"].as_array() {
                        self.state.skills = commands
                            .iter()
                            .map(|v| json!({"name":v["name"],"description":v["description"]}))
                            .collect();
                    }
                }
            }
            "system" if value["subtype"] == "session_state_changed" => {
                self.tracks_session_state = true;
                self.session_idle = value["state"] == "idle";
                match value["state"].as_str() {
                    Some("idle") if !self.configuring => {
                        if let Some(status) = self.pending_completion.take() {
                            self.state.finish_work();
                            self.state.status = status;
                            self.state.turn_id = None;
                            self.state.approvals.clear();
                            self.state.questions.clear();
                            self.state.elicitations.clear();
                            self.interrupting = false;
                        } else if self.state.turn_id.is_none() {
                            self.state.status = Status::Idle;
                        }
                        // Current SDK may publish idle before result: retain an
                        // active foreground turn until its result also arrives.
                    }
                    Some("running") if self.initialized && !self.configuring => {
                        self.state.status = if self.state.approvals.is_empty()
                            && self.state.questions.is_empty()
                            && self.state.elicitations.is_empty()
                        {
                            Status::Working
                        } else {
                            Status::Waiting
                        };
                    }
                    Some("requires_action")
                        if !self.state.approvals.is_empty()
                            || !self.state.questions.is_empty()
                            || !self.state.elicitations.is_empty() =>
                    {
                        self.state.status = Status::Waiting;
                    }
                    _ => {}
                }
            }
            "system" if value["subtype"] == "status" => {
                self.state.compacting = value["status"] == "compacting";
                if value["compact_result"]
                    .as_str()
                    .is_some_and(|result| result != "success")
                {
                    self.state.error = Some("Claude Code could not compact the context.".into());
                }
            }
            "system" if value["subtype"] == "compact_boundary" => {
                self.state.compacting = false;
                let metadata = &value["compact_metadata"];
                let id = value["uuid"]
                    .as_str()
                    .map_or_else(|| format!("compact-{}", nonce()), str::to_owned);
                let trigger = match metadata["trigger"].as_str() {
                    Some("manual") => "manual",
                    _ => "auto",
                };
                self.state.upsert_compaction(
                    &json!(id),
                    false,
                    Some(trigger),
                    metadata["pre_tokens"].as_u64(),
                );
                // The boundary's own post figure counts only the summary, not the
                // system prompt and tools, so measure the real post-compaction context.
                if self.process.is_some() {
                    let _ = self.control(json!({"subtype":"get_context_usage"}));
                }
            }
            "system" if value["subtype"] == "session_title_changed" => {
                self.state.set_provider_title(value["title"].as_str());
            }
            "system" if value["subtype"] == "init" => {
                self.state.resolved_model = value["model"].as_str().map(str::to_owned);
                self.state.resolved_access = value["permissionMode"].as_str().map(str::to_owned);
                self.state.resolved_effort = self.state.effort.clone();
                if self.state.skills.is_empty() {
                    if let Some(commands) = value["slash_commands"].as_array() {
                        self.state.skills = commands
                            .iter()
                            .map(|v| json!({"name":v,"description":"Claude Code command"}))
                            .collect();
                    }
                }
            }
            "control_request" => {
                let id = value["request_id"].clone();
                let request = &value["request"];
                if request["subtype"] == "elicitation" {
                    if self.state.elicitations.iter().any(|r| r.id == id) {
                        return Ok(());
                    }
                    match crate::elicitation::ElicitationRequest::parse_claude(id.clone(), request)
                    {
                        Ok(r) if self.state.status.busy() && !self.interrupting => {
                            self.state.elicitations.push(r);
                            self.state.status = Status::Waiting;
                        }
                        _ => {
                            self.respond(id, json!({"action":"cancel"}))?;
                            self.state.error=Some("Claude requested an invalid or unsupported MCP form. Use the terminal for this request.".into());
                        }
                    }
                    return Ok(());
                }
                if request["subtype"] != "can_use_tool" {
                    let subtype = request["subtype"].as_str().unwrap_or("unknown");
                    self.state.error = Some(format!(
                        "Claude requested unsupported host interaction: {subtype}. Use a terminal session for this capability."
                    ));
                    if subtype == "request_user_dialog" {
                        self.state.error = Some(format!(
                            "Claude requested unsupported dialog: {}. Grove does not advertise this dialog kind; use a terminal session.",
                            request["dialog_kind"].as_str().unwrap_or("unknown")
                        ));
                        // Undeclared dialogs can belong to another attached client.
                        // A cancellation would falsely settle that client's question.
                        // Claude ignores error-shaped responses and keeps it pending.
                        return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Grove does not advertise this dialog kind."}}));
                    }
                    return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Grove does not register this host capability. Use the Claude CLI for this interaction."}}));
                }
                if self.state.approvals.iter().any(|a| a["id"] == id) {
                    return Ok(());
                }
                if !self.state.status.busy() || self.interrupting {
                    return self.respond(
                        id,
                        json!({"behavior":"deny","message":"This request has expired."}),
                    );
                }
                if request["tool_name"] == "AskUserQuestion" {
                    if self.state.questions.iter().any(|q| q.id == id) {
                        return Ok(());
                    }
                    match QuestionRequest::parse(id.clone(), &request["input"], true) {
                        Ok(question) if self.state.status.busy() && !self.interrupting => {
                            self.state.questions.push(question);
                            self.state.status = Status::Waiting;
                            return Ok(());
                        }
                        _ => return self.respond(id, json!({"behavior":"deny","message":"Invalid or expired question request."})),
                    }
                }
                self.state.approvals.push(json!({"id":id,"method":"item/commandExecution/requestApproval","claudeInput":request["input"],"claudeRequest":request,"params":{"command":format!("{} {}", request["tool_name"].as_str().unwrap_or("Tool"), request["input"]),"cwd":self.state.saved.cwd,"reason":request["decision_reason"].as_str().unwrap_or("Claude Code requests permission for this action."),"itemId":request["tool_use_id"]}}));
                self.state.status = Status::Waiting;
            }
            "control_cancel_request" => {
                self.state.questions.retain(|q| q.id != value["request_id"]);
                self.state
                    .elicitations
                    .retain(|q| q.id != value["request_id"]);
                self.state
                    .approvals
                    .retain(|a| a["id"] != value["request_id"]);
                if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                    && self.state.status == Status::Waiting
                {
                    self.state.status = Status::Working;
                }
            }
            "stream_event" if value["parent_tool_use_id"].is_null() => self.stream(&value["event"]),
            "assistant" if value["parent_tool_use_id"].is_null() => {
                let message = &value["message"];
                let id = message["id"].as_str().unwrap_or("assistant");
                if let Some(content) = message["content"].as_array() {
                    let mut claimed = Vec::new();
                    for (index, block) in content.iter().enumerate() {
                        // SDK assistant frames can contain only a completed
                        // block, so their array index is not its stream index.
                        let item_id = self.assistant_block_id(id, index, block, &claimed);
                        self.block(&item_id, block);
                        claimed.push(item_id);
                    }
                }
                if let Some(error) = value["error"].as_str() {
                    self.state.error = Some(format!("Claude Code: {error}"));
                }
                if let Some(uuid) = value["uuid"].as_str() {
                    self.last_assistant_uuid = Some(uuid.to_owned());
                    if let Some(item) = self.state.saved.items.last_mut() {
                        item["claudeAssistantUuid"] = json!(uuid);
                    }
                }
                let usage = &message["usage"];
                let used = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                    "output_tokens",
                ]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .sum::<u64>();
                let model = message["model"].as_str().unwrap_or_default();
                if used > 0 && model != "<synthetic>" {
                    if !model.is_empty() {
                        self.context_model = Some(model.to_owned());
                    }
                    self.state.record_context(Some(used), None);
                }
            }
            "user" => {
                self.acknowledge_input(&value["uuid"]);
                if let Some(content) = value["message"]["content"].as_array() {
                    for block in content {
                        if block["type"] == "tool_result" {
                            self.tool_result_with_structured(block, value.get("tool_use_result"));
                        }
                    }
                }
            }
            "tool_progress" => {
                if let Some(item) = self
                    .state
                    .saved
                    .items
                    .iter_mut()
                    .find(|item| item["id"] == value["tool_use_id"])
                {
                    item["progress"] =
                        json!(format!("Running for {}s", value["elapsed_time_seconds"]));
                }
            }
            "result" => {
                if let Some(models) = value["modelUsage"].as_object() {
                    let window = self
                        .context_model
                        .as_deref()
                        .and_then(|model| models.get(model))
                        .and_then(|entry| entry["contextWindow"].as_u64())
                        .or_else(|| {
                            models
                                .values()
                                .filter_map(|entry| entry["contextWindow"].as_u64())
                                .max()
                        });
                    self.state.record_context(None, window);
                }
                let aborted = value["terminal_reason"]
                    .as_str()
                    .is_some_and(|reason| reason.starts_with("aborted"));
                // Claude Code reports queued_turn_count 0 even while a steer
                // waits, so an unechoed steer is the reliable signal that this
                // result closes an intermediate turn and the steer's turn follows.
                let steer_unacknowledged =
                    self.redirect_pending && self.in_flight.is_some() && !self.interrupting;
                let queued = value["queued_turn_count"].as_u64().unwrap_or(0) > 0
                    && (self.redirect_pending || value["terminal_reason"] == "aborted_tools");
                if self.state.turn_id.is_some() && (steer_unacknowledged || queued) {
                    for item in &mut self.state.saved.items {
                        if item["status"] == "inProgress" {
                            item["status"] =
                                json!(if aborted { "interrupted" } else { "completed" });
                        }
                    }
                    self.state.approvals.clear();
                    self.state.questions.clear();
                    self.state.elicitations.clear();
                    self.state.status = Status::Working;
                    return Ok(());
                }
                self.redirect_pending = false;
                if self.state.turn_id.is_none() {
                    return Ok(());
                }
                self.state.compacting = false;
                if self.in_flight.is_some() {
                    if !self.interrupting {
                        return Err("Claude Code finished before acknowledging the submitted message; the draft was preserved.".into());
                    }
                    // An immediate interrupt can finish a turn before the SDK
                    // echoes its input. Settle without acknowledging delivery;
                    // retain queued input and any composer edits for retry.
                    self.state.mark_queue_uncertain();
                    if let Some(prompt) = self.in_flight.take() {
                        self.state.rejected_submission = Some(prompt.id);
                        if !self
                            .state
                            .queued_prompts
                            .iter()
                            .any(|queued| queued.id == prompt.id)
                            && self.state.saved.draft.is_empty()
                            && self.state.saved.draft_images.is_empty()
                        {
                            self.state.saved.draft = prompt.text;
                            self.state.saved.draft_images = prompt.images;
                        }
                    }
                    self.input_uuid = None;
                    self.inline_uuid = None;
                    self.queue_ready = false;
                }
                self.state.turn_id = None;
                self.state.approvals.clear();
                self.state.questions.clear();
                self.state.elicitations.clear();
                self.state.status = if self.interrupting {
                    Status::Interrupted
                } else if value["is_error"] == true {
                    Status::Failed
                } else {
                    Status::Done
                };
                if value["is_error"] == true && !self.interrupting {
                    self.state.error = Some(
                        value["errors"]
                            .as_array()
                            .map(|errors| {
                                errors
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .filter(|s| !s.is_empty())
                            .or_else(|| value["result"].as_str().map(str::to_owned))
                            .unwrap_or_else(|| {
                                format!("Claude Code ended with {}", value["subtype"])
                            }),
                    );
                    self.queue_ready = false;
                }
                for item in &mut self.state.saved.items {
                    if item["status"] == "inProgress" {
                        item["status"] = json!(if self.interrupting {
                            "interrupted"
                        } else if value["is_error"] == true {
                            "failed"
                        } else {
                            "completed"
                        });
                    }
                }
                if value["is_error"] != true && !self.interrupting {
                    crate::rich_plan::import_completed(&mut self.state.saved);
                }
                if self.tracks_session_state && !self.session_idle {
                    self.pending_completion = Some(self.state.status);
                    self.state.status = Status::Working;
                    // result can be flushed before background continuations finish;
                    // session_state_changed idle is the authoritative boundary.
                    self.state.turn_id = Some("awaiting-session-idle".into());
                } else {
                    self.state.finish_work();
                    self.pending_completion = None;
                    self.interrupting = false;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn assistant_block_id(
        &self,
        message: &str,
        index: usize,
        block: &Value,
        claimed: &[String],
    ) -> String {
        let fallback = format!("{message}-{index}");
        let kind = match block["type"].as_str() {
            Some("text") => "agentMessage",
            Some("thinking") => "reasoning",
            _ => return fallback,
        };
        if self.message_id != message {
            return fallback;
        }
        let text = block[if kind == "reasoning" {
            "thinking"
        } else {
            "text"
        }]
        .as_str()
        .unwrap_or_default();
        let mut candidates = self
            .blocks
            .iter()
            .filter_map(|(stream_index, id)| {
                if claimed.contains(id) {
                    return None;
                }
                self.state
                    .saved
                    .items
                    .iter()
                    .find(|item| item["id"] == *id && item["type"] == kind)
                    .map(|item| (*stream_index, id, item))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(stream_index, _, _)| *stream_index);
        let content = |item: &Value| -> String {
            if kind == "reasoning" {
                item["content"][0].as_str().unwrap_or_default().to_owned()
            } else {
                item["text"].as_str().unwrap_or_default().to_owned()
            }
        };
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| content(item) == text) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| {
            let streamed = content(item);
            !streamed.is_empty() && (text.starts_with(&streamed) || streamed.starts_with(text))
        }) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates
            .iter()
            .find(|(stream_index, _, _)| *stream_index == index as u64)
        {
            return (*id).clone();
        }
        if candidates.len() == 1 {
            return candidates[0].1.clone();
        }
        fallback
    }
    fn block(&mut self, id: &str, block: &Value) {
        match block["type"].as_str().unwrap_or_default() {
            "text" => self.upsert(json!({"id":id,"type":"agentMessage","text":block["text"]})),
            "thinking" => self.upsert(
                json!({"id":id,"type":"reasoning","summary":[],"content":[block["thinking"]]}),
            ),
            "tool_use" => {
                self.register_subagent(block);
                let tool_id = block["id"].as_str().unwrap_or(id);
                let old = self
                    .state
                    .saved
                    .items
                    .iter()
                    .find(|i| i["id"] == tool_id)
                    .cloned();
                let mut item = if block["name"] == "Bash" {
                    json!({"id":tool_id,"type":"commandExecution","command":block["input"]["command"],"cwd":self.state.saved.cwd,"status":"inProgress","aggregatedOutput":""})
                } else {
                    json!({"id":tool_id,"type":"mcpToolCall","server":"Claude Code","tool":block["name"],"arguments":block["input"],"status":"inProgress"})
                };
                if let Some(old) = old {
                    for key in [
                        "status",
                        "result",
                        "error",
                        "aggregatedOutput",
                        "progress",
                        "structuredResult",
                        "plan",
                        "text",
                        "detached",
                    ] {
                        if let Some(value) = old.get(key) {
                            item[key] = value.clone();
                        }
                    }
                }
                self.transcript.normalize_tool_item(&mut item);
                self.upsert(item);
            }
            _ => {}
        }
    }
    fn stream(&mut self, event: &Value) {
        let index = event["index"].as_u64().unwrap_or(0);
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                self.message_id = event["message"]["id"].as_str().unwrap_or("stream").into();
                self.blocks.clear();
            }
            "content_block_start" => {
                let block = &event["content_block"];
                let id = format!("{}-{index}", self.message_id);
                let item_id = if block["type"] == "tool_use" {
                    block["id"].as_str().unwrap_or(&id).to_owned()
                } else {
                    id.clone()
                };
                self.block(&id, block);
                self.blocks.insert(index, item_id);
            }
            "content_block_delta" => {
                let Some(id) = self.blocks.get(&index) else {
                    return;
                };
                let Some(item) = self
                    .state
                    .saved
                    .items
                    .iter_mut()
                    .find(|item| item["id"] == *id)
                else {
                    return;
                };
                let delta = &event["delta"];
                if let Some(text) = delta["text"].as_str() {
                    let mut text_all = item["text"].as_str().unwrap_or_default().to_owned();
                    text_all.push_str(text);
                    item["text"] = json!(text_all);
                }
                if let Some(text) = delta["thinking"].as_str() {
                    let mut text_all = item["content"][0].as_str().unwrap_or_default().to_owned();
                    text_all.push_str(text);
                    item["content"] = json!([text_all]);
                }
                // Final assistant messages carry parsed tool input. Do not show
                // partial JSON as a command that could be mistaken for complete.
            }
            _ => {}
        }
    }
    #[cfg(test)]
    fn tool_result(&mut self, block: &Value) {
        self.tool_result_with_structured(block, None);
    }
    fn tool_result_with_structured(&mut self, block: &Value, structured: Option<&Value>) {
        self.subagent_tool_result(block, structured);
        let Some(item) = self
            .state
            .saved
            .items
            .iter_mut()
            .find(|item| item["id"] == block["tool_use_id"])
        else {
            return;
        };
        let content = if let Some(text) = block["content"].as_str() {
            vec![json!({"type":"text","text":text})]
        } else {
            block["content"].as_array().cloned().unwrap_or_default()
        };
        self.transcript.apply_tool_result(
            item,
            &json!(content),
            structured,
            block["is_error"] == true,
        );
        if item["type"] == "commandExecution" {
            item["aggregatedOutput"] = json!(content
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grove_workflow_guidance_preserves_custom_prompts_on_start_and_resume() {
        for resume in [false, true] {
            for custom in [
                vec!["--append-system-prompt".into(), "Custom guidance".into()],
                vec!["--append-system-prompt=Custom guidance".into()],
                Vec::new(),
            ] {
                let mut actor = actor();
                actor.state.saved.thread_id = resume.then(|| "existing-session".into());
                let mut args = custom.clone();
                args.extend(["--system-prompt".into(), "User system prompt".into()]);
                let command = launch_command(&args, &actor.state.saved).unwrap();
                assert!(command.get_envs().any(|(key, value)| {
                    key == "CLAUDE_CODE_ENABLE_TODO_TOOLS" && value == Some("1".as_ref())
                }));
                let argv = command
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let appended = argv
                    .windows(2)
                    .filter(|pair| pair[0] == "--append-system-prompt")
                    .collect::<Vec<_>>();
                assert_eq!(appended.len(), 1);
                assert!(appended[0][1].contains("You are running inside Grove."));
                assert!(appended[0][1]
                    .contains("even when a skill prescribes a different format or handoff"));
                assert!(appended[0][1].contains("Markdown link to the .grove-plan.json artifact"));
                assert!(
                    appended[0][1].contains("Grove's Review response and Send response controls")
                );
                assert!(appended[0][1].contains("does not override explicit user preferences"));
                assert!(appended[0][1].contains("TodoWrite or TaskCreate/TaskUpdate"));
                assert!(appended[0][1].contains("select:TaskCreate,TaskUpdate"));
                assert!(appended[0][1].contains("Keep task statuses current"));
                assert!(appended[0][1].contains("<!-- grove:integrated-report -->"));
                assert!(appended[0][1].contains(crate::reports::REPORT_GUIDANCE));
                assert!(appended[0][1].ends_with(crate::rich_plan::GUIDANCE));
                if !custom.is_empty() {
                    assert!(appended[0][1].starts_with("Custom guidance\n\n"));
                }
                assert!(argv
                    .windows(2)
                    .any(|pair| pair == ["--system-prompt", "User system prompt"]));
                assert_eq!(argv.iter().any(|arg| arg == "--resume"), resume);
            }
        }
    }
    fn actor() -> Actor {
        let saved:SavedSession=serde_json::from_value(json!({"key":"claude-test","provider":"claude","project":"test","cwd":"/tmp","label":"Claude","roots":[],"thread_id":null,"items":[]})).unwrap();
        let (_, input) = mpsc::channel();
        let (updates, _) = crate::latest::channel();
        Actor::new(
            saved,
            Vec::new(),
            input,
            updates,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
        )
    }
    #[test]
    fn subagent_prompt_is_task_fallback_and_activity_does_not_replace_it() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.register_subagent(&json!({"id":"child", "name":"Agent", "input":{"subagent_type":"Researcher", "prompt":"Inspect the parser"}}));
        actor.subagent_block("child", &json!({"type":"text", "text":"Reading source"}));
        let child = &actor.state.saved.work_periods[0].subagents[0];
        assert_eq!(child.role.as_deref(), Some("Researcher"));
        assert_eq!(child.task.as_deref(), Some("Inspect the parser"));
        assert_eq!(child.activity, "Reading source");
    }

    #[test]
    fn subagents_track_live_nested_work_without_primary_state_pollution() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.state.status = Status::Working;
        actor.state.saved.thread_id = Some("primary".into());
        let spawn = json!({"type":"tool_use","id":"child","name":"Agent","input":{"name":"Explorer","subagent_type":"Explore","description":"Find auth paths","prompt":"Search all auth entry points"}});
        actor.block("child", &spawn);
        actor.block("child", &spawn);
        assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
        let item_count = actor.state.saved.items.len();
        actor.message(json!({"type":"assistant","session_id":"other","parent_tool_use_id":"child","message":{"model":"child-model","usage":{"input_tokens":900},"content":[{"type":"tool_use","id":"read","name":"Read","input":{}},{"type":"tool_use","id":"nested","name":"Task","input":{"subagent_type":"Researcher","description":"Inspect tests"}}]}})).unwrap();
        assert_eq!(actor.state.saved.items.len(), item_count);
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("primary"));
        assert!(actor.context_model.is_none());
        assert_eq!(actor.state.status, Status::Working);
        let children = &actor.state.saved.work_periods[0].subagents;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "Explorer");
        assert_eq!(children[0].role.as_deref(), Some("Explore"));
        assert_eq!(children[0].task.as_deref(), Some("Find auth paths"));
        assert_eq!(children[0].activity, "Using Task");
        actor.message(json!({"type":"user","parent_tool_use_id":"child","message":{"content":[{"type":"tool_result","tool_use_id":"nested","is_error":true,"content":"failed"}]}})).unwrap();
        actor
            .message(json!({"type":"result","parent_tool_use_id":"child","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        let children = &actor.state.saved.work_periods[0].subagents;
        assert_eq!(children[0].status, SubagentStatus::Completed);
        assert_eq!(children[1].status, SubagentStatus::Failed);
        actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"content":[{"type":"text","text":"Late activity"}]}})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
    }
    #[test]
    fn claude_task_notifications_deduplicate_tool_lifecycle_and_ignore_shell_tasks() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.block("agent", &json!({"type":"tool_use","id":"agent","name":"Task","input":{"description":"Review changes"}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"sdk-task","tool_use_id":"agent","task_type":"local_agent","subagent_type":"Reviewer","description":"Review changes"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_progress","task_id":"sdk-task","tool_use_id":"agent","summary":"Checking error handling"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"shell","task_type":"local_bash","description":"Run server"})).unwrap();
        assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].activity,
            "Checking error handling"
        );
        actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"agent","content":"Launched agent","is_error":false}]},"tool_use_result":{"isAsync":true,"status":"async_launched"}})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Working
        );
        actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"failed","summary":"Review failed"})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Failed
        );
        actor.block("agent", &json!({"type":"tool_use","id":"agent","name":"Task","input":{"description":"Late replay"}}));
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Failed
        );
        actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"stopped"})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Unknown
        );
    }
    #[test]
    fn deferred_inline_preserves_identical_composer_after_configuration() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.reconfigure = true;
        actor.state.saved.draft = "same text".into();
        actor
            .action(Action::PromptInline {
                text: "same text".into(),
                images: vec![],
            })
            .unwrap();
        assert!(actor.deferred_inline);
        let _ = read(&mut reader);
        actor.state.status = Status::Idle;
        let prompt = actor.deferred.take().unwrap();
        actor.submit(prompt.id, prompt.text, prompt.images).unwrap();
        let sent = read(&mut reader);
        actor.acknowledge_input(&sent["uuid"]);
        assert_eq!(actor.state.saved.draft, "same text");
        actor.deferred_inline = true;
        actor.fail("failure".into());
        assert!(!actor.deferred_inline);
    }

    #[test]
    fn failed_redirect_does_not_disable_redirecting_after_reconnect() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("old".into());
        actor
            .redirect(91, "first correction".into(), vec![])
            .unwrap();
        let _ = read(&mut reader);
        actor.fail("transport disconnected".into());
        assert!(!actor.redirect_pending);
        let mut reconnected = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("new".into());
        actor
            .redirect(92, "second correction".into(), vec![])
            .unwrap();
        assert_eq!(read(&mut reconnected)["priority"], "next");
        // A startup that exits before spawn must also clear transient redirect state.
        actor.stopping.store(true, Ordering::SeqCst);
        assert!(actor.start().is_err());
        assert!(!actor.redirect_pending);
    }

    #[test]
    fn inline_prompt_echo_preserves_identical_independent_composer() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.saved.draft = "same text".into();
        actor
            .action(Action::PromptInline {
                text: "same text".into(),
                images: vec![],
            })
            .unwrap();
        let sent = read(&mut reader);
        actor.acknowledge_input(&sent["uuid"]);
        assert_eq!(actor.state.saved.draft, "same text");
    }

    #[test]
    fn edit_journal_recovers_files_and_only_commits_truncation_after_success() {
        let repo = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(repo.path())
            .status()
            .unwrap()
            .success());
        let root = repo.path().canonicalize().unwrap();
        let file = root.join("work");
        fs_err::write(&file, "current workspace").unwrap();
        let recovery = crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap();
        let mut actor = actor();
        actor.state.saved.key = format!("test-claude-edit-{}", nonce());
        let journal = codex::recovery_dir()
            .unwrap()
            .join(format!("{}.json", actor.state.saved.key));
        actor.state.saved.thread_id = Some("original".into());
        actor.state.saved.items = vec![
            json!({"id":"before","type":"agentMessage","text":"retained","claudeAssistantUuid":"assistant"}),
            json!({"id":"target","type":"userMessage","content":[{"type":"text","text":"original prompt"}],"claudeHumanUuid":"human","claudePreviousAssistantUuid":"assistant"}),
            json!({"id":"later","type":"agentMessage","text":"later"}),
        ];
        actor.state.saved.pending_edit = Some(PendingEdit {
            target: 1,
            turn_id: "human".into(),
            text: "original prompt".into(),
            images: vec![],
            recovery: recovery.clone(),
            composer_draft: Some(("independent draft".into(), vec![])),
            claude_fork_id: Some("fork".into()),
        });
        codex::persist(&actor.state.saved).unwrap();
        fs_err::write(&file, "pre target snapshot").unwrap();
        actor.recover_edit().unwrap();
        assert_eq!(fs_err::read_to_string(&file).unwrap(), "current workspace");
        assert_eq!(actor.state.saved.items.len(), 3);
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("original"));
        assert!(actor.state.saved.pending_edit.is_none());
        actor.state.saved.pending_edit = Some(PendingEdit {
            target: 1,
            turn_id: "human".into(),
            text: "original prompt".into(),
            images: vec![],
            recovery,
            composer_draft: Some(("independent draft".into(), vec![])),
            claude_fork_id: Some("fork".into()),
        });
        actor
            .action(Action::Draft("newer independent draft".into()))
            .unwrap();
        let newer_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .action(Action::DraftImages(newer_images.clone()))
            .unwrap();
        actor.finish_edit().unwrap();
        assert_eq!(actor.state.saved.draft_images, newer_images);
        assert_eq!(actor.state.saved.items.len(), 1);
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("fork"));
        assert_eq!(actor.state.saved.draft, "newer independent draft");
        assert_eq!(
            actor
                .state
                .saved
                .claude_resume
                .as_ref()
                .unwrap()
                .original_session
                .as_deref(),
            Some("original")
        );
        // Editing an earlier retained turn before resending must branch from
        // the persisted original, never from the not-yet-created fork.
        actor.state.saved.items.push(json!({"id":"earlier-human","type":"userMessage","claudePreviousAssistantUuid":"older-assistant"}));
        actor.state.saved.pending_edit = Some(PendingEdit {
            target: 1,
            turn_id: "older-human".into(),
            text: "older".into(),
            images: vec![],
            recovery: crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap(),
            composer_draft: None,
            claude_fork_id: Some("second-fork".into()),
        });
        let pending_command = launch_command(&[], &actor.state.saved).unwrap();
        let pending_args = pending_command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(pending_args
            .windows(2)
            .any(|pair| pair == ["--resume", "original"]));
        actor.finish_edit().unwrap();
        assert_eq!(
            actor
                .state
                .saved
                .claude_resume
                .as_ref()
                .unwrap()
                .original_session
                .as_deref(),
            Some("original")
        );
        let durable: SavedSession =
            serde_json::from_slice(&fs_err::read(&journal).unwrap()).unwrap();
        assert!(durable.pending_edit.is_none());
        assert!(durable.claude_resume.is_some());
        fs_err::remove_file(journal).unwrap();
    }

    #[test]
    fn naturally_completed_redirect_keeps_the_correction_until_its_echo() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("old".into());
        actor.tracks_session_state = true;
        actor.session_idle = false;
        actor.redirect(81, "correction".into(), vec![]).unwrap();
        let sent = read(&mut reader);
        actor
            .message(json!({"type":"result","terminal_reason":"completed","queued_turn_count":0}))
            .unwrap();
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert!(actor.state.status.busy());
        assert_eq!(actor.in_flight.as_ref().unwrap().id, 81);
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
            .unwrap();
        actor.acknowledge_input(&sent["uuid"]);
        actor
            .message(json!({"type":"result","terminal_reason":"completed","queued_turn_count":0}))
            .unwrap();
        assert!(actor.state.status.busy());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Done);
    }

    #[test]
    fn claude_fork_recipe_survives_save_and_reconnect_before_first_prompt() {
        let mut actor = actor();
        actor.state.saved.thread_id = Some("fork-id".into());
        actor.state.saved.claude_resume = Some(codex::ClaudeResume {
            original_session: Some("original-id".into()),
            assistant_uuid: Some("assistant-id".into()),
            human_uuid: "human-id".into(),
            fork_id: "fork-id".into(),
        });
        let saved: SavedSession =
            serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
        let command = launch_command(&[], &saved).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for pair in [
            ["--resume", "original-id"],
            ["--resume-session-at", "assistant-id"],
            ["--resume-drops-turn", "human-id"],
            ["--session-id", "fork-id"],
        ] {
            assert!(args.windows(2).any(|actual| actual == pair));
        }
        assert!(args.iter().any(|arg| arg == "--fork-session"));
        actor.state.saved.claude_resume = Some(saved.claude_resume.unwrap());
        actor
            .message(json!({"type":"system","subtype":"init","session_id":"fork-id"}))
            .unwrap();
        assert!(actor.state.saved.claude_resume.is_some());
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert!(actor.state.saved.claude_resume.is_some());
        actor.input_uuid = Some("accepted".into());
        actor.in_flight = Some(QueuedPrompt {
            id: 1,
            text: "revised".into(),
            images: vec![],
        });
        actor.acknowledge_input(&json!("unrelated"));
        assert!(actor.state.saved.claude_resume.is_some());
        actor.acknowledge_input(&json!("accepted"));
        assert!(actor.state.saved.claude_resume.is_none());
    }

    #[test]
    fn redirect_intermediate_result_retains_input_and_waits_for_authoritative_idle() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("old".into());
        actor.tracks_session_state = true;
        actor.session_idle = false;
        actor.state.saved.draft = "correction".into();
        actor
            .action(Action::SubmitWithMode {
                id: 91,
                text: "correction".into(),
                images: vec![],
                mode: SubmissionMode::Steer,
            })
            .unwrap();
        let sent = read(&mut reader);
        assert_eq!(sent["priority"], "next");
        assert_eq!(sent["origin"]["kind"], "human");
        actor.message(json!({"type":"result","terminal_reason":"aborted_tools","queued_turn_count":0,"is_error":false})).unwrap();
        assert_eq!(actor.in_flight.as_ref().unwrap().id, 91);
        assert!(actor.state.status.busy());
        actor.acknowledge_input(&sent["uuid"]);
        actor.acknowledge_input(&sent["uuid"]);
        assert_eq!(actor.state.saved.items.len(), 1);
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert!(actor.state.status.busy());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Done);
    }

    /// Starts a Claude turn that has a running Bash tool, then steers it.
    fn steered_turn(reader: &mut BufReader<std::process::ChildStdout>, actor: &mut Actor) -> Value {
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.submit(1, "build it".into(), vec![]).unwrap();
        let prompt = read(reader);
        actor
            .message(json!({"type":"user","uuid":prompt["uuid"],"message":{"role":"user","content":"build it"}}))
            .unwrap();
        actor.message(json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"sleep 5"}}]}})).unwrap();
        actor
            .action(Action::SubmitWithMode {
                id: 2,
                text: "use the other branch".into(),
                images: vec![],
                mode: SubmissionMode::Steer,
            })
            .unwrap();
        let steer = read(reader);
        assert_eq!(steer["priority"], "next");
        assert!(actor.state.status.busy());
        steer
    }

    fn steer_acknowledged(actor: &Actor) -> bool {
        actor.in_flight.is_none()
            && actor.state.accepted_submission == Some(2)
            && actor.state.saved.items.iter().any(|item| {
                item["type"] == "userMessage" && codex::item_text(item) == "use the other branch"
            })
    }

    fn assert_steer_turn_finishes(actor: &mut Actor) {
        actor.message(json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Switched."}]}})).unwrap();
        assert!(actor.state.status.busy());
        actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0})).unwrap();
        assert_eq!(actor.state.status, Status::Done);
        assert!(actor.state.turn_id.is_none());
        assert!(actor.state.error.is_none());
        assert!(actor.state.rejected_submission.is_none());
        assert!(!actor.redirect_pending);
    }

    #[test]
    fn claude_steer_joins_the_running_turn_at_a_tool_boundary() {
        // Sequence A: the echo arrives inside the same turn; one result ends it.
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        let steer = steered_turn(&mut reader, &mut actor);
        actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"done"}]}})).unwrap();
        actor
            .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
            .unwrap();
        assert!(steer_acknowledged(&actor));
        assert!(actor.state.status.busy());
        assert_steer_turn_finishes(&mut actor);
    }

    #[test]
    fn claude_steer_after_the_last_tool_runs_as_a_follow_up_turn() {
        // Sequence B: the turn completes before the echo, then the steer runs.
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.tracks_session_state = true;
        actor.session_idle = false;
        let steer = steered_turn(&mut reader, &mut actor);
        actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"done"}]}})).unwrap();
        actor.message(json!({"type":"assistant","message":{"id":"m1b","content":[{"type":"text","text":"Built."}]}})).unwrap();
        actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0,"result_index":0})).unwrap();
        assert!(actor.state.status.busy());
        assert!(actor.state.turn_id.is_some());
        assert!(actor.pending_completion.is_none());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert!(actor.state.status.busy());
        actor
            .message(json!({"type":"system","subtype":"init","model":"claude"}))
            .unwrap();
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
            .unwrap();
        actor
            .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
            .unwrap();
        assert!(steer_acknowledged(&actor));
        actor.message(json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Switched."}]}})).unwrap();
        actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0,"result_index":1,"user_message_uuids":[steer["uuid"]]})).unwrap();
        assert!(actor.state.status.busy());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Done);
        assert!(actor.state.error.is_none());
        assert!(actor.state.rejected_submission.is_none());
    }

    #[test]
    fn claude_aborted_results_before_a_steer_echo_keep_the_turn_alive() {
        // Sequences C and D: an aborted result precedes the steer's echo.
        for (reason, subtype, is_error) in [
            ("aborted_tools", "success", false),
            ("aborted_streaming", "error_during_execution", true),
        ] {
            let mut actor = actor();
            let mut reader = transport(&mut actor);
            let steer = steered_turn(&mut reader, &mut actor);
            actor.message(json!({"type":"result","subtype":subtype,"terminal_reason":reason,"is_error":is_error,"queued_turn_count":0})).unwrap();
            assert!(actor.state.status.busy(), "{reason}");
            assert!(actor.state.turn_id.is_some());
            assert!(actor.state.error.is_none());
            assert!(actor.state.rejected_submission.is_none());
            let tool = actor
                .state
                .saved
                .items
                .iter()
                .find(|item| item["id"] == "bash")
                .unwrap();
            assert_eq!(tool["status"], "interrupted");
            actor
                .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
                .unwrap();
            assert!(steer_acknowledged(&actor));
            assert_steer_turn_finishes(&mut actor);
        }
    }

    #[test]
    fn redirected_queue_locks_until_echo_and_preserves_independent_draft() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("old".into());
        actor.state.saved.draft = "independent".into();
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 12,
            text: "redirect".into(),
            images: vec![],
        });
        actor.action(Action::SteerQueued(12)).unwrap();
        let sent = read(&mut reader);
        assert!(actor.action(Action::RemoveQueued(12)).is_err());
        assert_eq!(actor.state.queue_steering, vec![12]);
        actor.acknowledge_input(&sent["uuid"]);
        assert!(actor.state.queue_steering.is_empty());
        assert!(actor.state.queued_prompts.is_empty());
        assert_eq!(actor.state.saved.draft, "independent");
    }

    #[test]
    fn historical_claude_messages_require_new_anchors_before_editing() {
        let mut actor = actor();
        actor.state.saved.provider = codex::Provider::Claude;
        actor.state.status = Status::Idle;
        actor
            .state
            .saved
            .items
            .push(json!({"id":"old","type":"userMessage","content":[]}));
        assert!(actor
            .state
            .editable_message(0)
            .unwrap_err()
            .contains("anchor"));
        actor.state.saved.items[0]["claudeHumanUuid"] = json!("old");
        assert!(actor
            .state
            .editable_message(0)
            .unwrap_err()
            .contains("checkpoint"));
    }

    #[test]
    fn work_period_waits_for_claude_idle_and_closes_on_disconnect() {
        let mut actor = actor();
        actor.tracks_session_state = true;
        actor.session_idle = false;
        actor.state.turn_id = Some("turn".into());
        actor.state.begin_work();
        actor
            .message(json!({"type":"result","subtype":"success","is_error":false}))
            .unwrap();
        assert!(actor.state.work_started.is_some());
        assert!(actor.state.saved.work_periods[0].end_item.is_none());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert!(actor.state.work_started.is_none());
        assert!(actor.state.saved.work_periods[0].duration_ms.is_some());
        actor.state.begin_work();
        actor.fail("disconnected".into());
        assert!(actor.state.work_started.is_none());
        assert!(actor.state.saved.work_periods[1].end_item.is_some());
    }

    #[test]
    fn deferred_queue_copy_is_locked_before_reconfiguration() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.reconfigure = true;
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 41,
            text: "original".into(),
            images: Vec::new(),
        });
        actor.submit(41, "original".into(), Vec::new()).unwrap();
        let _ = read(&mut reader);
        assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
        assert_eq!(actor.state.queue_sending, vec![41]);
        assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
        assert!(actor.action(Action::RemoveQueued(41)).is_err());
        assert_eq!(actor.state.queued_prompts[0].text, "original");
    }

    #[test]
    fn dismissing_uncertain_deferred_input_prevents_replay_after_configuration_failure() {
        let mut actor = actor();
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.reconfigure = true;
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 41,
            text: "deferred".into(),
            images: Vec::new(),
        });
        // No transport: the configuration control cannot be delivered, leaving a deferred copy.
        assert!(actor.submit(41, "deferred".into(), Vec::new()).is_err());
        assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
        assert_eq!(actor.state.queue_uncertain, vec![41]);
        actor.action(Action::DismissUncertainQueued(41)).unwrap();
        assert!(actor.deferred.is_none());
        assert!(actor.state.queued_prompts.is_empty());
        assert!(!actor.queue_ready);
    }

    #[test]
    fn queued_ack_preserves_independent_same_text_composer() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 41,
            text: "repeat".into(),
            images: Vec::new(),
        });
        actor.submit(41, "repeat".into(), Vec::new()).unwrap();
        let sent = read(&mut reader);
        actor.state.saved.draft = "repeat".into();
        actor.acknowledge_input(&sent["uuid"]);
        assert_eq!(actor.state.saved.draft, "repeat");
        assert!(actor.state.queued_prompts.is_empty());
    }

    #[test]
    fn uncertain_queue_dismissal_keeps_next_paused() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.queued_prompts = vec![
            QueuedPrompt {
                id: 41,
                text: "unknown".into(),
                images: Vec::new(),
            },
            QueuedPrompt {
                id: 42,
                text: "next".into(),
                images: Vec::new(),
            },
        ];
        actor.submit(41, "unknown".into(), Vec::new()).unwrap();
        let _ = read(&mut reader);
        actor.fail("disconnected before echo".into());
        assert_eq!(actor.state.queue_uncertain, vec![41]);
        assert!(actor.action(Action::ResumeQueue).is_err());
        actor.action(Action::DismissUncertainQueued(41)).unwrap();
        assert!(!actor.queue_ready);
        assert_eq!(actor.state.queued_prompts[0].id, 42);
    }

    #[test]
    fn queued_edit_and_remove_respect_provider_sending_lock() {
        let mut actor = actor();
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 41,
            text: "original".into(),
            images: Vec::new(),
        });
        actor.action(Action::BeginQueuedEdit(41)).unwrap();
        actor
            .action(Action::SaveQueuedEdit {
                id: 41,
                text: "revised".into(),
                images: Vec::new(),
            })
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].text, "revised");
        assert_eq!(actor.state.queue_edit, None);
        assert!(actor.action(Action::SteerQueued(41)).is_err());
        actor.state.queue_sending.push(41);
        assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
        assert!(actor.action(Action::RemoveQueued(41)).is_err());
        assert_eq!(actor.state.queued_prompts.len(), 1);
        actor.state.queue_sending.clear();
        actor.action(Action::RemoveQueued(41)).unwrap();
        assert!(actor.state.queued_prompts.is_empty());
    }

    #[test]
    fn clear_conversation_drops_resume_identity_and_old_event_receiver() {
        let mut actor = actor();
        let _reader = transport(&mut actor);
        actor.state.status = Status::Done;
        actor.state.saved.thread_id = Some("old-session".into());
        actor
            .state
            .saved
            .items
            .push(json!({"type":"agentMessage","text":"old"}));
        actor.state.saved.title = Some("Old title".into());
        actor.state.saved.draft = "draft".into();
        actor.args = vec![
            "--model".into(),
            "sonnet".into(),
            "old launch prompt".into(),
        ];
        let before = actor.state.saved.clone();
        let mut restored = None;
        actor
            .clear_conversation(|saved| {
                restored = Some(codex::recover_saved_for_test(saved));
                Ok(())
            })
            .unwrap();
        let restored = restored.unwrap();
        assert_eq!(restored.key, before.key);
        assert_eq!(restored.cwd, before.cwd);
        assert!(restored.thread_id.is_none());
        assert!(restored.items.is_empty());
        assert!(restored.title.is_none());
        assert!(restored.draft.is_empty());
        assert!(actor.process.is_none());
        assert_eq!(actor.state.cleared_conversations, 1);
        assert_eq!(launch_options(&actor.args).unwrap().1, None);
        let command = launch_command(&actor.args, &restored).unwrap();
        let args = command
            .get_args()
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(!args
            .iter()
            .any(|arg| arg == "--resume" || arg == "old-session" || arg == "old launch prompt"));
        // A new CLI's identity is accepted rather than filtered by the old one.
        actor
            .message(json!({"type":"system","subtype":"init","session_id":"fresh-session"}))
            .unwrap();
        assert_eq!(
            actor.state.saved.thread_id.as_deref(),
            Some("fresh-session")
        );
        actor.message(json!({"type":"assistant","session_id":"old-session","message":{"content":[{"type":"text","text":"late old text"}]}})).unwrap();
        assert!(actor.state.saved.items.is_empty());
    }
    #[test]
    fn clear_conversation_rejections_preserve_existing_claude_context() {
        let mut actor = actor();
        actor.state.saved.thread_id = Some("old-session".into());
        actor.state.status = Status::Working;
        assert_eq!(
            actor.action(Action::ClearConversation).unwrap_err(),
            codex::CLEAR_BUSY
        );
        actor.state.status = Status::Done;
        assert_eq!(
            actor.action(Action::ClearConversation).unwrap_err(),
            codex::CLEAR_DISCONNECTED
        );
        let _reader = transport(&mut actor);
        assert!(actor
            .clear_conversation(|_| Err("disk full".into()))
            .is_err());
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("old-session"));
        assert!(actor.process.is_some());
        assert_eq!(actor.state.cleared_conversations, 0);
    }
    #[test]
    fn launch_preserves_config_and_rejects_transport_takeover() {
        assert_eq!(
            launch_options(&["--model=sonnet".into(), "hello".into()]).unwrap(),
            (
                vec!["--model".into(), "sonnet".into()],
                Some("hello".into())
            )
        );
        for flag in [
            "--output-format",
            "--resume",
            "--no-session-persistence",
            "--worktree",
            "--print",
        ] {
            assert!(launch_options(&[flag.into()]).is_err());
        }
        assert!(launch_options(&["--effort".into()]).is_err());
    }
    #[test]
    fn images_use_anthropic_content_blocks() {
        assert_eq!(
            user_content("look", &["data:image/png;base64,aGVsbG8=".into()])[1],
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}})
        );
    }
    #[test]
    fn streams_replace_final_text_without_duplication_and_preserve_tool_results() {
        let mut actor = actor();
        for event in [
            json!({"type":"message_start","message":{"id":"m"}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}),
        ] {
            actor.stream(&event);
        }
        actor.message(json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"Hello!"}]}})).unwrap();
        assert_eq!(actor.state.saved.items.len(), 1);
        assert_eq!(actor.state.saved.items[0]["text"], "Hello!");
        let tool = json!({"type":"tool_use","id":"tool1","name":"Bash","input":{"command":"pwd"}});
        actor.block("m-1", &tool);
        actor.tool_result(&json!({"tool_use_id":"tool1","content":"/tmp","is_error":false}));
        actor.block("m-1", &tool);
        assert_eq!(actor.state.saved.items[1]["aggregatedOutput"], "/tmp");
        assert_eq!(actor.state.saved.items[1]["status"], "completed");
    }
    #[test]
    fn failed_or_interrupted_turn_pauses_queue_and_clears_approvals() {
        let mut actor = actor();
        actor.state.approvals.push(json!({"id":"approval"}));
        actor.state.turn_id = Some("turn".into());
        actor.message(json!({"type":"result","is_error":true,"errors":["auth failed"],"session_id":"persisted-id"})).unwrap();
        assert_eq!(actor.state.status, Status::Failed);
        assert!(!actor.queue_ready);
        assert!(actor.state.approvals.is_empty());
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("persisted-id"));
        assert_eq!(actor.state.error.as_deref(), Some("auth failed"));
    }
    #[test]
    fn steer_rejection_preserves_draft() {
        let mut actor = actor();
        actor.state.saved.draft = "keep".into();
        assert!(actor
            .action(Action::SubmitWithMode {
                id: 7,
                text: "steer".into(),
                images: Vec::new(),
                mode: SubmissionMode::Steer
            })
            .is_err());
        assert_eq!(actor.state.rejected_submission, Some(7));
        assert_eq!(actor.state.saved.draft, "keep");
    }
    #[test]
    fn native_permission_mappings_are_explicit() {
        assert_eq!(permission_mode("read-only").unwrap(), "plan");
        assert_eq!(permission_mode("workspace-write").unwrap(), "acceptEdits");
        assert!(permission_mode("unknown").is_err());
    }
    fn transport(actor: &mut Actor) -> BufReader<std::process::ChildStdout> {
        let mut command = Command::new("cat");
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        output
    }
    #[test]
    fn startup_queue_waits_for_handshake_and_sends_images_once_in_order() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.status = Status::Starting;
        let image = "data:image/png;base64,aGVsbG8=".to_string();
        for (id, text, images) in [(41, "first", vec![image.clone()]), (42, "second", vec![])] {
            actor
                .action(Action::SubmitWithMode {
                    id,
                    text: text.into(),
                    images,
                    mode: SubmissionMode::Queue,
                })
                .unwrap();
        }
        // Multiple actor ticks during a delayed handshake cannot write a prompt.
        for _ in 0..3 {
            assert!(!actor.drain_queue());
        }
        assert!(actor.in_flight.is_none());
        assert_eq!(actor.state.queued_prompts.len(), 2);
        actor.pending.insert("init".into(), "initialize".into());
        actor.message(json!({"type":"control_response","response":{"request_id":"init","subtype":"success","response":{}}})).unwrap();
        assert!(actor.drain_queue());
        let first = read(&mut reader);
        assert_eq!(
            first["message"]["content"],
            json!(user_content("first", &[image]))
        );
        assert!(!actor.drain_queue());
        actor.acknowledge_input(&first["uuid"]);
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert!(actor.drain_queue());
        let second = read(&mut reader);
        assert_eq!(
            second["message"]["content"],
            json!(user_content("second", &[]))
        );
        assert!(!actor.drain_queue());
        actor.acknowledge_input(&second["uuid"]);
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert!(actor.state.queued_prompts.is_empty());
        assert!(!actor.drain_queue());
    }

    #[test]
    fn startup_failure_retains_unsent_queue_for_explicit_resume_after_reconnect() {
        let mut actor = actor();
        let _reader = transport(&mut actor);
        actor.state.status = Status::Starting;
        let images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .action(Action::SubmitWithMode {
                id: 51,
                text: "retain".into(),
                images: images.clone(),
                mode: SubmissionMode::Queue,
            })
            .unwrap();
        actor.fail("Claude Code initialization timed out".into());
        assert_eq!(actor.state.queued_prompts[0].text, "retain");
        assert_eq!(actor.state.queued_prompts[0].images, images);
        assert!(actor.state.queue_uncertain.is_empty());
        assert!(!actor.drain_queue());
        let mut reader = transport(&mut actor);
        actor.pending.insert("retry".into(), "initialize".into());
        actor.message(json!({"type":"control_response","response":{"request_id":"retry","subtype":"success","response":{}}})).unwrap();
        assert!(!actor.drain_queue());
        actor.action(Action::ResumeQueue).unwrap();
        assert!(actor.drain_queue());
        let sent = read(&mut reader);
        assert_eq!(
            sent["message"]["content"],
            json!(user_content("retain", &images))
        );
        assert!(!actor.drain_queue());
        actor.acknowledge_input(&sent["uuid"]);
        assert!(actor.state.queued_prompts.is_empty());
    }

    #[test]
    fn protocol_batches_yield_with_backlog_and_preserve_pending_order() {
        let mut actor = actor();
        let _reader = transport(&mut actor);
        let (send, lines) = mpsc::channel();
        actor.process.as_mut().unwrap().lines = lines;
        // Unknown future protocol events are deliberately ignored by message(),
        // but must still be bounded and consumed in transport order.
        for sequence in 0..600 {
            send.send(Ok(json!({"type":"future_event", "sequence":sequence})))
                .unwrap();
        }
        assert!(actor.receive_batch());
        assert!(actor.dirty);
        let next = actor
            .process
            .as_ref()
            .unwrap()
            .lines
            .try_recv()
            .unwrap()
            .unwrap();
        assert_eq!(next["sequence"], 256);
        assert!(actor.receive_batch());
        let next = actor
            .process
            .as_ref()
            .unwrap()
            .lines
            .try_recv()
            .unwrap()
            .unwrap();
        assert_eq!(next["sequence"], 513);
        assert!(actor.receive_batch());
        assert!(!actor.receive_batch());
    }
    fn read(reader: &mut impl BufRead) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    #[test]
    fn approvals_roundtrip_exact_input_and_reject_expired_ids() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        let input = json!({"command":"printf '%s' '$HOME'","timeout":123});
        actor.message(json!({"type":"control_request","request_id":"approval","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool","input":input}})).unwrap();
        assert_eq!(actor.state.status, Status::Waiting);
        actor
            .action(Action::Approval(json!("approval"), true))
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"approval","response":{"behavior":"allow","updatedInput":input}}})
        );
        assert!(actor
            .action(Action::Approval(json!("approval"), true))
            .is_err());
        assert!(actor.state.approvals.is_empty());
    }
    #[test]
    fn questions_roundtrip_multi_custom_and_preserve_original_input() {
        let mut actor = actor();
        actor.state.status = Status::Working;
        let mut reader = transport(&mut actor);
        let input = json!({"questions":[
            {"header":"Choice","question":"Which?","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}],"multiSelect":true},
            {"header":"Name","question":"Name?","options":[{"label":"Default","description":"Suggested"}],"multiSelect":false}
        ],"metadata":{"source":"fixture"}});
        let frame = json!({"type":"control_request","request_id":"questions","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}});
        actor.message(frame.clone()).unwrap();
        actor.message(frame.clone()).unwrap();
        assert_eq!(actor.state.questions.len(), 1);
        assert_eq!(actor.state.status, Status::Waiting);
        let answers = HashMap::from([
            ("Which?".into(), vec!["A".into(), "B".into()]),
            ("Name?".into(), vec!["Custom name".into()]),
        ]);
        actor
            .action(Action::AnswerQuestions(json!("questions"), Some(answers)))
            .unwrap();
        let mut expected = input;
        expected["answers"] = json!({"Which?":"A, B","Name?":"Custom name"});
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"questions","response":{"behavior":"allow","updatedInput":expected}}})
        );
        assert!(actor.state.questions.is_empty());
        assert_eq!(actor.state.question_answer_acks.len(), 1);
        assert_eq!(actor.state.question_answer_acks[0].id, json!("questions"));
        assert!(actor
            .action(Action::AnswerQuestions(json!("questions"), None))
            .is_err());
        actor.message(frame.clone()).unwrap();
        actor
            .action(Action::AnswerQuestions(json!("questions"), None))
            .unwrap();
        assert_eq!(
            read(&mut reader)["response"]["response"]["behavior"],
            "deny"
        );
        assert_eq!(
            actor.state.question_answer_acks.len(),
            1,
            "denial is not an answer"
        );
        actor.message(frame).unwrap();
        actor
            .message(json!({"type":"control_cancel_request","request_id":"questions"}))
            .unwrap();
        assert!(actor.state.questions.is_empty());
        assert!(actor
            .action(Action::AnswerQuestions(json!("questions"), None))
            .is_err());
    }
    #[test]
    fn extended_questions_roundtrip_text_number_and_reject_invalid_answers() {
        let mut actor = actor();
        actor.state.status = Status::Working;
        let mut reader = transport(&mut actor);
        let input = json!({"title":"Before building", "questions":[
            {"header":"Title","question":"What title?","kind":"text","placeholder":"e.g. Quarterly briefing","description":"Shown on the cover."},
            {"header":"Size","question":"How many slides?","kind":"number","min":1,"max":20,"step":1,"defaultValue":8,"unit":"slides"}
        ]});
        let frame = json!({"type":"control_request","request_id":"extended","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}});
        actor.message(frame.clone()).unwrap();
        assert_eq!(actor.state.questions.len(), 1);
        assert_eq!(actor.state.status, Status::Waiting);
        let answers = |number: &str| {
            HashMap::from([
                ("What title?".into(), vec!["Quarterly briefing".into()]),
                ("How many slides?".into(), vec![number.into()]),
            ])
        };
        for invalid in ["21", "NaN", "eight"] {
            assert!(actor
                .action(Action::AnswerQuestions(
                    json!("extended"),
                    Some(answers(invalid))
                ))
                .is_err());
            assert_eq!(actor.state.questions.len(), 1);
        }
        actor
            .action(Action::AnswerQuestions(
                json!("extended"),
                Some(answers("8")),
            ))
            .unwrap();
        let mut expected = input;
        expected["answers"] = json!({"What title?":"Quarterly briefing","How many slides?":"8"});
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"extended","response":{"behavior":"allow","updatedInput":expected}}})
        );
        actor.message(frame.clone()).unwrap();
        actor
            .action(Action::AnswerQuestions(json!("extended"), None))
            .unwrap();
        assert_eq!(
            read(&mut reader)["response"]["response"]["behavior"],
            "deny"
        );
        actor.message(frame).unwrap();
        actor
            .message(json!({"type":"control_cancel_request","request_id":"extended"}))
            .unwrap();
        assert!(actor.state.questions.is_empty());
        assert!(actor
            .action(Action::AnswerQuestions(
                json!("extended"),
                Some(answers("8"))
            ))
            .is_err());
    }
    #[test]
    fn mcp_questions_reject_invalid_form_answers_and_roundtrip_decline_cancel() {
        let mut actor = actor();
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("turn".into());
        let mut reader = transport(&mut actor);
        let form = json!({"type":"control_request","request_id":"form","request":{
            "subtype":"elicitation","mcp_server_name":"server","message":"Choose settings",
            "requested_schema":{"type":"object","properties":{
                "name":{"type":"string","minLength":2},
                "count":{"type":"integer","minimum":1,"maximum":10},
                "ready":{"type":"boolean"},
                "color":{"type":"string","enum":["blue","green"]},
                "features":{"type":"array","items":{"type":"string","enum":["a","b"]},"minItems":1}
            },"required":["name","count","ready","color","features"]}
        }});
        actor.message(form.clone()).unwrap();
        assert_eq!(actor.state.elicitations.len(), 1);
        let valid =
            json!({"name":"Deck","count":8,"ready":false,"color":"blue","features":["a","b"]});
        for (field, invalid) in [
            ("name", json!("x")),
            ("count", json!(11)),
            ("ready", json!("yes")),
            ("color", json!("red")),
            ("features", json!([])),
        ] {
            let mut content = valid.clone();
            content[field] = invalid;
            assert!(actor
                .action(Action::RespondElicitation {
                    id: json!("form"),
                    action: "accept".into(),
                    content: Some(content)
                })
                .is_err());
            assert_eq!(actor.state.elicitations.len(), 1);
        }
        actor
            .action(Action::RespondElicitation {
                id: json!("form"),
                action: "accept".into(),
                content: Some(valid.clone()),
            })
            .unwrap();
        assert_eq!(
            read(&mut reader)["response"]["response"],
            json!({"action":"accept","content":valid})
        );
        for action in ["decline", "cancel"] {
            actor.message(form.clone()).unwrap();
            actor
                .action(Action::RespondElicitation {
                    id: json!("form"),
                    action: action.into(),
                    content: None,
                })
                .unwrap();
            assert_eq!(
                read(&mut reader)["response"]["response"],
                json!({"action":action})
            );
        }
    }
    #[test]
    fn malformed_extended_question_constraints_are_denied_without_waiting() {
        for row in [
            json!({"kind":"number","min":10,"max":1}),
            json!({"kind":"number","min":1}),
            json!({"kind":"number","min":1,"max":10,"step":0}),
            json!({"kind":"number","min":1,"max":10,"defaultValue":11}),
            json!({"kind":"number","min":1,"max":10,"multiSelect":true}),
            json!({"kind":"text","options":[{"label":"A"}]}),
        ] {
            let mut actor = actor();
            actor.state.status = Status::Working;
            let mut reader = transport(&mut actor);
            let mut row = row;
            row["question"] = json!("How many?");
            row["header"] = json!("Size");
            actor.message(json!({"type":"control_request","request_id":"invalid","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[row]}}})).unwrap();
            assert!(actor.state.questions.is_empty());
            assert_eq!(actor.state.status, Status::Working);
            assert_eq!(
                read(&mut reader)["response"]["response"]["behavior"],
                "deny"
            );
        }
    }
    #[test]
    fn launch_advertises_only_rendered_extended_question_schema() {
        let actor = actor();
        let command = launch_command(&[], &actor.state.saved).unwrap();
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "CLAUDE_CODE_QUESTION_EXTENDED")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("1"))
        );
    }
    #[test]
    fn accepted_prompt_recovery_preserves_newer_text_and_attachments() {
        let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
        for newer in [false, true] {
            let mut actor = actor();
            let mut reader = transport(&mut actor);
            actor.initialized = true;
            actor.state.status = Status::Idle;
            actor.state.saved.draft = "sent".into();
            actor.state.saved.draft_images = images.clone();
            actor.submit(42, "sent".into(), images.clone()).unwrap();
            let sent = read(&mut reader);
            if newer {
                actor
                    .action(Action::Draft("new unsent draft".into()))
                    .unwrap();
            }
            actor
                .message(json!({"type":"user","uuid":sent["uuid"]}))
                .unwrap();
            assert_eq!(actor.state.accepted_submission, Some(42));
            let recovered = codex::recover_saved_for_test(&actor.state.saved);
            assert_eq!(recovered.draft, if newer { "new unsent draft" } else { "" });
            assert_eq!(
                recovered.draft_images,
                if newer { images.clone() } else { Vec::new() }
            );
        }
    }

    #[test]
    fn queued_prompt_recovery_preserves_independent_draft() {
        let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
        for newer in [false, true] {
            let mut actor = actor();
            actor.state.saved.draft = if newer { "new unsent draft" } else { "sent" }.into();
            actor.state.saved.draft_images = images.clone();
            actor
                .action(Action::SubmitWithMode {
                    id: 42,
                    text: "sent".into(),
                    images: images.clone(),
                    mode: SubmissionMode::Queue,
                })
                .unwrap();
            assert_eq!(actor.state.accepted_submission, Some(42));
            assert_eq!(actor.state.queued_prompts.len(), 1);
            assert_eq!(actor.state.queued_prompts[0].images, images);
            let recovered = codex::recover_saved_for_test(&actor.state.saved);
            assert_eq!(recovered.draft, if newer { "new unsent draft" } else { "" });
            assert_eq!(
                recovered.draft_images,
                if newer { images.clone() } else { Vec::new() }
            );
        }
    }

    #[test]
    fn submission_ack_requires_matching_uuid_and_preserves_newer_draft() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.saved.draft = "sent".into();
        actor.submit(4, "sent".into(), Vec::new()).unwrap();
        let sent = read(&mut reader);
        assert!(sent["uuid"].as_str().is_some());
        assert_eq!(actor.state.accepted_submission, None);
        assert_eq!(actor.state.saved.draft, "sent");
        actor
            .message(json!({"type":"user","uuid":"stale","message":{"content":"sent"}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, None);
        assert!(actor
            .action(Action::Approval(json!("stale"), true))
            .is_err());
        assert!(actor.in_flight.is_some());
        actor.state.saved.draft = "new draft".into();
        actor
            .message(json!({"type":"user","uuid":sent["uuid"],"message":{"content":"sent"}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(4));
        assert_eq!(actor.state.saved.items.len(), 1);
        assert_eq!(actor.state.saved.draft, "new draft");
        actor
            .message(json!({"type":"user","uuid":sent["uuid"]}))
            .unwrap();
        assert_eq!(actor.state.saved.items.len(), 1);
    }
    #[test]
    fn configure_merges_fields_and_applies_native_control_requests() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor
            .action(Action::Configure {
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                access: Some("plan".into()),
            })
            .unwrap();
        actor
            .action(Action::Configure {
                model: None,
                effort: Some("low".into()),
                access: None,
            })
            .unwrap();
        assert_eq!(actor.state.model.as_deref(), Some("sonnet"));
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        actor.configure_live().unwrap();
        let first = read(&mut reader);
        let second = read(&mut reader);
        let third = read(&mut reader);
        assert_eq!(
            first["request"],
            json!({"subtype":"set_model","model":"sonnet"})
        );
        assert_eq!(
            second["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        assert_eq!(
            third["request"],
            json!({"subtype":"apply_flag_settings","settings":{"effortLevel":"low"}})
        );
        for request in [first, second, third] {
            actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        }
        assert_eq!(actor.state.status, Status::Idle);
        actor
            .action(Action::Configure {
                model: Some("haiku".into()),
                effort: None,
                access: None,
            })
            .unwrap();
        assert!(actor.state.effort.is_none());
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        actor.action(Action::ResetConfiguration).unwrap();
        assert!(actor.state.model.is_none());
        assert!(actor.state.access.is_none());
        assert!(actor.reset_configuration);
    }
    #[test]
    fn interrupt_pauses_queue_and_foreign_session_cannot_complete_turn() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("active".into());
        actor.state.saved.thread_id = Some("primary".into());
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 3,
            text: "later".into(),
            images: Vec::new(),
        });
        actor.action(Action::Interrupt).unwrap();
        assert_eq!(read(&mut reader)["request"]["subtype"], "interrupt");
        assert!(!actor.queue_ready);
        actor
            .message(json!({"type":"result","session_id":"foreign","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(actor.state.saved.thread_id.as_deref(), Some("primary"));
        actor.message(json!({"type":"result","session_id":"primary","parent_tool_use_id":"subagent","is_error":false})).unwrap();
        assert_eq!(actor.state.status, Status::Working);
        actor
            .message(json!({"type":"result","session_id":"primary","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Interrupted);
        assert!(!actor.queue_ready);
        assert_eq!(actor.state.queued_prompts.len(), 1);
    }
    #[test]
    fn recovery_restores_launch_configuration() {
        let mut original = actor();
        original.state.saved.config_args =
            vec!["--model".into(), "sonnet".into(), "--chrome".into()];
        let (_, input) = mpsc::channel();
        let (updates, _) = crate::latest::channel();
        let restored = Actor::new(
            original.state.saved,
            Vec::new(),
            input,
            updates,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(restored.args, vec!["--model", "sonnet", "--chrome"]);
    }
    #[test]
    fn queue_waits_for_authoritative_idle_after_result() {
        let mut actor = actor();
        actor.tracks_session_state = true;
        actor.session_idle = false;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("turn".into());
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.state.turn_id.is_some());
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Done);
        assert!(actor.state.turn_id.is_none());
    }
    #[test]
    fn idle_before_result_retains_foreground_and_latest_failure_wins() {
        let mut actor = actor();
        actor.tracks_session_state = true;
        actor.state.turn_id = Some("turn".into());
        actor.state.status = Status::Working;
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.state.turn_id.is_some());
        actor
            .message(json!({"type":"result","is_error":true,"errors":["failure"]}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Failed);
        assert!(actor.state.turn_id.is_none());
        actor.session_idle = false;
        actor.state.turn_id = Some("next".into());
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        actor
            .message(json!({"type":"result","is_error":true,"errors":["continuation failed"]}))
            .unwrap();
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Failed);
        assert_eq!(actor.state.error.as_deref(), Some("continuation failed"));
    }
    #[test]
    fn queued_prompt_is_retained_until_echo_even_after_transport_failure() {
        let mut actor = actor();
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 8,
            text: "keep queued".into(),
            images: Vec::new(),
        });
        let prompt = actor.state.queued_prompts.first().cloned().unwrap();
        assert!(actor.submit(prompt.id, prompt.text, prompt.images).is_err());
        assert_eq!(actor.state.queued_prompts.len(), 1);
        let mut reader = transport(&mut actor);
        let prompt = actor.state.queued_prompts[0].clone();
        actor.submit(prompt.id, prompt.text, prompt.images).unwrap();
        let _ = read(&mut reader);
        actor.fail("disconnected before echo".into());
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert!(!actor.queue_ready);
        assert_eq!(actor.state.queued_prompts[0].text, "keep queued");
    }
    #[test]
    fn interrupt_before_input_echo_preserves_unaccepted_prompt_without_disconnect() {
        for idle_first in [false, true] {
            let mut actor = actor();
            let mut reader = transport(&mut actor);
            actor.initialized = true;
            actor.tracks_session_state = true;
            actor.state.status = Status::Idle;
            actor.state.saved.draft = "keep draft".into();
            actor.state.queued_prompts.push(QueuedPrompt {
                id: 9,
                text: "not yet accepted".into(),
                images: Vec::new(),
            });
            actor
                .submit(9, "not yet accepted".into(), Vec::new())
                .unwrap();
            let sent = read(&mut reader);
            actor.action(Action::Interrupt).unwrap();
            let _ = read(&mut reader);
            if idle_first {
                actor
                    .message(
                        json!({"type":"system","subtype":"session_state_changed","state":"idle"}),
                    )
                    .unwrap();
            }
            actor
                .message(json!({"type":"result","is_error":true,"errors":["interrupted"]}))
                .unwrap();
            if !idle_first {
                actor
                    .message(
                        json!({"type":"system","subtype":"session_state_changed","state":"idle"}),
                    )
                    .unwrap();
            }
            assert_eq!(actor.state.status, Status::Interrupted);
            assert!(actor.state.error.is_none());
            assert_eq!(actor.state.accepted_submission, None);
            assert_eq!(actor.state.rejected_submission, Some(9));
            assert_eq!(actor.state.saved.draft, "keep draft");
            assert_eq!(actor.state.queued_prompts.len(), 1);
            assert!(actor.process.is_some());
            assert!(!actor.queue_ready);
            actor
                .message(json!({"type":"user","uuid":sent["uuid"]}))
                .unwrap();
            assert_eq!(actor.state.accepted_submission, None);
            assert!(actor.state.saved.items.is_empty());
        }
    }
    #[test]
    fn fragmented_final_frames_preserve_original_stream_block_identity() {
        let mut actor = actor();
        actor.stream(&json!({"type":"message_start","message":{"id":"mixed"}}));
        let tool =
            json!({"type":"tool_use","id":"pwd-tool","name":"Bash","input":{"command":"pwd"}});
        actor.stream(&json!({"type":"content_block_start","index":0,"content_block":tool}));
        actor
            .message(json!({"type":"assistant","message":{"id":"mixed","content":[tool]}}))
            .unwrap();
        actor.stream(&json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}));
        actor.stream(&json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"# MARIGOLD\n\nResult."}}));
        let text = json!({"type":"text","text":"# MARIGOLD\n\nResult."});
        actor
            .message(json!({"type":"assistant","message":{"id":"mixed","content":[text]}}))
            .unwrap();
        assert_eq!(actor.state.saved.items.len(), 2);
        assert_eq!(actor.state.saved.items[1]["id"], "mixed-1");
        actor.stream(&json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}));
        actor.stream(&json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Second block."}}));
        let second = json!({"type":"text","text":"Second block."});
        actor
            .message(json!({"type":"assistant","message":{"id":"mixed","content":[second]}}))
            .unwrap();
        actor
            .message(
                json!({"type":"assistant","message":{"id":"mixed","content":[tool,text,second]}}),
            )
            .unwrap();
        assert_eq!(actor.state.saved.items.len(), 3);
        assert_eq!(
            actor
                .state
                .saved
                .items
                .iter()
                .filter(|item| item["type"] == "agentMessage")
                .count(),
            2
        );
        assert_eq!(actor.state.saved.items[1]["text"], text["text"]);
        assert_eq!(actor.state.saved.items[2]["text"], second["text"]);
    }
    #[test]
    fn advanced_tool_and_mcp_controls_roundtrip_and_cancel() {
        let mut actor = actor();
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("turn".into());
        let mut reader = transport(&mut actor);
        actor.message(json!({"type":"control_request","request_id":"plan","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","input":{"plan":"Build it"},"permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}})).unwrap();
        actor
            .action(Action::RespondApproval {
                id: json!("plan"),
                choice: "claude_suggestion_0".into(),
                input: Some(json!({"plan":"Revised"})),
                feedback: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"plan","response":{"behavior":"allow","updatedInput":{"plan":"Revised"},"updatedPermissions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}}})
        );
        let request = json!({"type":"control_request","request_id":"form","request":{"subtype":"elicitation","mcp_server_name":"trusted-server","message":"Ready?","mode":"form","requested_schema":{"type":"object","properties":{"ready":{"type":"boolean"}},"required":["ready"]}}});
        actor.message(request.clone()).unwrap();
        actor.message(request.clone()).unwrap();
        assert_eq!(actor.state.elicitations.len(), 1);
        assert_eq!(actor.state.elicitations[0].server_name, "trusted-server");
        actor
            .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Waiting);
        actor
            .action(Action::RespondElicitation {
                id: json!("form"),
                action: "accept".into(),
                content: Some(json!({"ready":false})),
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"form","response":{"action":"accept","content":{"ready":false}}}})
        );
        actor.message(json!({"type":"control_request","request_id":"url","request":{"subtype":"elicitation","mcp_server_name":"server","message":"Authorize","mode":"url","elicitation_id":"flow","url":"https://example.com/auth"}})).unwrap();
        actor
            .action(Action::RespondElicitation {
                id: json!("url"),
                action: "accept".into(),
                content: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"url","response":{"action":"accept"}}})
        );
        actor.message(request).unwrap();
        actor
            .message(json!({"type":"control_cancel_request","request_id":"form"}))
            .unwrap();
        assert!(actor.state.elicitations.is_empty());
        assert!(actor
            .action(Action::RespondElicitation {
                id: json!("form"),
                action: "accept".into(),
                content: Some(json!({"ready":true}))
            })
            .is_err());
        actor.message(json!({"type":"control_request","request_id":"dialog","request":{"subtype":"request_user_dialog","dialog_kind":"future_dialog","payload":{}}})).unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"type":"control_response","response":{"subtype":"error","request_id":"dialog","error":"Grove does not advertise this dialog kind."}})
        );
        assert!(actor
            .state
            .error
            .as_deref()
            .unwrap()
            .contains("future_dialog"));
        assert!(!serde_json::to_string(&actor.state.saved)
            .unwrap()
            .contains("trusted-server"));
    }
    #[test]
    fn assistant_usage_and_result_window_feed_the_context_meter() {
        let mut actor = actor();
        actor.state.turn_id = Some("turn".into());
        actor.state.status = Status::Working;
        actor.message(json!({"type":"assistant","parent_tool_use_id":null,"message":{"id":"m","model":"claude-opus-5-5","content":[{"type":"text","text":"OK."}],
            "usage":{"input_tokens":2,"cache_creation_input_tokens":12922,"cache_read_input_tokens":12654,"output_tokens":5}}})).unwrap();
        // The window is unknown until the result reports it.
        assert!(actor.state.saved.context.is_none());
        // Subagent traffic never moves the primary meter.
        actor.message(json!({"type":"assistant","parent_tool_use_id":"task","message":{"id":"s","model":"claude-haiku","content":[],
            "usage":{"input_tokens":900000,"output_tokens":1}}})).unwrap();
        actor
            .message(json!({"type":"result","is_error":false,"modelUsage":{
            "claude-haiku":{"contextWindow":200000},
            "claude-opus-5-5":{"contextWindow":1000000}}}))
            .unwrap();
        assert_eq!(
            actor.state.saved.context,
            Some(codex::ContextUsage {
                used: 25_583,
                window: 1_000_000
            })
        );
    }
    #[test]
    fn host_compact_replays_the_observed_claude_sequence_without_a_false_ack_error() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.tracks_session_state = true;
        actor.state.status = Status::Done;
        actor.state.saved.thread_id = Some("session".into());
        assert_eq!(
            actor.action(Action::Compact).unwrap_err(),
            codex::NOTHING_TO_COMPACT
        );
        actor.state.saved.items.push(
            json!({"id":"u","type":"userMessage","content":[{"type":"text","text":"Reply OK"}]}),
        );
        actor.state.saved.context = Some(codex::ContextUsage {
            used: 25_583,
            window: 1_000_000,
        });
        actor.action(Action::Compact).unwrap();
        let sent = read(&mut reader);
        assert_eq!(sent["type"], "user");
        assert_eq!(sent["message"]["content"][0]["text"], "/compact");
        let uuid = sent["uuid"].clone();
        assert_eq!(
            actor.action(Action::Compact).unwrap_err(),
            codex::COMPACT_BUSY
        );
        // Claude Code 2.1.302, captured verbatim in shape (bulky fields elided).
        for frame in [
            json!({"type":"system","subtype":"session_state_changed","state":"running","session_id":"session"}),
            json!({"type":"command_lifecycle","command_uuid":uuid,"state":"started","session_id":"session"}),
            json!({"type":"system","subtype":"status","status":"compacting","session_id":"session"}),
        ] {
            actor.message(frame).unwrap();
        }
        assert!(actor.state.compacting);
        assert_eq!(actor.state.status, Status::Working);
        for frame in [
            json!({"type":"system","subtype":"status","status":null,"compact_result":"success","session_id":"session"}),
            json!({"type":"system","subtype":"init","model":"claude-opus-5-5","permissionMode":"auto","session_id":"session"}),
            json!({"type":"system","subtype":"compact_boundary","session_id":"session","uuid":"boundary",
                "compact_metadata":{"trigger":"manual","pre_tokens":25583,"post_tokens":4200}}),
        ] {
            actor.message(frame).unwrap();
        }
        assert!(!actor.state.compacting);
        let probe = read(&mut reader);
        assert_eq!(probe["request"]["subtype"], "get_context_usage");
        for frame in [
            json!({"type":"user","message":{"role":"user","content":"This session is being continued from a previous conversation..."},"session_id":"session","parent_tool_use_id":null}),
            json!({"type":"user","message":{"role":"user","content":"<local-command-stdout>Compacted </local-command-stdout>"},"session_id":"session","parent_tool_use_id":null,"uuid":"stdout","isReplay":true}),
            json!({"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"},"session_id":"session","parent_tool_use_id":null,"uuid":uuid,"isReplay":true}),
            json!({"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","session_id":"session",
                "modelUsage":{"claude-opus-5-5":{"contextWindow":1000000}}}),
            json!({"type":"system","subtype":"session_state_changed","state":"idle","session_id":"session"}),
        ] {
            actor.message(frame).unwrap();
        }
        assert_eq!(actor.state.status, Status::Done);
        assert!(actor.state.error.is_none());
        assert!(actor.process.is_some());
        // The divider stands in for the command; no "/compact" bubble or summary leaks in.
        assert_eq!(actor.state.saved.items.len(), 2);
        let divider = &actor.state.saved.items[1];
        assert_eq!(divider["type"], codex::COMPACTION_ITEM);
        assert_eq!(
            codex::compaction_label(divider),
            "Context compacted \u{b7} manual"
        );
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":probe["request_id"],
            "response":{"totalTokens":23327,"maxTokens":1000000,"percentage":2}}})).unwrap();
        assert_eq!(
            codex::compaction_label(&actor.state.saved.items[1]),
            "Context compacted \u{b7} manual \u{b7} 26k \u{2192} 23k"
        );
        assert_eq!(actor.state.saved.context.unwrap().used, 23_327);
    }
    #[test]
    fn automatic_boundary_fills_after_from_next_usage_and_probe_errors_are_harmless() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.turn_id = Some("turn".into());
        actor.state.status = Status::Working;
        actor
            .message(json!({"type":"system","subtype":"status","status":"compacting"}))
            .unwrap();
        assert!(actor.state.compacting);
        actor.message(json!({"type":"system","subtype":"compact_boundary","uuid":"b","compact_metadata":{"trigger":"auto","pre_tokens":182000}})).unwrap();
        let probe = read(&mut reader);
        actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":probe["request_id"],"error":"unsupported"}})).unwrap();
        assert!(actor.state.error.is_none());
        assert!(actor.process.is_some());
        assert_eq!(
            codex::compaction_label(&actor.state.saved.items[0]),
            "Context compacted \u{b7} auto"
        );
        actor.message(json!({"type":"assistant","message":{"id":"m","model":"claude-opus-5-5","content":[],
            "usage":{"input_tokens":4000,"cache_read_input_tokens":20000,"output_tokens":10}}})).unwrap();
        assert_eq!(
            codex::compaction_label(&actor.state.saved.items[0]),
            "Context compacted \u{b7} auto \u{b7} 182k \u{2192} 24k"
        );
    }
    #[test]
    fn ordinary_prompt_without_echo_still_reports_unacknowledged_finish() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.submit(5, "hello".into(), Vec::new()).unwrap();
        let _ = read(&mut reader);
        assert!(actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap_err()
            .contains("before acknowledging"));
    }
    #[test]
    fn native_plan_actor_journals_queues_and_confirms_one_review_without_touching_draft() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
        actor.state.saved.draft = "independent composer".into();

        assert!(actor
            .send_plan_response("plan", "r1", |_| Err("disk full".into()))
            .is_err());
        assert!(actor.state.queued_prompts.is_empty());
        let journal = std::cell::RefCell::new(Vec::new());
        for _ in 0..2 {
            actor
                .send_plan_response("plan", "r1", |saved| {
                    journal
                        .borrow_mut()
                        .push(serde_json::to_vec(saved).unwrap());
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(journal.borrow().len(), 1);
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert!(!actor.drain_queue());
        let id = actor.state.queued_prompts[0].id;
        assert!(!actor.state.queue_editable(id));
        actor.state.status = Status::Idle;
        actor.queue_ready = true;
        assert!(actor.drain_queue());
        let wire = read(&mut reader);
        assert_eq!(wire["type"], "user");
        assert!(wire["message"]["content"]
            .to_string()
            .contains("Plan review: Native"));
        actor.acknowledge_input(&wire["uuid"]);
        assert!(actor.state.queued_prompts.is_empty());
        assert!(matches!(
            actor.state.saved.rich_plans.revisions[0].review.delivery,
            crate::rich_plan::ResponseDelivery::Sent { .. }
        ));
        actor
            .send_plan_response("plan", "r1", |_| {
                panic!("confirmed feedback must not be journaled again")
            })
            .unwrap();
        assert!(actor.state.queued_prompts.is_empty());
        assert_eq!(actor.state.saved.draft, "independent composer");
        let mut restarted = Snapshot::new(
            serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
        );
        assert!(restarted
            .saved
            .rich_plans
            .prepare_delivery("plan", "r1")
            .unwrap()
            .is_none());
    }
}
