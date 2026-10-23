//! Live, read-only child activity smoke through Grove's production provider adapters.
//! Supply distinct fresh empty cwd/config directories; uses installed provider auth.
use grove_core::codex::{
    self, Action, Connection, SavedSession, Status, SubagentActivityKind, SubagentStatus,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

fn run(connection: &Connection, deadline: Instant, claude: bool) -> Result<(), String> {
    let next = || {
        connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| format!("No provider update before deadline: {e}"))
    };
    let ready = loop {
        let state = next()?;
        if matches!(state.status, Status::Failed | Status::Disconnected) {
            return Err(format!("Initialization failed: {}", state.status.label()));
        }
        if state.status == Status::Idle {
            break state;
        }
    };
    connection.handle.send(Action::Prompt(format!(
        "Verify read-only subagent activity. Delegate exactly one foreground child using {}. \
         The child must use one normal read-only tool to list the files in this current empty \
         working directory, then report a concise finding. Do not access any other directory. \
         Do not use git, network, mutations, permission escalation, or bypass modes. \
         Do not create further children. Parent: do not list files yourself; wait for that child \
         to finish and give a concise final report. Use normal tool defaults.",
        if claude {
            "the Agent tool (general-purpose agent, not background)"
        } else {
            "spawn_agent"
        }
    )))?;
    let mut roster = HashMap::new();
    let mut event_ids = HashSet::new();
    let mut canonical_tools = 0;
    let mut canonical_messages = 0;
    let mut tools = 0;
    let mut commentary = 0;
    let mut truncated = false;
    let mut last_status = ready.status;
    loop {
        let state = next().map_err(|error| {
            let completed = roster.values().filter(|s| **s == SubagentStatus::Completed).count();
            let working = roster.values().filter(|s| **s == SubagentStatus::Working).count();
            let unknown = roster.values().filter(|s| **s == SubagentStatus::Unknown).count();
            format!("{error}; parent_status={} children={} completed={completed} working={working} unknown={unknown} tool_events={tools} commentary_events={commentary}", last_status.label(), roster.len())
        })?;
        last_status = state.status;
        if matches!(state.status, Status::Failed | Status::Disconnected) || state.error.is_some() {
            return Err(format!(
                "Provider transport failed: {}",
                state.status.label()
            ));
        }
        if !state.approvals.is_empty()
            || !state.questions.is_empty()
            || !state.elicitations.is_empty()
        {
            return Err(
                "Unexpected interaction; smoke does not approve tools or answer provider questions"
                    .into(),
            );
        }
        if ready.saved.thread_id.is_some() && state.saved.thread_id != ready.saved.thread_id {
            return Err("Parent thread identity changed while observing child activity".into());
        }
        for agent in state.saved.work_periods.iter().flat_map(|p| &p.subagents) {
            roster.insert(agent.id.clone(), agent.status);
        }
        for (id, activity) in &state.subagent_activity {
            if !roster.contains_key(id) {
                return Err(
                    "Observed activity does not belong to the parent's child roster".into(),
                );
            }
            if activity.refresh_error {
                return Err("Child observation refresh reported an error".into());
            }
            canonical_tools = canonical_tools.max(
                activity
                    .items
                    .iter()
                    .filter(|item| {
                        !matches!(item["type"].as_str(), Some("agentMessage" | "userMessage"))
                    })
                    .count(),
            );
            canonical_messages = canonical_messages.max(
                activity
                    .items
                    .iter()
                    .filter(|item| item["type"] == "agentMessage")
                    .count(),
            );
            truncated |= activity.truncated;
            for event in &activity.events {
                if event.id.is_empty()
                    || event.text.trim().is_empty()
                    || event.observed_at > Instant::now()
                {
                    return Err("Invalid observed child event metadata".into());
                }
                if event_ids.insert((id.clone(), event.id.clone())) {
                    match event.kind {
                        SubagentActivityKind::Tool => tools += 1,
                        SubagentActivityKind::Commentary => commentary += 1,
                    }
                }
            }
        }
        let completed = roster
            .values()
            .filter(|s| **s == SubagentStatus::Completed)
            .count();
        if roster.values().any(|s| *s == SubagentStatus::Failed) {
            return Err("Provider observed child failure".into());
        }
        if roster.len() > 1 {
            return Err("Provider created more than the requested one child".into());
        }
        if state.status == Status::Done
            && state.accepted_prompts > ready.accepted_prompts
            && roster.len() == 1
            && completed == 1
            && tools > 0
            && canonical_tools > 0
            && canonical_messages > 0
        {
            println!("PASS provider={} children=1 completed=1 tool_events={tools} commentary_events={commentary} truncated={truncated} canonical_tools={canonical_tools} canonical_messages={canonical_messages} parent_thread_unchanged=true", if claude {"claude"} else {"codex"});
            return Ok(());
        }
    }
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 6
        || args[0] != "--provider"
        || args[2] != "--cwd"
        || args[4] != "--timeout-seconds"
    {
        return Err("Usage: GROVE_CONFIG_DIR=<fresh empty dir> native_subagent_activity_smoke --provider codex|claude --cwd <fresh empty dir> --timeout-seconds 180".into());
    }
    let claude = match args[1].as_str() {
        "claude" => true,
        "codex" => false,
        _ => return Err("Unknown provider".into()),
    };
    let seconds: u64 = args[5].parse().map_err(|_| "Invalid timeout")?;
    if !(120..=180).contains(&seconds) {
        return Err("Timeout must be 120..180 seconds".into());
    }
    let cwd = fs_err::canonicalize(&args[3]).map_err(|e| e.to_string())?;
    let config = fs_err::canonicalize(
        std::env::var("GROVE_CONFIG_DIR").map_err(|_| "Set disposable GROVE_CONFIG_DIR")?,
    )
    .map_err(|e| e.to_string())?;
    for path in [&cwd, &config] {
        if !path.is_dir()
            || fs_err::read_dir(path)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err(format!(
                "Expected fresh empty directory: {}",
                path.display()
            ));
        }
    }
    if cwd.starts_with(&config) || config.starts_with(&cwd) {
        return Err("Config and cwd must be distinct, non-nested directories".into());
    }
    grove_core::env_path::ensure_login_path();
    let saved: SavedSession = serde_json::from_value(json!({
        "key": grove_core::identity::random_id()?, "project": "Child activity smoke",
        "cwd": cwd, "label": "Child activity smoke", "provider": if claude {"claude"} else {"codex"},
        "roots": [], "thread_id": null, "items": [], "config_args": [], "native_config_version": 1
    })).map_err(|e| e.to_string())?;
    let connection = codex::connect(saved, vec![]);
    let result = run(
        &connection,
        Instant::now() + Duration::from_secs(seconds),
        claude,
    );
    let shutdown = connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("Shutdown failed: {e}"));
    result?;
    shutdown
}
