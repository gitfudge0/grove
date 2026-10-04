//! Agent commands reuse Grove's live services, with durable receipts around mutations.
use crate::{entities::session_registry::SessionId, runtime::Runtime, settings::SettingsState};
use gpui::Context;
use grove_core::{
    agent::Agent,
    control::{self, ControlCommand, ControlRequest, ControlResponse},
    control_state::ControlState,
    storage::Project,
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct ControlRuntime {
    pub state: ControlState,
    ids: HashMap<SessionId, String>,
    initialized: bool,
}
impl ControlRuntime {
    pub fn new(state: ControlState) -> Self {
        Self {
            state,
            ids: HashMap::new(),
            initialized: false,
        }
    }
}
fn project(store: &grove_core::storage::Store, selector: &str) -> Result<Project, String> {
    let path = fs_err::canonicalize(selector).ok();
    let matches: Vec<_> = store
        .projects
        .iter()
        .filter(|p| {
            p.name == selector
                || path
                    .as_ref()
                    .is_some_and(|path| fs_err::canonicalize(&p.path).is_ok_and(|p| p == *path))
        })
        .collect();
    match matches.as_slice() {
        [project] => Ok((*project).clone()),
        [] => Err("Project is not registered in Grove".into()),
        _ => Err("Project name is ambiguous; use its absolute path".into()),
    }
}
fn selected_worktree(project: &Project, path: &str) -> Result<String, String> {
    let canonical = fs_err::canonicalize(path).map_err(|e| e.to_string())?;
    if canonical == fs_err::canonicalize(&project.path).map_err(|e| e.to_string())? {
        return Ok(canonical.to_string_lossy().into_owned());
    }
    let worktrees =
        grove_core::git::list_worktrees_checked(&project.path).map_err(|e| e.to_string())?;
    if worktrees
        .iter()
        .any(|wt| fs_err::canonicalize(&wt.path).is_ok_and(|p| p == canonical))
    {
        return Ok(canonical.to_string_lossy().into_owned());
    }
    Err("Worktree does not belong to the selected project".into())
}
fn mutates(command: &ControlCommand) -> bool {
    matches!(
        command,
        ControlCommand::CreateWorktree { .. }
            | ControlCommand::StartSession { .. }
            | ControlCommand::Focus { .. }
            | ControlCommand::Stop { .. }
            | ControlCommand::CompleteTask { .. }
    )
}
impl Runtime {
    pub(crate) fn handle_control(
        &mut self,
        request: ControlRequest,
        cx: &mut Context<Self>,
    ) -> ControlResponse {
        if request.version != control::VERSION {
            return ControlResponse::error(
                "protocol_version",
                "Unsupported control protocol version",
            );
        }
        let Some(mut owner) = self.control.take() else {
            return ControlResponse::error("unavailable", "Control service is unavailable");
        };
        let mutation = mutates(&request.command);
        if mutation {
            match owner.state.begin(&request.request_id, &request.command) {
                Ok(Some(cached)) => {
                    self.control = Some(owner);
                    return cached;
                }
                Err(error) => {
                    self.control = Some(owner);
                    return ControlResponse::error("request_conflict", error);
                }
                Ok(None) => {}
            }
        }
        let response = match self.control_command(&mut owner, request.command, cx) {
            Ok(value) => ControlResponse::success(value),
            Err(error) => {
                let code = if error.starts_with("Request outcome is uncertain") {
                    "outcome_unknown"
                } else {
                    "command_failed"
                };
                ControlResponse::error(code, error)
            }
        };
        let uncertain = response
            .error
            .as_ref()
            .is_some_and(|error| error.code == "outcome_unknown");
        let response = if mutation && !uncertain {
            match owner.state.finish(&request.request_id, response.clone()) {
                Ok(()) => response,
                Err(error) => ControlResponse::error(
                    "persistence_failed",
                    format!(
                        "Request outcome is uncertain: {error}. Inspect state before retrying."
                    ),
                ),
            }
        } else {
            response
        };
        self.control = Some(owner);
        response
    }
    fn control_sessions(
        &mut self,
        owner: &mut ControlRuntime,
        cx: &mut Context<Self>,
    ) -> Result<Vec<Value>, String> {
        let metas = self.registry.read(cx).all().to_vec();
        let mut rows = Vec::new();
        for meta in metas {
            let id = if let Some(id) = owner.ids.get(&meta.id) {
                id.clone()
            } else {
                let id = if let Some(name) = &meta.tmux_name {
                    let mut sidecar = grove_core::session_meta::read(name)
                        .ok_or("Session sidecar is unavailable")?;
                    if let Some(id) = &sidecar.control_id {
                        id.clone()
                    } else {
                        let id = control::random_id()?;
                        sidecar.control_id = Some(id.clone());
                        grove_core::session_meta::write(name, &sidecar)
                            .map_err(|e| e.to_string())?;
                        id
                    }
                } else {
                    control::random_id()?
                };
                owner.ids.insert(meta.id, id.clone());
                id
            };
            let term = self.registry.read(cx).session(meta.id).cloned();
            let (status, error) = term.map_or(("unavailable", None), |term| {
                term.update(cx, |term, _| {
                    let error = term.spawn_error().map(str::to_owned);
                    let alive = if let Some(name) = &meta.tmux_name {
                        grove_core::tmux::has_session(name)
                    } else {
                        term.alive()
                    };
                    (
                        if error.is_some() {
                            "failed"
                        } else if alive {
                            "running"
                        } else {
                            "exited"
                        },
                        error,
                    )
                })
            });
            let task_id = owner
                .state
                .tasks()
                .find(|task| task.session_id.as_deref() == Some(&id))
                .map(|task| task.id.clone());
            rows.push(json!({"id":id,"project":meta.project,"worktree":meta.wt_path,"agent":meta.agent,"label":meta.label,"backend":if meta.tmux {"tmux"} else {"native"},"status":status,"spawn_error":error,"task_id":task_id}));
        }
        let tasks: Vec<_> = owner
            .state
            .tasks()
            .filter(|task| !task.terminal())
            .map(|task| (task.id.clone(), task.session_id.clone()))
            .collect();
        for (id, session) in tasks {
            let status = rows
                .iter()
                .find(|row| row["id"].as_str() == session.as_deref())
                .and_then(|row| row["status"].as_str());
            match status {
                Some("exited" | "failed") => owner.state.set_status(&id, "awaiting_result")?,
                None => owner.state.set_status(
                    &id,
                    if owner.initialized {
                        "awaiting_result"
                    } else {
                        "interrupted"
                    },
                )?,
                _ => {}
            }
        }
        owner.initialized = true;
        Ok(rows)
    }
    fn control_command(
        &mut self,
        owner: &mut ControlRuntime,
        command: ControlCommand,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let sessions = self.control_sessions(owner, cx)?;
        match command {
            ControlCommand::ListProjects => Ok(json!(cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .map(|p| json!({"name":p.name,"path":p.path,"archived":p.archived}))
                .collect::<Vec<_>>())),
            ControlCommand::ListWorktrees { project: selector } => {
                let project = project(&cx.global::<SettingsState>().store, &selector)?;
                let rows = if grove_core::git::is_repo(&project.path) {
                    grove_core::git::list_worktrees_checked(&project.path)
                        .map_err(|e| e.to_string())?
                } else {
                    vec![grove_core::git::Worktree {
                        path: project.path.clone(),
                        branch: String::new(),
                        mtime: None,
                        is_main: true,
                    }]
                };
                Ok(json!(rows
                    .iter()
                    .map(|wt| json!({"path":wt.path,"branch":wt.branch,"is_main":wt.is_main}))
                    .collect::<Vec<_>>()))
            }
            ControlCommand::CreateWorktree {
                project: selector,
                name,
                base,
            } => {
                let project = project(&cx.global::<SettingsState>().store, &selector)?;
                crate::project_service::worktree_prerequisite(&project.path)?;
                let path = self.projects.update(cx, |service, cx| {
                    service.create_background_worktree(&project, &name, base.as_deref(), cx)
                })?;
                Ok(json!({"path":path,"project":project.path}))
            }
            ControlCommand::ListSessions => Ok(json!(sessions)),
            ControlCommand::ShowSession { id } => sessions
                .into_iter()
                .find(|row| row["id"] == id)
                .ok_or_else(|| "Session does not exist in this Grove run".into()),
            ControlCommand::StartSession {
                project: selector,
                worktree,
                agent,
                prompt,
                backend,
                task,
            } => {
                let project = project(&cx.global::<SettingsState>().store, &selector)?;
                if project.archived {
                    return Err("Unarchive this project before launching a session".into());
                }
                let cwd = selected_worktree(&project, &worktree)?;
                if !agent.available() {
                    return Err(format!(
                        "{} is not available on Grove's PATH",
                        agent.label()
                    ));
                }
                if agent == Agent::Terminal && (prompt.is_some() || task.is_some()) {
                    return Err(
                        "Terminal sessions do not accept agent prompts or delegated tasks".into(),
                    );
                }
                let backend = backend
                    .or(cx.global::<SettingsState>().store.tmux_enabled)
                    .ok_or("Choose --backend native|tmux; no saved backend preference exists")?;
                if backend && !grove_core::tmux::available() {
                    return Err("tmux is unavailable; choose --backend native".into());
                }
                if self.has_pending_managed_launch() {
                    return Err(
                        "Resolve the desktop's pending backend choice before launching".into(),
                    );
                }
                let task_id = if let Some(spec) = task {
                    let id = control::random_id()?;
                    owner.state.create_task(id.clone(), spec)?;
                    owner.state.set_launch_context(
                        &id,
                        project.path.clone(),
                        cwd.clone(),
                        if backend { "tmux" } else { "native" }.into(),
                    )?;
                    Some(id)
                } else {
                    None
                };
                let mut args = agent.launch_args(
                    cx.global::<SettingsState>()
                        .store
                        .dangerously_skip_permissions_enabled
                        .unwrap_or(false),
                    cx.global::<SettingsState>()
                        .store
                        .chrome_enabled
                        .unwrap_or(false),
                );
                let instructions = task_id.as_deref().and_then(|id|owner.state.task(id)).map(|record| format!("{}\n\nGrove task ID: {}. Submit an explicit result with `grove tasks complete {} --result-file FILE`. Result JSON requires status (completed or failed), summary, changed_files, checks, and unresolved. Do not mark success from terminal idleness. GROVE_TASK_ID contains your task ID.",record.instructions,record.id,record.id));
                let text = [prompt, instructions]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.is_empty() {
                    let text = format!("Grove launch instructions:\n{text}");
                    if agent == Agent::OpenCode {
                        args.push("--prompt".into());
                    }
                    args.push(text);
                }
                let before: Vec<_> = self
                    .registry
                    .read(cx)
                    .all()
                    .iter()
                    .map(|meta| meta.id)
                    .collect();
                let launched = self.spawn_session_with_options(
                    project.name.clone(),
                    cwd,
                    agent,
                    args,
                    Vec::new(),
                    None,
                    false,
                    Some(backend),
                    task_id.clone(),
                    cx,
                );
                let local = self
                    .registry
                    .read(cx)
                    .all()
                    .iter()
                    .find(|meta| !before.contains(&meta.id))
                    .map(|meta| meta.id);
                if !launched {
                    if let Some(id) = &task_id {
                        owner.state.set_status(id, "failed")?;
                    }
                    if let Some(id) = local {
                        self.kill_session(id, cx);
                    }
                    return Err("Failed to spawn session; inspect Grove's error message".into());
                }
                (|| -> Result<Value, String> {
                let local = local.ok_or("Launch returned no session")?;
                let rows = self.control_sessions(owner, cx)?;
                let id = owner
                    .ids
                    .get(&local)
                    .ok_or("Session has no control ID")?
                    .clone();
                if let Some(task) = &task_id {
                    owner.state.set_session(task, id.clone())?;
                    let actual = rows
                        .iter()
                        .find(|row| row["id"] == id)
                        .and_then(|row| row["backend"].as_str())
                        .unwrap_or("native");
                    owner.state.set_launch_context(
                        task,
                        project.path.clone(),
                        selected_worktree(&project, &worktree)?,
                        actual.into(),
                    )?;
                }
                let mut row = rows
                    .into_iter()
                    .find(|row| row["id"] == id)
                    .ok_or("Session is unavailable")?;
                row["task_id"] = json!(task_id);
                Ok(row)
                })().map_err(|error| format!("Request outcome is uncertain; a session was launched: {error}. Inspect sessions and tasks before choosing a new request ID."))
            }
            ControlCommand::Logs { id, lines } => {
                if lines == 0 || lines > 10_000 {
                    return Err("Lines must be between 1 and 10000".into());
                }
                let local = live_id(owner, &sessions, &id)?;
                let term = self
                    .registry
                    .read(cx)
                    .session(local)
                    .cloned()
                    .ok_or("Session terminal is unavailable")?;
                let text = term.update(cx, |term, cx| {
                    if term.is_pending_attach() {
                        term.attach_now(cx);
                    }
                    term.tail_contents(lines)
                });
                Ok(json!({"id":id,"text":text,"scope":"live_screen","requested_lines":lines}))
            }
            ControlCommand::Focus { id } => {
                let local = live_id(owner, &sessions, &id)?;
                let snap = self.snapshot(cx);
                let old = self.state.read(cx).proj_idx();
                crate::entities::project_tree::ProjectTree::adopt_session_project(
                    &self.tree.clone(),
                    &snap,
                    local,
                    old,
                    cx,
                );
                self.state.update(cx, |state, cx| {
                    state.select_session(local, &snap);
                    cx.notify();
                });
                Ok(json!({"id":id,"focused":true}))
            }
            ControlCommand::Stop { id } => {
                let local = live_id(owner, &sessions, &id)?;
                let tasks: Vec<_> = owner
                    .state
                    .tasks()
                    .filter(|task| task.session_id.as_deref() == Some(&id) && !task.terminal())
                    .map(|task| task.id.clone())
                    .collect();
                for task in tasks {
                    owner.state.set_status(&task, "cancelled")?;
                }
                self.kill_session(local, cx);
                Ok(json!({"id":id,"stopped":true}))
            }
            ControlCommand::ListTasks => Ok(json!(owner.state.tasks().collect::<Vec<_>>())),
            ControlCommand::ShowTask { id } => owner
                .state
                .task(&id)
                .map(|task| json!(task))
                .ok_or_else(|| "Task does not exist".into()),
            ControlCommand::CompleteTask { id, result } => {
                owner.state.complete(&id, result)?;
                Ok(json!(owner.state.task(&id)))
            }
        }
    }
}
fn live_id(owner: &ControlRuntime, sessions: &[Value], id: &str) -> Result<SessionId, String> {
    if !sessions.iter().any(|row| row["id"] == id) {
        return Err("Session does not exist in this Grove run".into());
    }
    owner
        .ids
        .iter()
        .find_map(|(local, external)| (external == id).then_some(*local))
        .ok_or_else(|| "Session does not exist".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> Fixture {
        let path = std::env::temp_dir().join(format!(
            "grove-control-runtime-{}",
            control::random_id().unwrap()
        ));
        fs_err::create_dir(&path).unwrap();
        Fixture(path)
    }
    fn setup(cx: &mut gpui::App, path: &std::path::Path) -> gpui::Entity<Runtime> {
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![Project {
                name: "control fixture".into(),
                path: path.to_string_lossy().into_owned(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(Runtime::new);
        runtime.update(cx, |runtime, _| {
            runtime.control = Some(ControlRuntime::new(
                ControlState::load(path.join("control-state.json")).unwrap(),
            ));
        });
        runtime
    }
    fn request(command: ControlCommand, id: &str) -> ControlRequest {
        ControlRequest {
            version: control::VERSION,
            request_id: id.into(),
            command,
        }
    }
    fn start(path: &std::path::Path) -> ControlCommand {
        ControlCommand::StartSession {
            project: path.to_string_lossy().into_owned(),
            worktree: path.to_string_lossy().into_owned(),
            agent: Agent::Terminal,
            prompt: None,
            backend: Some(false),
            task: None,
        }
    }
    #[gpui::test]
    fn native_launch_retry_preserves_selection_and_stop_targets_one_session(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = fixture();
        let runtime = cx.update(|cx| setup(cx, &dir.0));
        let command = start(&dir.0);
        let first = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(request(command.clone(), "first"), cx)
        });
        assert!(first.ok, "{:?}", first.error);
        let retry = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(request(command, "first"), cx)
        });
        assert_eq!(retry.data, first.data);
        let second = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(request(start(&dir.0), "second"), cx)
        });
        assert!(second.ok);
        runtime.update(cx, |runtime, cx| {
            assert_eq!(runtime.registry.read(cx).len(), 2);
            assert_eq!(runtime.state.read(cx).active_session(), None);
        });
        let id = first.data["id"].as_str().unwrap().to_owned();
        let stopped = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(request(ControlCommand::Stop { id }, "stop"), cx)
        });
        assert!(stopped.ok);
        runtime.update(cx, |runtime, cx| {
            assert_eq!(runtime.registry.read(cx).len(), 1);
        });
        let invalid = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(
                request(
                    ControlCommand::StartSession {
                        project: dir.0.to_string_lossy().into_owned(),
                        worktree: "/missing-grove-control-worktree".into(),
                        agent: Agent::Terminal,
                        prompt: None,
                        backend: Some(false),
                        task: None,
                    },
                    "invalid",
                ),
                cx,
            )
        });
        assert!(!invalid.ok);
        let second_id = second.data["id"].as_str().unwrap().to_owned();
        runtime.update(cx, |runtime, cx| {
            assert!(
                runtime
                    .handle_control(
                        request(ControlCommand::Stop { id: second_id }, "stop-second"),
                        cx
                    )
                    .ok
            );
        });
    }
    #[gpui::test]
    fn a_native_task_is_interrupted_after_owner_restart(cx: &mut gpui::TestAppContext) {
        let dir = fixture();
        let mut store = ControlState::load(dir.0.join("control-state.json")).unwrap();
        store
            .create_task(
                "old-task".into(),
                control::TaskSpec {
                    title: "old".into(),
                    instructions: "work".into(),
                    parent: None,
                },
            )
            .unwrap();
        store
            .set_session("old-task", "old-native-session".into())
            .unwrap();
        let runtime = cx.update(|cx| setup(cx, &dir.0));
        let response = runtime.update(cx, |runtime, cx| {
            runtime.handle_control(
                request(
                    ControlCommand::ShowTask {
                        id: "old-task".into(),
                    },
                    "read",
                ),
                cx,
            )
        });
        assert!(response.ok);
        assert_eq!(response.data["status"], "interrupted");
    }
}
