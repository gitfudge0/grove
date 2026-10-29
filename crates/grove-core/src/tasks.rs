//! The agent's progress checklist, read from saved transcript items.
//!
//! A transcript item is a checklist snapshot iff `type == "plan"` and it
//! carries a structured `plan` array of `{id, step, status, activeForm?}`.
//! Claude's TodoWrite/TaskCreate/TaskUpdate/TaskList and Codex's
//! `turn/plan/updated` both write that shape. Codex *proposed plans*
//! (plan-mode text streamed by `item/plan/delta`) have no array and remain
//! ordinary transcript content.
use serde_json::Value;
use std::borrow::Borrow;

// Keep the outcome/evidence policy identical across providers while respecting
// their different task APIs. This is agent guidance, not inferred task state.
macro_rules! task_guidance {
    ($provider:literal) => {
        concat!(
            "Grove displays your structured task list. For substantial multi-step work, record tasks before implementation and maintain them as work proceeds. ",
            "Create tasks around observable, independently completable outcomes; titles should describe results, for example 'Run shell snippets in the correct checkout' or 'Verify draft preservation with live Codex and Claude'. ",
            "Split substantial shared prerequisites from the features that depend on them, and split broad feature groups into meaningful deliverables. Separate meaningful verification outcomes such as live provider checks and native UI inspection from implementation and from each other when they require different evidence. ",
            "Refine the list as scope becomes clearer: split or add substantial work and discovered defects. Keep routine compiler fixes, repeated checks and cache cleanup in progress notes unless they become substantial blockers. Avoid tasks for every file, mechanical task inflation, and vague buckets such as 'implement everything' or 'verify integration'. Preserve small lists for genuinely small requests; skip task tracking for simple single-step requests. ",
            "Mark an outcome completed only when the stated result has appropriate evidence; worker reports alone are not verification. Keep unverified coverage as explicitly unfinished verification work, distinguishable from completed implementation; record blockers or unavailable checks in progress notes rather than claiming they passed. ",
            "Preserve task ownership and existing task state when revising the list. Task tracking does not override user disclosure preferences, permission boundaries or explicit instructions. A prose checklist alone does not update Grove's task list. ",
            "Grove also displays concise assistant progress updates. Phrase completed work in past tense and work currently in progress in present continuous tense; do not describe unfinished work as complete. ",
            $provider
        )
    };
}

pub(crate) const CLAUDE_GUIDANCE: &str = task_guidance!(
    "Use TodoWrite or TaskCreate/TaskUpdate. If TaskCreate/TaskUpdate are deferred, load them with ToolSearch (select:TaskCreate,TaskUpdate) before starting. Keep task statuses current as work starts and finishes, including delegated work. TaskCreate creates pending tasks; use TaskUpdate to mark each started task in_progress, including tasks being implemented by workers. TaskCreate/TaskUpdate can represent multiple active tasks; preserve owners and dependencies, and read the latest task state before updating shared tasks. TodoWrite replaces the list and its instructions limit it to one in_progress task: when using it, keep other delegated outcomes listed and disclose their active work in progress notes without claiming completion."
);
pub(crate) const CODEX_GUIDANCE: &str = task_guidance!(
    "Use update_plan. Keep step statuses current as work starts and finishes, including delegated work. update_plan replaces the plan and allows at most one in_progress step: retain all unfinished outcomes, use that active step for the outcome currently being coordinated, and identify other concurrently active or delegated outcomes by title in the explanation or progress notes without collapsing their tasks into a vague bucket. Do not mark parallel work completed just to satisfy that limit, and do not treat pending as proof that no worker is active."
);

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
    /// First snapshot in the continuous lifetime, independent of activation.
    pub identity_at: usize,
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
pub fn current<T: Borrow<Value>>(items: &[T]) -> Option<TaskList> {
    let index = items
        .iter()
        .map(Borrow::<Value>::borrow)
        .rposition(is_checklist)?;
    let entries = entries(items[index].borrow());
    if entries.is_empty() {
        return None;
    }
    let tasks = entries
        .into_iter()
        .map(|entry| {
            let (identity_at, started_at) = task_history(items, index, &entry);
            Task {
                started_at,
                identity_at,
                ..entry
            }
        })
        .collect();
    let explanation = items[index].borrow()["explanation"]
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

/// Task owning one assistant progress item at the time it was recorded.
/// Parallel tasks, final answers and legacy overwritten Codex plans provide no
/// unambiguous ownership. This deliberately does not infer an owner from prose.
pub fn activity_owner<T: Borrow<Value>>(items: &[T], index: usize) -> Option<Task> {
    let item = items.get(index)?.borrow();
    if item["type"] != "agentMessage"
        || item["phase"] == "final_answer"
        || item["groveTaskOwnershipUnavailable"] == true
    {
        return None;
    }
    let list = current(&items[..index])?;
    let snapshot = items[list.item].borrow();
    if snapshot["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("grove-plan-"))
        && snapshot["taskSnapshot"] != true
    {
        return None;
    }
    let mut active = list
        .tasks
        .into_iter()
        .filter(|task| task.status == TaskStatus::InProgress);
    let owner = active.next()?;
    active.next().is_none().then_some(owner)
}

impl Task {
    /// The same continuous task across checklist revisions. Positional provider
    /// IDs never move history to a different task after insertion or reordering.
    pub fn owns_activity(&self, owner: &Self) -> bool {
        self.identity_at == owner.identity_at && same_task(self, owner)
    }
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
pub fn changes_at<T: Borrow<Value>>(items: &[T], index: usize) -> Vec<Change> {
    let Some(item) = items
        .get(index)
        .map(Borrow::<Value>::borrow)
        .filter(|item| is_checklist(item))
    else {
        return Vec::new();
    };
    let now = entries(item);
    let before = items[..index]
        .iter()
        .map(Borrow::<Value>::borrow)
        .rposition(is_checklist)
        .map(|at| entries(items[at].borrow()))
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
                identity_at: 0,
            })
        })
        .collect()
}

/// Positional ids (`step-N` from Codex, `todo-N` from TodoWrite) shift when a
/// list is rewritten, so only agent-assigned ids identify a task by id alone.
fn same_task(a: &Task, b: &Task) -> bool {
    let positional = |id: &str| id.starts_with("step-") || id.starts_with("todo-");
    if !positional(&a.id) && !positional(&b.id) {
        a.id == b.id
    } else {
        a.step == b.step
    }
}

/// Walks back through contiguous snapshots that still contain `task`, keeping
/// the earliest one where it was InProgress, else the earliest it appeared.
fn task_history<T: Borrow<Value>>(items: &[T], index: usize, task: &Task) -> (usize, usize) {
    let mut appeared = index;
    let mut started = (task.status == TaskStatus::InProgress).then_some(index);
    for at in (0..index)
        .rev()
        .filter(|&at| is_checklist(items[at].borrow()))
    {
        let Some(old) = entries(items[at].borrow())
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
    (appeared, started.unwrap_or(appeared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pending_to_active_keeps_identity_but_removal_recreation_does_not() {
        let pending = json!({"id":"pending","type":"plan","plan":[{"id":"step-0","step":"Verify","status":"pending"}]});
        let active = json!({"id":"active","type":"plan","plan":[{"id":"step-0","step":"Verify","status":"inProgress"}]});
        let before = current(&[pending.clone()]).unwrap().tasks.remove(0);
        let after = current(&[pending.clone(), active.clone()])
            .unwrap()
            .tasks
            .remove(0);
        assert_eq!(before.identity_at, after.identity_at);
        assert_ne!(before.started_at, after.started_at);
        assert!(after.owns_activity(&before));
        let recreated = current(&[
            pending,
            active.clone(),
            json!({"type":"plan","plan":[]}),
            active,
        ])
        .unwrap()
        .tasks
        .remove(0);
        assert!(!recreated.owns_activity(&before));
    }
    #[test]
    fn activity_ownership_survives_progression_reordering_and_reload() {
        for prefix in ["step-", "todo-"] {
            let items = vec![
                json!({"type":"plan","plan":[{"id":format!("{prefix}0"),"step":"Inspect","status":"inProgress"},{"id":format!("{prefix}1"),"step":"Verify","status":"pending"}]}),
                json!({"type":"agentMessage","text":"Inspecting repository"}),
                json!({"type":"commandExecution","command":"pwd"}),
                json!({"type":"plan","plan":[{"id":format!("{prefix}0"),"step":"Verify","status":"inProgress"},{"id":format!("{prefix}1"),"step":"Inspect","status":"completed"}]}),
                json!({"type":"agentMessage","text":"Running checks"}),
                json!({"type":"plan","plan":[{"id":format!("{prefix}0"),"step":"Verify","status":"completed"},{"id":format!("{prefix}1"),"step":"Inspect","status":"completed"}]}),
                json!({"type":"agentMessage","text":"Done","phase":"final_answer"}),
                json!({"type":"userMessage","text":"Follow up"}),
                json!({"type":"agentMessage","text":"Following up"}),
            ];
            let restored: Vec<Value> =
                serde_json::from_str(&serde_json::to_string(&items).unwrap()).unwrap();
            for history in [&items, &restored] {
                let list = current(history).unwrap();
                let first = activity_owner(history, 1).unwrap();
                let second = activity_owner(history, 4).unwrap();
                assert_eq!(first.step, "Inspect");
                assert_eq!(second.step, "Verify");
                assert!(list.tasks[1].owns_activity(&first));
                assert!(list.tasks[0].owns_activity(&second));
                assert!(!list.tasks[0].owns_activity(&first));
                assert!(activity_owner(history, 2).is_none());
                assert!(activity_owner(history, 6).is_none());
                assert!(activity_owner(history, 8).is_none());
            }
        }
    }

    #[test]
    fn activity_ownership_rejects_ambiguous_and_legacy_snapshots() {
        let progress = json!({"type":"agentMessage","text":"Working"});
        assert!(activity_owner(&[progress.clone()], 0).is_none());
        let parallel = json!({"type":"plan","plan":[{"id":"1","step":"A","status":"inProgress"},{"id":"2","step":"B","status":"inProgress"}]});
        assert!(activity_owner(&[parallel, progress.clone()], 1).is_none());
        let mut legacy = json!({"id":"grove-plan-turn-1","type":"plan","plan":[{"step":"A","status":"inProgress"}]});
        assert!(activity_owner(&[legacy.clone(), progress.clone()], 1).is_none());
        legacy["taskSnapshot"] = json!(true);
        assert_eq!(activity_owner(&[legacy, progress], 1).unwrap().step, "A");
    }

    #[test]
    fn distinct_stable_tasks_with_the_same_title_do_not_share_activity() {
        let items = vec![
            json!({"type":"plan","plan":[{"id":"1","step":"Run checks","status":"inProgress"},{"id":"2","step":"Run checks","status":"pending"}]}),
            json!({"type":"agentMessage","text":"Running first checks"}),
            json!({"type":"plan","plan":[{"id":"1","step":"Run checks","status":"completed"},{"id":"2","step":"Run checks","status":"inProgress"}]}),
            json!({"type":"agentMessage","text":"Running second checks"}),
        ];
        let list = current(&items).unwrap();
        let first = activity_owner(&items, 1).unwrap();
        let second = activity_owner(&items, 3).unwrap();
        assert!(list.tasks[0].owns_activity(&first));
        assert!(!list.tasks[1].owns_activity(&first));
        assert!(list.tasks[1].owns_activity(&second));
        assert!(!list.tasks[0].owns_activity(&second));
    }

    #[test]
    fn stable_claude_task_id_preserves_rename_and_replacement_does_not_inherit_history() {
        let items = vec![
            json!({"type":"plan","plan":[{"id":"7","step":"Old title","status":"inProgress"}]}),
            json!({"type":"agentMessage","text":"Working"}),
            json!({"type":"plan","plan":[{"id":"7","step":"Renamed title","status":"completed"}]}),
        ];
        let owner = activity_owner(&items, 1).unwrap();
        assert!(current(&items).unwrap().tasks[0].owns_activity(&owner));
        let mut replaced = items;
        replaced.push(json!({"type":"plan","plan":[]}));
        replaced.push(
            json!({"type":"plan","plan":[{"id":"7","step":"Renamed title","status":"inProgress"}]}),
        );
        assert!(!current(&replaced).unwrap().tasks[0].owns_activity(&owner));
    }

    #[test]
    fn provider_guidance_shares_outcome_and_evidence_policy() {
        for guidance in [CODEX_GUIDANCE, CLAUDE_GUIDANCE] {
            for clause in [
                "observable, independently completable outcomes",
                "Split substantial shared prerequisites",
                "Separate meaningful verification outcomes",
                "live provider checks and native UI inspection",
                "split or add substantial work and discovered defects",
                "routine compiler fixes, repeated checks and cache cleanup in progress notes",
                "Preserve small lists for genuinely small requests",
                "appropriate evidence; worker reports alone are not verification",
                "Keep unverified coverage as explicitly unfinished verification work",
                "Preserve task ownership and existing task state",
                "user disclosure preferences, permission boundaries or explicit instructions",
                "Phrase completed work in past tense and work currently in progress in present continuous tense",
            ] {
                assert!(guidance.contains(clause), "Missing task policy: {clause}");
            }
        }
    }

    #[test]
    fn provider_guidance_respects_active_task_limits() {
        assert!(CODEX_GUIDANCE.contains("at most one in_progress step"));
        assert!(CODEX_GUIDANCE.contains("retain all unfinished outcomes"));
        assert!(CODEX_GUIDANCE.contains("other concurrently active or delegated outcomes by title"));
        assert!(CLAUDE_GUIDANCE.contains("TaskCreate creates pending tasks"));
        assert!(
            CLAUDE_GUIDANCE.contains("TaskCreate/TaskUpdate can represent multiple active tasks")
        );
        assert!(CLAUDE_GUIDANCE.contains(
            "TodoWrite replaces the list and its instructions limit it to one in_progress task"
        ));
        assert!(CLAUDE_GUIDANCE.contains("read the latest task state"));
    }

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
        assert!(current::<Value>(&[]).is_none());
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
