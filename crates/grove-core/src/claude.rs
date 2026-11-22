//! Grove-owned Claude Code sessions using the CLI's streaming SDK transport.
//! The subprocess owns credentials, tools, permissions, and conversation persistence.
mod commands;
mod context;
mod events;
mod permissions;
mod subagents;

pub use permissions::can_select_permission_mode;
use permissions::{available_permission_modes, permission_mode};
use subagents::ChildTranscript;

use crate::provider;
use crate::provider::{
    Action, Connection, EditCheckpoint, Handle, PendingEdit, QueuedPrompt, SavedSession, Snapshot,
    Status,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod transport;

#[cfg(test)]
use transport::{launch_command, launch_command_profile, CLAUDE_AI_ARTIFACT_GUIDANCE};
use transport::{spawn, spawn_profile, Process};

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

/// Initialize a temporary, non-persistent CLI peer solely to read composer metadata.
pub(crate) fn discover_composer(
    saved: SavedSession,
    args: Vec<String>,
) -> Result<Snapshot, String> {
    let (_handle, input) = Handle::channel();
    let (updates, _output) = crate::latest::channel();
    let stopping = Arc::new(AtomicBool::new(false));
    let mut actor = Actor::new(
        saved,
        args.clone(),
        input,
        updates,
        Arc::new(AtomicU32::new(0)),
        stopping.clone(),
    );
    let (mut options, _) = launch_options(&args)?;
    options.push("--no-session-persistence".into());
    actor.model_catalog_key = crate::model_catalog::Key::new(&actor.state.saved, &args);
    actor.state.permission_modes = available_permission_modes(&stopping);
    actor.process = Some(spawn(&options, &actor.state.saved)?);
    actor.control(json!({"subtype":"initialize","hooks":null}))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !actor.initialized {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Timed out reading Claude composer options.".into());
        }
        let message = actor
            .process
            .as_ref()
            .ok_or("Claude discovery disconnected")?
            .lines
            .recv_timeout(remaining)
            .map_err(|error| format!("Could not read Claude composer options: {error}"))??;
        // Only the host-requested response is relevant; no user message or
        // conversation activity is accepted by this metadata-only peer.
        if message["type"] == "control_response" {
            actor.message(message)?;
        }
        if let Some(error) = &actor.state.error {
            return Err(error.clone());
        }
    }
    Ok(actor.state.clone())
}

pub fn connect(saved: SavedSession, args: Vec<String>) -> Connection {
    connect_deferred(saved, args, provider::deferred_connection(Arc::new(|| {})))
}
pub(crate) fn connect_deferred(
    saved: SavedSession,
    args: Vec<String>,
    deferred: provider::DeferredConnection,
) -> Connection {
    connect_deferred_with_validation(saved, args, deferred, None)
}
pub(crate) fn connect_deferred_with_validation(
    saved: SavedSession,
    args: Vec<String>,
    deferred: provider::DeferredConnection,
    validation: Option<provider::StartupValidation>,
) -> Connection {
    let provider::DeferredConnection {
        handle,
        receiver,
        thread,
    } = deferred;
    let (updates, output) = crate::latest::channel();
    let pid = handle.1.clone();
    let stopping = handle.2.clone();
    std::thread::spawn(move || {
        let _ = thread.set(std::thread::current());
        let mut actor = Actor::new(saved, args, receiver, updates, pid, stopping);
        actor.startup_validation = validation;
        actor.run();
    });
    Connection {
        handle,
        updates: output,
    }
}
/// Subscription-authenticated side conversation using the regular SDK transport.
pub(crate) fn connect_side_chat(mut saved: SavedSession) -> Connection {
    saved.composer = provider::ComposerSettings {
        model: Some("haiku".into()),
        access: Some("dontAsk".into()),
        ..Default::default()
    };
    saved.title_generation = None;
    let args = vec![
        "--model".into(),
        "haiku".into(),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--tools".into(),
        "Read,Glob,Grep".into(),
        "--allowedTools".into(),
        "Read,Glob,Grep".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        r#"{"mcpServers":{}}"#.into(),
        "--disable-slash-commands".into(),
    ];
    saved.config_args.clone_from(&args);
    saved.native_config_version = 1;
    let (handle, input, actor_thread) = Handle::actor_channel();
    let (updates, output) = crate::latest::channel();
    let pid = handle.1.clone();
    let stopping = handle.2.clone();
    std::thread::spawn(move || {
        let _ = actor_thread.set(std::thread::current());
        let mut actor = Actor::new(saved, args, input, updates, pid, stopping);
        actor.side_chat = true;
        actor.run();
    });
    Connection {
        handle,
        updates: output,
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
    side_chat: bool,
    startup_validation: Option<provider::StartupValidation>,
    startup_error: Option<String>,
    model_catalog_key: Option<crate::model_catalog::Key>,
    state: Snapshot,
    args: Vec<String>,
    input: mpsc::Receiver<Action>,
    updates: crate::latest::Sender<Snapshot>,
    pid: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
    process: Option<Process>,
    provider_use: Option<crate::connections::ProviderUse>,
    initialized: bool,
    started: Instant,
    first_output: bool,
    mcp_polled_at: Option<Instant>,
    context_query_started: Option<Instant>,
    mcp_status_supported: bool,
    reconfigure: bool,
    reset_configuration: bool,
    configuring: bool,
    pending_access: Option<String>,
    permission_changes: HashMap<String, (String, u64)>,
    permission_error: Option<String>,
    permission_mode_revision: u64,
    interaction_children: crate::interaction::Children,
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
    restore_reservation: Option<crate::checkpoint::RestoreReservation>,
    blocks: HashMap<u64, String>,
    subagent_tasks: HashMap<String, String>,
    child_transcripts: HashMap<String, ChildTranscript>,
    message_id: String,
    title_generator: crate::session_title::Generator,
    last_persist: Instant,
    last_emit: Instant,
    publication_pending: bool,
    persistence: provider::BackgroundPersistence,
    last_work_active: bool,
    last_activity_context: Option<(u64, Option<String>)>,
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
            saved.migrate_native_config();
            saved.config_args.clone()
        } else {
            args
        };
        saved.config_args = launch_options(&args).map_or_else(|_| args.clone(), |v| v.0);
        saved.native_config_version = 1;
        let mut transcript = crate::claude_transcript::ClaudeTranscriptState::default();
        transcript.restore(&mut saved.items);
        let last_assistant_uuid = saved
            .items
            .iter()
            .rev()
            .find_map(|item| item["claudeAssistantUuid"].as_str().map(str::to_owned));
        let mut state = Snapshot::new(saved);
        state.edit_supported = true;
        let subagent_tasks = state
            .saved
            .claude_subagents
            .iter()
            .flat_map(|(id, child)| {
                child
                    .task_ids
                    .iter()
                    .map(move |task| (task.clone(), id.clone()))
            })
            .collect();
        let child_transcripts = state
            .saved
            .claude_subagents
            .iter()
            .map(|(id, child)| (id.clone(), ChildTranscript::restore(child.items.clone())))
            .collect();
        Self {
            side_chat: false,
            startup_validation: None,
            startup_error: None,
            model_catalog_key: None,
            state,
            args,
            input,
            updates,
            pid,
            stopping,
            process: None,
            provider_use: None,
            initialized: false,
            started: Instant::now(),
            first_output: false,
            mcp_polled_at: None,
            context_query_started: None,
            mcp_status_supported: true,
            reconfigure: false,
            reset_configuration: false,
            configuring: false,
            pending_access: None,
            permission_changes: HashMap::new(),
            permission_error: None,
            permission_mode_revision: 0,
            interaction_children: crate::interaction::Children::default(),
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
            restore_reservation: None,
            blocks: HashMap::new(),
            subagent_tasks,
            child_transcripts,
            message_id: String::new(),
            title_generator: crate::session_title::Generator::default(),
            last_persist: Instant::now(),
            last_emit: Instant::now(),
            publication_pending: false,
            persistence: provider::BackgroundPersistence::default(),
            last_work_active: false,
            last_activity_context: None,
            dirty: false,
        }
    }
    fn stop(
        &mut self,
        forget: bool,
        done: mpsc::Sender<()>,
        archive_outcome: Option<mpsc::Sender<Result<(), String>>>,
    ) {
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        let outcome = if forget && !self.side_chat {
            self.save();
            provider::archive_closed_session(&self.state.saved)
        } else {
            self.save();
            Ok(())
        };
        if let Err(error) = &outcome {
            self.save();
            eprintln!("Could not archive closed native session: {error}");
        }
        if let Some(sender) = archive_outcome {
            let _ = sender.send(outcome);
        }
        let _ = done.send(());
    }
    fn run(mut self) {
        if !self.stopping.load(Ordering::SeqCst) {
            if let Err(error) = self.start() {
                self.fail(error);
            }
        }
        self.save();
        self.publish();
        loop {
            match self.input.try_recv() {
                Ok(Action::Stop {
                    forget,
                    done,
                    archive_outcome,
                }) => {
                    self.stop(forget, done, archive_outcome);
                    break;
                }
                Ok(action) => {
                    std::thread::current().unpark();
                    if let Err(error) = self.action(action) {
                        self.state.error = Some(error);
                    }
                    self.dirty = true;
                    self.publish();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.save();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            // stop_completion kills the process immediately; do not recreate it or
            // write recovery while its Stop action is still in flight.
            if self.stopping.load(Ordering::SeqCst) {
                self.title_generator.cancel();
                if self.title_generator.update_pending(&mut self.state, true) {
                    self.publish();
                }
                std::thread::park_timeout(Duration::from_secs(1));
                continue;
            }
            // The UI consumes complete state, so intermediate protocol deltas
            // need not each clone the accumulated conversation. A bounded batch
            // also lets actions and publication run during continuous output.
            if let Some(process) = &self.process {
                if process.visuals.sync(&mut self.state, self.side_chat) {
                    self.dirty = true;
                    self.publish();
                }
            }
            self.poll_context_query();
            self.poll_mcp_status();
            if self.receive_batch() {
                std::thread::current().unpark();
                self.publication_pending = true;
            }
            if self.publication_pending
                && (self.last_emit.elapsed() >= Duration::from_millis(16)
                    || !self.state.provider_work_active())
            {
                self.publish();
            }
            let title_changed = !self.side_chat
                && self.startup_error.is_none()
                && self
                    .title_generator
                    .tick(&mut self.state.saved, self.state.status, false);
            let pending_changed = self.title_generator.update_pending(&mut self.state, false);
            if title_changed || pending_changed {
                self.dirty = true;
                self.publish();
            }
            if self.dirty && self.last_persist.elapsed() >= Duration::from_secs(2) {
                if let Err(error) = self.persistence.schedule(&self.state.saved) {
                    self.state.error = Some(error);
                }
                self.dirty = false;
                self.last_persist = Instant::now();
            }
            if let Some(error) = self.persistence.take_error() {
                self.state.error = Some(error);
                self.dirty = true;
                self.publish();
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
            let wait = if self.publication_pending {
                Duration::from_millis(16).saturating_sub(self.last_emit.elapsed())
            } else {
                Duration::from_secs(1)
            };
            std::thread::park_timeout(wait);
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
    }
    fn drain_queue(&mut self) -> bool {
        if crate::connections::credentials_busy(crate::agent::Agent::Claude) {
            return false;
        }
        if self.initialized
            && self.permission_changes.is_empty()
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
        if self.side_chat {
            self.last_persist = Instant::now();
            self.dirty = false;
            return;
        }
        self.last_persist = Instant::now();
        if let Err(error) = self.persistence.flush(&self.state.saved) {
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
    fn poll_context_query(&mut self) {
        if self.state.context_query_pending
            && self
                .context_query_started
                .is_some_and(|started| started.elapsed() >= Duration::from_secs(15))
        {
            self.pending
                .retain(|_, kind| !kind.starts_with("get_context_usage:"));
            self.state.context_query_pending = false;
            self.context_query_started = None;
            self.state.context_query_error =
                Some("Context measurement timed out. Try again.".into());
            self.publish();
        }
    }
    fn poll_mcp_status(&mut self) {
        if self.side_chat
            || !self.initialized
            || self.process.is_none()
            || !self.mcp_status_supported
        {
            return;
        }
        if self.pending.values().any(|kind| kind == "mcp_status") {
            // Optional health discovery must never hold editing/configuration
            // hostage on an older CLI that does not answer the request.
            if self
                .mcp_polled_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(15))
            {
                self.pending.retain(|_, kind| kind != "mcp_status");
                self.mcp_status_supported = false;
            }
            return;
        }
        if self.configuring
            || !self.pending.is_empty()
            || self
                .mcp_polled_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.mcp_polled_at = Some(Instant::now());
        if self.control(json!({"subtype":"mcp_status"})).is_err() {
            self.pending.retain(|_, kind| kind != "mcp_status");
            self.mcp_status_supported = false;
        }
    }
    fn observe_mcp_servers(&mut self, servers: &Value) {
        for server in servers.as_array().into_iter().flatten() {
            if let (Some(name), Some(status)) = (server["name"].as_str(), server["status"].as_str())
            {
                self.state.mcp_server_status(
                    name,
                    status,
                    server["error"].as_str(),
                    status == "needs-auth",
                );
            }
        }
    }
    fn publish(&mut self) {
        let active = self.state.provider_work_active();
        if self.last_work_active && !active {
            self.save();
        }
        self.last_work_active = active;
        if self.process.is_none()
            || (!self.state.provider_work_active()
                && self.in_flight.is_none()
                && self.pending_completion.is_none()
                && !self.compact_pending
                && (!self.tracks_session_state || self.session_idle))
        {
            self.provider_use.take();
        }
        if let Some(process) = &self.process {
            let context = (
                self.state.saved.items.revision(),
                self.state.turn_id.clone(),
            );
            if self.last_activity_context.as_ref() != Some(&context) {
                process
                    .activity
                    .set_context_items(&self.state.saved.items, self.state.turn_id.clone());
                self.last_activity_context = Some(context);
            }
            process.visuals.sync(&mut self.state, self.side_chat);
        }
        let mut snapshot = self.state.clone();
        snapshot.can_redirect = self.initialized
            && self.process.is_some()
            && self.state.status.busy()
            && self.state.turn_id.is_some()
            && !self.state.compacting
            && self.in_flight.is_none()
            && self.pending_completion.is_none()
            && !self.configuring
            && self.permission_changes.is_empty()
            && !self.interrupting
            && !self.redirect_pending
            && self.state.saved.pending_edit.is_none();
        let _ = self.updates.send(snapshot);
        self.last_emit = Instant::now();
        self.publication_pending = false;
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
        self.state.status = Status::Disconnected;
        self.state.reconnecting = false;
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
        self.title_generator.update_pending(&mut self.state, false);
    }
    fn write(&mut self, value: Value) -> Result<(), String> {
        let model_input = value["type"] == "user";
        if model_input && self.provider_use.is_none() {
            self.provider_use = Some(crate::connections::ProviderUse::acquire(
                provider::Provider::Claude,
            )?);
        }
        let result = (|| {
            let process = self
                .process
                .as_mut()
                .ok_or("Claude Code is disconnected; reconnect to continue")?;
            serde_json::to_writer(&mut process.input, &value).map_err(|error| error.to_string())?;
            process
                .input
                .write_all(b"\n")
                .and_then(|()| process.input.flush())
                .map_err(|error| format!("Cannot send to Claude Code: {error}"))?;
            process.capture.record(true, &value);
            Ok(())
        })();
        if model_input && result.is_err() {
            self.process.take();
            self.pid.store(0, Ordering::SeqCst);
            self.provider_use.take();
            self.state.status = Status::Disconnected;
        }
        result
    }
    fn refresh_context(&mut self) {
        if self.state.context_query_pending {
            return;
        }
        self.state.context_query_error = None;
        if self.process.is_none() || !self.initialized {
            self.state.context_query_error = Some("Reconnect to measure context.".into());
            return;
        }
        if self.state.status.busy() || self.state.compacting {
            self.state.context_query_error =
                Some("Wait for the current turn to finish before measuring context.".into());
            return;
        }
        if let Err(error) = self.control(json!({"subtype":"get_context_usage"})) {
            self.state.context_query_pending = false;
            self.state.context_query_error = Some(error);
            self.pending
                .retain(|_, kind| !kind.starts_with("get_context_usage:"));
            self.context_query_started = None;
        }
    }
    fn control(&mut self, request: Value) -> Result<(), String> {
        self.control_id += 1;
        let id = format!("grove-{}", self.control_id);
        if request["subtype"] == "set_permission_mode" {
            if let Some(mode) = request["mode"].as_str() {
                self.permission_changes
                    .insert(id.clone(), (mode.into(), self.permission_mode_revision));
            }
        }
        let subtype = if request["subtype"] == "get_context_usage" {
            self.state.context_query_pending = true;
            self.context_query_started = Some(Instant::now());
            format!(
                "get_context_usage:{}:{}",
                self.state.context_revision,
                self.state.resolved_model.as_deref().unwrap_or_default()
            )
        } else {
            request["subtype"].as_str().unwrap_or_default().into()
        };
        self.pending.insert(id.clone(), subtype);
        self.write(json!({"type":"control_request","request_id":id,"request":request}))
    }
    fn start(&mut self) -> Result<(), String> {
        crate::env_path::wait_for_initial_environment();
        self.startup_error = None;
        if let Some(validate) = &self.startup_validation {
            if let Err(error) = validate() {
                self.startup_error = Some(error.clone());
                self.title_generator.cancel();
                self.state.status = Status::Disconnected;
                self.state.error = Some(error.clone());
                return Err(error);
            }
        }
        self.last_activity_context = None;
        self.state.context_measurement_live = false;
        // Every startup attempt retires steering state, including an early stop.
        self.redirect_pending = false;
        if self.stopping.load(Ordering::SeqCst) {
            return Err("Claude Code session is stopping".into());
        }
        let mut roots = vec![std::path::PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| std::path::PathBuf::from(&root.wt_path)),
        );
        if provider::workspace_recovery_blocked(&roots, Some(&self.state.saved.key))? {
            return Err("Another conversation has unresolved workspace recovery in this directory. Reconnect to that conversation before starting work here.".into());
        }
        if self.restore_reservation.is_none() {
            if let Some(recovery) = self
                .state
                .saved
                .pending_edit
                .as_ref()
                .and_then(|edit| edit.recovery.as_ref())
            {
                self.restore_reservation =
                    Some(crate::checkpoint::acquire_restore(&recovery.roots())?);
            }
        }
        self.state.saved.activity_reporting = !self.side_chat;
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
            if let Some(prompt) = initial.as_deref() {
                crate::session_title::Generator::anticipate_initial(&mut self.state, prompt);
            }
            self.initial = initial;
        }
        self.model_catalog_key = (!self.side_chat)
            .then(|| crate::model_catalog::Key::new(&self.state.saved, &self.args))
            .flatten();
        self.state.permission_modes = available_permission_modes(&self.stopping);
        let process = spawn_profile(&args, &self.state.saved, self.side_chat)?;
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
        self.mcp_polled_at = None;
        self.mcp_status_supported = true;
        self.state.mcp_statuses.clear();
        self.reconfigure = false;
        self.reset_configuration = false;
        self.configuring = false;
        self.tracks_session_state = false;
        self.pending_completion = None;
        self.pending.clear();
        self.state.context_query_pending = false;
        self.state.context_revision = self.state.context_revision.wrapping_add(1);
        self.permission_changes.clear();
        self.permission_error = None;
        self.blocks.clear();
        self.state.status = Status::Starting;
        self.state.reported_speed = None;
        self.state.speed_disabled_reason = None;
        self.state.clear_error();
        self.interaction_children = crate::interaction::Children::default();
        self.state.interaction_owners.clear();
        self.state.retired_interactions.clear();
        self.state.interaction_epoch = provider::next_interaction_epoch();
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
        if let Some(access) = self.pending_access.take() {
            self.control(
                json!({"subtype":"set_permission_mode","mode":permission_mode(&access)?}),
            )?;
        }
        let mut settings = json!({"effortLevel":self.state.effort});
        if let Some(speed) = self.state.saved.composer.speed {
            // False is explicit Standard; null would remove the session overlay
            // and could expose an inherited fastMode=true user setting.
            settings["fastMode"] = json!(
                speed == provider::Speed::Fast && self.state.speed_unavailable(speed).is_none()
            );
        }
        self.control(json!({"subtype":"apply_flag_settings","settings":settings}))?;
        Ok(())
    }
    fn configure_pending_access(&mut self) -> Result<(), String> {
        // Model/default restoration may still require a restart on the next
        // prompt. A later explicit access choice must reach the live process
        // now; its ACK also persists the choice for that eventual restart.
        if !self.initialized || !self.permission_changes.is_empty() {
            return Ok(());
        }
        if let Some(access) = self.pending_access.take() {
            self.control(
                json!({"subtype":"set_permission_mode","mode":permission_mode(&access)?}),
            )?;
        }
        Ok(())
    }
    fn capture_edit_checkpoint(&mut self) {
        if self.side_chat {
            return;
        }
        let mut roots = vec![std::path::PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| std::path::PathBuf::from(&root.wt_path)),
        );
        let files = crate::checkpoint::capture_for_send(&roots).ok();
        self.state
            .saved
            .edit_checkpoints
            .retain(|record| record.turn_id.is_some());
        self.state.saved.edit_checkpoints.push(EditCheckpoint {
            start_item: self.state.saved.items.len(),
            turn_id: None,
            files,
        });
    }
    fn redirect(&mut self, id: u64, text: String, images: Vec<String>) -> Result<(), String> {
        provider::validate_prompt(&text, &images)?;
        if !self.initialized
            || !self.state.status.busy()
            || self.state.turn_id.is_none()
            || self.state.compacting
            || self.in_flight.is_some()
            || self.pending_completion.is_some()
            || self.configuring
            || !self.permission_changes.is_empty()
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
        self.state.clear_error();
        Ok(())
    }
    fn edit_message(
        &mut self,
        index: usize,
        message_id: String,
        composer_draft: Option<(String, Vec<String>)>,
    ) -> Result<(), String> {
        self.edit_message_with_options(index, message_id, composer_draft, true, false)
    }
    fn edit_message_with_options(
        &mut self,
        index: usize,
        message_id: String,
        composer_draft: Option<(String, Vec<String>)>,
        restore_files: bool,
        rewind: bool,
    ) -> Result<(), String> {
        let checkpoint = self.state.rewind_target(index)?;
        if restore_files && checkpoint.files.is_none() {
            return Err("The workspace checkpoint is missing.".into());
        }
        if rewind && restore_files {
            if let Some(reason) = self.state.rewind_files_unavailable(index) {
                return Err(reason);
            }
        }
        if self.process.is_none()
            || self
                .pending
                .values()
                .any(|kind| kind != "mcp_status" && !kind.starts_with("get_context_usage:"))
            || self.in_flight.is_some()
            || !self.initialized
            || (self.tracks_session_state && !self.session_idle)
        {
            return Err("Wait for Claude Code to finish reconnecting before editing.".into());
        }
        let args = launch_options(&self.args)?.0;
        let item = &self.state.saved.items[index];
        if item["id"] != message_id {
            return Err("This message changed; select Edit again.".into());
        }
        let text = provider::item_text(item);
        let images = item["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "image")
            .filter_map(|part| part["url"].as_str().map(str::to_owned))
            .collect();
        if rewind && restore_files && self.restore_reservation.is_none() {
            let roots = checkpoint
                .files
                .as_ref()
                .ok_or("The workspace checkpoint is missing.")?
                .roots();
            self.restore_reservation = Some(crate::checkpoint::acquire_restore(&roots)?);
        }
        let mut roots = vec![std::path::PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| std::path::PathBuf::from(&root.wt_path)),
        );
        let recovery = if restore_files {
            Some(crate::checkpoint::capture(
                &crate::checkpoint::repository_roots(&roots)?,
            )?)
        } else {
            None
        };
        self.state.saved.pending_edit = Some(PendingEdit {
            target: checkpoint.start_item,
            turn_id: checkpoint.turn_id.ok_or("Missing human turn identity")?,
            text,
            images,
            recovery,
            rewind,
            composer_draft,
            claude_fork_id: Some(nonce()),
        });
        // Persist the complete original transcript and recovery snapshot before
        // touching files. Fork failure cannot destroy the original conversation.
        self.persistence.flush(&self.state.saved)?;
        if restore_files {
            if let Err(error) = checkpoint
                .files
                .as_ref()
                .ok_or("The workspace checkpoint is missing.")?
                .restore()
            {
                self.recover_edit()?;
                return Err(error);
            }
        }
        self.process.take();
        self.pid.store(0, Ordering::SeqCst);
        let process = match spawn(&args, &self.state.saved) {
            Ok(process) => process,
            Err(error) => {
                self.state.status = Status::Disconnected;
                self.initialized = false;
                self.recover_edit()?;
                return Err(format!(
                    "Claude rewind failed; reconnect to continue the original conversation: {error}"
                ));
            }
        };
        self.pid.store(process.child.id(), Ordering::SeqCst);
        self.process = Some(process);
        self.pending.clear();
        self.state.context_query_pending = false;
        self.state.context_revision = self.state.context_revision.wrapping_add(1);
        self.permission_changes.clear();
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
        if let Err(error) = self.control(json!({"subtype":"initialize","hooks":null})) {
            self.process.take();
            self.pid.store(0, Ordering::SeqCst);
            self.state.status = Status::Disconnected;
            return Err(format!(
                "Claude rewind startup is uncertain. Reconnect to recover the original conversation: {error}"
            ));
        }
        Ok(())
    }
    fn recover_edit(&mut self) -> Result<(), String> {
        if let Some(recovery) = self
            .state
            .saved
            .pending_edit
            .as_ref()
            .and_then(|edit| edit.recovery.as_ref())
        {
            if let Err(error) = recovery.restore() {
                self.state.status = Status::Disconnected;
                return Err(format!(
                    "Workspace recovery needs attention. Reconnect to retry: {error}"
                ));
            }
        }
        let mut saved = self.state.saved.clone();
        saved.pending_edit = None;
        if let Err(error) = self.persistence.flush(&saved) {
            self.state.status = Status::Disconnected;
            return Err(format!(
                "Edit recovery could not be saved. Reconnect to reconcile: {error}"
            ));
        }
        self.state.saved = saved;
        self.restore_reservation = None;
        Ok(())
    }
    fn finish_edit(&mut self) -> Result<(), String> {
        let Some(edit) = self.state.saved.pending_edit.clone() else {
            return Ok(());
        };
        let mut saved = self.state.saved.clone();
        saved.claude_resume = Some(provider::ClaudeResume {
            original_session: saved.claude_resume.as_ref().map_or_else(
                || saved.thread_id.clone(),
                |resume| resume.original_session.clone(),
            ),
            assistant_uuid: saved
                .items
                .iter()
                .skip(edit.target)
                .find(|item| item["type"] == "userMessage")
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
        saved.session_review.pending = None;
        saved.session_review.last_completed_period = None;
        saved.session_review.reviewed_through =
            saved.session_review.reviewed_through.min(edit.target);
        saved.session_review.summary.clear();
        saved.retain_claude_subagents();
        if edit.rewind {
            saved.finish_rewind_draft(edit.text, edit.images);
        } else {
            let (text, images) = edit.composer_draft.unwrap_or((edit.text, edit.images));
            saved.draft = text;
            saved.draft_images = images;
        }
        saved.pending_edit = None;
        saved.context = None;
        saved.context_details = None;
        if let Err(error) = self.persistence.flush(&saved) {
            self.state.status = Status::Disconnected;
            return Err(format!(
                "Edit recovery could not be saved. Reconnect to reconcile: {error}"
            ));
        }
        self.state.saved = saved;
        self.restore_reservation = None;
        self.subagent_tasks
            .retain(|_, id| self.state.saved.claude_subagents.contains_key(id));
        self.child_transcripts
            .retain(|id, _| self.state.saved.claude_subagents.contains_key(id));
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
        if crate::connections::credentials_busy(crate::agent::Agent::Claude) {
            return Err("Wait for Claude authentication to finish before starting a turn.".into());
        }
        self.state.context_revision = self.state.context_revision.wrapping_add(1);
        provider::validate_prompt(&text, &images)?;
        if !self.initialized || self.state.status.busy() || self.deferred.is_some() {
            return Err(
                "Claude Code is busy; queue this message to send it after the current turn".into(),
            );
        }
        if self.state.queued_prompts.iter().any(|p| p.id == id)
            && !self.state.queue_sending.contains(&id)
        {
            self.state.queue_sending.push(id);
        }
        if !self.permission_changes.is_empty() {
            // Keep an explicitly submitted prompt behind the permission ACK;
            // selecting a mode alone never starts a turn.
            self.deferred = Some(QueuedPrompt { id, text, images });
            return Ok(());
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
        if let Err(error) = self.state.saved.rich_plans.validate_dispatch(id) {
            self.state.queued_prompts.retain(|p| p.id != id);
            self.state.queue_sending.retain(|held| *held != id);
            return Err(error);
        }
        self.capture_edit_checkpoint();
        let mut content = user_content(&text, &images);
        // Slash command arguments belong to Claude Code. Appending host instructions
        // here changes their meaning (including the host-generated /compact).
        if !self.side_chat && !text.trim_start().starts_with('/') {
            content.push(json!({"type":"text", "text":crate::review_brief::context(&self.state.saved.session_review)}));
        }
        let uuid = nonce();
        self.write(json!({"type":"user","uuid":uuid,"session_id":self.state.saved.thread_id.as_deref().unwrap_or(""),"message":{"role":"user","content":content},"parent_tool_use_id":null}))?;
        if !self.side_chat {
            crate::session_title::Generator::anticipate_initial(&mut self.state, &text);
        }
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
        self.state.clear_error();
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
            self.state.accept_submission(prompt.id);
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
        if let Some(uuid) = uuid.as_str() {
            self.state.submission_items.insert(prompt.id, uuid.into());
        }
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
        self.state.accept_submission(prompt.id);
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
            return Err(provider::CLEAR_BUSY.into());
        }
        if self.process.is_none() {
            return Err(provider::CLEAR_DISCONNECTED.into());
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
        self.state.context_query_pending = false;
        self.state.context_revision = self.state.context_revision.wrapping_add(1);
        self.permission_changes.clear();
        self.blocks.clear();
        self.message_id.clear();
        self.subagent_tasks.clear();
        self.child_transcripts.clear();
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
        self.deliver_plan(plan_id, revision_id, false, persist_saved)
    }
    fn deliver_plan(
        &mut self,
        plan_id: &str,
        revision_id: &str,
        approve: bool,
        persist_saved: impl Fn(&SavedSession) -> Result<(), String>,
    ) -> Result<(), String> {
        self.persistence.drain()?;
        if crate::connections::credentials_busy(crate::agent::Agent::Claude) {
            return Err(
                "Wait for provider authentication to finish before sending plan actions.".into(),
            );
        }

        if self.process.is_none() || !self.initialized {
            return Err("Reconnect before sending plan feedback".into());
        }
        let prepared = if approve {
            self.state
                .saved
                .rich_plans
                .prepare_approval(plan_id, revision_id)?
        } else {
            self.state
                .saved
                .rich_plans
                .prepare_delivery(plan_id, revision_id)?
        };
        if let Some((id, text)) = prepared {
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
}

#[cfg(test)]
mod tests;
