//! Explicit live context measurement through Grove's production provider adapter.
//! Supply claude|codex and a disposable cwd, and set a disposable GROVE_CONFIG_DIR.
use grove_core::provider::{self, Action, Provider, SavedSession, Snapshot, Status};
use std::time::{Duration, Instant};
struct Stop(provider::Handle);
impl Drop for Stop {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(true)
            .recv_timeout(Duration::from_secs(5));
    }
}
fn wait(
    connection: &provider::Connection,
    condition: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| error.to_string())?;
        if snapshot.status == Status::Disconnected || snapshot.status == Status::Failed {
            return Err(snapshot
                .error
                .unwrap_or_else(|| "Provider unavailable".into()));
        }
        if condition(&snapshot) {
            return Ok(snapshot);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set disposable GROVE_CONFIG_DIR".into());
    }
    let mut args = std::env::args().skip(1);
    let provider = match args.next().as_deref() {
        Some("claude") => Provider::Claude,
        Some("codex") => Provider::Codex,
        _ => return Err("Supply claude|codex and disposable cwd".into()),
    };
    let cwd = args.next().ok_or("Supply disposable cwd")?;
    let saved = SavedSession::empty(
        grove_core::identity::random_id()?,
        provider,
        "Context smoke".into(),
        cwd,
    );
    let connection = grove_core::codex::connect(saved, Vec::new());
    let _stop = Stop(connection.handle.clone());
    wait(&connection, |state| state.status == Status::Idle)?;
    // Claude details are queried at idle; occupancy remains provider-event driven.
    if provider == Provider::Claude {
        connection.handle.send(Action::RefreshContext)?;
        let initial =
            wait(&connection, |state| {
                !state.context_query_pending
                    && (state.saved.context_details.as_ref().is_some_and(|details| {
                        details.source == provider::ContextMeasurement::Full
                    }) || state.context_query_error.is_some())
            })?;
        if let Some(error) = initial.context_query_error {
            return Err(error);
        }
        println!(
            "initial Claude: used={} revision={}",
            initial
                .saved
                .context
                .ok_or("Missing initial occupancy")?
                .used,
            initial.context_measurement_revision()
        );
    }
    let mut revision = 0;
    let mut last_state = None;
    for turn in 1..=2 {
        let prompt = if provider == Provider::Codex && turn == 1 {
            "Use a built-in command tool to run pwd once, then reply OK. Do not call external tools or change any files."
        } else {
            "Reply with OK only. Do not use tools or change files."
        };
        connection.handle.send(Action::Prompt(prompt.into()))?;
        let fresh = wait(&connection, |state| {
            state.status == Status::Done
                && state.saved.context.is_some()
                && state.context_measurement_revision() > revision
        })?;
        revision = fresh.context_measurement_revision();
        println!(
            "turn {turn} {:?}: used={} revision={revision}",
            provider,
            fresh.saved.context.unwrap().used
        );
        if provider == Provider::Claude {
            if fresh
                .saved
                .context_details
                .as_ref()
                .is_some_and(|details| !details.categories.is_empty())
            {
                return Err("Old attribution survived a new occupancy report".into());
            }
            connection.handle.send(Action::RefreshContext)?;
            let measured = wait(&connection, |state| {
                !state.context_query_pending
                    && (state.saved.context_details.as_ref().is_some_and(|details| {
                        details.source == provider::ContextMeasurement::Full
                    }) || state.context_query_error.is_some())
            })?;
            if let Some(error) = measured.context_query_error.as_ref() {
                return Err(error.clone());
            }
            if measured.context_measurement_revision() <= revision {
                return Err("Idle measurement did not advance".into());
            }
            revision = measured.context_measurement_revision();
            println!(
                "idle Claude: used={} revision={revision} categories={}",
                measured.saved.context.unwrap().used,
                measured
                    .saved
                    .context_details
                    .as_ref()
                    .unwrap()
                    .categories
                    .len()
            );
            last_state = Some(measured);
        } else {
            last_state = Some(fresh);
        }
    }
    let state = last_state.ok_or("No turns observed")?;
    if let Some(error) = state.context_query_error {
        return Err(error);
    }
    let usage = state.saved.context.ok_or("Missing context occupancy")?;
    let details = state
        .saved
        .context_details
        .ok_or("Missing context provenance")?;
    println!(
        "PASS {:?}: used={} window={} source={:?} categories={} measured_at={}",
        provider,
        usage.used,
        usage.window,
        details.source,
        details.categories.len(),
        details.measured_at
    );
    if provider == Provider::Codex {
        let accounting = details
            .accounting
            .as_ref()
            .ok_or("Missing Codex accounting")?;
        let last = accounting
            .last
            .as_ref()
            .ok_or("Missing latest-request accounting")?;
        if last.total_tokens != Some(usage.used) {
            return Err("Latest request total differs from observed occupancy".into());
        }
        println!(
            "accounting: last={} total={}",
            serde_json::to_string(last).map_err(|e| e.to_string())?,
            serde_json::to_string(&accounting.total).map_err(|e| e.to_string())?
        );
        let commands: Vec<_> = state
            .saved
            .items
            .iter()
            .filter(|item| item["type"] == "commandExecution")
            .collect();
        if commands.is_empty() {
            return Err("Expected the requested built-in command call".into());
        }
        println!(
            "tool_activity: built_in_commands={} reported_durations={}",
            commands.len(),
            commands
                .iter()
                .filter(|item| item["durationMs"].as_u64().is_some())
                .count()
        );
    }
    for category in details.categories {
        println!(
            "category: {} tokens={} kind={:?} entries={}",
            category.label,
            category.tokens,
            category.kind,
            category.entries.len()
        );
    }
    Ok(())
}
