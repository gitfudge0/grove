//! Explicit provider-authored action reports. Commentary is never completion evidence.
use crate::tasks::{self, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Borrow;

pub const GUIDANCE: &str = "Grove requires explicit live action reporting through its internal grove_activity report_activity tool, separately from ordinary commentary and the task checklist. Load report_activity with tool search if deferred. Before the FIRST meaningful action of a turn, open an in_progress report. Before beginning EACH task's work, open an action report for that task; do not perform a task's work silently and report only the final verification task. Supply a stable id, task_id (the exact checklist task id or uniquely matching exact task title; empty only without a checklist), status in_progress, and a short present-continuous action label such as 'Checking session navigation'. After observing the result, update the SAME id and task_id with status completed and a short past-tense label such as 'Checked session navigation', or failed with a truthful failure label. Close each finished action explicitly before moving to another task or marking its checklist task completed. Never infer completion from another action starting, a task status changing or a turn ending. If work is interrupted or its result is unknown, leave it unfinished rather than reporting success. Report meaningful changes of action as work progresses. Open a distinct step when moving to a different investigation target, implementation area, or verification method, or when a milestone would clarify progress during a long action. Use specific labels such as 'Tracing title loading state' and 'Checking naming transition tests'. Group related tool calls into each action; do not report every command, commentary message, intention, scope description or generic finding. Keep labels to at most 100 characters, without first-person narration, commands, paths, or explanations. Reports are internal UI metadata: do not repeat their JSON in commentary or final answers. Ordinary commentary remains available for context. Task status and step status are independent; completing a task does not implicitly complete its steps.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    InProgress,
    Completed,
    Failed,
}
/// A terminal report cannot be reopened or changed to a different outcome.
pub fn valid_transition(before: StepStatus, after: StepStatus) -> bool {
    before == StepStatus::InProgress || before == after
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub id: String,
    pub task_id: String,
    pub status: StepStatus,
    pub label: String,
}
pub fn validate_report(value: &Value) -> Result<(), String> {
    let report: Report = serde_json::from_value(value.clone()).map_err(|_| {
        "Provide id, task_id, status and label with the advertised types".to_owned()
    })?;
    if report.id.trim().is_empty() || report.id.len() > 128 || report.task_id.len() > 1024 {
        return Err(
            "Use a nonempty stable id (at most 128 bytes) and a checklist task identity".into(),
        );
    }
    if report.label.trim().is_empty()
        || report.label.chars().count() > 100
        || report.label.contains(['\n', '\r'])
        || report.label.trim() != report.label
    {
        return Err("Use a single short action label of at most 100 characters".into());
    }
    Ok(())
}
/// Match the reserved transport identity, never an arbitrary similarly named tool.
pub fn is_internal(item: &Value) -> bool {
    (item["server"] == "grove_activity" && item["tool"] == "report_activity")
        || item["tool"] == "mcp__grove_activity__report_activity"
}
fn accepted(item: &Value) -> Option<(Report, Option<String>, Option<Value>)> {
    if !is_internal(item)
        || item["status"] != "completed"
        || !item["error"].is_null()
        || item["result"]["isError"] == true
    {
        return None;
    }
    let mut candidates = vec![
        item["result"]["structuredContent"].clone(),
        item["structuredResult"]["structuredContent"].clone(),
    ];
    if let Some(parts) = item["result"]["content"].as_array() {
        candidates.extend(
            parts
                .iter()
                .filter_map(|part| serde_json::from_str(part["text"].as_str()?).ok()),
        );
    }
    candidates.into_iter().find_map(|ack| {
        if ack["accepted"] != true
            || ack["report"] != item["arguments"]
            || validate_report(&ack["report"]).is_err()
        {
            return None;
        }
        serde_json::from_value(ack["report"].clone())
            .ok()
            .map(|report| {
                (
                    report,
                    ack["turn_id"].as_str().map(str::to_owned),
                    ack.get("owner").cloned(),
                )
            })
    })
}
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub index: usize,
    pub id: String,
    pub label: String,
    pub status: StepStatus,
    pub task: Option<Task>,
    pub current_turn: bool,
    pub turn_id: Option<String>,
}
/// Replaying successful provider tool results makes restart and duplicate delivery idempotent.
/// Each id is scoped to its user turn and binds once to a continuous checklist task.
type TurnKey = (Option<String>, Option<usize>);

pub fn steps<T: Borrow<Value>>(items: &[T]) -> Vec<Step> {
    let latest_prompt = items
        .iter()
        .rposition(|item| item.borrow()["type"] == "userMessage");
    let mut result: Vec<(TurnKey, Step)> = Vec::new();
    let mut prompt = None;
    for (index, item) in items.iter().enumerate() {
        let item = item.borrow();
        if item["type"] == "userMessage" {
            prompt = Some(index);
        }
        let Some((report, turn_id, accepted_owner)) = accepted(item) else {
            continue;
        };
        let list = tasks::current(&items[..index]);
        if accepted_owner.is_none()
            && list
                .as_ref()
                .is_some_and(|list| items[list.item].borrow()["taskSnapshot"] == false)
        {
            continue;
        }
        let task = if let Some(owner) = accepted_owner {
            // The provider can insert a call before a concurrent checklist update.
            // Its successful result binds the acceptance snapshot, not arrival order.
            owner["snapshot_id"].as_str().and_then(|snapshot| {
                let at = items
                    .iter()
                    .position(|item| item.borrow()["id"] == snapshot)?;
                let list = tasks::current(&items[..=at])?;
                list.tasks.into_iter().find(|task| {
                    Some(task.id.as_str()) == owner["id"].as_str()
                        && Some(task.step.as_str()) == owner["step"].as_str()
                })
            })
        } else if report.task_id.is_empty() {
            if list.is_some() {
                continue;
            }
            None
        } else {
            let Some(list) = list else {
                continue;
            };
            let mut matches = list
                .tasks
                .into_iter()
                .filter(|task| task.id == report.task_id || task.step == report.task_id);
            let Some(task) = matches.next() else {
                continue;
            };
            if matches.next().is_some() {
                continue;
            }
            Some(task)
        };
        let key = (
            turn_id.clone(),
            if turn_id.is_some() { None } else { prompt },
        );
        if let Some((_, existing)) = result
            .iter_mut()
            .find(|(turn, step)| *turn == key && step.id == report.id)
        {
            let same_owner = match (&existing.task, &task) {
                (Some(old), Some(new)) => new.owns_activity(old),
                (None, None) => true,
                _ => false,
            };
            if !same_owner || !valid_transition(existing.status, report.status) {
                continue;
            }
            existing.label = report.label;
            existing.status = report.status;
        } else {
            result.push((
                key,
                Step {
                    index,
                    id: report.id,
                    label: report.label,
                    status: report.status,
                    task,
                    current_turn: prompt == latest_prompt,
                    turn_id,
                },
            ));
        }
    }
    result.into_iter().map(|(_, step)| step).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(id: &str, task: &str, state: &str, label: &str) -> Value {
        let report = json!({"id":id,"task_id":task,"status":state,"label":label});
        json!({"type":"mcpToolCall","server":"grove_activity","tool":"report_activity","status":"completed","arguments":report,"result":{"structuredContent":{"accepted":true,"report":report}}})
    }
    fn plan() -> Value {
        json!({"type":"plan","taskSnapshot":true,"plan":[{"id":"a","step":"Inspect","status":"inProgress"},{"id":"b","step":"Verify","status":"inProgress"}]})
    }
    #[test]
    fn pending_report_completion_follows_continuous_task_activation() {
        let mut pending = plan();
        pending["id"] = json!("pending");
        pending["plan"][0]["status"] = json!("pending");
        let mut active = plan();
        active["id"] = json!("active");
        let mut start = event("verify", "a", "in_progress", "Checking totals");
        start["result"]["structuredContent"]["owner"] =
            json!({"snapshot_id":"pending","id":"a","step":"Inspect"});
        let mut done = event("verify", "a", "completed", "Checked totals");
        done["result"]["structuredContent"]["owner"] =
            json!({"snapshot_id":"active","id":"a","step":"Inspect"});
        let projected = steps(&[pending, start, active.clone(), done]);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].status, StepStatus::Completed);
        assert!(tasks::current(&[plan(), active]).unwrap().tasks[0]
            .owns_activity(projected[0].task.as_ref().unwrap()));
    }
    #[test]
    fn accepted_snapshot_binds_parallel_call_and_survives_index_shift_or_missing_anchor() {
        let mut before = plan();
        before["id"] = json!("before");
        let mut after = plan();
        after["id"] = json!("after");
        after["plan"][0]["step"] = json!("Different task");
        after["plan"][1]["step"] = json!("Inspect");
        let mut call = event("s", "b", "completed", "Inspected navigation");
        call["result"]["structuredContent"]["owner"] =
            json!({"snapshot_id":"after","id":"b","step":"Inspect"});
        // Tool starts before the concurrent accepted checklist snapshot appears.
        let history = vec![before, call.clone(), after.clone()];
        let step = steps(&history).remove(0);
        assert_eq!(step.task.as_ref().unwrap().id, "b");
        let mut rebased = vec![json!({"type":"providerNotice"})];
        rebased.extend(history);
        let moved = steps(&rebased).remove(0);
        assert_eq!(
            moved.task.as_ref().unwrap().started_at,
            step.task.as_ref().unwrap().started_at + 1
        );
        assert_eq!(moved.label, step.label);
        let unavailable = steps(&[call]).remove(0);
        assert!(unavailable.task.is_none());
        assert_eq!(unavailable.status, StepStatus::Completed);
    }
    #[test]
    fn explicit_updates_keep_identity_and_require_acknowledgement() {
        let start = event("step", "Inspect", "in_progress", "Inspecting navigation");
        let mut failed_ack = event("step", "Inspect", "completed", "Checked navigation");
        failed_ack["status"] = json!("failed");
        let mut history = vec![
            plan(),
            start.clone(),
            json!({"type":"agentMessage","text":"A commentary update"}),
            failed_ack,
        ];
        assert_eq!(steps(&history)[0].status, StepStatus::InProgress);
        history.push(event("step", "Verify", "completed", "Checked another task"));
        assert_eq!(steps(&history)[0].status, StepStatus::InProgress);
        history.push(event("step", "Inspect", "completed", "Checked navigation"));
        history.push(start);
        let projected = steps(&history);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].status, StepStatus::Completed);
        assert_eq!(projected[0].label, "Checked navigation");
        assert_eq!(
            projected,
            steps(
                &serde_json::from_str::<Vec<Value>>(&serde_json::to_string(&history).unwrap())
                    .unwrap()
            )
        );
    }
    #[test]
    fn rejects_ambiguous_owners_and_invalid_or_mismatched_results() {
        let mut ambiguous = plan();
        ambiguous["plan"][1]["step"] = json!("Inspect");
        assert!(steps(&[ambiguous, event("s", "Inspect", "completed", "Inspected")]).is_empty());
        let mut event = event("s", "a", "completed", "Inspected");
        event["arguments"]["label"] = json!("Other");
        assert!(steps(&[plan(), event]).is_empty());
        assert!(validate_report(
            &json!({"id":"s","task_id":"a","status":"completed","label":"long\nline"})
        )
        .is_err());
    }
    #[test]
    fn new_turns_do_not_reuse_steps_or_complete_previous_work() {
        let projected = steps(&[
            plan(),
            event("s", "a", "in_progress", "Inspecting"),
            json!({"type":"userMessage"}),
            event("s", "b", "failed", "Verification failed"),
        ]);
        assert_eq!(projected.len(), 2);
        assert!(!projected[0].current_turn);
        assert_eq!(projected[0].status, StepStatus::InProgress);
        assert_eq!(projected[1].status, StepStatus::Failed);
    }
}
