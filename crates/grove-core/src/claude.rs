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
    // A session overlay must preserve all caller-provided settings. Never write
    // the global fastMode preference. New/explicit sessions launch Standard;
    // a saved Fast preference is applied through the live catalog-gated control
    // before the next user turn.
    if saved.composer.speed.is_some() {
        let mut settings = serde_json::Map::new();
        let mut remaining = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let source = if args[i] == "--settings" {
                i += 1;
                Some(
                    args.get(i)
                        .ok_or("Claude Code --settings needs a value")?
                        .clone(),
                )
            } else {
                args[i].strip_prefix("--settings=").map(str::to_owned)
            };
            if let Some(source) = source {
                let text = if source.trim_start().starts_with('{') {
                    source
                } else {
                    fs_err::read_to_string(std::path::Path::new(&saved.cwd).join(&source))
                        .map_err(|e| format!("Cannot read Claude settings: {e}"))?
                };
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("Invalid Claude settings: {e}"))?;
                let object = value
                    .as_object()
                    .ok_or("Claude settings must be an object")?;
                settings.clone_from(object);
            } else {
                remaining.push(args[i].clone());
            }
            i += 1;
        }
        settings.insert("fastMode".into(), json!(false));
        remaining.extend(["--settings".into(), Value::Object(settings).to_string()]);
        args = remaining;
    }
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
            // Editing intentionally drops the selected prompt and every later
            // turn in a new session. --resume-drops-turn only allows one turn
            // and rejects middle-of-chat edits; the original is kept by forking.
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
fn permission_modes_from_help(help: &str) -> Vec<String> {
    let Some(section) = help.split("--permission-mode <mode>").nth(1) else {
        return Vec::new();
    };
    let Some(choices) = section
        .split("(choices:")
        .nth(1)
        .and_then(|v| v.split(')').next())
    else {
        return Vec::new();
    };
    choices
        .split(',')
        .filter_map(|value| {
            let mode = value.trim().trim_matches('"');
            [
                "default",
                "manual",
                "acceptEdits",
                "plan",
                "dontAsk",
                "bypassPermissions",
                "auto",
            ]
            .contains(&mode)
            .then(|| mode.to_owned())
        })
        .collect()
}
fn discover_permission_modes(
    mut command: Command,
    stopping: &AtomicBool,
    timeout: Duration,
) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut output) = tempfile::tempfile() else {
        return Vec::new();
    };
    let Ok(writer) = output.try_clone() else {
        return Vec::new();
    };
    command
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let Ok(mut child) = command.spawn() else {
        return Vec::new();
    };
    let started = Instant::now();
    let success = loop {
        if stopping.load(Ordering::SeqCst)
            || started.elapsed() >= timeout
            || output.metadata().map_or(true, |v| v.len() > 128 * 1024)
        {
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break false,
        }
    };
    // Reap the owned helper and its group even if a CLI shim left descendants.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    if !success || output.seek(SeekFrom::Start(0)).is_err() {
        return Vec::new();
    }
    let mut help = String::new();
    if output.take(128 * 1024).read_to_string(&mut help).is_err() {
        return Vec::new();
    }
    permission_modes_from_help(&help)
}
fn available_permission_modes(stopping: &AtomicBool) -> Vec<String> {
    let (program, args) = crate::agent::Agent::Claude.session_invocation(&["--help".into()]);
    let mut command = Command::new(program);
    command.args(args);
    if let Some(env) = crate::env_path::session_environment() {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    discover_permission_modes(command, stopping, Duration::from_secs(2))
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

/// Bypass is a process launch capability, not a permission the live picker can grant.
pub fn can_select_permission_mode(saved: &SavedSession, mode: &str) -> bool {
    if mode != "bypassPermissions" {
        return true;
    }
    let Ok((options, _)) = launch_options(&saved.config_args) else {
        return false;
    };
    let mut index = 0;
    while let Some(option) = options.get(index) {
        match option.as_str() {
            "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions" => {
                return true
            }
            "--strict-mcp-config" | "--disable-slash-commands" | "--chrome" | "--no-chrome" => {
                index += 1;
            }
            _ => index += 2, // The remaining accepted launch options take a value.
        }
    }
    false
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
#[derive(Default)]
struct ChildTranscript {
    transcript: crate::claude_transcript::ClaudeTranscriptState,
    items: Vec<Value>,
    message_id: String,
    blocks: HashMap<u64, String>,
}
impl ChildTranscript {
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
                self.items
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
    fn upsert(&mut self, item: Value) {
        if let Some(old) = self.items.iter_mut().find(|old| old["id"] == item["id"]) {
            *old = item;
        } else {
            self.items.push(item);
        }
    }
    fn report(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(item) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| item["type"] == "agentMessage" && item["text"].as_str() == Some(text))
        {
            item["phase"] = json!("final_answer");
        } else {
            self.upsert(json!({"id":format!("child-final-report-{}", self.items.len()),"type":"agentMessage","phase":"final_answer","text":text}));
        }
    }
    fn block(&mut self, id: &str, block: &Value, cwd: &str) {
        match block["type"].as_str() {
            Some("text") => {
                self.upsert(json!({"id":id,"type":"agentMessage","text":block["text"]}));
            }
            Some("tool_use") => {
                let id = block["id"].as_str().unwrap_or(id);
                let mut item = if block["name"] == "Bash" {
                    json!({"id":id,"type":"commandExecution","command":block["input"]["command"],"cwd":cwd,"status":"inProgress","aggregatedOutput":""})
                } else {
                    json!({"id":id,"type":"mcpToolCall","server":"Claude Code","tool":block["name"],"arguments":block["input"],"status":"inProgress"})
                };
                if let Some(old) = self.items.iter().find(|old| old["id"] == id) {
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
            _ => {} // Private reasoning is never a child transcript message.
        }
    }
    fn message(&mut self, value: &Value, cwd: &str) {
        match value["type"].as_str() {
            Some("assistant") => {
                let message = value["message"]["id"]
                    .as_str()
                    .or(value["uuid"].as_str())
                    .unwrap_or(&self.message_id)
                    .to_owned();
                if let Some(blocks) = value["message"]["content"].as_array() {
                    let mut claimed = Vec::new();
                    for (index, block) in blocks.iter().enumerate() {
                        let id = self.assistant_block_id(&message, index, block, &claimed);
                        self.block(&id, block, cwd);
                        claimed.push(id);
                    }
                }
            }
            Some("stream_event") => {
                let event = &value["event"];
                let index = event["index"].as_u64().unwrap_or(0);
                match event["type"].as_str() {
                    Some("message_start") => {
                        self.message_id = event["message"]["id"]
                            .as_str()
                            .unwrap_or("child-stream")
                            .into();
                        self.blocks.clear();
                    }
                    Some("content_block_start") => {
                        let block = &event["content_block"];
                        let id = block["id"]
                            .as_str()
                            .map_or_else(|| format!("{}-{index}", self.message_id), str::to_owned);
                        self.block(&id, block, cwd);
                        self.blocks.insert(index, id);
                    }
                    Some("content_block_delta") => {
                        if let Some(text) = event["delta"]["text"].as_str() {
                            if let Some(item) = self
                                .blocks
                                .get(&index)
                                .and_then(|id| self.items.iter_mut().find(|item| item["id"] == *id))
                            {
                                let mut full = item["text"].as_str().unwrap_or_default().to_owned();
                                full.push_str(text);
                                item["text"] = json!(full);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("user") => {
                let content = &value["message"]["content"];
                let blocks = content.as_array().cloned().unwrap_or_else(|| {
                    content
                        .as_str()
                        .map(|text| vec![json!({"type":"text","text":text})])
                        .unwrap_or_default()
                });
                let visible: Vec<Value> = blocks
                    .iter()
                    .filter(|block| {
                        block["type"] != "tool_result"
                            && block["type"] != "thinking"
                            && block["type"] != "redacted_thinking"
                    })
                    .cloned()
                    .collect();
                if !visible.is_empty() {
                    let id = value["uuid"]
                        .as_str()
                        .or(value["message"]["id"].as_str())
                        .map_or_else(|| format!("child-user-{}", self.items.len()), str::to_owned);
                    self.upsert(json!({"id":id,"type":"userMessage","content":visible}));
                }
                for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                    if let Some(item) = self
                        .items
                        .iter_mut()
                        .find(|item| item["id"] == block["tool_use_id"])
                    {
                        self.transcript.apply_tool_result(
                            item,
                            &block["content"],
                            value.get("tool_use_result"),
                            block["is_error"] == true,
                        );
                        if item["type"] == "commandExecution" {
                            item["aggregatedOutput"] =
                                json!(block["content"].as_str().map_or_else(
                                    || block["content"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|part| part["text"].as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    str::to_owned
                                ));
                        }
                    }
                }
            }
            Some("tool_progress") => {
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|item| item["id"] == value["tool_use_id"])
                {
                    item["progress"] =
                        json!(format!("Running for {}s", value["elapsed_time_seconds"]));
                }
            }
            Some("result") => {
                if let Some(text) = value["result"].as_str() {
                    self.report(text);
                }

                for item in &mut self.items {
                    if item["status"] == "inProgress" && item["detached"] != true {
                        item["status"] = json!(if value["is_error"] == true {
                            "failed"
                        } else {
                            "completed"
                        });
                    }
                }
            }
            _ => {}
        }
    }
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
    blocks: HashMap<u64, String>,
    subagent_tasks: HashMap<String, String>,
    child_transcripts: HashMap<String, ChildTranscript>,
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
            .map(|(id, child)| {
                let mut transcript = ChildTranscript {
                    items: child.items.clone(),
                    ..Default::default()
                };
                transcript.transcript.restore(&mut transcript.items);
                (id.clone(), transcript)
            })
            .collect();
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
            blocks: HashMap::new(),
            subagent_tasks,
            child_transcripts,
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
            && self.permission_changes.is_empty()
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
        self.state.status = Status::Disconnected;
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
        if request["subtype"] == "set_permission_mode" {
            if let Some(mode) = request["mode"].as_str() {
                self.permission_changes
                    .insert(id.clone(), (mode.into(), self.permission_mode_revision));
            }
        }
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
        self.state.permission_modes = available_permission_modes(&self.stopping);
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
        self.state.interaction_epoch = codex::next_interaction_epoch();
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
            settings["fastMode"] =
                json!(speed == codex::Speed::Fast && self.state.speed_unavailable(speed).is_none());
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
        saved.retain_claude_subagents();
        let (text, images) = edit.composer_draft.unwrap_or((edit.text, edit.images));
        saved.draft = text;
        saved.draft_images = images;
        saved.pending_edit = None;
        saved.context = None;
        codex::persist(&saved)?;
        self.state.saved = saved;
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
        codex::validate_prompt(&text, &images)?;
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
    fn action(&mut self, action: Action) -> Result<(), String> {
        if self.state.saved.pending_edit.is_some()
            && !matches!(
                action,
                Action::Reconnect
                    | Action::Stop { .. }
                    | Action::Draft(_)
                    | Action::DraftImages(_)
                    | Action::DraftReference(_)
            )
        {
            return Err("An interrupted edit needs recovery. Reconnect before continuing.".into());
        }
        match action {
            Action::Interaction { epoch, action } => {
                if epoch != self.state.interaction_epoch {
                    return Err("This interaction belongs to a previous connection".into());
                }
                return self.action(*action);
            }
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
            Action::ApprovePlan {
                plan_id,
                revision_id,
            } => self.deliver_plan(&plan_id, &revision_id, true, codex::persist)?,
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
            Action::ProjectPrompt { id, text } => {
                codex::validate_prompt(&text, &[])?;
                if self.state.status == Status::Disconnected {
                    self.state.rejected_submission = Some(id);
                    return Err("Reconnect before sending a prompt".into());
                }
                self.state.queued_prompts.push(QueuedPrompt {
                    id,
                    text,
                    images: Vec::new(),
                });
                self.state.accepted_submission = Some(id);
            }
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
            Action::DraftReference(reference) => {
                if let Some(reference) = &reference {
                    codex::validate_prompt(
                        &reference.serialize(&self.state.saved.draft),
                        &self.state.saved.draft_images,
                    )?;
                }
                self.state.saved.draft_reference = reference;
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
                let configure_model_effort = model.is_some() || effort.is_some();
                if let Some(effort) = &effort {
                    if !["low", "medium", "high", "xhigh", "max"].contains(&effort.as_str()) {
                        return Err(format!("Unsupported Claude Code effort: {effort}"));
                    }
                }
                if let Some(access) = &access {
                    let mode = permission_mode(access)?;
                    if !can_select_permission_mode(&self.state.saved, mode) {
                        return Err("Bypass permissions is unavailable for this Claude Code session because it was launched without bypass permission support".into());
                    }
                    if !self.state.permission_modes.is_empty()
                        && !self
                            .state
                            .permission_modes
                            .iter()
                            .any(|value| value == mode)
                    {
                        return Err(format!(
                            "This Claude Code version does not advertise permission mode {mode}"
                        ));
                    }
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
                    self.pending_access = Some(permission_mode(&access)?.into());
                }
                self.state.saved.composer.reset_to_configured = false;
                self.reconfigure |= configure_model_effort;
                self.configure_pending_access()?;
            }

            Action::SetSpeed(speed) => {
                if let Some(reason) = self.state.speed_unavailable(speed) {
                    return Err(reason.into());
                }
                self.state.saved.composer.speed = Some(speed);
                self.reconfigure = true;
            }
            Action::ResetModelEffort => {
                // Restarting is required to remove a live model override. Preserve
                // inherited permissions explicitly so that restart cannot reset them.
                if self.state.access.is_none() {
                    self.state.access = self.state.resolved_access.clone();
                }
                self.state.saved.composer.model = None;
                self.state.saved.composer.effort = None;
                self.state.saved.composer.access = self.state.access.clone();
                self.state.model = None;
                self.state.effort = None;
                self.state.resolved_model = None;
                self.state.resolved_effort = None;
                self.reset_configuration = true;
                self.reconfigure = true;
            }
            Action::ResetConfiguration => {
                self.pending_access = None;
                self.permission_changes.clear();
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
                let resolved = self.state.questions[at].id.clone();
                self.state.retire_interaction(&resolved);
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
                let resolved = self.state.approvals[at]["id"].clone();
                self.state.retire_interaction(&resolved);
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
                let resolved = self.state.elicitations[at].id.clone();
                self.state.retire_interaction(&resolved);
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
        self.register_subagent_from(block, None);
    }
    fn register_subagent_from(&mut self, block: &Value, parent: Option<&str>) {
        if !matches!(block["name"].as_str(), Some("Agent" | "Task")) {
            return;
        }
        let Some(id) = block["id"].as_str() else {
            return;
        };
        let root = self
            .state
            .saved
            .thread_id
            .as_deref()
            .unwrap_or(&self.state.saved.key);
        let owner = parent.unwrap_or(root);
        if owner == root || self.interaction_children.known(owner) {
            self.interaction_children.register(
                root,
                owner,
                id,
                block["input"]["name"]
                    .as_str()
                    .or(block["input"]["subagent_type"].as_str()),
                None,
            );
            self.interaction_children.start(id, None);
        }
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
        let canonical = self
            .subagent_tasks
            .get(parent)
            .cloned()
            .unwrap_or_else(|| parent.into());
        let parent = canonical.as_str();
        // Child frames must never reach the primary transcript, usage, or turn lifecycle.
        self.state
            .upsert_subagent(parent, None, None, SubagentStatus::Unknown);
        self.subagent_system_message(value);
        let child = self.child_transcripts.entry(parent.to_owned()).or_default();
        child.message(value, &self.state.saved.cwd);
        for item in &child.items {
            self.state.upsert_subagent_item(parent, item.clone());
        }
        if value["type"] == "result" {
            if let Some(report) = value["result"].as_str() {
                self.subagent_report(parent, report);
            }
        }
        if value["type"] == "result" {
            if let Some(expired) = self.interaction_children.finish(parent, None) {
                self.state.expire_interactions(&expired);
                self.state.refresh_interaction_status();
            }
        }
        match value["type"].as_str() {
            Some("assistant") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for (index, block) in blocks.iter().enumerate() {
                        let key = value["message"]["id"]
                            .as_str()
                            .or_else(|| value["uuid"].as_str())
                            .map(|id| format!("{id}:{index}"));
                        self.subagent_block_with_key(parent, block, key.as_deref(), true);
                    }
                }
            }
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_start" {
                    self.subagent_block_with_key(parent, &event["content_block"], None, false);
                }
            }
            Some("user") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for block in blocks {
                        self.subagent_tool_result(block, value.get("tool_use_result"));
                    }
                }
            }
            Some("result") if !self.child_interrupted(parent) => self.state.upsert_subagent(
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
    #[cfg(test)]
    fn subagent_block(&mut self, parent: &str, block: &Value) {
        self.subagent_block_with_key(parent, block, None, true);
    }
    fn subagent_block_with_key(
        &mut self,
        parent: &str,
        block: &Value,
        event_key: Option<&str>,
        observe: bool,
    ) {
        if observe {
            if let Some(text) = match block["type"].as_str() {
                Some("tool_use") => block["name"]
                    .as_str()
                    .map(|name| format!("Using {name}\n{}", block["input"])),
                Some("text") => block["text"].as_str().map(str::to_owned),
                _ => None,
            } {
                let kind = if block["type"] == "text" {
                    crate::codex::SubagentActivityKind::Commentary
                } else {
                    crate::codex::SubagentActivityKind::Tool
                };
                let key = block["id"]
                    .as_str()
                    .or(event_key)
                    .map_or_else(|| format!("text:{text}"), str::to_owned);
                self.state.observe_subagent(parent, &key, kind, &text);
            }
        }
        match block["type"].as_str() {
            Some("tool_use") => {
                self.register_subagent_from(block, Some(parent));
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
    fn subagent_report(&mut self, id: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let child = self.child_transcripts.entry(id.into()).or_default();
        child.report(text);
        for item in &child.items {
            self.state.upsert_subagent_item(id, item.clone());
        }
        self.state.observe_subagent(
            id,
            &format!("child-final-report:{text}"),
            crate::codex::SubagentActivityKind::Commentary,
            text,
        );
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
        if let Some(report) =
            structured.and_then(|result| result["handbackReport"]["text"].as_str())
        {
            self.subagent_report(id, report);
        }
        if let Some(expired) = self.interaction_children.finish(id, None) {
            self.state.expire_interactions(&expired);
            self.state.refresh_interaction_status();
        }
        if self.child_interrupted(id) {
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
    fn child_interrupted(&self, id: &str) -> bool {
        self.state
            .saved
            .work_periods
            .iter()
            .flat_map(|period| &period.subagents)
            .any(|agent| agent.id == id && agent.status == SubagentStatus::Interrupted)
    }
    fn subagent_system_message(&mut self, value: &Value) {
        if value["type"] != "system" {
            return;
        }
        let task = value["task_id"].as_str();
        let observed_id = value["tool_use_id"]
            .as_str()
            .or_else(|| task.and_then(|task| self.subagent_tasks.get(task).map(String::as_str)))
            .or(task)
            .map(str::to_owned);
        if let Some(id) = &observed_id {
            let known = self
                .state
                .saved
                .work_periods
                .iter()
                .any(|period| period.subagents.iter().any(|agent| agent.id == *id));
            if known || value["task_type"] == "local_agent" || value["subagent_type"].is_string() {
                let child = self
                    .state
                    .saved
                    .claude_subagents
                    .entry(id.clone())
                    .or_default();
                if let Some(run) = value["run_id"].as_str() {
                    // A Claude Agent tool invocation has one run. A delayed
                    // lifecycle frame must not settle a different observed run.
                    if child
                        .run_id
                        .as_deref()
                        .is_some_and(|current| current != run)
                    {
                        return;
                    }
                    child.run_id = Some(run.into());
                }
                if let Some(task) = task {
                    if !child.task_ids.iter().any(|old| old == task) {
                        child.task_ids.push(task.into());
                    }
                }
            }
        }
        if let Some(task) = task {
            let root = self
                .state
                .saved
                .thread_id
                .as_deref()
                .unwrap_or(&self.state.saved.key);
            if value["subtype"] == "task_started"
                && (value["task_type"] == "local_agent"
                    || value["subagent_type"].is_string()
                    || value["tool_use_id"]
                        .as_str()
                        .is_some_and(|id| self.interaction_children.known(id)))
            {
                let parent = value["parent_task_id"].as_str().unwrap_or(root);
                if self.interaction_children.claude_task(
                    root,
                    parent,
                    task,
                    value["tool_use_id"].as_str(),
                    value["subagent_type"].as_str(),
                ) {
                    self.interaction_children
                        .start(task, value["run_id"].as_str());
                }
            }
            if value["subtype"] == "task_notification"
                || (value["subtype"] == "task_updated"
                    && matches!(
                        value["patch"]["status"].as_str(),
                        Some("completed" | "failed" | "killed")
                    ))
            {
                if let Some(expired) = self
                    .interaction_children
                    .finish(task, value["run_id"].as_str())
                {
                    self.state.expire_interactions(&expired);
                    self.state.refresh_interaction_status();
                }
            }
        }
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
        let mut status = match value["subtype"].as_str() {
            Some("task_started") => SubagentStatus::Working,
            Some("task_progress") => SubagentStatus::Unknown,
            Some("task_notification" | "task_updated") => match value["status"]
                .as_str()
                .or(value["patch"]["status"].as_str())
            {
                Some("completed") => SubagentStatus::Completed,
                Some("failed") => SubagentStatus::Failed,
                Some("stopped" | "killed" | "cancelled" | "canceled" | "interrupted") => {
                    SubagentStatus::Interrupted
                }
                _ => SubagentStatus::Unknown,
            },
            _ => return,
        };
        if self.child_interrupted(&id) {
            status = SubagentStatus::Interrupted;
        }
        let activity = value["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| value["description"].as_str());
        self.state
            .upsert_subagent(&id, value["subagent_type"].as_str(), activity, status);
        self.state.set_subagent_metadata(
            &id,
            value["subagent_type"].as_str(),
            (value["subtype"] == "task_started")
                .then(|| value["description"].as_str())
                .flatten(),
        );
        if value["subtype"] == "task_notification" {
            if let Some(report) = value["handback_report"]["text"]
                .as_str()
                .or(value["handbackReport"]["text"].as_str())
            {
                self.subagent_report(&id, report);
            }
        }
    }
    fn control_owner(&self, value: &Value) -> Option<crate::interaction::InteractionOwner> {
        let agent = value["request"]["agent_id"].as_str();
        let parent = value["parent_tool_use_id"].as_str();
        if let Some(id) = agent.or(parent) {
            let owner = self.interaction_children.owner(id, None)?;
            if parent
                .is_some_and(|parent| self.interaction_children.canonical(parent) != owner.thread)
            {
                return None;
            }
            return Some(owner);
        }
        if !self.state.status.busy() || self.interrupting {
            return None;
        }
        Some(crate::interaction::InteractionOwner {
            thread: self.state.saved.thread_id.clone().unwrap_or_default(),
            turn: self.state.turn_id.clone(),
            agent: None,
        })
    }
    fn effective_permission_mode(&mut self, mode: &str) {
        self.permission_mode_revision += 1;
        self.state.resolved_access = Some(mode.into());
        if self.state.access.is_some() {
            self.state.access = Some(mode.into());
            self.state.saved.composer.access = Some(mode.into());
        }
    }
    fn message(&mut self, value: Value) -> Result<(), String> {
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
        if !matches!(
            value["type"].as_str(),
            Some("control_request" | "control_cancel_request")
        ) {
            if let Some(parent) = value["parent_tool_use_id"].as_str() {
                self.subagent_message(parent, &value);
                return Ok(());
            }
        }
        // These fields describe actual provider state, unlike a settings ACK.
        if let Some(speed) = value["fast_mode_state"].as_str() {
            self.state.reported_speed = Some(speed.into());
            self.state.speed_disabled_reason = value["fast_mode_disabled_reason"]
                .as_str()
                .map(str::to_owned);
        }
        self.subagent_system_message(&value);
        match value["type"].as_str().unwrap_or_default() {
            "control_response" => {
                let response = &value["response"];
                let permission_change = response["request_id"]
                    .as_str()
                    .and_then(|id| self.permission_changes.remove(id));
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
                } else if response["subtype"] == "error"
                    && pending.as_deref() == Some("set_permission_mode")
                {
                    // A startup selection is sent before the launch prompt is
                    // drained into `deferred`. Cancel it on rejection as well;
                    // it must never run under the previous permission mode.
                    self.initial = None;
                    if let Some(prompt) = self.deferred.take() {
                        self.state.rejected_submission = Some(prompt.id);
                        self.state.queue_sending.retain(|id| *id != prompt.id);
                    }
                    self.queue_ready = false;
                    let error = response["error"]
                        .as_str()
                        .unwrap_or("Claude Code rejected the permission mode")
                        .to_owned();
                    self.state.error = Some(error.clone());
                    self.permission_error = Some(error);
                } else if response["subtype"] == "error" {
                    return Err(response["error"]
                        .as_str()
                        .unwrap_or("Claude Code rejected a control request")
                        .into());
                }
                if response["subtype"] == "success"
                    && pending.as_deref() == Some("set_permission_mode")
                {
                    if let Some((requested, revision)) = permission_change {
                        // Status can precede its control ACK. Keep its authoritative
                        // effective mode while persisting the acknowledged explicit choice.
                        let access = if revision == self.permission_mode_revision {
                            requested
                        } else {
                            self.state.resolved_access.clone().unwrap_or(requested)
                        };
                        self.state.resolved_access = Some(access.clone());
                        self.state.access = Some(access.clone());
                        self.state.saved.composer.access = Some(access);
                        if self
                            .permission_error
                            .take()
                            .is_some_and(|error| self.state.error.as_ref() == Some(&error))
                        {
                            self.state.clear_error();
                        }
                    }
                }
                if self.configuring && self.pending.is_empty() {
                    self.configuring = false;
                    self.state.status = Status::Idle;
                    if self.state.model.is_some() {
                        self.state.resolved_model = self.state.model.clone();
                    }

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
                    if let Some(mode) = data["current_permission_mode"].as_str() {
                        self.effective_permission_mode(mode);
                    }
                    self.state.resolved_model = data["models"]
                        .as_array()
                        .and_then(|models| models.iter().find(|m| m["value"] == "default"))
                        .and_then(|m| m["resolvedModel"].as_str())
                        .map(str::to_owned);

                    if let Some(models) = data["models"].as_array() {
                        self.state.models=models.iter().map(|m| json!({"model":m["value"],"resolvedModel":m["resolvedModel"],"displayName":m["displayName"],"description":m["description"],"supportsFastMode":m["supportsFastMode"],"supportedReasoningEfforts":m["supportedEffortLevels"].as_array().map(|values| values.iter().map(|v| json!({"reasoningEffort":v})).collect::<Vec<_>>()).unwrap_or_default()})).collect();
                    }
                    if self.state.saved.composer.speed == Some(codex::Speed::Fast) {
                        self.reconfigure = true;
                    }
                    if let Some(speed) = data["fast_mode_state"].as_str() {
                        self.state.reported_speed = Some(speed.into());
                        self.state.speed_disabled_reason = data["fast_mode_disabled_reason"]
                            .as_str()
                            .map(str::to_owned);
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
                            let root = self.state.saved.thread_id.clone().unwrap_or_default();
                            self.state.expire_interactions(&root);
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
                if let Some(mode) = value["permissionMode"].as_str() {
                    // A later provider status supersedes even an in-flight ACK.
                    self.effective_permission_mode(mode);
                }
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
                if let Some(mode) = value["permissionMode"].as_str() {
                    self.effective_permission_mode(mode);
                }
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
                let owner = self.control_owner(&value);
                if owner.is_none()
                    && matches!(
                        request["subtype"].as_str(),
                        Some("can_use_tool" | "elicitation")
                    )
                {
                    return self.respond(id, if request["subtype"] == "elicitation" {json!({"action":"cancel"})} else {json!({"behavior":"deny","message":"This request has expired or its agent is not part of this session."})});
                }
                if request["subtype"] == "elicitation" {
                    if self.state.elicitations.iter().any(|r| r.id == id) {
                        return Ok(());
                    }
                    match crate::elicitation::ElicitationRequest::parse_claude(id.clone(), request)
                    {
                        Ok(r) if owner.is_some() && !self.interrupting => {
                            if let Some(owner) = owner {
                                self.state.interaction_owners.insert(id.to_string(), owner);
                            }
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
                if owner.is_none() || self.interrupting {
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
                        Ok(question) if owner.is_some() && !self.interrupting => {
                            if let Some(owner)=owner {self.state.interaction_owners.insert(id.to_string(),owner);}
                            self.state.questions.push(question);
                            self.state.status = Status::Waiting;
                            return Ok(());
                        }
                        _ => return self.respond(id, json!({"behavior":"deny","message":"Invalid or expired question request."})),
                    }
                }
                let agent = owner.as_ref().and_then(|owner| owner.agent.clone());
                if let Some(owner) = owner {
                    self.state.interaction_owners.insert(id.to_string(), owner);
                }
                self.state.approvals.push(json!({"groveAgent":agent,"id":id,"method":"item/commandExecution/requestApproval","claudeInput":request["input"],"claudeRequest":request,"params":{"command":format!("{} {}", request["tool_name"].as_str().unwrap_or("Tool"), request["input"]),"cwd":self.state.saved.cwd,"reason":request["decision_reason"].as_str().unwrap_or("Claude Code requests permission for this action."),"itemId":request["tool_use_id"]}}));
                self.state.status = Status::Waiting;
            }
            "control_cancel_request" => {
                if let Some(parent) = value["parent_tool_use_id"]
                    .as_str()
                    .or(value["agent_id"].as_str())
                {
                    if self
                        .state
                        .interaction_owners
                        .get(&value["request_id"].to_string())
                        .is_none_or(|owner| {
                            self.interaction_children.canonical(parent) != owner.thread
                        })
                    {
                        return Ok(());
                    }
                }
                self.state
                    .interaction_owners
                    .remove(&value["request_id"].to_string());
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
            "rate_limit_event" => {
                let info = &value["rate_limit_info"];
                match info["status"].as_str() {
                    Some("rejected") => self.state.set_provider_error(
                        "Claude",
                        "Claude usage limit reached.".into(),
                        false,
                        Some(
                            if matches!(
                                info["rateLimitType"].as_str(),
                                Some(
                                    "five_hour"
                                        | "seven_day"
                                        | "seven_day_opus"
                                        | "seven_day_sonnet"
                                        | "overage"
                                )
                            ) {
                                "usage_limit_reached"
                            } else {
                                "rate_limit_error"
                            },
                        ),
                        info["resetsAt"].as_i64().or(info["resets_at"].as_i64()),
                    ),
                    Some("allowed_warning") => self.state.provider_warning(
                        "Approaching usage limit",
                        "Claude reports that usage is approaching a limit. Work can continue.",
                    ),
                    Some("allowed")
                        if self.state.effective_notice().is_some_and(|notice| {
                            notice.provider == "Claude"
                                && (notice.kind == codex::SessionNoticeKind::UsageLimit
                                    || notice.reset_at.is_some())
                        }) =>
                    {
                        self.state.clear_error();
                    }
                    _ => {}
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
                        if block["type"] == "text" {
                            if let Some(item) = self
                                .state
                                .saved
                                .items
                                .iter_mut()
                                .find(|item| item["id"] == item_id)
                            {
                                item["groveCompleted"] = json!(true);
                            }
                        }
                        claimed.push(item_id);
                    }
                }
                if let Some(error) = value["error"].as_str() {
                    self.state.set_provider_error(
                        "Claude",
                        format!("Claude Code: {error}"),
                        false,
                        Some(error),
                        None,
                    );
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
                            && self.state.saved.draft_reference.is_none()
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
                let root = self.state.saved.thread_id.clone().unwrap_or_default();
                let owners = &self.state.interaction_owners;
                let child = |id: &Value| {
                    owners
                        .get(&id.to_string())
                        .is_some_and(|owner| owner.thread != root)
                };
                self.state.approvals.retain(|a| child(&a["id"]));
                self.state.questions.retain(|q| child(&q.id));
                self.state.elicitations.retain(|r| child(&r.id));
                self.state
                    .interaction_owners
                    .retain(|_, owner| owner.thread != root);
                self.state.status = if self.interrupting {
                    Status::Interrupted
                } else if value["is_error"] == true {
                    Status::Failed
                } else {
                    Status::Done
                };
                if value["is_error"] == true && !self.interrupting {
                    // The terminal result often has a generic subtype. Retain
                    // a preceding explicit rejection's category/reset metadata.
                    let usage_notice = self
                        .state
                        .effective_notice()
                        .filter(|notice| notice.kind == codex::SessionNoticeKind::UsageLimit)
                        .cloned();
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
                    if let Some(body) = self.state.error.clone() {
                        self.state.set_provider_error(
                            "Claude",
                            body,
                            false,
                            if usage_notice.is_some() {
                                Some("usage_limit_reached")
                            } else {
                                value["subtype"].as_str()
                            },
                            usage_notice.and_then(|notice| notice.reset_at),
                        );
                    }
                    self.queue_ready = false;
                } else if value["is_error"] != true && self.state.effective_notice().is_some() {
                    self.state.clear_error();
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
        self.state.refresh_interaction_status();
        self.configure_pending_access()?;
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
            "content_block_stop" => {
                if let Some(id) = self.blocks.get(&index) {
                    if let Some(item) = self
                        .state
                        .saved
                        .items
                        .iter_mut()
                        .find(|item| item["id"] == *id && item["type"] == "agentMessage")
                    {
                        item["groveCompleted"] = json!(true);
                    }
                }
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
                assert!(appended[0][1].contains(crate::tasks::CLAUDE_GUIDANCE));
                assert!(!appended[0][1].contains(crate::tasks::CODEX_GUIDANCE));
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
        actor_with_saved(saved)
    }
    fn actor_with_saved(saved: SavedSession) -> Actor {
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
    #[cfg(unix)]
    fn speed_live_control_is_session_scoped_and_reported_separately() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.resolved_model = Some("claude-opus-test".into());
        actor.state.models = vec![
            json!({"model":"opus", "resolvedModel":"claude-opus-test", "supportsFastMode":true}),
        ];
        actor.state.saved.draft = "preserve".into();
        actor.state.resolved_access = Some("plan".into());
        actor.action(Action::SetSpeed(codex::Speed::Fast)).unwrap();
        actor.configure_live().unwrap();
        let request = read(&mut reader);
        assert_eq!(request["request"]["settings"]["fastMode"], true);
        assert!(
            !actor.reset_configuration,
            "Speed applies without restarting or changing model/access"
        );
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        assert_eq!(
            actor.state.resolved_model.as_deref(),
            Some("claude-opus-test")
        );
        assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
        assert!(
            actor.state.reported_speed.is_none(),
            "ACK is not effective tier evidence"
        );
        actor.message(json!({"type":"system","subtype":"init","model":"claude-opus-test","fast_mode_state":"off","fast_mode_disabled_reason":"extra_usage_disabled"})).unwrap();
        assert_eq!(actor.state.reported_speed.as_deref(), Some("off"));
        assert_eq!(actor.state.saved.composer.speed, Some(codex::Speed::Fast));
        actor.action(Action::ResetModelEffort).unwrap();
        assert_eq!(actor.state.saved.composer.speed, Some(codex::Speed::Fast));
        assert_eq!(actor.state.saved.draft, "preserve");
        actor
            .action(Action::SetSpeed(codex::Speed::Standard))
            .unwrap();
        actor.configure_live().unwrap();
        let request = read(&mut reader);
        assert_eq!(request["request"]["settings"]["fastMode"], false);
        assert!(actor
            .action(Action::SetSpeed(codex::Speed::Ultrafast))
            .is_err());
    }
    #[test]
    fn speed_launch_overlay_preserves_settings_and_never_enables_fast_before_catalog() {
        let mut actor = actor();
        actor.state.saved.composer.speed = Some(codex::Speed::Fast);
        let cmd = launch_command(
            &[
                "--settings".into(),
                r#"{"env":{"CUSTOM":"kept"},"fastMode":true}"#.into(),
            ],
            &actor.state.saved,
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let settings: Value = serde_json::from_str(
            args.windows(2).find(|a| a[0] == "--settings").unwrap()[1].as_str(),
        )
        .unwrap();
        assert_eq!(settings["env"]["CUSTOM"], "kept");
        assert_eq!(settings["fastMode"], false);
    }

    #[test]
    fn provider_rate_limit_events_preserve_continue_and_child_boundaries() {
        let mut actor = actor();
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("turn-1".into());
        actor
            .message(
                json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning"}}),
            )
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.state.error.is_none());
        assert_eq!(
            actor.state.saved.items.last().unwrap()["severity"],
            "warning"
        );
        actor.message(json!({"type":"rate_limit_event","parent_tool_use_id":"child","rate_limit_info":{"status":"rejected"}})).unwrap();
        assert!(actor.state.error.is_none());
        actor.message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1234}})).unwrap();
        let notice = actor.state.effective_notice().unwrap();
        assert_eq!(notice.kind, codex::SessionNoticeKind::UsageLimit);
        assert_eq!(notice.reset_at, Some(1234));
        assert_eq!(actor.state.status, Status::Working);
        actor
            .message(json!({"type":"assistant","error":"rate_limit","message":{"content":[]}}))
            .unwrap();
        assert_eq!(actor.state.effective_notice().unwrap().reset_at, Some(1234));
        actor.message(json!({"type":"result","is_error":true,"subtype":"error_during_execution","errors":["Request rejected"]})).unwrap();
        assert_eq!(
            actor.state.effective_notice().unwrap().kind,
            codex::SessionNoticeKind::UsageLimit
        );
        assert_eq!(actor.state.effective_notice().unwrap().reset_at, Some(1234));
    }
    #[test]
    fn provider_allowed_recovers_usage_but_preserves_other_errors_and_transport_disconnects() {
        let mut actor = actor();
        actor.state.turn_id = Some("turn-1".into());
        actor.message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1234}})).unwrap();
        actor
            .message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}))
            .unwrap();
        assert!(actor.state.error.is_none());
        actor.state.set_provider_error(
            "Claude",
            "Authentication failed".into(),
            false,
            Some("authentication_failed"),
            None,
        );
        actor
            .message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}))
            .unwrap();
        assert_eq!(actor.state.error.as_deref(), Some("Authentication failed"));
        actor.fail("Process exited".into());
        assert_eq!(actor.state.status, Status::Disconnected);
    }
    #[test]
    fn provider_errors_keep_claude_result_details() {
        let mut actor = actor();
        actor.state.turn_id = Some("turn-1".into());
        actor
            .message(json!({"type":"assistant","error":"rate_limit","message":{"content":[]}}))
            .unwrap();
        assert_eq!(
            actor.state.effective_notice().unwrap().kind,
            codex::SessionNoticeKind::Stopped
        );
        assert!(actor.state.effective_notice().unwrap().usage_url.is_none());
        actor.state.error = None;
        actor.state.notice = None;
        actor
            .message(
                json!({"type":"result","is_error":true,"errors":["First detail","Second detail"]}),
            )
            .unwrap();
        assert_eq!(
            actor.state.error.as_deref(),
            Some("First detail\nSecond detail")
        );
        assert_eq!(
            actor.state.effective_notice().unwrap().kind,
            codex::SessionNoticeKind::Stopped
        );
    }
    #[test]
    #[cfg(unix)]
    fn permission_mode_discovery_times_out_and_honors_cancellation() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 10"]);
        let started = Instant::now();
        assert!(discover_permission_modes(
            command,
            &AtomicBool::new(false),
            Duration::from_millis(50)
        )
        .is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 10"]);
        assert!(
            discover_permission_modes(command, &AtomicBool::new(true), Duration::from_secs(2))
                .is_empty()
        );
    }
    #[test]
    fn claude_mode_discovery_requires_advertised_choices() {
        assert_eq!(permission_modes_from_help("--permission-mode <mode> Permission mode (choices: \"acceptEdits\", \"manual\", \"plan\")"), vec!["acceptEdits", "manual", "plan"]);
        assert!(permission_modes_from_help("unknown CLI version").is_empty());
    }
    #[test]
    fn permission_selection_dispatches_before_first_prompt_and_waits_for_ack() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.resolved_access = Some("default".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        assert_eq!(
            actor.permission_changes.len(),
            1,
            "selection must send a permission control request without requiring a prompt"
        );
        let request = read(&mut reader);
        assert_eq!(
            request["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        assert_eq!(actor.state.status, Status::Idle);
        assert!(actor.state.access.is_none());
        assert!(actor.state.saved.composer.access.is_none());
        actor
            .submit(41, "an explicit user prompt".into(), vec![])
            .unwrap();
        assert!(actor.submit(42, "another prompt".into(), vec![]).is_err());
        assert_eq!(actor.state.rejected_submission, Some(42));
        assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
        assert!(actor.in_flight.is_none());
        assert!(!actor.drain_queue());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
        assert!(actor.drain_queue());
        assert_eq!(read(&mut reader)["type"], "user");
    }

    #[test]
    fn permission_selection_after_restoring_defaults_dispatches_without_a_prompt() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.access = Some("dontAsk".into());
        actor.state.resolved_access = Some("dontAsk".into());
        actor.state.saved.composer.access = Some("dontAsk".into());
        actor.action(Action::ResetConfiguration).unwrap();
        assert!(actor.reset_configuration);
        for (mode, success) in [("plan", false), ("acceptEdits", true)] {
            actor
                .action(Action::Configure {
                    model: None,
                    effort: None,
                    access: Some(mode.into()),
                })
                .unwrap();
            assert_eq!(
                actor.permission_changes.len(),
                1,
                "restoring defaults must not block later live permission selections"
            );
            let request = read(&mut reader);
            assert_eq!(
                request["request"],
                json!({"subtype":"set_permission_mode","mode":mode})
            );
            assert_eq!(actor.state.resolved_access.as_deref(), Some("dontAsk"));
            assert!(actor.state.saved.composer.access.is_none());
            actor
                .message(json!({"type":"control_response","response":{
                    "subtype":if success { "success" } else { "error" },
                    "request_id":request["request_id"], "error":"mode unavailable"
                }}))
                .unwrap();
        }
        assert_eq!(actor.state.access.as_deref(), Some("acceptEdits"));
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("acceptEdits")
        );
        assert!(actor.state.error.is_none());
        assert!(
            actor.reset_configuration,
            "model/default restoration still requires restart"
        );
        assert!(actor.in_flight.is_none());
        assert!(actor.deferred.is_none());
    }

    #[test]
    fn permission_selection_requires_launch_capability_for_bypass() {
        let mut actor = actor();
        actor.state.permission_modes = vec!["default".into(), "bypassPermissions".into()];
        let selection = || Action::Configure {
            model: None,
            effort: None,
            access: Some("bypassPermissions".into()),
        };
        assert!(actor.action(selection()).is_err());
        assert!(actor.pending_access.is_none());
        for args in [
            vec!["--append-system-prompt=--dangerously-skip-permissions".into()],
            vec![
                "--append-system-prompt".into(),
                "--allow-dangerously-skip-permissions".into(),
            ],
        ] {
            actor.state.saved.config_args = args;
            assert!(actor.action(selection()).is_err());
        }
        for flag in [
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
        ] {
            actor.state.saved.config_args = vec![flag.into()];
            actor.action(selection()).unwrap();
            assert_eq!(actor.pending_access.as_deref(), Some("bypassPermissions"));
        }
    }

    #[test]
    fn rejected_permission_selection_allows_acknowledged_recovery() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.resolved_access = Some("default".into());
        for (mode, success) in [("plan", false), ("acceptEdits", true)] {
            actor
                .action(Action::Configure {
                    model: None,
                    effort: None,
                    access: Some(mode.into()),
                })
                .unwrap();
            let request = read(&mut reader);
            assert_eq!(request["request"]["mode"], mode);
            assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
            actor
                .message(json!({"type":"control_response","response":{
                    "subtype":if success { "success" } else { "error" },
                    "request_id":request["request_id"], "error":"mode unavailable"
                }}))
                .unwrap();
        }
        assert_eq!(actor.state.resolved_access.as_deref(), Some("acceptEdits"));
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("acceptEdits")
        );
        assert!(actor.state.error.is_none());
        assert!(actor.permission_changes.is_empty());
        assert_eq!(actor.state.status, Status::Idle);
    }

    #[test]
    fn permission_ack_preserves_a_newer_unrelated_error() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.permission_error = Some("mode unavailable".into());
        actor.state.error = Some("authentication expired".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        let request = read(&mut reader);
        actor
            .message(json!({"type":"control_response","response":{
                "subtype":"success", "request_id":request["request_id"]
            }}))
            .unwrap();
        assert_eq!(actor.state.error.as_deref(), Some("authentication expired"));
        assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
    }

    #[test]
    fn permission_selection_survives_initialization_and_rejection_keeps_effective_access() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initial = Some("perform the launch task".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        assert!(actor.permission_changes.is_empty());
        actor
            .pending
            .insert("initialize".into(), "initialize".into());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
        assert_eq!(actor.permission_changes.len(), 1);
        let request = read(&mut reader);
        assert_eq!(
            request["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":"mode unavailable"}})).unwrap();
        assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
        assert!(actor.state.saved.composer.access.is_none());
        assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
        assert!(actor.initial.is_none());
        assert!(actor.deferred.is_none());
        assert!(actor.in_flight.is_none());
        assert_eq!(actor.state.status, Status::Idle);
        assert!(!actor.drain_queue());
        assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
    }

    #[test]
    fn startup_launch_prompt_waits_for_successful_permission_selection() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initial = Some("perform the launch task".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        actor
            .pending
            .insert("initialize".into(), "initialize".into());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
        let request = read(&mut reader);
        assert!(!actor.drain_queue());
        assert_eq!(actor.initial.as_deref(), Some("perform the launch task"));
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        assert!(actor.drain_queue());
        let prompt = read(&mut reader);
        assert_eq!(prompt["type"], "user");
        assert_eq!(
            prompt["message"]["content"][0]["text"],
            "perform the launch task"
        );
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        assert!(actor.initial.is_none());
        assert!(!actor.drain_queue());
    }

    #[test]
    fn busy_permission_selection_serializes_changes_without_applying_model_or_effort() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("active".into());
        actor
            .action(Action::Configure {
                model: Some("sonnet".into()),
                effort: Some("low".into()),
                access: Some("plan".into()),
            })
            .unwrap();
        assert_eq!(actor.permission_changes.len(), 1);
        let first = read(&mut reader);
        assert_eq!(actor.pending.len(), 1);
        assert!(actor
            .redirect(42, "steer while the mode is pending".into(), vec![])
            .is_err());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("acceptEdits".into()),
            })
            .unwrap();
        assert_eq!(actor.permission_changes.len(), 1);
        assert_eq!(actor.pending_access.as_deref(), Some("acceptEdits"));
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":first["request_id"]}})).unwrap();
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        let second = read(&mut reader);
        assert_eq!(
            second["request"],
            json!({"subtype":"set_permission_mode","mode":"acceptEdits"})
        );
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":second["request_id"]}})).unwrap();
        assert_eq!(actor.state.access.as_deref(), Some("acceptEdits"));
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(actor.state.turn_id.as_deref(), Some("active"));
        assert!(
            actor.reconfigure,
            "model and effort remain pending for the next turn"
        );
        assert!(actor.pending.is_empty());
        assert!(actor.in_flight.is_none());
    }

    #[test]
    fn claude_permission_change_ack_is_bound_to_request_and_fallback_is_authoritative() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.resolved_access = Some("default".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        actor.configure_live().unwrap();
        let request = read(&mut reader);
        let effort = read(&mut reader);
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("acceptEdits".into()),
            })
            .unwrap();
        assert!(actor.state.access.is_none());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        assert_eq!(actor.state.access.as_deref(), Some("plan"));
        assert_eq!(actor.pending_access.as_deref(), Some("acceptEdits"));
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":effort["request_id"]}})).unwrap();
        actor
            .message(json!({"type":"system","subtype":"status","permissionMode":"default"}))
            .unwrap();
        assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("default")
        );
    }
    #[test]
    fn pending_access_selection_survives_model_reset_without_claiming_acknowledgement() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.resolved_access = Some("default".into());
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        actor.action(Action::ResetModelEffort).unwrap();
        assert_eq!(actor.pending_access.as_deref(), Some("plan"));
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("default")
        );
        actor
            .pending
            .insert("initialize".into(), "initialize".into());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
        assert!(actor.reconfigure);
        assert!(actor.pending_access.is_none());
        let request = read(&mut reader);
        assert_eq!(
            request["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        assert_eq!(actor.permission_changes.len(), 1);
        assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("default")
        );
        actor
            .message(json!({"type":"control_response","response":{
                "subtype":"success", "request_id":request["request_id"]
            }}))
            .unwrap();
        assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
        assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
    }
    #[test]
    fn permission_status_before_ack_preserves_acknowledged_explicit_mode_and_fallback() {
        for effective in ["plan", "default"] {
            let mut actor = actor();
            let mut reader = transport(&mut actor);
            actor
                .action(Action::Configure {
                    model: None,
                    effort: None,
                    access: Some("plan".into()),
                })
                .unwrap();
            actor.configure_live().unwrap();
            let request = read(&mut reader);
            let _effort = read(&mut reader);
            actor
                .message(json!({"type":"system","subtype":"status","permissionMode":effective}))
                .unwrap();
            assert!(actor.state.saved.composer.access.is_none());
            actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
            assert_eq!(
                actor.state.saved.composer.access.as_deref(),
                Some(effective)
            );
            assert_eq!(actor.state.resolved_access.as_deref(), Some(effective));
        }
    }
    #[test]
    fn rejected_claude_permission_change_retains_previous_access_and_stops_deferred_prompt() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.state.access = Some("bypassPermissions".into());
        actor.state.saved.composer.access = actor.state.access.clone();
        actor.state.resolved_access = actor.state.access.clone();
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        actor.configure_live().unwrap();
        let request = read(&mut reader);
        let effort = read(&mut reader);
        actor.deferred = Some(QueuedPrompt {
            id: 99,
            text: "do work".into(),
            images: vec![],
        });
        actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":"mode unavailable"}})).unwrap();
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":effort["request_id"]}})).unwrap();
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some("bypassPermissions")
        );
        assert_eq!(
            actor.state.resolved_access.as_deref(),
            Some("bypassPermissions")
        );
        assert!(actor.deferred.is_none());
        assert!(!actor.queue_ready);
        assert_eq!(actor.state.rejected_submission, Some(99));
        assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
        assert_eq!(actor.state.status, Status::Idle);
    }
    #[test]
    fn child_authoritative_handback_report_is_visible_and_deduplicated() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.register_subagent(&json!({"id":"child","name":"Agent","input":{"prompt":"Inspect"}}));
        let report = "Full child final report. ".repeat(100);
        let structured = json!({"agentId":"provider-child","status":"completed","handback":"send","handbackReport":{"text":report},"content":[{"type":"text","text":"Report delivered elsewhere, not repeated here"}]});
        let result = json!({"type":"tool_result","tool_use_id":"child","content":"Report delivered elsewhere, not repeated here"});
        actor.subagent_tool_result(&result, Some(&structured));
        actor.subagent_tool_result(&result, Some(&structured));
        let items = &actor.state.subagent_activity["child"].items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "agentMessage");
        assert_eq!(items[0]["phase"], "final_answer");
        assert_eq!(items[0]["text"], report);
        assert_eq!(actor.state.subagent_activity["child"].events.len(), 1);
        let mut child = ChildTranscript::default();
        child.message(&json!({"type":"assistant","message":{"id":"message","content":[{"type":"text","text":report}]}}), "/repo");
        child.message(&json!({"type":"result","result":report}), "/repo");
        assert_eq!(child.items.len(), 1);
        assert_eq!(child.items[0]["phase"], "final_answer");
    }

    #[test]
    fn child_fragmented_final_frames_preserve_stream_block_identity() {
        let mut child = ChildTranscript::default();
        child.message(&json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"fragmented"}}}), "/repo");
        for (index, text) in [(0, "First response"), (2, "Second response")] {
            child.message(&json!({"type":"stream_event","event":{"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}}), "/repo");
            child.message(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}}), "/repo");
        }
        for text in ["First response", "Second response"] {
            child.message(&json!({"type":"assistant","message":{"id":"fragmented","content":[{"type":"text","text":text}]}}), "/repo");
        }
        assert_eq!(child.items.len(), 2);
        assert_eq!(child.items[0]["id"], "fragmented-0");
        assert_eq!(child.items[0]["text"], "First response");
        assert_eq!(child.items[1]["id"], "fragmented-2");
        assert_eq!(child.items[1]["text"], "Second response");
    }

    #[test]
    fn child_transcript_preserves_full_streamed_messages_and_tool_results() {
        let mut child = ChildTranscript::default();
        let text = "Detailed child output. ".repeat(160);
        child.message(&json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"message"}}}), "/repo");
        child.message(&json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}), "/repo");
        child.message(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}}), "/repo");
        assert_eq!(child.items[0]["text"], text);
        child.message(&json!({"type":"assistant","message":{"id":"message","content":[{"type":"text","text":text},{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"cat file"}},{"type":"thinking","thinking":"private"}]}}), "/repo");
        assert_eq!(child.items.len(), 2);
        assert_eq!(child.items[1]["type"], "commandExecution");
        assert_eq!(child.items[1]["status"], "inProgress");
        child.message(&json!({"type":"user","uuid":"user","message":{"content":[{"type":"text","text":text},{"type":"tool_result","tool_use_id":"bash","content":text,"is_error":true}]}}), "/repo");
        assert_eq!(child.items.len(), 3);
        assert_eq!(child.items[1]["aggregatedOutput"], text);
        assert_eq!(child.items[1]["result"]["content"][0]["text"], text);
        assert_eq!(child.items[1]["status"], "failed");
        assert_eq!(child.items[2]["content"][0]["text"], text);
        child.message(&json!({"type":"assistant","message":{"id":"edit","content":[{"type":"tool_use","id":"write","name":"Write","input":{"file_path":"/repo/file","content":"new"}}]}}), "/repo");
        child.message(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"write","content":"written"}]},"tool_use_result":{"type":"create","filePath":"/repo/file","content":"new"}}), "/repo");
        assert_eq!(child.items[3]["type"], "fileChange");
        assert_eq!(child.items[3]["changes"][0]["diff"], "+new\n");
        assert_eq!(child.items[3]["status"], "completed");
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
    fn background_child_reports_and_terminal_patches_reach_the_roster() {
        let mut actor = actor();
        actor.state.begin_work();
        let spawn = json!({"id":"child","name":"Agent","input":{"subagent_type":"general-purpose","description":"Inspect directory"}});
        actor.register_subagent(&spawn);
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"child","run_id":"run","task_type":"local_agent"})).unwrap();
        let done = json!({"type":"system","subtype":"task_notification","task_id":"task","tool_use_id":"child","run_id":"run","status":"completed","handback_report":{"text":"FINAL_REPORT"}});
        actor.message(done.clone()).unwrap();
        actor.message(done).unwrap();
        assert_eq!(
            actor.state.subagent_activity["child"]
                .items
                .iter()
                .filter(|item| item["type"] == "agentMessage" && item["text"] == "FINAL_REPORT")
                .count(),
            1
        );
        actor.register_subagent(&json!({"id":"stopped","name":"Task","input":{}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"stopped-task","tool_use_id":"stopped","run_id":"stopped-run","task_type":"local_agent"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_updated","task_id":"stopped-task","run_id":"stopped-run","patch":{"status":"killed"}})).unwrap();
        actor.register_subagent(&json!({"id":"stopped","name":"Task","input":{}}));
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[1].status,
            SubagentStatus::Interrupted
        );
        actor.subagent_tool_result(&json!({"tool_use_id":"stopped","is_error":false}), None);
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[1].status,
            SubagentStatus::Interrupted
        );
        actor.message(json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":"old-run","status":"failed","handback_report":{"text":"STALE"}})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
        assert!(!actor.state.subagent_activity["child"]
            .items
            .iter()
            .any(|item| item["text"] == "STALE"));
        actor
            .message(json!({"type":"result","parent_tool_use_id":"stopped","is_error":false}))
            .unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[1].status,
            SubagentStatus::Interrupted
        );
        for (index, (status, expected)) in [
            ("completed", SubagentStatus::Completed),
            ("failed", SubagentStatus::Failed),
            ("killed", SubagentStatus::Interrupted),
            ("stopped", SubagentStatus::Interrupted),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("patch-{index}");
            actor.register_subagent(&json!({"id":id,"name":"Agent","input":{}}));
            actor.message(json!({"type":"system","subtype":"task_started","task_id":id,"tool_use_id":id,"run_id":id,"task_type":"local_agent"})).unwrap();
            let subtype = if status == "stopped" {
                "task_notification"
            } else {
                "task_updated"
            };
            actor.message(json!({"type":"system","subtype":subtype,"task_id":id,"run_id":id,"status":status,"patch":{"status":status}})).unwrap();
            assert_eq!(
                actor.state.saved.work_periods[0].subagents[index + 2].status,
                expected
            );
        }
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"nested","tool_use_id":"nested-tool","parent_task_id":"task","run_id":"nested-run","task_type":"local_agent","subagent_type":"Explore","description":"Nested scan"})).unwrap();
        let nested_report = json!({"type":"system","subtype":"task_notification","task_id":"nested","run_id":"nested-run","status":"completed","handback_report":{"text":"NESTED_FINAL"}});
        actor.message(nested_report.clone()).unwrap();
        actor.message(nested_report).unwrap();
        assert_eq!(
            actor.state.subagent_activity["nested-tool"]
                .items
                .iter()
                .filter(|item| item["type"] == "agentMessage" && item["text"] == "NESTED_FINAL")
                .count(),
            1
        );
    }

    #[test]
    fn child_history_roundtrip_restores_full_messages_tools_and_alias_updates() {
        use crate::codex::SubagentTranscriptAvailability as Availability;
        let mut actor = actor();
        actor.state.begin_work();
        actor.register_subagent(&json!({"id":"child","name":"Agent","input":{"name":"Ada","subagent_type":"Explore","description":"Inspect directory"}}));
        assert_eq!(
            actor.state.subagent_transcript_availability("child"),
            Availability::Awaiting
        );
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"child","run_id":"run","task_type":"local_agent"})).unwrap();
        let text = "Full provider-visible report. ".repeat(200);
        actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message","content":[{"type":"text","text":text},{"type":"thinking","thinking":"PRIVATE"},{"type":"tool_use","id":"read","name":"Read","input":{"file_path":"file"}}]}})).unwrap();
        let pending = actor_with_saved(
            serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
        );
        assert_eq!(
            pending.state.subagent_activity["child"].items[1]["status"],
            "inProgress"
        );
        assert_eq!(
            pending.state.subagent_snapshot("child").status,
            Status::Idle
        );
        actor.message(json!({"type":"user","parent_tool_use_id":"child","message":{"content":[{"type":"tool_result","tool_use_id":"read","content":"full tool output"}]}})).unwrap();
        assert_eq!(
            actor.state.subagent_transcript_availability("child"),
            Availability::Live
        );
        let bytes = serde_json::to_vec(&actor.state.saved).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));
        let mut restored = actor_with_saved(codex::recover_saved_for_test(&actor.state.saved));
        assert_eq!(
            restored.state.subagent_transcript_availability("child"),
            Availability::Restored
        );
        assert_eq!(
            restored.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Unknown
        );
        assert_eq!(
            restored.state.saved.work_periods[0].subagents[0].name,
            "Ada"
        );
        assert_eq!(
            restored.state.subagent_activity["child"].items,
            actor.state.subagent_activity["child"].items
        );
        assert!(restored.state.subagent_activity["child"]
            .observed_at
            .is_none());
        assert!(restored.state.subagent_activity["child"].events.is_empty());
        restored.message(json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":"run","status":"completed","handback_report":{"text":text}})).unwrap();
        assert_eq!(restored.state.subagent_activity["child"].items.len(), 2);
        assert_eq!(
            restored.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
        let complete = actor_with_saved(codex::recover_saved_for_test(&restored.state.saved));
        assert_eq!(
            complete.state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
        assert_eq!(complete.state.subagent_activity["child"].items.len(), 2);
        assert!(complete
            .state
            .saved
            .fresh_conversation()
            .claude_subagents
            .is_empty());
    }

    #[test]
    fn subagents_track_live_nested_work_without_primary_state_pollution() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("primary-turn".into());
        actor.state.saved.thread_id = Some("primary".into());
        let spawn = json!({"type":"tool_use","id":"child","name":"Agent","input":{"name":"Explorer","subagent_type":"Explore","description":"Find auth paths","prompt":"Search all auth entry points"}});
        actor.block("child", &spawn);
        actor.block("child", &spawn);
        assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
        let item_count = actor.state.saved.items.len();
        actor.message(json!({"type":"assistant","session_id":"primary","parent_tool_use_id":"child","message":{"model":"child-model","usage":{"input_tokens":900},"content":[{"type":"tool_use","id":"read","name":"Read","input":{}},{"type":"tool_use","id":"nested","name":"Task","input":{"subagent_type":"Researcher","description":"Inspect tests"}}]}})).unwrap();
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
    fn subagent_recent_activity_claude_attribution_dedup_and_reasoning_exclusion() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.register_subagent(
            &json!({"id":"child", "name":"Agent", "input":{"description":"Inspect"}}),
        );
        let message = json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message-1","content":[{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"src/main.rs"}},{"type":"text","text":"Found the entry point"},{"type":"thinking","thinking":"private chain"}]}});
        actor.message(message.clone()).unwrap();
        actor.message(message).unwrap();
        let activity = &actor.state.subagent_activity["child"];
        assert_eq!(activity.events.len(), 2);
        assert_eq!(
            activity.events[0].kind,
            crate::codex::SubagentActivityKind::Tool
        );
        assert!(activity.events[0].text.contains("src/main.rs"));
        assert_eq!(
            activity.events[1].kind,
            crate::codex::SubagentActivityKind::Commentary
        );
        assert!(!activity
            .events
            .iter()
            .any(|e| e.text.contains("private chain")));
        assert_eq!(actor.state.subagent_activity.len(), 1);
        actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message-2","content":[{"type":"text","text":"Found the entry point"}]}})).unwrap();
        assert_eq!(actor.state.subagent_activity["child"].events.len(), 3);
        actor
            .message(json!({"type":"result","parent_tool_use_id":"child","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.subagent_activity["child"].events.len(), 3);
    }
    #[test]
    fn claude_task_notifications_deduplicate_tool_lifecycle_and_ignore_shell_tasks() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.block("agent", &json!({"type":"tool_use","id":"agent","name":"Task","input":{"description":"Review changes"}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"sdk-task","tool_use_id":"agent","task_type":"local_agent","subagent_type":"Reviewer","description":"Review changes"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_progress","task_id":"sdk-task","tool_use_id":"agent","summary":"Checking error handling"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_progress","task_id":"sdk-task","tool_use_id":"agent","description":"Running List files tool"})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0]
                .task
                .as_deref(),
            Some("Review changes")
        );
        actor.block("shell-tool", &json!({"type":"tool_use","id":"shell-tool","name":"Bash","input":{"command":"sleep 2","description":"Run a two-second timing probe"}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"shell","run_id":"shell-run","tool_use_id":"shell-tool","task_type":"local_bash","is_backgrounded":true,"description":"Run a two-second timing probe"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_progress","task_id":"shell","tool_use_id":"shell-tool","summary":"Running shell"})).unwrap();
        actor.message(json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed","summary":"Shell completed"})).unwrap();
        assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[0].activity,
            "Running List files tool"
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
            SubagentStatus::Interrupted
        );
    }

    #[test]
    fn claude_child_permission_uses_task_alias_and_parent_completion_preserves_it() {
        let mut actor = actor();
        actor.state.begin_work();
        actor.state.status = Status::Working;
        actor.state.saved.thread_id = Some("primary".into());
        actor.block("agent-tool", &json!({"type":"tool_use","id":"agent-tool","name":"Task","input":{"name":"Reviewer","description":"Review permissions"}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":"sdk-task","tool_use_id":"agent-tool","task_type":"local_agent","subagent_type":"Reviewer"})).unwrap();
        actor.message(json!({"type":"control_request","request_id":"child-request","parent_tool_use_id":"agent-tool","request":{"subtype":"can_use_tool","agent_id":"sdk-task","tool_use_id":"bash-1","tool_name":"Bash","input":"cargo test"}})).unwrap();
        assert_eq!(actor.state.approvals.len(), 1);
        assert_eq!(
            actor.state.interaction_agent(&json!("child-request")),
            Some("Reviewer · ent-tool")
        );
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.approvals.len(), 1);
        assert_eq!(actor.state.status, Status::Waiting);
        actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"completed"})).unwrap();
        assert_eq!(actor.state.approvals.len(), 0);
        assert_eq!(actor.state.status, Status::Idle);
    }
    #[test]
    fn reference_quote_only_interrupt_before_echo_retains_raw_draft_for_retry() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        let reference = codex::ReferenceDraft {
            id: "quote-only".into(),
            thread_id: None,
            sources: vec![codex::ReferenceSource {
                message_id: "source".into(),
                role: "assistant".into(),
            }],
            text: "literal quote".into(),
        };
        actor.state.saved.draft_reference = Some(reference.clone());
        let canonical = reference.serialize("");
        actor.submit(91, canonical.clone(), vec![]).unwrap();
        let sent = read(&mut reader);
        assert_eq!(sent["message"]["content"][0]["text"], canonical);
        actor.interrupting = true;
        actor
            .message(json!({"type":"result","is_error":false}))
            .unwrap();
        assert_eq!(actor.state.rejected_submission, Some(91));
        assert!(actor.state.saved.draft.is_empty());
        assert_eq!(actor.state.saved.draft_reference, Some(reference.clone()));
        actor
            .submit(92, reference.serialize(&actor.state.saved.draft), vec![])
            .unwrap();
        let retry = read(&mut reader);
        assert_eq!(retry["message"]["content"][0]["text"], canonical);
    }
    #[test]
    fn reference_claude_dispatch_literal_once_and_acknowledgement_owns_identity() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        let reference = codex::ReferenceDraft {
            id: "old".into(),
            thread_id: None,
            sources: vec![codex::ReferenceSource {
                message_id: "source".into(),
                role: "assistant".into(),
            }],
            text: "/compact $skill\n<User reply:>\n```rust\n```".into(),
        };
        actor.state.saved.draft_reference = Some(reference.clone());
        let canonical = reference.serialize("");
        actor
            .action(Action::Submit {
                id: 91,
                text: canonical.clone(),
                images: vec![],
            })
            .unwrap();
        let sent = read(&mut reader);
        assert_eq!(sent["message"]["content"][0]["text"], canonical);
        let mut replacement = reference.clone();
        replacement.id = "new".into();
        actor
            .action(Action::DraftReference(Some(replacement.clone())))
            .unwrap();
        actor.acknowledge_input(&sent["uuid"]);
        assert_eq!(actor.state.saved.draft_reference, Some(replacement.clone()));
        assert_eq!(
            actor.state.saved.items.last().unwrap()["content"][0]["text"],
            canonical
        );
        actor.state.status = Status::Idle;
        actor
            .action(Action::Submit {
                id: 92,
                text: replacement.serialize(""),
                images: vec![],
            })
            .unwrap();
        let sent = read(&mut reader);
        actor.acknowledge_input(&sent["uuid"]);
        assert!(actor.state.saved.draft_reference.is_none());
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
            json!({"id":"next-human","type":"userMessage","content":[{"type":"text","text":"a subsequent turn"}],"claudeHumanUuid":"next-human","claudePreviousAssistantUuid":"later-assistant"}),
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
        // Editing in the middle intentionally discards all subsequent turns in
        // the fork. Claude's optional single-turn discard guard rejects this.
        let command = launch_command(&[], &actor.state.saved).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for pair in [
            ["--resume", "original"],
            ["--resume-session-at", "assistant"],
            ["--session-id", "fork"],
        ] {
            assert!(args.windows(2).any(|actual| actual == pair));
        }
        assert!(args.iter().any(|arg| arg == "--fork-session"));
        assert!(!args.iter().any(|arg| arg == "--resume-drops-turn"));
        codex::persist(&actor.state.saved).unwrap();
        fs_err::write(&file, "pre target snapshot").unwrap();
        actor.recover_edit().unwrap();
        assert_eq!(fs_err::read_to_string(&file).unwrap(), "current workspace");
        assert_eq!(actor.state.saved.items.len(), 4);
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
            ["--session-id", "fork-id"],
        ] {
            assert!(args.windows(2).any(|actual| actual == pair));
        }
        assert!(args.iter().any(|arg| arg == "--fork-session"));
        assert!(!args.iter().any(|arg| arg == "--resume-drops-turn"));
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
        actor
            .state
            .saved
            .claude_subagents
            .insert("old-child".into(), codex::SavedClaudeSubagent::default());
        actor
            .subagent_tasks
            .insert("old-task".into(), "old-child".into());
        actor
            .child_transcripts
            .insert("old-child".into(), ChildTranscript::default());
        let before = actor.state.saved.clone();
        let mut restored = None;
        actor
            .clear_conversation(|saved| {
                restored = Some(codex::recover_saved_for_test(saved));
                Ok(())
            })
            .unwrap();
        assert!(actor.subagent_tasks.is_empty());
        assert!(actor.child_transcripts.is_empty());
        assert!(actor.state.saved.claude_subagents.is_empty());
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
        assert_eq!(actor.state.saved.items[0]["groveCompleted"], true);
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
    fn project_prompt_preserves_composer_and_uses_native_queue() {
        let mut actor = actor();
        let _reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("busy".into());
        actor.state.saved.draft = "same text".into();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .action(Action::ProjectPrompt {
                id: 41,
                text: "same text".into(),
            })
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].text, "same text");
        assert_eq!(actor.state.saved.draft, "same text");
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        assert!(actor.state.queue_sending.is_empty());
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
        assert_eq!(
            actor.state.questions.len(),
            1,
            "a resolved provider request id may be reused after the response settles"
        );
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
    fn scoped_model_effort_reset_restarts_with_preserved_access() {
        for explicit in [false, true] {
            let mut actor = actor();
            actor.state.resolved_access = Some("plan".into());
            actor
                .action(Action::Configure {
                    model: Some("sonnet".into()),
                    effort: Some("high".into()),
                    access: explicit.then(|| "plan".into()),
                })
                .unwrap();
            actor.action(Action::ResetModelEffort).unwrap();
            assert!(actor.reset_configuration);
            assert!(actor.reconfigure);
            assert!(actor.state.model.is_none());
            assert!(actor.state.effort.is_none());
            assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
            let recovered: SavedSession =
                serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
            assert!(recovered.composer.model.is_none());
            assert!(recovered.composer.effort.is_none());
            assert_eq!(recovered.composer.access.as_deref(), Some("plan"));
            let command = launch_command(&[], &recovered).unwrap();
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(!args.iter().any(|arg| arg == "--model" || arg == "--effort"));
            assert!(args
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "plan"]));
            actor
                .action(Action::Configure {
                    model: Some("haiku".into()),
                    effort: Some("low".into()),
                    access: None,
                })
                .unwrap();
            // A later selection must still restart, rather than configuring the
            // original process with its stale model/effort override.
            assert!(actor.reset_configuration);
            assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
            assert_eq!(actor.state.saved.composer.model.as_deref(), Some("haiku"));
            assert_eq!(actor.state.saved.composer.effort.as_deref(), Some("low"));
        }
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
        assert!(actor.state.access.is_none());
        assert_eq!(actor.pending_access.as_deref(), Some("plan"));
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
    fn native_plan_approval_stale_queue_never_reaches_provider() {
        let mut actor = actor();
        let _reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
        actor.state.saved.draft = "independent composer".into();

        actor.deliver_plan("plan", "r1", true, |_| Ok(())).unwrap();
        let mut next = actor.state.saved.rich_plans.revisions[0].clone();
        next.document.revision_id = "r2".into();
        next.review = crate::rich_plan::ReviewState::default();
        actor.state.saved.rich_plans.revisions.push(next);
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.queue_ready = true;
        assert!(actor.drain_queue());
        assert!(actor.state.error.as_deref().unwrap().contains("outdated"));
        assert!(actor.in_flight.is_none());
        assert!(actor.state.queue_sending.is_empty());
        assert!(actor.state.queued_prompts.is_empty());
        assert!(matches!(
            actor.state.saved.rich_plans.revisions[0].review.approval,
            crate::rich_plan::ResponseDelivery::Failed { .. }
        ));
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
    #[test]
    fn native_plan_approval_journals_queues_and_confirms_without_touching_draft() {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Working;
        actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
        actor.state.saved.draft = "independent composer".into();

        assert!(actor
            .deliver_plan("plan", "r1", true, |_| Err("disk full".into()))
            .is_err());
        assert!(actor.state.queued_prompts.is_empty());
        let journal = std::cell::RefCell::new(Vec::new());
        for _ in 0..2 {
            actor
                .deliver_plan("plan", "r1", true, |saved| {
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
            .contains("Explicit plan approval: Native"));
        actor.acknowledge_input(&wire["uuid"]);
        assert!(actor.state.queued_prompts.is_empty());
        assert!(matches!(
            actor.state.saved.rich_plans.revisions[0].review.approval,
            crate::rich_plan::ResponseDelivery::Sent { .. }
        ));
        actor
            .deliver_plan("plan", "r1", true, |_| {
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
            .prepare_approval("plan", "r1")
            .unwrap()
            .is_none());
    }
}
