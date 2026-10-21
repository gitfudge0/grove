//! The agent's progress checklist, read from saved transcript items.
//!
//! A transcript item is a checklist snapshot iff `type == "plan"` and it
//! carries a structured `plan` array of `{id, step, status, activeForm?}`.
//! Claude's TodoWrite/TaskCreate/TaskUpdate/TaskList and Codex's
//! `turn/plan/updated` both write that shape. Codex *proposed plans*
//! (plan-mode text streamed by `item/plan/delta`) have no array and remain
//! ordinary transcript content.
use serde_json::Value;

pub(crate) const CLAUDE_GUIDANCE: &str = "Grove displays your structured task list. For multi-step work, use TodoWrite or TaskCreate/TaskUpdate to record the actionable steps before implementation. If TaskCreate/TaskUpdate are deferred, load them with ToolSearch (select:TaskCreate,TaskUpdate) before starting. Break long-running work into concrete, independently completable tasks whose titles describe the actual outcome. Split broad implementation work by meaningful deliverable, and track distinct checks, installation, and inspection separately when applicable. Refine the list as scope becomes clear so progress remains visible; avoid trivial steps or artificial checklist inflation. Keep task statuses current as work starts and finishes, including delegated work. A prose checklist alone does not update Grove's task list. Skip task tracking for simple single-step requests.";
pub(crate) const CODEX_GUIDANCE: &str = "Grove displays your structured task list. For multi-step work, use update_plan to record the actionable steps before implementation. Break long-running work into concrete, independently completable tasks whose titles describe the actual outcome. Split broad implementation work by meaningful deliverable, and track distinct checks, installation, and inspection separately when applicable. Refine the list as scope becomes clear so progress remains visible; avoid trivial steps or artificial checklist inflation. Keep step statuses current as work starts and finishes, including delegated work. A prose checklist alone does not update Grove's task list. Skip task tracking for simple single-step requests.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

impl TaskStatus {
    /// Lenient parse: `completed`, `in_progress`/`inProgress`, else pending.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("completed") => Self::Completed,
            Some("in_progress" | "inProgress") => Self::InProgress,
            _ => Self::Pending,
        }
    }

    /// The canonical saved spelling: `pending`, `inProgress` or `completed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "inProgress",
            Self::Completed => "completed",
        }
    }
}

/// Normalises an agent's status spelling to `pending|inProgress|completed`.
pub fn normalize_status(value: Option<&str>) -> &'static str {
    TaskStatus::parse(value).as_str()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub step: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    /// Index into items of the first checklist snapshot where this task was
    /// InProgress (else where it first appeared); used for jump-to-start.
    pub started_at: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskList {
    pub tasks: Vec<Task>,
    pub explanation: Option<String>,
    /// Index of the snapshot item.
    pub item: usize,
}

impl TaskList {
    pub fn done(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .count()
    }

    pub fn total(&self) -> usize {
        self.tasks.len()
    }

    /// The first InProgress task.
    pub fn current(&self) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|task| task.status == TaskStatus::InProgress)
    }

    pub fn all_done(&self) -> bool {
        self.total() > 0 && self.done() == self.total()
    }

    /// The current task's `active_form`, else its step.
    pub fn now_label(&self) -> Option<&str> {
        let task = self.current()?;
        Some(task.active_form.as_deref().unwrap_or(&task.step))
    }
}

pub fn is_checklist(item: &Value) -> bool {
    item["type"] == "plan" && item["plan"].is_array()
}

/// Latest session checklist snapshot, retained across turns until explicitly
/// updated or replaced. None if none or if the latest snapshot is empty.
pub fn current(items: &[Value]) -> Option<TaskList> {
    let index = items.iter().rposition(is_checklist)?;
    let entries = entries(&items[index]);
    if entries.is_empty() {
        return None;
    }
    let tasks = entries
        .into_iter()
        .map(|entry| {
            let started_at = started_at(items, index, &entry);
            Task {
                started_at,
                ..entry
            }
        })
        .collect();
    let explanation = items[index]["explanation"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    Some(TaskList {
        tasks,
        explanation,
        item: index,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Completed(String),
    Started(String),
    Added(String),
    Removed(String),
}

/// Changes between the previous checklist snapshot before `index` and the one
/// at `index` (first snapshot => every task as Added, except in-progress
/// tasks, which are Started instead). Order: Completed, Started, Added, Removed. Empty when
/// `items[index]` is not a checklist.
pub fn changes_at(items: &[Value], index: usize) -> Vec<Change> {
    let Some(item) = items.get(index).filter(|item| is_checklist(item)) else {
        return Vec::new();
    };
    let now = entries(item);
    let before = items[..index]
        .iter()
        .rposition(is_checklist)
        .map(|at| entries(&items[at]))
        .unwrap_or_default();
    let (mut completed, mut started, mut added, mut removed) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for task in &now {
        match before.iter().find(|old| same_task(old, task)) {
            Some(old) if old.status != task.status => match task.status {
                TaskStatus::Completed => completed.push(Change::Completed(task.step.clone())),
                TaskStatus::InProgress => started.push(Change::Started(task.step.clone())),
                TaskStatus::Pending => {}
            },
            Some(_) => {}
            // A new task already in progress reads once, as Started.
            None if task.status == TaskStatus::InProgress => {
                started.push(Change::Started(task.step.clone()));
            }
            None => added.push(Change::Added(task.step.clone())),
        }
    }
    for old in &before {
        if !now.iter().any(|task| same_task(old, task)) {
            removed.push(Change::Removed(old.step.clone()));
        }
    }
    completed
        .into_iter()
        .chain(started)
        .chain(added)
        .chain(removed)
        .collect()
}

/// The tasks of one snapshot, `started_at` set to the snapshot itself.
fn entries(item: &Value) -> Vec<Task> {
    item["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, entry)| {
            let step = entry["step"].as_str()?.trim();
            if step.is_empty() {
                return None;
            }
            let id = match &entry["id"] {
                Value::String(id) => id.clone(),
                Value::Number(id) => id.to_string(),
                _ => format!("step-{i}"),
            };
            Some(Task {
                id,
                step: step.to_owned(),
                active_form: entry["activeForm"]
                    .as_str()
                    .map(str::trim)
                    .filter(|form| !form.is_empty())
                    .map(str::to_owned),
                status: TaskStatus::parse(entry["status"].as_str()),
                started_at: 0,
            })
        })
        .collect()
}

/// Positional ids (`step-N` from Codex, `todo-N` from TodoWrite) shift when a
/// list is rewritten, so only agent-assigned ids identify a task by id alone.
fn same_task(a: &Task, b: &Task) -> bool {
    let positional = |id: &str| id.starts_with("step-") || id.starts_with("todo-");
    a.step == b.step || (a.id == b.id && !positional(&a.id))
}

/// Walks back through contiguous snapshots that still contain `task`, keeping
/// the earliest one where it was InProgress, else the earliest it appeared.
fn started_at(items: &[Value], index: usize, task: &Task) -> usize {
    let mut appeared = index;
    let mut started = (task.status == TaskStatus::InProgress).then_some(index);
    for at in (0..index).rev().filter(|&at| is_checklist(&items[at])) {
        let Some(old) = entries(&items[at])
            .into_iter()
            .find(|old| same_task(old, task))
        else {
            break;
        };
        appeared = at;
        if old.status == TaskStatus::InProgress {
            started = Some(at);
        }
    }
    started.unwrap_or(appeared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codex(turn: &str, plan: Value) -> Value {
        json!({"id":format!("grove-plan-{turn}"),"type":"plan","text":"","plan":plan})
    }
    fn user() -> Value {
        json!({"id":"u","type":"userMessage","content":[]})
    }

    #[test]
    fn codex_snapshot_parses_status_and_explanation() {
        let mut item = codex(
            "t1",
            json!([
                {"id":"step-0","step":"Inspect","status":"completed"},
                {"id":"step-1","step":"Implement","status":"inProgress"},
                {"id":"step-2","step":"Test","status":"pending"}
            ]),
        );
        item["explanation"] = json!("Fixing the bug");
        let items = vec![user(), item];
        let list = current(&items).unwrap();
        assert_eq!(list.item, 1);
        assert_eq!((list.done(), list.total()), (1, 3));
        assert!(!list.all_done());
        assert_eq!(list.explanation.as_deref(), Some("Fixing the bug"));
        assert_eq!(list.current().unwrap().step, "Implement");
        assert_eq!(list.now_label(), Some("Implement"));
        assert_eq!(list.tasks[2].status, TaskStatus::Pending);
    }

    #[test]
    fn claude_snapshot_uses_active_form_and_snake_case_status() {
        let items = vec![json!({"id":"c","type":"plan","tool":"TodoWrite","plan":[
            {"id":"todo-0","step":"Run tests","activeForm":"Running tests","status":"in_progress"},
            {"id":"todo-1","step":"Ship","status":"completed"}
        ]})];
        let list = current(&items).unwrap();
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
        assert_eq!(list.tasks[0].active_form.as_deref(), Some("Running tests"));
        assert_eq!(list.now_label(), Some("Running tests"));
        assert!(list.explanation.is_none());
    }

    #[test]
    fn now_label_falls_back_to_step_and_is_none_without_current() {
        let items = vec![codex(
            "t1",
            json!([{"id":"step-0","step":"Build","status":"inProgress","activeForm":"  "}]),
        )];
        assert_eq!(current(&items).unwrap().now_label(), Some("Build"));
        let items = vec![codex(
            "t1",
            json!([{"id":"step-0","step":"Build","status":"completed"}]),
        )];
        let list = current(&items).unwrap();
        assert_eq!(list.now_label(), None);
        assert!(list.all_done());
    }

    #[test]
    fn session_checklist_survives_question_answer_and_transcript_reload() {
        let mut items = vec![
            user(),
            codex(
                "t1",
                json!([
                    {"step":"Inspect","status":"completed"},
                    {"step":"Implement","status":"inProgress"},
                    {"step":"Test","status":"pending"}
                ]),
            ),
            json!({"type":"agentMessage","text":"Which option should I implement?"}),
        ];
        let before = current(&items).unwrap();
        items.push(json!({"type":"userMessage","content":[{"text":"Both equally"}]}));
        items.push(json!({"type":"agentMessage","text":"Continuing implementation"}));
        assert_eq!(current(&items), Some(before.clone()));
        let restored: Vec<Value> =
            serde_json::from_str(&serde_json::to_string(&items).unwrap()).unwrap();
        assert_eq!(current(&restored), Some(before));

        items.push(codex(
            "t2",
            json!([
                {"step":"Inspect","status":"completed"},
                {"step":"Implement","status":"completed"},
                {"step":"Test","status":"inProgress"}
            ]),
        ));
        let updated = current(&items).unwrap();
        assert_eq!((updated.done(), updated.total()), (2, 3));
        assert_eq!(updated.now_label(), Some("Test"));
        assert_eq!(updated.tasks[1].started_at, 1);
        items.push(codex(
            "t3",
            json!([{"step":"Follow up","status":"pending"}]),
        ));
        assert_eq!(current(&items).unwrap().tasks[0].step, "Follow up");
        items.push(codex("t4", json!([])));
        assert!(current(&items).is_none());
    }

    #[test]
    fn empty_latest_snapshot_and_missing_lists_yield_none() {
        assert!(current(&[]).is_none());
        assert!(current(&[user()]).is_none());
        let items = vec![
            codex("t1", json!([{"id":"step-0","step":"A","status":"pending"}])),
            codex("t2", json!([])),
        ];
        assert!(current(&items).is_none());
    }

    #[test]
    fn proposed_plans_without_an_array_are_ignored() {
        let proposed = json!({"id":"p","type":"plan","text":"1. Do it\n2. Ship"});
        assert!(!is_checklist(&proposed));
        let checklist = codex("t1", json!([{"id":"step-0","step":"A","status":"pending"}]));
        let items = vec![checklist, proposed.clone()];
        assert_eq!(current(&items).unwrap().item, 0);
        assert!(changes_at(&items, 1).is_empty());
        assert!(current(&[proposed]).is_none());
    }

    #[test]
    fn legacy_saved_items_still_parse() {
        // Claude items saved before activeForm; Codex items saved before the
        // structured array (text only) are not checklists.
        let legacy_claude = json!({"id":"c","type":"plan","text":"- [ ] A","plan":[
            {"id":"9","step":"A","status":"pending"},
            {"id":"10","step":"B","status":"inProgress"}
        ]});
        let legacy_codex = json!({"id":"grove-plan-t0","type":"plan","text":"\n- [x] Old"});
        let items = vec![legacy_claude, legacy_codex];
        let list = current(&items).unwrap();
        assert_eq!(list.item, 0);
        assert_eq!(list.now_label(), Some("B"));
        assert!(list.tasks.iter().all(|task| task.active_form.is_none()));
    }

    #[test]
    fn started_at_points_at_first_in_progress_snapshot() {
        let items = vec![
            codex(
                "t1",
                json!([{"id":"step-0","step":"A","status":"inProgress"},{"id":"step-1","step":"B","status":"pending"}]),
            ),
            json!({"type":"agentMessage"}),
            codex(
                "t2",
                json!([{"id":"step-0","step":"A","status":"completed"},{"id":"step-1","step":"B","status":"inProgress"}]),
            ),
            codex(
                "t3",
                json!([{"id":"step-0","step":"A","status":"completed"},{"id":"step-1","step":"B","status":"inProgress"},{"id":"step-2","step":"C","status":"pending"}]),
            ),
        ];
        let list = current(&items).unwrap();
        assert_eq!(list.tasks[0].started_at, 0);
        assert_eq!(list.tasks[1].started_at, 2);
        assert_eq!(list.tasks[2].started_at, 3);
    }

    #[test]
    fn changes_compare_against_the_previous_snapshot() {
        let items = vec![
            codex(
                "t1",
                json!([{"id":"step-0","step":"A","status":"inProgress"},{"id":"step-1","step":"B","status":"pending"},{"id":"step-2","step":"Drop","status":"pending"}]),
            ),
            json!({"type":"agentMessage"}),
            codex(
                "t2",
                json!([{"id":"step-0","step":"A","status":"completed"},{"id":"step-1","step":"B","status":"inProgress"},{"id":"step-2","step":"New","status":"pending"}]),
            ),
        ];
        assert_eq!(
            changes_at(&items, 0),
            vec![
                Change::Started("A".into()),
                Change::Added("B".into()),
                Change::Added("Drop".into()),
            ]
        );
        assert_eq!(
            changes_at(&items, 2),
            vec![
                Change::Completed("A".into()),
                Change::Started("B".into()),
                Change::Added("New".into()),
                Change::Removed("Drop".into()),
            ]
        );
        assert!(changes_at(&items, 1).is_empty());
        assert!(changes_at(&items, 99).is_empty());
    }

    #[test]
    fn agent_assigned_ids_track_renamed_tasks() {
        let items = vec![
            json!({"type":"plan","plan":[{"id":"7","step":"Old name","status":"pending"}]}),
            json!({"type":"plan","plan":[{"id":"7","step":"New name","status":"completed"}]}),
        ];
        assert_eq!(
            changes_at(&items, 1),
            vec![Change::Completed("New name".into())]
        );
        assert_eq!(current(&items).unwrap().tasks[0].started_at, 0);
    }

    #[test]
    fn normalize_status_accepts_both_spellings() {
        assert_eq!(normalize_status(Some("in_progress")), "inProgress");
        assert_eq!(normalize_status(Some("inProgress")), "inProgress");
        assert_eq!(normalize_status(Some("completed")), "completed");
        assert_eq!(normalize_status(Some("weird")), "pending");
        assert_eq!(normalize_status(None), "pending");
    }
}
