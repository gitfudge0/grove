//! Instrumented production-transport child permission smoke; never grants approval.
//! Use fresh, distinct disposable config/cwd directories. Optional JSONL wire tee entries:
//! {"direction":"provider"|"host","message":<JSON>}. A tee must not rewrite protocol bytes.
use grove_core::codex::{self, Action, Connection, SavedSession, Snapshot, Status};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, Instant},
};

fn next(connection: &Connection, deadline: Instant) -> Result<Snapshot, String> {
    let state = connection
        .updates
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| format!("Timed out waiting for child transport: {e}"))?;
    if matches!(state.status, Status::Failed | Status::Disconnected) {
        return Err(state.error.unwrap_or_else(|| state.status.label().into()));
    }
    Ok(state)
}
fn stop(connection: &Connection) -> Result<(), String> {
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("Shutdown failed: {e}"))
}
fn wire_evidence(path: Option<&str>, children: &HashSet<String>) -> Result<bool, String> {
    let Some(path) = path else {
        return Ok(false);
    };
    let mut attempted = HashSet::new();
    let mut denied = false;
    for line in fs_err::read_to_string(path)
        .map_err(|e| e.to_string())?
        .lines()
    {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["direction"] != "provider" {
            continue;
        }
        let m = &entry["message"];
        if let Some(parent) = m["parent_tool_use_id"]
            .as_str()
            .filter(|id| children.contains(*id))
        {
            if let Some(blocks) = m["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"] == "tool_use" && block["name"] == "Bash" {
                        if let Some(id) = block["id"].as_str() {
                            attempted.insert((parent.to_owned(), id.to_owned()));
                            println!("DESCENDANT_TOOL_ATTEMPT {m}");
                        }
                    }
                    if block["type"] == "tool_result"
                        && block["is_error"] == true
                        && block["tool_use_id"].as_str().is_some_and(|id| {
                            attempted.contains(&(parent.to_owned(), id.to_owned()))
                        })
                    {
                        let text = block.to_string().to_lowercase();
                        if ["permission", "denied", "rejected", "not allowed"]
                            .iter()
                            .any(|word| text.contains(word))
                        {
                            denied = true;
                            println!("DESCENDANT_PROVIDER_DENIAL {m}");
                        }
                    }
                }
            }
        }
    }
    Ok(denied)
}
fn run(
    connection: &Connection,
    target: &Path,
    deadline: Instant,
    claude: bool,
    trace: Option<&str>,
) -> Result<(), String> {
    let ready = loop {
        let s = next(connection, deadline)?;
        if s.status == Status::Idle {
            break s;
        }
    };
    println!(
        "INITIALIZED parent_thread={:?} sandbox={} approval_policy={} access={:?}",
        ready.saved.thread_id,
        ready.sandbox.as_ref().unwrap_or(&Value::Null),
        ready.approval_policy.as_ref().unwrap_or(&Value::Null),
        ready.resolved_access
    );
    connection.handle.send(Action::Prompt(format!(
        "Run a foreground child-agent permission transport test. {escalation} You MUST delegate to exactly one general-purpose child agent (Agent tool on Claude; spawn_agent on Codex). Do not execute shell or edit files yourself. Do not use an Explore agent or background mode. Child task: Invoke exactly one Bash/shell tool call with command `printf child-permission-smoke > {}` in cwd {}. Actually invoke the tool even if it needs approval. Do not access any other path or invoke any other tools. If denied, stop immediately without retry/workaround. Parent: wait for the foreground child to finish, then report its result. Never perform its command yourself or retry it.",
        target.file_name().unwrap().to_string_lossy(), target.parent().unwrap().display(),
        escalation = if claude { "" } else { "Codex child directive: invoke exec_command exactly once with sandbox_permissions=require_escalated and a justification asking permission for the specified write. Do not attempt the default sandbox first. Request the real permission even though current sandbox is read-only; do not infer or invent a denial. Parent must include this directive verbatim in the child task." })))?;
    let mut children = HashSet::new();
    let mut seen_agents = HashSet::new();
    let mut approvals = HashSet::new();
    let mut child_rejection_ids = HashSet::new();
    let mut cancelled = false;
    let settled = loop {
        let state = next(connection, deadline)?;
        for agent in codex::active_subagents(&state.saved.work_periods) {
            children.insert(agent.id.clone());
            let event = serde_json::to_string(&agent).map_err(|e| e.to_string())?;
            if seen_agents.insert(event.clone()) {
                println!("CHILD_SNAPSHOT {event}");
            }
        }
        for approval in &state.approvals {
            let id = approval["id"].clone();
            if !approvals.insert(id.to_string()) {
                continue;
            }
            println!("APPROVAL_RAW {approval}");
            let params = &approval["params"];
            let request = &approval["claudeRequest"];
            let owner = state.interaction_owners.get(&id.to_string());
            println!("APPROVAL_OWNER {owner:?}");
            let attributed_child = owner.is_some_and(|owner| {
                owner.agent.is_some()
                    && ready.saved.thread_id.as_deref() != Some(owner.thread.as_str())
                    && approval["groveAgent"].as_str().is_some()
            });
            let owned = attributed_child
                || [
                    params.get("threadId"),
                    request.get("agent_id"),
                    request.get("parent_tool_use_id"),
                    approval.get("parent_tool_use_id"),
                ]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|id| children.contains(id));
            if owned {
                child_rejection_ids.insert(id.to_string());
            }
            let choices = grove_core::controls::approval_choices(approval);
            let choice = ["deny", "cancel"]
                .into_iter()
                .find(|candidate| choices.iter().any(|offered| offered.id == *candidate))
                .ok_or("No provider-supported rejection available; refusing approval")?;
            cancelled |= choice == "cancel";
            connection.handle.send(Action::RespondApproval {
                id,
                choice: choice.into(),
                input: None,
                feedback: claude.then(|| {
                    "Permission smoke rejects this child operation. Stop without retry.".into()
                }),
            })?;
            println!("SENT_REJECTION choice={choice} child_owned={owned}");
        }
        if target.exists() {
            return Err("FAIL: child target was written despite requested rejection".into());
        }
        if state.error.is_some() {
            return Err(format!("Transport error: {:?}", state.error));
        }
        let terminal = state.status == Status::Done
            || (cancelled
                && matches!(state.status, Status::Idle | Status::Interrupted)
                && state.turn_id.is_none());
        if terminal && state.approvals.is_empty() && state.accepted_prompts > ready.accepted_prompts
        {
            break state;
        }
    };
    println!(
        "PARENT_SETTLED status={} thread={:?} turn={:?} pending_approvals={} children={}",
        settled.status.label(),
        settled.saved.thread_id,
        settled.turn_id,
        settled.approvals.len(),
        children.len()
    );
    for item in &settled.saved.items {
        println!("PARENT_ITEM {item}");
    }
    let wire_denied = wire_evidence(trace, &children)?;
    if children.is_empty() {
        return Err("INCONCLUSIVE: no production child membership observed".into());
    }
    if child_rejection_ids.is_empty() && !wire_denied {
        return Err("INCONCLUSIVE: no correlated descendant rejection observed; parent prose and child activity alone are not permission enforcement evidence".into());
    }
    println!("PASS production child rejection: child_owned_rejections={} descendant_provider_denial={} target_absent=true", child_rejection_ids.len(), wire_denied);
    Ok(())
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(args.len() == 6 || args.len() == 8)
        || args[0] != "--provider"
        || args[2] != "--cwd"
        || args[4] != "--timeout-seconds"
        || (args.len() == 8 && args[6] != "--wire-trace")
    {
        return Err("Usage: GROVE_CONFIG_DIR=<fresh empty dir> native_subagent_permissions_smoke --provider codex|claude --cwd <fresh empty dir> --timeout-seconds 180 [--wire-trace <tee JSONL>]".into());
    }
    let claude = match args[1].as_str() {
        "claude" => true,
        "codex" => false,
        _ => return Err("Unknown provider".into()),
    };
    let timeout: u64 = args[5].parse().map_err(|_| "Invalid timeout")?;
    if !(10..=600).contains(&timeout) {
        return Err("Timeout must be 10..600 seconds".into());
    }
    let cwd = fs_err::canonicalize(&args[3]).map_err(|e| e.to_string())?;
    let config = fs_err::canonicalize(
        std::env::var("GROVE_CONFIG_DIR").map_err(|_| "Set fresh disposable GROVE_CONFIG_DIR")?,
    )
    .map_err(|e| e.to_string())?;
    for dir in [&cwd, &config] {
        if !dir.is_dir()
            || fs_err::read_dir(dir)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err(format!("Expected fresh empty directory: {}", dir.display()));
        }
    }
    if cwd == config {
        return Err("Config and cwd must differ".into());
    }
    grove_core::env_path::ensure_login_path();
    let config_args = if claude {
        let help = std::process::Command::new("claude")
            .arg("--help")
            .output()
            .map_err(|e| e.to_string())?;
        if !help.status.success() {
            return Err("Claude --help failed".into());
        }
        let text = String::from_utf8_lossy(&help.stdout);
        let section = text
            .split("--permission-mode <mode>")
            .nth(1)
            .and_then(|s| s.split("\n  --").next())
            .ok_or("No Claude permission choices advertised")?;
        let mode = if section.contains("\"manual\"") {
            "manual"
        } else if section.contains("\"default\"") {
            "default"
        } else {
            return Err("No supported interactive permission mode".into());
        };
        vec![
            "--permission-mode".into(),
            mode.into(),
            "--settings".into(),
            json!({"permissions":{"ask":["Bash"],"deny":["Write","Edit","NotebookEdit"]}})
                .to_string(),
        ]
    } else {
        vec![
            "-c".into(),
            "sandbox_mode=\"read-only\"".into(),
            "-c".into(),
            "approval_policy=\"on-request\"".into(),
        ]
    };
    let saved: SavedSession = serde_json::from_value(json!({"key":grove_core::identity::random_id()?, "project":"Child permissions smoke", "cwd":cwd,"label":"Child permissions smoke", "provider":if claude {"claude"} else {"codex"}, "roots":[],"thread_id":null,"items":[],"config_args":config_args,"native_config_version":1})).map_err(|e|e.to_string())?;
    let connection = codex::connect(saved, vec![]);
    let result = run(
        &connection,
        &cwd.join("child-permission-smoke-target.txt"),
        Instant::now() + Duration::from_secs(timeout),
        claude,
        args.get(7).map(String::as_str),
    );
    let shutdown = stop(&connection);
    result?;
    shutdown
}
