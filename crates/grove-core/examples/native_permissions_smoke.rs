//! Live production-transport permission check. Run only with empty disposable cwd/config dirs.
//! Never approves a tool; an unattempted tool is inconclusive, not a successful denial test.
use grove_core::codex::{self, Action, Connection, Provider, SavedSession, Snapshot, Status};
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn next(connection: &Connection, deadline: Instant) -> Result<Snapshot, String> {
    let state = connection
        .updates
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| format!("Timed out waiting for provider: {e}"))?;
    if matches!(state.status, Status::Failed | Status::Disconnected) {
        return Err(state.error.unwrap_or_else(|| state.status.label().into()));
    }
    Ok(state)
}
fn until(
    connection: &Connection,
    deadline: Instant,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    loop {
        let state = next(connection, deadline)?;
        if predicate(&state) {
            return Ok(state);
        }
    }
}
fn stop(connection: &Connection) -> Result<(), String> {
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("Provider shutdown: {e}"))
}
fn policies(label: &str, state: &Snapshot) {
    println!(
        "{label}: sandbox={} approval_policy={} resolved_access={:?} explicit_access={:?}",
        state.sandbox.as_ref().unwrap_or(&Value::Null),
        state.approval_policy.as_ref().unwrap_or(&Value::Null),
        state.resolved_access,
        state.access
    );
}
fn run(
    connection: &Connection,
    target: &Path,
    deadline: Instant,
    claude: bool,
) -> Result<SavedSession, String> {
    let ready = until(connection, deadline, |s| s.status == Status::Idle)?;
    policies("INITIALIZED", &ready);
    connection.handle.send(Action::Prompt(format!(
        "Permission transport test in this disposable directory. Attempt exactly one Bash/shell tool call: `printf permission-smoke > {}`. Actually invoke the tool even if it requires user approval. Do not use any other tools or paths. If denied, do not retry or find a workaround. Then report the denial.",
        target.file_name().unwrap().to_string_lossy()
    )))?;
    let mut declined = std::collections::HashSet::new();
    let mut cancellation_sent = false;
    let done = loop {
        let state = next(connection, deadline)?;
        for approval in &state.approvals {
            let id = approval["id"].clone();
            if declined.insert(id.to_string()) {
                println!(
                    "APPROVAL method={} params={}",
                    approval["method"], approval["params"]
                );
                let choices = grove_core::controls::approval_choices(approval);
                let choice = ["deny", "cancel"]
                    .into_iter()
                    .find(|candidate| choices.iter().any(|offered| offered.id == *candidate))
                    .ok_or("Provider offered no supported rejection choice; refusing to approve")?;
                cancellation_sent |= choice == "cancel";
                connection.handle.send(Action::RespondApproval {
                    id,
                    choice: choice.into(),
                    input: None,
                    feedback: claude
                        .then(|| "Permission smoke: decline this operation; do not retry.".into()),
                })?;
                println!("SENT production RespondApproval choice={choice}");
            }
        }
        if target.exists() {
            return Err("Restrictive boundary failed: target file exists".into());
        }
        if !declined.is_empty() && state.error.is_some() {
            return Err(format!(
                "Approval response failed: {}",
                state.error.as_deref().unwrap()
            ));
        }
        let settled = state.status == Status::Done
            || (cancellation_sent
                && matches!(state.status, Status::Idle | Status::Interrupted)
                && state.turn_id.is_none());
        if settled && state.approvals.is_empty() && state.accepted_prompts > ready.accepted_prompts
        {
            println!(
                "APPROVALS_RESOLVED pending=0 status={} turn_active={}",
                state.status.label(),
                state.turn_id.is_some()
            );
            break state;
        }
    };
    let tools: Vec<_> = done
        .saved
        .items
        .iter()
        .filter(|i| {
            matches!(
                i["type"].as_str(),
                Some("commandExecution" | "fileChange" | "mcpToolCall")
            )
        })
        .collect();
    for item in &tools {
        println!("TOOL_EVENT {item}");
    }
    // An assistant saying it cannot write is not evidence of enforcement.
    let denied_tool = tools.iter().any(|item| {
        let text = item.to_string().to_lowercase();
        [
            "permission denied",
            "operation not permitted",
            "denied by",
            "rejected",
            "not allowed",
        ]
        .iter()
        .any(|reason| text.contains(reason))
    });
    if declined.is_empty() && !denied_tool {
        return Err("INCONCLUSIVE: no real approval request or tool denial observed (model refusal does not establish the provider boundary)".into());
    }
    println!(
        "PASS real permission boundary: declined_requests={} target_absent=true",
        declined.len()
    );
    let restricted = if claude { "plan" } else { "read-only" };
    connection.handle.send(Action::Configure {
        model: None,
        effort: None,
        access: Some(restricted.into()),
    })?;
    // A subsequent accepted turn proves the configuration operation reached the transport.
    let accepted = done.accepted_prompts;
    connection.handle.send(Action::Prompt(
        "Reply only PERMISSION_MODE_CHECK. Do not invoke tools or change files.".into(),
    ))?;
    let changed = until(connection, deadline, |s| {
        s.status == Status::Done && s.accepted_prompts > accepted
    })?;
    policies("AFTER_RESTRICTIVE_CONFIGURE", &changed);
    if changed.resolved_access.as_deref() != Some(restricted) {
        return Err(format!(
            "Provider did not acknowledge restrictive mode {restricted}: {:?}",
            changed.resolved_access
        ));
    }
    if target.exists() {
        return Err("Target appeared after restrictive configuration".into());
    }
    Ok(changed.saved)
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 6
        || args[0] != "--provider"
        || args[2] != "--cwd"
        || args[4] != "--timeout-seconds"
    {
        return Err("Usage: GROVE_CONFIG_DIR=<empty disposable dir> cargo run -p grove-core --example native_permissions_smoke -- --provider codex|claude --cwd <empty disposable dir> --timeout-seconds 180".into());
    }
    let claude = match args[1].as_str() {
        "claude" => true,
        "codex" => false,
        _ => return Err("Unknown provider".into()),
    };
    let timeout: u64 = args[5].parse().map_err(|_| "Invalid timeout")?;
    if !(10..=600).contains(&timeout) {
        return Err("Timeout must be between 10 and 600 seconds".into());
    }
    let config =
        std::env::var("GROVE_CONFIG_DIR").map_err(|_| "Set disposable GROVE_CONFIG_DIR")?;
    let cwd = fs_err::canonicalize(&args[3]).map_err(|e| e.to_string())?;
    for dir in [Path::new(&config), cwd.as_path()] {
        if !dir.is_dir()
            || fs_err::read_dir(dir)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err(format!(
                "Expected existing empty disposable directory: {}",
                dir.display()
            ));
        }
    }
    if fs_err::canonicalize(&config).map_err(|e| e.to_string())? == cwd {
        return Err("Use separate config and cwd directories".into());
    }
    grove_core::env_path::ensure_login_path();
    let config_args = if claude {
        let help = std::process::Command::new("claude")
            .arg("--help")
            .output()
            .map_err(|e| format!("Cannot discover Claude permission modes: {e}"))?;
        if !help.status.success() {
            return Err("Claude --help failed while discovering permission modes".into());
        }
        let help = String::from_utf8_lossy(&help.stdout);
        let permission_help = help
            .split("--permission-mode <mode>")
            .nth(1)
            .and_then(|section| section.split("\n  --").next())
            .ok_or("Claude --help did not advertise permission-mode choices")?;
        let manual = if permission_help.contains("\"manual\"") {
            "manual"
        } else if permission_help.contains("\"default\"") {
            "default"
        } else {
            return Err(
                "Installed Claude CLI advertises neither manual nor default permission mode".into(),
            );
        };
        println!("CLAUDE restrictive interactive permission mode={manual}");
        vec![
            "--permission-mode".into(),
            manual.into(),
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
    let key = grove_core::identity::random_id()?;
    let saved: SavedSession = serde_json::from_value(json!({
        "key":key,"project":"Permission smoke","cwd":cwd,"label":"Permission smoke",
        "provider":if claude {"claude"} else {"codex"}, "roots":[],"thread_id":null,"items":[],
        "config_args":config_args,"native_config_version":1
    }))
    .map_err(|e| e.to_string())?;
    // Use the enum explicitly too, to catch provider serialization drift before spawning.
    if saved.provider
        != if claude {
            Provider::Claude
        } else {
            Provider::Codex
        }
    {
        return Err("Provider mismatch".into());
    }
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let target = cwd.join("permission-smoke-target.txt");
    let connection = codex::connect(saved, Vec::new());
    let result = run(&connection, &target, deadline, claude);
    let stopped = stop(&connection);
    let saved = result?;
    stopped?;
    let restored = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|s| s.key == key)
        .ok_or("Missing persisted smoke session")?;
    if restored.config_args != saved.config_args
        || restored.native_config_version != 1
        || restored.composer.access != saved.composer.access
    {
        return Err("Explicit native permission configuration changed during persistence".into());
    }
    let resumed = codex::connect(restored, Vec::new());
    let resumed_result =
        until(&resumed, deadline, |s| s.status == Status::Idle).and_then(|state| {
            policies("RECONNECTED", &state);
            if state.resolved_access != saved.composer.access {
                return Err("Reconnect lost restrictive access".into());
            }
            if target.exists() {
                return Err("Target exists after reconnect".into());
            }
            println!("PASS explicit native permissions persisted and resumed");
            Ok(())
        });
    let stopped = stop(&resumed);
    resumed_result?;
    stopped
}
