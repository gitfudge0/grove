//! Grove-owned Claude Code sessions using the CLI's streaming SDK transport.
//! The subprocess owns credentials, tools, permissions, and conversation persistence.
use crate::codex::{
    self, Action, Connection, Handle, QuestionRequest, QueuedPrompt, SavedSession, Snapshot,
    Status, SubmissionMode,
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

pub fn connect(saved: SavedSession, args: Vec<String>, task_id: Option<String>) -> Connection {
    let (handle, input) = Handle::channel();
    let (updates, output) = mpsc::channel();
    let pid = handle.1.clone();
    let stopping = handle.2.clone();
    std::thread::spawn(move || {
        Actor::new(saved, args, task_id, input, updates, pid, stopping).run();
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
fn launch_command(
    args: &[String],
    saved: &SavedSession,
    task_id: Option<&str>,
) -> Result<Command, String> {
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
    // The native question panel renders the extended text and number schema.
    command.env("CLAUDE_CODE_QUESTION_EXTENDED", "1");
    command.env(
        "GROVE_CONFIG_DIR",
        crate::storage::config_dir().map_err(|e| e.to_string())?,
    );
    if let Some(task) = task_id {
        command.env("GROVE_TASK_ID", task);
    }
    if let Some(id) = &saved.thread_id {
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
fn spawn(args: &[String], saved: &SavedSession, task_id: Option<&str>) -> Result<Process, String> {
    let mut command = launch_command(args, saved, task_id)?;
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
    task_id: Option<String>,
    input: mpsc::Receiver<Action>,
    updates: mpsc::Sender<Snapshot>,
    pid: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
    process: Option<Process>,
    initialized: bool,
    started: Instant,
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
    /// A host `/compact` is in flight; its echo is represented by the divider, not a bubble.
    compact_pending: bool,
    /// Model of the latest primary assistant message, used to pick its context window.
    context_model: Option<String>,
    queue_ready: bool,
    interrupting: bool,
    control_id: u64,
    pending: HashMap<String, String>,
    blocks: HashMap<u64, String>,
    message_id: String,
    last_persist: Instant,
    dirty: bool,
}
impl Actor {
    fn new(
        mut saved: SavedSession,
        args: Vec<String>,
        task_id: Option<String>,
        input: mpsc::Receiver<Action>,
        updates: mpsc::Sender<Snapshot>,
        pid: Arc<AtomicU32>,
        stopping: Arc<AtomicBool>,
    ) -> Self {
        let args = if args.is_empty() {
            saved.config_args.clone()
        } else {
            args
        };
        let task_id = task_id.or_else(|| saved.task_id.clone());
        saved.config_args = launch_options(&args).map_or_else(|_| args.clone(), |v| v.0);
        saved.task_id.clone_from(&task_id);
        Self {
            state: Snapshot::new(saved),
            args,
            task_id,
            input,
            updates,
            pid,
            stopping,
            process: None,
            initialized: false,
            started: Instant::now(),
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
            compact_pending: false,
            context_model: None,
            queue_ready: true,
            interrupting: false,
            control_id: 0,
            pending: HashMap::new(),
            blocks: HashMap::new(),
            message_id: String::new(),
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
                continue;
            }
            loop {
                let message = self.process.as_ref().and_then(|p| p.lines.try_recv().ok());
                let Some(message) = message else { break };
                match message {
                    Ok(value) => {
                        if let Err(error) = self.message(value) {
                            self.fail(error);
                        }
                    }
                    Err(error) => self.fail(error),
                }
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
                    self.save();
                    self.publish();
                }
            }
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
    }
    fn save(&mut self) {
        self.last_persist = Instant::now();
        if let Err(error) = codex::persist(&self.state.saved) {
            self.state.error = Some(error);
        } else {
            self.dirty = false;
        }
    }
    fn publish(&self) {
        let _ = self.updates.send(self.state.clone());
    }
    fn fail(&mut self, error: String) {
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
        if self.in_flight.is_some() {
            self.state.mark_queue_uncertain();
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        if self.stopping.load(Ordering::SeqCst) {
            return Err("Claude Code session is stopping".into());
        }
        let (args, initial) = launch_options(&self.args)?;
        if self.state.saved.thread_id.is_none() && self.initial.is_none() && self.deferred.is_none()
        {
            self.initial = initial;
        }
        let process = spawn(&args, &self.state.saved, self.task_id.as_deref())?;
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
        self.compact_pending = false;
        self.state.compacting = false;
        self.initialized = false;
        self.started = Instant::now();
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
        self.control(json!({"subtype":"initialize","hooks":null}))
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
    fn submit(&mut self, id: u64, text: String, images: Vec<String>) -> Result<(), String> {
        let result = self.submit_inner(id, text, images);
        if result.is_err() {
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
        let content = user_content(&text, &images);
        let uuid = nonce();
        self.write(json!({"type":"user","uuid":uuid,"session_id":self.state.saved.thread_id.as_deref().unwrap_or(""),"message":{"role":"user","content":content},"parent_tool_use_id":null}))?;
        self.input_uuid = Some(uuid.clone());
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
        self.input_uuid = None;
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
        self.state
            .saved
            .items
            .push(json!({"id":uuid,"type":"userMessage","content":parts}));
        if self.state.saved.title.is_none() {
            self.state.saved.title = Some(prompt.text.chars().take(80).collect());
        }
        if !self.state.queued_prompts.iter().any(|p| p.id == prompt.id) {
            self.state
                .saved
                .clear_submitted_draft(&prompt.text, &prompt.images);
        }
        self.state
            .queued_prompts
            .retain(|queued| queued.id != prompt.id);
        self.state.queue_sending.retain(|id| *id != prompt.id);
        self.state.queue_uncertain.retain(|id| *id != prompt.id);
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
        self.context_model = None;
        self.queue_ready = true;
        let cleared = self.state.cleared_conversations + 1;
        self.state = Snapshot::new(saved);
        self.state.cleared_conversations = cleared;
        Ok(())
    }
    fn action(&mut self, action: Action) -> Result<(), String> {
        match action {
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
                    self.state.rejected_submission = Some(id);
                    return Err("Claude Code does not support steering an active turn. Queue the message or interrupt first.".into());
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
            Action::SteerQueued(_) => {
                return Err("Claude Code does not support steering an active turn".into());
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
                self.state.saved.draft = text;
            }
            Action::DraftImages(images) => {
                codex::validate_images(&images)?;
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
    fn upsert(&mut self, item: Value) {
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
    fn message(&mut self, value: Value) -> Result<(), String> {
        if !value["parent_tool_use_id"].is_null() {
            return Ok(());
        }
        if let Some(session) = value["session_id"].as_str().filter(|id| !id.is_empty()) {
            if let Some(primary) = &self.state.saved.thread_id {
                if primary != session {
                    return Ok(());
                }
            } else {
                self.state.saved.thread_id = Some(session.into());
            }
        }
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
                    self.initialized = true;
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
                            self.tool_result(block);
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
                    for key in ["status", "result", "error", "aggregatedOutput", "progress"] {
                        if let Some(value) = old.get(key) {
                            item[key] = value.clone();
                        }
                    }
                }
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
    fn tool_result(&mut self, block: &Value) {
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
        item["status"] = json!(if block["is_error"] == true {
            "failed"
        } else {
            "completed"
        });
        if item["type"] == "commandExecution" {
            item["aggregatedOutput"] = json!(content
                .iter()
                .filter_map(|v| v["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"));
        } else {
            item["result"] = json!({"content":content});
        }
        if block["is_error"] == true {
            item["error"] = json!({"message":content.iter().filter_map(|v|v["text"].as_str()).collect::<Vec<_>>().join("\n")});
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn actor() -> Actor {
        let saved:SavedSession=serde_json::from_value(json!({"key":"claude-test","provider":"claude","project":"test","cwd":"/tmp","label":"Claude","roots":[],"thread_id":null,"items":[]})).unwrap();
        let (_, input) = mpsc::channel();
        let (updates, _) = mpsc::channel();
        Actor::new(
            saved,
            Vec::new(),
            None,
            input,
            updates,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
        )
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
        let command = launch_command(&actor.args, &restored, None).unwrap();
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
        let command = launch_command(&[], &actor.state.saved, None).unwrap();
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
    fn recovery_restores_launch_configuration_and_task_environment() {
        let mut original = actor();
        original.state.saved.config_args =
            vec!["--model".into(), "sonnet".into(), "--chrome".into()];
        original.state.saved.task_id = Some("task".into());
        let (_, input) = mpsc::channel();
        let (updates, _) = mpsc::channel();
        let restored = Actor::new(
            original.state.saved,
            Vec::new(),
            None,
            input,
            updates,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(restored.args, vec!["--model", "sonnet", "--chrome"]);
        assert_eq!(restored.task_id.as_deref(), Some("task"));
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
}
