//! Durable launch receipts and delegated task results. No terminal heuristics decide success.
use crate::control::{ControlCommand, ControlResponse, TaskResult, TaskSpec};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub instructions: String,
    pub parent: Option<String>,
    pub session_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    pub status: String,
    pub result: Option<TaskResult>,
}

impl TaskRecord {
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    command: ControlCommand,
    response: Option<ControlResponse>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Records {
    tasks: BTreeMap<String, TaskRecord>,
    receipts: BTreeMap<String, Receipt>,
}

pub struct ControlState {
    path: PathBuf,
    records: Records,
}

impl ControlState {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let records = match fs_err::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("Invalid control state: {e}"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Records::default(),
            Err(e) => return Err(e.to_string()),
        };
        Ok(Self { path, records })
    }

    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.records).map_err(|e| e.to_string())?;
        crate::storage::write_atomic_private(&self.path, &bytes).map_err(|e| e.to_string())?;
        fs_err::File::open(&self.path)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs_err::File::open(
            self.path
                .parent()
                .ok_or("Control state requires a directory")?,
        )
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Persist intent before any side effect. An uncertain receipt must never launch again.
    pub fn begin(
        &mut self,
        id: &str,
        command: &ControlCommand,
    ) -> Result<Option<ControlResponse>, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err(
                "Request ID must contain 1–128 letters, digits, hyphens or underscores".into(),
            );
        }
        if let Some(receipt) = self.records.receipts.get(id) {
            if serde_json::to_value(&receipt.command).map_err(|e| e.to_string())?
                != serde_json::to_value(command).map_err(|e| e.to_string())?
            {
                return Err("Request ID was already used with different arguments".into());
            }
            return receipt.response.clone().map(Some).ok_or_else(|| "Request outcome is uncertain; inspect sessions/worktrees/tasks before issuing a new request ID".into());
        }
        if self.records.receipts.len() >= 4096 {
            return Err("Control receipt limit reached (4096); archive control state only after all callers have retired their request IDs".into());
        }
        self.records.receipts.insert(
            id.into(),
            Receipt {
                command: command.clone(),
                response: None,
            },
        );
        if let Err(error) = self.save() {
            self.records.receipts.remove(id);
            return Err(error);
        }
        Ok(None)
    }

    pub fn finish(&mut self, id: &str, response: ControlResponse) -> Result<(), String> {
        let receipt = self
            .records
            .receipts
            .get_mut(id)
            .ok_or("Unknown request receipt")?;
        receipt.response = Some(response);
        if let Err(error) = self.save() {
            if let Some(receipt) = self.records.receipts.get_mut(id) {
                receipt.response = None;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn tasks(&self) -> impl Iterator<Item = &TaskRecord> {
        self.records.tasks.values()
    }
    pub fn task(&self, id: &str) -> Option<&TaskRecord> {
        self.records.tasks.get(id)
    }

    pub fn create_task(&mut self, id: String, spec: TaskSpec) -> Result<(), String> {
        if spec.title.trim().is_empty()
            || spec.title.len() > 200
            || spec.instructions.trim().is_empty()
            || spec.instructions.len() > 128 * 1024
        {
            return Err(
                "Tasks require a title (up to 200 bytes) and instructions (up to 128 KiB)".into(),
            );
        }
        if self.records.tasks.len() >= 4096 {
            return Err("Task record limit reached (4096)".into());
        }
        if self.tasks().filter(|task| !task.terminal()).count() >= 8 {
            return Err("At most eight unfinished delegated tasks may run at once".into());
        }
        let mut parent = spec.parent.as_deref();
        let mut depth = 0;
        while let Some(parent_id) = parent {
            let task = self.task(parent_id).ok_or("Parent task does not exist")?;
            if task.terminal() {
                return Err("Cannot delegate from a terminal parent task".into());
            }
            depth += 1;
            if depth >= 4 {
                return Err("Delegation is limited to four task levels".into());
            }
            parent = task.parent.as_deref();
        }
        if self.records.tasks.contains_key(&id) {
            return Err("Task ID already exists".into());
        }
        let previous = self.records.tasks.insert(
            id.clone(),
            TaskRecord {
                id: id.clone(),
                title: spec.title,
                instructions: spec.instructions,
                parent: spec.parent,
                session_id: None,
                project: None,
                worktree: None,
                backend: None,
                status: "starting".into(),
                result: None,
            },
        );
        if previous.is_some() {
            return Err("Task ID already exists".into());
        }
        if let Err(error) = self.save() {
            self.records.tasks.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    pub fn set_launch_context(
        &mut self,
        id: &str,
        project: String,
        worktree: String,
        backend: String,
    ) -> Result<(), String> {
        self.update_task(id, |task| {
            task.project = Some(project);
            task.worktree = Some(worktree);
            task.backend = Some(backend);
        })
    }

    pub fn set_session(&mut self, id: &str, session: String) -> Result<(), String> {
        self.update_task(id, |task| {
            task.session_id = Some(session);
            task.status = "running".into();
        })
    }

    pub fn set_status(&mut self, id: &str, status: &str) -> Result<(), String> {
        self.update_task(id, |task| {
            if !task.terminal() {
                task.status = status.into();
            }
        })
    }

    pub fn complete(&mut self, id: &str, result: TaskResult) -> Result<(), String> {
        if !matches!(result.status.as_str(), "completed" | "failed")
            || result.summary.trim().is_empty()
        {
            return Err("Result requires status completed|failed and a nonempty summary".into());
        }
        let task = self.task(id).ok_or("Task does not exist")?;
        if task.terminal() && task.status != "interrupted" {
            if task.result.as_ref().is_some_and(|old| {
                serde_json::to_value(old).ok() == serde_json::to_value(&result).ok()
            }) {
                return Ok(());
            }
            return Err("Task already has a terminal outcome".into());
        }
        self.update_task(id, |task| {
            task.status.clone_from(&result.status);
            task.result = Some(result);
        })
    }

    fn update_task(
        &mut self,
        id: &str,
        update: impl FnOnce(&mut TaskRecord),
    ) -> Result<(), String> {
        let original = self.task(id).ok_or("Task does not exist")?.clone();
        if let Some(task) = self.records.tasks.get_mut(id) {
            update(task);
        }
        if let Err(error) = self.save() {
            self.records.tasks.insert(id.into(), original);
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_survive_restart_and_reject_conflicting_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let command = ControlCommand::Stop {
            id: "session".into(),
        };
        let mut state = ControlState::load(path.clone()).unwrap();
        assert!(state.begin("retry", &command).unwrap().is_none());
        let mut state = ControlState::load(path.clone()).unwrap();
        assert!(state
            .begin("retry", &command)
            .unwrap_err()
            .contains("uncertain"));
        let response = ControlResponse::success(serde_json::json!({"stopped":true}));
        state.finish("retry", response).unwrap();
        let mut state = ControlState::load(path).unwrap();
        assert!(state.begin("retry", &command).unwrap().unwrap().ok);
        assert!(state
            .begin("retry", &ControlCommand::Stop { id: "other".into() })
            .is_err());
    }
    #[test]
    fn completion_is_explicit_durable_and_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = ControlState::load(path.clone()).unwrap();
        state
            .create_task(
                "task".into(),
                TaskSpec {
                    title: "work".into(),
                    instructions: "inspect".into(),
                    parent: None,
                },
            )
            .unwrap();
        state
            .set_launch_context(
                "task",
                "/project".into(),
                "/worktree".into(),
                "native".into(),
            )
            .unwrap();
        state.set_session("task", "session".into()).unwrap();
        state.set_status("task", "awaiting_result").unwrap();
        assert!(!state.task("task").unwrap().terminal());
        state.set_status("task", "interrupted").unwrap();
        let restored = ControlState::load(path.clone()).unwrap();
        assert_eq!(
            restored.task("task").unwrap().worktree.as_deref(),
            Some("/worktree")
        );
        let result = TaskResult {
            summary: "checked".into(),
            changed_files: vec![],
            checks: vec!["test passed".into()],
            unresolved: vec![],
            status: "completed".into(),
        };
        state.complete("task", result.clone()).unwrap();
        let mut state = ControlState::load(path).unwrap();
        state.complete("task", result).unwrap();
        state.set_status("task", "interrupted").unwrap();
        assert_eq!(state.task("task").unwrap().status, "completed");
        assert!(state
            .create_task(
                "child".into(),
                TaskSpec {
                    title: "child".into(),
                    instructions: "work".into(),
                    parent: Some("task".into())
                }
            )
            .is_err());
    }
}
