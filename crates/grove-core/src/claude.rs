//! Grove-owned Claude Code sessions using the CLI's streaming SDK transport.
//! The subprocess owns credentials, tools, permissions, and conversation persistence.
mod subagents;

use crate::provider;
use crate::provider::{
    Action, Connection, EditCheckpoint, Handle, PendingEdit, QuestionRequest, QueuedPrompt,
    SavedSession, Snapshot, Status, SubagentStatus, SubmissionMode,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod transport;

#[cfg(test)]
use transport::{launch_command, launch_command_profile, CLAUDE_AI_ARTIFACT_GUIDANCE};
use transport::{spawn, spawn_profile, Process};

/// Normalize the optional Claude diagnostic payload without treating absent data as zero.
fn context_measurement(data: &Value) -> Option<(provider::ContextUsage, provider::ContextDetails)> {
    use provider::{
        ContextCategory, ContextCategoryKind, ContextDetails, ContextEntry, ContextMeasurement,
        ContextUsage,
    };
    let usage = ContextUsage {
        used: data["totalTokens"].as_u64()?,
        window: data["maxTokens"].as_u64().filter(|window| *window > 0)?,
    };
    let entries = |values: &Value, label: &str, loaded: Option<bool>| -> Vec<ContextEntry> {
        values
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| {
                loaded.is_none_or(|loaded| entry["isLoaded"].as_bool().unwrap_or(true) == loaded)
            })
            .filter_map(|entry| {
                let tokens = entry["tokens"].as_u64()?;
                let name = entry[label].as_str()?;
                let name = entry["serverName"]
                    .as_str()
                    .filter(|server| *server != name)
                    .map_or_else(|| name.to_owned(), |server| format!("{server} / {name}"));
                Some(ContextEntry {
                    label: name,
                    tokens,
                })
            })
            .collect()
    };
    let categories = data["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let label = row["name"].as_str()?.to_owned();
            let tokens = row["tokens"].as_u64()?;
            let id = row["id"].as_str().map_or_else(
                || label.to_ascii_lowercase().replace([' ', '-'], "_"),
                str::to_owned,
            );
            let kind = match row["kind"].as_str() {
                Some("used") => ContextCategoryKind::Used,
                Some("free") => ContextCategoryKind::Free,
                Some("buffer") => ContextCategoryKind::Buffer,
                Some("deferred") => ContextCategoryKind::Deferred,
                _ if row["isDeferred"] == true => ContextCategoryKind::Deferred,
                _ if id == "free_space" => ContextCategoryKind::Free,
                _ if matches!(
                    id.as_str(),
                    "autocompact_buffer" | "auto_compact_buffer" | "compact_buffer"
                ) =>
                {
                    ContextCategoryKind::Buffer
                }
                _ if matches!(
                    id.as_str(),
                    "system_prompt"
                        | "system_tools"
                        | "mcp_tools"
                        | "mcp_instructions"
                        | "custom_agents"
                        | "memory_files"
                        | "skills"
                        | "messages"
                ) =>
                {
                    ContextCategoryKind::Used
                }
                _ => ContextCategoryKind::Unknown,
            };
            let mut children = match id.as_str() {
                "mcp_tools_deferred" => entries(&data["mcpTools"], "name", Some(false)),
                "system_tools_deferred" => {
                    entries(&data["deferredBuiltinTools"], "name", Some(false))
                }
                "memory_files" => entries(&data["memoryFiles"], "path", None),
                "mcp_tools" => entries(&data["mcpTools"], "name", Some(true)),
                "mcp_instructions" => entries(&data["mcpInstructions"], "serverName", None),
                "system_tools" => {
                    let mut tools = entries(&data["systemTools"], "name", None);
                    tools.extend(entries(&data["deferredBuiltinTools"], "name", Some(true)));
                    tools
                }
                "system_prompt" => entries(&data["systemPromptSections"], "name", None),
                "custom_agents" => entries(&data["agents"], "agentType", None),
                "skills" => entries(&data["skills"]["skillFrontmatter"], "name", None),
                _ => Vec::new(),
            };
            if id == "messages" {
                for (key, label) in [
                    ("toolCallTokens", "Tool calls"),
                    ("toolResultTokens", "Tool results"),
                    ("attachmentTokens", "Attachments"),
                    ("assistantMessageTokens", "Assistant messages"),
                    ("userMessageTokens", "User messages"),
                    ("redirectedContextTokens", "Redirected context"),
                    ("unattributedTokens", "Unattributed"),
                ] {
                    if let Some(tokens) = data["messageBreakdown"][key].as_u64() {
                        children.push(ContextEntry {
                            label: label.into(),
                            tokens,
                        });
                    }
                }
            }
            if id == "messages" {
                for entry in data["messageBreakdown"]["toolCallsByType"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = entry["name"].as_str() {
                        for (key, suffix) in [("callTokens", "calls"), ("resultTokens", "results")]
                        {
                            if let Some(tokens) = entry[key].as_u64() {
                                children.push(ContextEntry {
                                    label: format!("By tool / {name} / {suffix}"),
                                    tokens,
                                });
                            }
                        }
                    }
                }
                for mut entry in
                    entries(&data["messageBreakdown"]["attachmentsByType"], "name", None)
                {
                    entry.label = format!("By attachment / {}", entry.label);
                    children.push(entry);
                }
            }
            Some(ContextCategory {
                id,
                label,
                tokens,
                kind,
                entries: children,
            })
        })
        .collect();
    Some((
        usage,
        ContextDetails {
            accounting: None,
            model: data["model"].as_str().map(str::to_owned),
            measured_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs()),
            source: ContextMeasurement::Full,
            categories,
            auto_compact_enabled: data["isAutoCompactEnabled"].as_bool(),
            auto_compact_threshold: data["autoCompactThreshold"].as_u64(),
        },
    ))
}

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
const PERMISSION_HELP_MAX_AGE: Duration = Duration::from_hours(24);
const PERMISSION_HELP_RETRY: Duration = Duration::from_secs(30);

enum PermissionProbe {
    Loading,
    Ready(Vec<String>, Instant),
}
#[derive(Default)]
struct PermissionProbeCache {
    entries: Mutex<HashMap<String, PermissionProbe>>,
    changed: Condvar,
}
impl PermissionProbeCache {
    fn get(
        &self,
        key: String,
        stopping: &AtomicBool,
        load: impl FnOnce() -> Vec<String>,
    ) -> Vec<String> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if stopping.load(Ordering::SeqCst) {
                return Vec::new();
            }
            match entries.get(&key) {
                Some(PermissionProbe::Ready(modes, at))
                    if at.elapsed()
                        < if modes.is_empty() {
                            PERMISSION_HELP_RETRY
                        } else {
                            PERMISSION_HELP_MAX_AGE
                        } =>
                {
                    return modes.clone()
                }
                Some(PermissionProbe::Loading) => {
                    entries = self
                        .changed
                        .wait_timeout(entries, Duration::from_millis(20))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
                _ => break,
            }
        }
        // Bound completed identities without disturbing in-flight owners.
        if entries.len() >= 16 {
            entries.retain(|_, value| matches!(value, PermissionProbe::Loading));
        }
        entries.insert(key.clone(), PermissionProbe::Loading);
        drop(entries);
        let modes = load();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stopping.load(Ordering::SeqCst) {
            entries.remove(&key);
        } else {
            entries.insert(key, PermissionProbe::Ready(modes.clone(), Instant::now()));
        }
        self.changed.notify_all();
        modes
    }
}

fn permission_probe_key(
    program: &str,
    args: &[String],
    environment: &[(String, String)],
) -> String {
    use std::hash::{Hash, Hasher};
    let mut key = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut key);
    args.hash(&mut key);
    let mut environment = environment.to_vec();
    environment.sort();
    environment.hash(&mut key);
    // session_invocation can be a shell wrapper; also fingerprint the actual CLI.
    let (binary, prefix) = crate::agent::Agent::Claude.invocation();
    for path in std::iter::once(binary).chain(prefix) {
        path.hash(&mut key);
        if let Ok(metadata) = fs_err::metadata(path) {
            metadata.len().hash(&mut key);
            metadata.modified().ok().hash(&mut key);
        }
    }
    format!("{:016x}", key.finish())
}

fn available_permission_modes(stopping: &AtomicBool) -> Vec<String> {
    static CACHE: OnceLock<PermissionProbeCache> = OnceLock::new();
    let (program, args) = crate::agent::Agent::Claude.session_invocation(&["--help".into()]);
    let environment = crate::env_path::session_environment()
        .map_or_else(|| std::env::vars().collect(), |vars| vars.to_vec());
    let key = permission_probe_key(&program, &args, &environment);
    CACHE
        .get_or_init(PermissionProbeCache::default)
        .get(key, stopping, || {
            let mut command = Command::new(program);
            command.args(args).env_clear().envs(environment);
            discover_permission_modes(command, stopping, Duration::from_secs(2))
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
                return true;
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
    items: provider::TranscriptItems,
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
            *old = item.into();
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
                            if let Some(item) = self.blocks.get(&index).and_then(|id| {
                                self.items.iter_mut().rev().find(|item| item["id"] == *id)
                            }) {
                                provider::append_json_text(&mut item["text"], text);
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
    fn action(&mut self, action: Action) -> Result<(), String> {
        if let (Some(error), Some(submission)) =
            (&self.startup_error, provider::startup_submission(&action))
        {
            if let Some(id) = submission {
                self.state.rejected_submission = Some(id);
            }
            return Err(format!(
                "{error}. Request was not sent; reconnect after resolving checkout validation."
            ));
        }

        if self.side_chat
            && !matches!(
                &action,
                Action::Submit { .. }
                    | Action::UpdateSkills(_)
                    | Action::UpdateModels(_)
                    | Action::Prompt(_)
                    | Action::Interrupt
                    | Action::Reconnect
                    | Action::Stop { .. }
            )
        {
            return Err("This action is unavailable in Side chat.".into());
        }
        if matches!(
            &action,
            Action::RewindMessage { .. }
                | Action::RewindMessageReserved { .. }
                | Action::RestoreRewindDraft { .. }
        ) {
            self.state.rewind_attempts = self.state.rewind_attempts.saturating_add(1);
        }
        if self.state.saved.pending_edit.is_some()
            && !matches!(
                action,
                Action::Reconnect
                    | Action::Stop { .. }
                    | Action::UpdateSkills(_)
                    | Action::UpdateModels(_)
                    | Action::Draft(_)
                    | Action::SetTitle(_)
                    | Action::RegenerateTitle
                    | Action::SideChat { .. }
                    | Action::SideChatDraft { .. }
                    | Action::DraftImages(_)
                    | Action::DraftReference(_)
            )
        {
            return Err("An interrupted edit needs recovery. Reconnect before continuing.".into());
        }
        match action {
            Action::MarkReviewed { work_period } => {
                self.state.mark_reviewed(work_period)?;
            }
            Action::UpdateSkills(skills) => self.state.skills = (skills).into(),
            Action::UpdateModels(models) => self.state.models = (models).into(),
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
                    .update_review(&plan_id, &revision_id, *review)?;
                self.persistence.flush(&self.state.saved)?;
            }
            Action::RetryPlanImport { message_id, source } => {
                crate::rich_plan::retry_import(&mut self.state.saved, &message_id, &source)?;
                self.persistence.flush(&self.state.saved)?;
            }
            Action::ApprovePlan {
                plan_id,
                revision_id,
            } => self.deliver_plan(&plan_id, &revision_id, true, provider::persist)?,
            Action::SendPlanResponse {
                plan_id,
                revision_id,
            } => self.send_plan_response(&plan_id, &revision_id, provider::persist)?,
            Action::RewindMessageReserved {
                index,
                message_id,
                reservation,
            } => {
                let target = self.state.rewind_target(index)?;
                let files = target
                    .files
                    .as_ref()
                    .ok_or("The workspace checkpoint is missing.")?;
                let mut reserved = reservation.roots();
                let mut expected = files.roots();
                reserved.sort();
                expected.sort();
                if reserved != expected {
                    return Err("The reserved worktrees no longer match this rewind target.".into());
                }
                self.restore_reservation = Some(reservation);
                if let Err(error) =
                    self.edit_message_with_options(index, message_id, None, true, true)
                {
                    if self.state.saved.pending_edit.is_none() {
                        self.restore_reservation = None;
                    }
                    return Err(error);
                }
            }
            Action::RewindMessage {
                index,
                message_id,
                restore_files,
            } => {
                if let Err(error) =
                    self.edit_message_with_options(index, message_id, None, restore_files, true)
                {
                    if self.state.saved.pending_edit.is_none() {
                        self.restore_reservation = None;
                    }
                    return Err(error);
                }
            }
            Action::RestoreRewindDraft { id } => {
                let mut saved = self.state.saved.clone();
                saved.restore_rewind_draft(&id)?;
                self.persistence.flush(&saved)?;
                self.state.saved = saved;
                self.state.edited_messages += 1;
            }
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
                self.persistence.flush(&self.state.saved)?;
                self.clear_conversation(provider::persist)?;
                if let Err(error) = self.start() {
                    self.fail(error);
                }
            }
            Action::Prompt(text) => self.submit(0, text, Vec::new())?,
            Action::RefreshContext => self.refresh_context(),
            Action::Compact => {
                if self.process.is_none() {
                    return Err(provider::COMPACT_DISCONNECTED.into());
                }
                if self.state.status.busy() || self.in_flight.is_some() {
                    return Err(provider::COMPACT_BUSY.into());
                }
                if !self.initialized {
                    return Err(provider::COMPACT_DISCONNECTED.into());
                }
                if self.state.saved.thread_id.is_none()
                    || !provider::has_conversation(&self.state.saved.items)
                {
                    return Err(provider::NOTHING_TO_COMPACT.into());
                }
                // Claude Code's own slash command, through the ordinary prompt path.
                self.submit(0, COMPACT_COMMAND.into(), Vec::new())?;
                self.compact_pending = true;
            }
            Action::PromptWithImages { text, images } => self.submit(0, text, images)?,
            Action::Submit { id, text, images } => self.submit(id, text, images)?,
            Action::ActivityPrompt {
                id,
                text,
                mode,
                expected_turn,
            } => {
                let result = (|| {
                    provider::validate_prompt(&text, &[])?;
                    if mode == SubmissionMode::Steer {
                        if expected_turn.is_none()
                            || self.state.turn_id != expected_turn
                            || !self.state.status.busy()
                        {
                            return Err("The active turn changed. Your reply was not sent; review it before sending again.".into());
                        }
                        self.state.queued_prompts.push(QueuedPrompt {
                            id,
                            text,
                            images: Vec::new(),
                        });
                        self.action(Action::SteerQueued(id))
                    } else {
                        if mode == SubmissionMode::Send
                            && (self.state.status.busy() || self.state.turn_id.is_some())
                        {
                            return Err("The session started working. Your reply was not sent; queue it when ready.".into());
                        }
                        self.action(Action::ProjectPrompt { id, text })
                    }
                })();
                if let Err(error) = result {
                    if !self.state.queue_uncertain.contains(&id) {
                        self.state.queued_prompts.retain(|prompt| prompt.id != id);
                    }
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::ProjectPrompt { id, text } => {
                provider::validate_prompt(&text, &[])?;
                if self.state.status == Status::Disconnected {
                    self.state.rejected_submission = Some(id);
                    return Err("Reconnect before sending a prompt".into());
                }
                self.state.queued_prompts.push(QueuedPrompt {
                    id,
                    text,
                    images: Vec::new(),
                });
                self.state.accept_submission(id);
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
                    if let Err(error) = provider::validate_prompt(&text, &images) {
                        self.state.rejected_submission = Some(id);
                        return Err(error);
                    }
                    self.state.saved.clear_submitted_draft(&text, &images);
                    self.state
                        .queued_prompts
                        .push(QueuedPrompt { id, text, images });
                    self.state.accept_submission(id);
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
                    provider::validate_prompt(
                        &reference.serialize(&self.state.saved.draft),
                        &self.state.saved.draft_images,
                    )?;
                }
                self.state.saved.draft_reference = reference;
            }
            Action::RegenerateTitle => {
                self.title_generator.cancel();
                crate::session_title::request_regeneration(&mut self.state.saved);
            }
            Action::SetTitle(title) => {
                if !crate::session_title::set_manual_title(&mut self.state.saved, &title) {
                    return Err("Title must contain 1–80 characters".into());
                }
                self.title_generator.cancel();
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
            Action::SideChat { checkout, chat } => {
                self.state.saved.side_chats.insert(checkout, chat);
            }
            Action::SideChatDraft { checkout, draft } => {
                self.state
                    .saved
                    .side_chats
                    .entry(checkout)
                    .or_default()
                    .draft = draft;
            }
            Action::DraftImages(images) => {
                provider::validate_images(&images)?;
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
                        self.state.context_revision = self.state.context_revision.wrapping_add(1);
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
                self.state.saved.composer = provider::ComposerSettings::default();
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
                self.state.reconnecting = true;
                if let Err(error) = self.start() {
                    self.state.reconnecting = false;
                    return Err(error);
                }
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
        provider::stamp_message_received(&mut item, previous.map(AsRef::as_ref));
        if let Some(existing) = self
            .state
            .saved
            .items
            .iter_mut()
            .find(|old| old["id"] == item["id"])
        {
            *existing = item.into();
        } else {
            self.state.saved.items.push(item);
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
                if let Some(ownership) = self.process.as_mut().and_then(|p| p.ownership.as_mut()) {
                    ownership.claim(session)?;
                }
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
                if pending.as_deref() == Some("mcp_status") {
                    if response["subtype"] == "success" {
                        self.observe_mcp_servers(&response["response"]["mcpServers"]);
                    } else {
                        self.mcp_status_supported = false;
                    }
                    return Ok(());
                }
                if let Some(revision) = pending
                    .as_deref()
                    .and_then(|value| value.strip_prefix("get_context_usage:"))
                {
                    self.state.context_query_pending = self
                        .pending
                        .values()
                        .any(|kind| kind.starts_with("get_context_usage:"));
                    let current = format!(
                        "{}:{}",
                        self.state.context_revision,
                        self.state.resolved_model.as_deref().unwrap_or_default()
                    );
                    if revision != current {
                        return Ok(());
                    }
                    if response["subtype"] == "success" {
                        let data = &response["response"];
                        if let Some((usage, details)) = context_measurement(data) {
                            self.state
                                .record_context(Some(usage.used), Some(usage.window));
                            self.state.saved.context_details = Some(details);
                            self.state.context_query_error = None;
                        } else {
                            self.state.context_query_error =
                                Some("Claude did not return a usable context measurement.".into());
                        }
                    } else {
                        self.state.context_query_error = Some(
                            response["error"]
                                .as_str()
                                .unwrap_or(
                                    "Context details are unavailable with this Claude version.",
                                )
                                .into(),
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
                if self.configuring
                    && !self
                        .pending
                        .values()
                        .any(|kind| kind != "mcp_status" && !kind.starts_with("get_context_usage:"))
                {
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
                    self.state.reconnecting = false;
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
                        self.state.models=(models.iter().map(|m| json!({"model":m["value"],"resolvedModel":m["resolvedModel"],"displayName":m["displayName"],"description":m["description"],"supportsFastMode":m["supportsFastMode"],"supportedReasoningEfforts":m["supportedEffortLevels"].as_array().map(|values| values.iter().map(|v| json!({"reasoningEffort":v})).collect::<Vec<_>>()).unwrap_or_default()})).collect::<Vec<_>>()).into();
                    }
                    if let Some(key) = &self.model_catalog_key {
                        crate::model_catalog::remember(key, &self.state.models);
                    }
                    if self.state.saved.composer.speed == Some(provider::Speed::Fast) {
                        self.reconfigure = true;
                    }
                    if let Some(speed) = data["fast_mode_state"].as_str() {
                        self.state.reported_speed = Some(speed.into());
                        self.state.speed_disabled_reason = data["fast_mode_disabled_reason"]
                            .as_str()
                            .map(str::to_owned);
                    }
                    if let Some(commands) = data["commands"].as_array() {
                        self.state.skills = (commands
                            .iter()
                            .map(|v| json!({"name":v["name"],"description":v["description"]}))
                            .collect::<Vec<_>>())
                        .into();
                    }
                }
            }
            "system" if value["subtype"] == "session_state_changed" => {
                self.tracks_session_state = true;
                self.session_idle = value["state"] == "idle";
                match value["state"].as_str() {
                    Some("idle") if !self.configuring => {
                        if let Some(status) = self.pending_completion.take() {
                            self.settle_root_items(status);
                            self.state.finish_work();
                            self.state.status = status;
                            if status == Status::Done {
                                self.state.completed_for_review();
                            }
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
                if self.state.compacting {
                    self.state.context_measurement_live = false;
                    self.state.saved.context_details = None;
                    self.state.context_revision = self.state.context_revision.wrapping_add(1);
                }
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
                self.observe_mcp_servers(&value["mcp_servers"]);
                let model = value["model"].as_str().map(str::to_owned);
                if self.state.resolved_model != model {
                    self.state.context_measurement_live = false;
                    self.state.context_revision = self.state.context_revision.wrapping_add(1);
                    self.state.saved.context_details = None;
                }
                self.state.resolved_model = model;
                if let Some(mode) = value["permissionMode"].as_str() {
                    self.effective_permission_mode(mode);
                }
                self.state.resolved_effort = self.state.effort.clone();
                if self.state.skills.is_empty() {
                    if let Some(commands) = value["slash_commands"].as_array() {
                        self.state.skills = (commands
                            .iter()
                            .map(|v| json!({"name":v,"description":"Claude Code command"}))
                            .collect::<Vec<_>>())
                        .into();
                    }
                }
            }
            "control_request" => {
                let id = value["request_id"].clone();
                let request = &value["request"];
                if self.side_chat {
                    if request["subtype"] == "can_use_tool" {
                        let allowed = matches!(
                            request["tool_name"].as_str(),
                            Some("Read" | "Glob" | "Grep")
                        );
                        return self.respond(id, if allowed {
                            json!({"behavior":"allow","updatedInput":request["input"]})
                        } else {
                            json!({"behavior":"deny","message":"Side chat only supports Read, Glob and Grep."})
                        });
                    }
                    return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Side chat does not expose host interactions."}}));
                }
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
                // This exact private tool only records UI metadata and cannot
                // access files, commands or permissions. Other tools retain the
                // user's existing permission decisions.
                if request["tool_name"] == "mcp__grove_activity__report_activity"
                    && crate::activity::validate_report(&request["input"]).is_ok()
                {
                    return self.respond(
                        id,
                        json!({"behavior":"allow","updatedInput":request["input"]}),
                    );
                }
                // Private visual tools enforce host-read permissions and root ownership
                // in their own session-bound endpoint; they grant no provider capability.
                if matches!(
                    request["tool_name"].as_str(),
                    Some("mcp__grove_visuals__render_html" | "mcp__grove_visuals__preview_html")
                ) && serde_json::from_value::<crate::visuals::Input>(request["input"].clone())
                    .is_ok()
                {
                    return self.respond(
                        id,
                        json!({"behavior":"allow","updatedInput":request["input"]}),
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
                self.state.account_usage =
                    crate::account_usage::claude(info, crate::account_usage::now());
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
                                && (notice.kind == provider::SessionNoticeKind::UsageLimit
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
                        if self
                            .context_model
                            .as_deref()
                            .is_some_and(|current| current != model)
                        {
                            self.state.saved.context = None;
                        }
                        self.context_model = Some(model.to_owned());
                        self.state.resolved_model = Some(model.to_owned());
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
                let result_failed =
                    value["is_error"] == true || value["terminal_reason"] == "api_error";
                if let Some(models) = value["modelUsage"].as_object() {
                    let window = self
                        .context_model
                        .as_deref()
                        .and_then(|model| models.get(model))
                        .and_then(|entry| entry["contextWindow"].as_u64())
                        .or_else(|| {
                            // Multiple entries can include subagents; never borrow a
                            // larger child's capacity for the main conversation.
                            if models.len() == 1 {
                                models
                                    .values()
                                    .next()
                                    .and_then(|entry| entry["contextWindow"].as_u64())
                            } else {
                                None
                            }
                        });
                    // A full query can report a lower effective compaction window.
                    // A result's model capacity must not replace that denominator.
                    if !self
                        .state
                        .saved
                        .context_details
                        .as_ref()
                        .is_some_and(|details| details.source == provider::ContextMeasurement::Full)
                    {
                        self.state.record_context(None, window);
                    }
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
                } else if result_failed {
                    Status::Failed
                } else {
                    Status::Done
                };
                if result_failed && !self.interrupting {
                    // The terminal result often has a generic subtype. Retain
                    // a preceding explicit rejection's category/reset metadata.
                    let usage_notice = self
                        .state
                        .effective_notice()
                        .filter(|notice| notice.kind == provider::SessionNoticeKind::UsageLimit)
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
                            .or_else(|| {
                                value["result"]
                                    .as_str()
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned)
                            })
                            .or_else(|| self.state.error.clone())
                            .unwrap_or_else(|| {
                                format!(
                                    "Claude Code ended with {}",
                                    value["terminal_reason"]
                                        .as_str()
                                        .or_else(|| value["subtype"].as_str())
                                        .unwrap_or("an unknown error")
                                )
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
                } else if !result_failed && self.state.effective_notice().is_some() {
                    self.state.clear_error();
                }
                if !result_failed && !self.interrupting {
                    crate::rich_plan::import_completed(&mut self.state.saved);
                }
                if self.tracks_session_state && !self.session_idle {
                    self.pending_completion = Some(self.state.status);
                    self.state.status = Status::Working;
                    // result can be flushed before background continuations finish;
                    // session_state_changed idle is the authoritative boundary.
                    self.state.turn_id = Some("awaiting-session-idle".into());
                } else {
                    self.settle_root_items(self.state.status);
                    self.state.finish_work();
                    if self.state.status == Status::Done {
                        self.state.completed_for_review();
                    }
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
    /// Only an authoritative terminal boundary may settle still-open root tools.
    /// Real tool results arriving before idle retain their own status and output.
    fn settle_root_items(&mut self, status: Status) {
        let terminal = match status {
            Status::Interrupted => "interrupted",
            Status::Failed => "failed",
            _ => "completed",
        };
        for item in &mut self.state.saved.items {
            if item["status"] == "inProgress" {
                item["status"] = json!(terminal);
            }
        }
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
                    .rev()
                    .find(|item| item["id"] == *id)
                else {
                    return;
                };
                let delta = &event["delta"];
                if let Some(text) = delta["text"].as_str() {
                    provider::append_json_text(&mut item["text"], text);
                }
                if let Some(text) = delta["thinking"].as_str() {
                    if !item["content"].is_array()
                        || item["content"].as_array().is_some_and(Vec::is_empty)
                    {
                        item["content"] = json!([""]);
                    }
                    provider::append_json_text(&mut item["content"][0], text);
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
mod tests;
