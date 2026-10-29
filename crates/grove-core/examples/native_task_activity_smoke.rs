//! Opt-in real-provider task/activity smoke. Requires disposable cwd/config and authenticated CLI.
use grove_core::{
    activity::{self, StepStatus},
    codex::{self, Action, Connection, Provider, SavedSession, Snapshot, Status},
    tasks::{self, TaskStatus},
};
use std::time::{Duration, Instant};

const FIRST: &str = "Inspect task activity ownership";
const SECOND: &str = "Verify task activity retention";
const MARKERS: [&str; 2] = ["GROVE_TASK_FIRST_PROGRESS", "GROVE_TASK_SECOND_PROGRESS"];

fn wait(
    connection: &Connection,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut last = "no provider snapshot".to_owned();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("Timed out: {last}"));
        }
        let state = connection
            .updates
            .recv_timeout(remaining)
            .map_err(|error| format!("Provider receive ended ({error}): {last}"))?;
        let snapshots = state
            .saved
            .items
            .iter()
            .filter(|item| tasks::is_checklist(item))
            .count();
        last = format!(
            "status={:?}, items={}, checklist_snapshots={snapshots}, error_present={}",
            state.status,
            state.saved.items.len(),
            state.error.is_some()
        );
        if !state.approvals.is_empty() || !state.questions.is_empty() {
            return Err(format!(
                "Unexpected approval/question; smoke grants neither: {last}"
            ));
        }
        if matches!(state.status, Status::Failed | Status::Disconnected) {
            return Err(format!("Provider failed: {last}"));
        }
        if predicate(&state) {
            return Ok(state);
        }
    }
}

/// Validate saved production transcript facts, rather than assuming prompt compliance.
fn verify(items: &[codex::TranscriptItem]) -> Result<Vec<(usize, String)>, String> {
    let snapshots: Vec<_> = items
        .iter()
        .filter(|item| tasks::is_checklist(item))
        .collect();
    if snapshots.len() < 3 {
        return Err(format!(
            "Expected retained task transitions; found {} checklist snapshots",
            snapshots.len()
        ));
    }
    for (title, expected) in [
        (FIRST, TaskStatus::InProgress),
        (SECOND, TaskStatus::InProgress),
    ] {
        let observed = snapshots.iter().any(|item| {
            item["plan"].as_array().is_some_and(|plan| {
                plan.iter().any(|task| {
                    task["step"] == title && TaskStatus::parse(task["status"].as_str()) == expected
                })
            })
        });
        if !observed {
            return Err(format!("Missing retained in-progress snapshot for {title}"));
        }
    }
    let list = tasks::current(items).ok_or("Missing current task list")?;
    if list.tasks.len() != 2 || !list.all_done() {
        return Err("Final task list did not retain exactly two completed tasks".into());
    }
    let mut evidence = Vec::new();
    for (marker, title) in MARKERS.into_iter().zip([FIRST, SECOND]) {
        let matching: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item["type"] == "agentMessage" && codex::item_text(item).contains(marker)
            })
            .collect();
        if matching.is_empty() {
            return Err(format!("Missing provider progress marker {marker}"));
        }
        let owned = matching
            .into_iter()
            .find_map(|(index, _)| {
                tasks::activity_owner(items, index)
                    .filter(|task| task.step == title)
                    .map(|task| (index, task.step))
            })
            .ok_or_else(|| format!("Progress {marker} did not retain ownership under {title}"))?;
        evidence.push(owned);
    }
    Ok(evidence)
}

struct StopOnDrop(Option<codex::Handle>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            if let Err(error) = handle
                .stop_completion(false)
                .recv_timeout(Duration::from_secs(10))
            {
                eprintln!("Smoke cleanup did not confirm actor stop: {error}");
            }
        }
    }
}

fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set disposable GROVE_CONFIG_DIR".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => {
            return Err(
                "Usage: native_task_activity_smoke codex|claude EMPTY_DISPOSABLE_CWD".into(),
            )
        }
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing disposable cwd")?)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("Use an empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let saved: SavedSession = serde_json::from_value(serde_json::json!({
        "provider":provider, "key":key, "project":"Task activity smoke", "cwd":cwd,
        "label":"Task activity smoke", "roots":[], "thread_id":null, "items":[]
    }))
    .map_err(|error| error.to_string())?;
    let connection = codex::connect(saved, Vec::new());
    let mut cleanup = StopOnDrop(Some(connection.handle.clone()));
    wait(&connection, |state| state.status == Status::Idle)?;
    if args.iter().any(|arg| arg == "--normal") {
        // A normal useful request: provider guidance alone must produce action reports.
        connection.handle.send(Action::Prompt("Compare three annual subscription options for a 12-person team. A costs 18 dollars per person monthly with 10 percent off annual billing; B costs 190 dollars per person annually plus a 120 dollar team fee; C costs 210 dollars per person annually with a 15 percent nonprofit discount. We qualify for the discount. Work through the annual costs, then independently check the arithmetic and recommend the cheapest option with the savings versus each other option. Keep a short two-task checklist for calculation and verification. Do not use files, shell, network or subagents.".into()))?;
        let settled = wait(&connection, |state| {
            state.status == Status::Done && state.turn_id.is_none()
        })?;
        let steps = activity::steps(&settled.saved.items);
        if steps.len() < 2
            || steps
                .iter()
                .filter(|step| step.status == StepStatus::Completed)
                .count()
                < 2
        {
            return Err(format!("Normal guidance did not produce at least two explicitly completed action reports: {steps:?}"));
        }
        for step in steps {
            println!(
                "PASS {provider:?} normal guidance: {:?} · {}",
                step.status, step.label
            );
        }
        println!("PASS {provider:?}: ordinary request used concise explicit activity without prompt-specific reporting instructions");
        return Ok(());
    }
    let prompt = format!(
        "This is a bounded task-list and commentary integration smoke. Do not read or write any files, run shell commands, access the network, use subagents, or access other directories. Use only your structured task tools (Codex update_plan, or Claude TodoWrite / TaskCreate and TaskUpdate) and Grove report_activity; tool search is allowed to load these tools. Report three action steps through report_activity: first-step under the exact first task title, second-step and failure-step under the second task title. Each starts in_progress. Complete first-step with label Inspected task activity ownership before moving tasks. Complete second-step with label Verified task activity retention. Mark failure-step failed with label Checked an expected failure. Do not claim other steps complete implicitly. Ordinary commentary below must remain separate from these reports. Perform this exact sequence in this single turn: (1) Record exactly two tasks with the exact titles '{FIRST}' and '{SECOND}', first in_progress and second pending. (2) Send a separate brief progress/commentary message containing '{first_marker}: I am inspecting task activity ownership.' while the first task is still in_progress. (3) Update the first task to completed and second to in_progress. (4) Send a separate brief progress/commentary message containing '{second_marker}: I am verifying task activity retention.' while the second task is still in_progress. (5) Mark the second task completed, retaining both tasks. (6) Send a final answer exactly 'TASK_ACTIVITY_SMOKE_DONE'. The commentary messages must be separate from the final answer, not combined with it. This test needs actual task-tool calls and actual commentary in the stated order; do not merely describe the sequence.",
        first_marker = MARKERS[0], second_marker = MARKERS[1]
    );
    connection.handle.send(Action::Prompt(prompt))?;
    let settled = wait(&connection, |state| {
        state.status == Status::Done
            && state.turn_id.is_none()
            && state.saved.items.iter().any(|item| {
                item["type"] == "agentMessage"
                    && codex::item_text(item).trim() == "TASK_ACTIVITY_SMOKE_DONE"
            })
    })?;
    let evidence = verify(&settled.saved.items)?;
    let explicit = activity::steps(&settled.saved.items);
    for (id, status) in [
        ("first-step", StepStatus::Completed),
        ("second-step", StepStatus::Completed),
        ("failure-step", StepStatus::Failed),
    ] {
        let step = explicit.iter().find(|step| step.id == id).ok_or_else(|| {
            format!(
                "Missing accepted report {id}; internal items: {:?}",
                settled
                    .saved
                    .items
                    .iter()
                    .filter(|item| activity::is_internal(item))
                    .map(|item| &item["status"])
                    .collect::<Vec<_>>()
            )
        })?;
        if step.status != status || step.turn_id.is_none() || step.task.is_none() {
            return Err(format!("Incorrect explicit report: {step:?}"));
        }
    }
    if settled
        .saved
        .items
        .iter()
        .filter(|item| activity::is_internal(item))
        .any(|item| !codex::item_text(item).is_empty())
    {
        return Err("Internal report metadata leaked into text export".into());
    }
    println!("PASS {provider:?}: accepted explicit reports retain completed/failed labels and provider turn identity; metadata absent textual export");

    println!("PASS {provider:?}: real task transitions and commentary owners {evidence:?}; {} retained checklist snapshots", settled.saved.items.iter().filter(|item| tasks::is_checklist(item)).count());
    let serialized = serde_json::to_vec(&settled.saved).map_err(|error| error.to_string())?;
    let restored: SavedSession =
        serde_json::from_slice(&serialized).map_err(|error| error.to_string())?;
    if verify(&restored.items)? != evidence {
        return Err("Task ownership changed after serialization/reload".into());
    }
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    cleanup.0 = None;
    let recovered = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|saved| saved.key == key)
        .ok_or("Missing persisted smoke session")?;
    if verify(&recovered.items)? != evidence {
        return Err("Task ownership changed in persisted session".into());
    }
    let thread = recovered.thread_id.clone();
    if thread.is_none() {
        return Err("Persisted smoke session has no provider thread to reconnect".into());
    }
    let resumed_connection = codex::connect(recovered, Vec::new());
    let mut resumed_cleanup = StopOnDrop(Some(resumed_connection.handle.clone()));
    // Wait for the real provider's resume handshake/history response. No prompt
    // is sent: this must exercise recovery of the existing task history.
    let resumed = wait(&resumed_connection, |state| state.status == Status::Idle)?;
    if resumed.saved.key != key || resumed.saved.thread_id != thread {
        return Err("Reconnect changed the smoke session or provider thread".into());
    }
    let resumed_evidence = verify(&resumed.saved.items)?;
    let resumed_steps = activity::steps(&resumed.saved.items);
    for before in &explicit {
        let after = resumed_steps
            .iter()
            .find(|step| step.id == before.id && step.turn_id == before.turn_id)
            .ok_or("Reconnect lost accepted explicit report")?;
        if after.label != before.label
            || after.status != before.status
            || after.task.as_ref().map(|task| &task.step)
                != before.task.as_ref().map(|task| &task.step)
        {
            return Err("Reconnect changed explicit action lifecycle/ownership".into());
        }
    }
    println!("PASS {provider:?}: provider reconnect preserved explicit step identities, labels, states and owners");
    if resumed_evidence
        .iter()
        .map(|(_, title)| title)
        .collect::<Vec<_>>()
        != evidence.iter().map(|(_, title)| title).collect::<Vec<_>>()
    {
        return Err("Task ownership changed after the real provider reconnect".into());
    }
    resumed_connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    resumed_cleanup.0 = None;
    println!("PASS {provider:?}: real provider reconnect preserved both task owners and the completed checklist without another prompt; owners {resumed_evidence:?}");
    if fs_err::read_dir(&cwd)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("Provider unexpectedly wrote files in disposable cwd".into());
    }
    println!("PASS {provider:?}: serialized and persisted reload preserved task ownership; no approvals granted and disposable cwd stayed empty");
    Ok(())
}
