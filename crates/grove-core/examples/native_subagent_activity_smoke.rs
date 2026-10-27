//! Live, read-only child activity smoke through Grove's production provider adapters.
//! Supply distinct fresh empty cwd/config directories; uses installed provider auth.
use grove_core::codex::{
    self, Action, Connection, SavedSession, Snapshot, Status, SubagentActivityKind, SubagentStatus,
    SubagentTranscriptAvailability,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const REPORT: &str = "GROVE_CHILD_FINAL_REPORT";

fn run(
    connection: &Connection,
    deadline: Instant,
    claude: bool,
    scenario: &str,
) -> Result<(), String> {
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
    let delegation = match scenario {
        "background" => {
            "Delegate exactly one general-purpose child using Agent with run_in_background true. Wait for its completion notification, even if your first turn ends before it finishes."
        }
        "nested" => {
            "Delegate exactly one foreground general-purpose child using Agent. That child's sole task is to delegate exactly one foreground general-purpose grandchild using Agent and wait for it. The grandchild must list this directory. Both descendants must give their own final report. Total descendants must be exactly two, maximum depth two."
        }
        _ if claude => {
            "Delegate exactly one foreground general-purpose child using Agent, not background."
        }
        _ => "Delegate exactly one foreground child using spawn_agent.",
    };
    connection.handle.send(Action::Prompt(format!(
        "Verify read-only subagent activity. {delegation} \
         The leaf child must use one normal read-only tool to list files in this current empty \
         working directory. Every child must finish with a final report containing the exact marker \
         {REPORT} plus a concise finding. Include that instruction in every delegated prompt. \
         Do not access any other directory. Do not use git, network, mutations, permission escalation, \
         or bypass modes. Do not create further children beyond those explicitly requested. \
         Parent: do not list files yourself; wait for every child to finish and give a concise report."
    )))?;
    let expected = if scenario == "nested" { 2 } else { 1 };
    let mut roster = HashMap::new();
    let mut event_ids = HashSet::new();
    let mut canonical_tools = 0;
    let mut canonical_messages = 0;
    let mut tools = 0;
    let mut commentary = 0;
    let mut truncated = false;
    let mut last_status = ready.status;
    let mut reported = HashSet::new();
    loop {
        let state = next().map_err(|error| {
            let completed = roster.values().filter(|s| **s == SubagentStatus::Completed).count();
            let working = roster.values().filter(|s| **s == SubagentStatus::Working).count();
            let unknown = roster.values().filter(|s| **s == SubagentStatus::Unknown).count();
            format!("{error}; parent_status={} children={} completed={completed} working={working} unknown={unknown} tool_events={tools} commentary_events={commentary} final_reports={}", last_status.label(), roster.len(), reported.len())
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
            if activity.items.iter().any(|item| {
                item["type"] == "agentMessage"
                    && item["phase"] == "final_answer"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(REPORT))
            }) {
                reported.insert(id.clone());
            }
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
        if roster.len() > expected {
            return Err("Provider created more than the requested bounded descendants".into());
        }
        if state.status == Status::Done
            && state.accepted_prompts > ready.accepted_prompts
            && roster.len() == expected
            && completed == expected
            && reported.len() == expected
            && tools > 0
            && canonical_tools > 0
            && canonical_messages > 0
        {
            let agents: HashMap<_, _> = state
                .saved
                .work_periods
                .iter()
                .flat_map(|p| &p.subagents)
                .map(|a| (a.id.clone(), a.clone()))
                .collect();
            for agent in agents.values() {
                if agent.name.trim().is_empty()
                    || (claude
                        && (agent.role.as_deref().is_none_or(str::is_empty)
                            || agent.task.as_deref().is_none_or(str::is_empty)))
                {
                    return Err(format!("Missing child metadata: {agent:?}"));
                }
            }
            let saved: SavedSession = serde_json::from_slice(
                &serde_json::to_vec(&state.saved).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let restored = Snapshot::new(saved);
            for (id, agent) in &agents {
                let restored_agent = restored
                    .saved
                    .work_periods
                    .iter()
                    .flat_map(|p| &p.subagents)
                    .rev()
                    .find(|a| &a.id == id)
                    .ok_or("Restored child missing from roster")?;
                if serde_json::to_value(agent).map_err(|e| e.to_string())?
                    != serde_json::to_value(restored_agent).map_err(|e| e.to_string())?
                {
                    return Err(format!("Restored child metadata/status changed: {id}"));
                }
                if claude {
                    let live = state
                        .subagent_activity
                        .get(id)
                        .ok_or("Live child activity missing")?;
                    let historical = restored
                        .subagent_activity
                        .get(id)
                        .ok_or("Restored Claude transcript missing")?;
                    if restored.subagent_transcript_availability(id)
                        != SubagentTranscriptAvailability::Restored
                    {
                        return Err(format!("Restored child availability is not Restored: {id}"));
                    }
                    if historical.items.iter().any(|item| {
                        matches!(
                            item["type"].as_str(),
                            Some("reasoning" | "reasoningMessage")
                        )
                    }) {
                        return Err(format!("Private reasoning persisted for child: {id}"));
                    }
                    if historical.items != live.items {
                        return Err(format!("Restored Claude transcript changed: {id}"));
                    }
                    if historical.observed_at.is_some()
                        || !historical.events.is_empty()
                        || historical.refreshing
                    {
                        return Err(format!("Restore fabricated fresh observations: {id}"));
                    }
                }
            }
            println!(
                "PASS provider={} scenario={scenario} children={expected} completed={completed} final_reports={} tool_events={tools} commentary_events={commentary} truncated={truncated} max_child_canonical_tools={canonical_tools} max_child_canonical_messages={canonical_messages} metadata_roundtrip=true claude_metadata={} claude_transcript_restore={} parent_thread_unchanged=true",
                if claude { "claude" } else { "codex" },
                reported.len(),
                if claude {
                    "verified"
                } else {
                    "optional_provider_metadata"
                },
                if claude { "verified" } else { "not_applicable" }
            );
            return Ok(());
        }
    }
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !matches!(args.len(), 6 | 8)
        || args[0] != "--provider"
        || args[2] != "--cwd"
        || args[4] != "--timeout-seconds"
    {
        return Err("Usage: GROVE_CONFIG_DIR=<fresh empty dir> native_subagent_activity_smoke --provider codex|claude --cwd <fresh empty dir> --timeout-seconds 180 [--scenario foreground|background|nested]".into());
    }
    let claude = match args[1].as_str() {
        "claude" => true,
        "codex" => false,
        _ => return Err("Unknown provider".into()),
    };
    let scenario = if args.len() == 8 {
        if args[6] != "--scenario"
            || !matches!(args[7].as_str(), "foreground" | "background" | "nested")
        {
            return Err("Invalid scenario; use foreground, background, or nested".into());
        }
        args[7].as_str()
    } else {
        "foreground"
    };
    if !claude && scenario != "foreground" {
        return Err("Background/nested scenarios currently target Claude only".into());
    }
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
        scenario,
    );
    let shutdown = connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("Shutdown failed: {e}"));
    result?;
    shutdown
}
