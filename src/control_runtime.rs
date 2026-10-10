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
fn skill_target(
    agent: &str,
    selector: Option<&str>,
    store: &grove_core::storage::Store,
) -> Result<
    (
        grove_core::skill_install::AgentTarget,
        grove_core::skill_install::InstallScope,
    ),
    String,
> {
    use grove_core::skill_install::{AgentTarget, InstallScope};
    let agent = match agent {
        "codex" => AgentTarget::Codex,
        "claude" => AgentTarget::Claude,
        "opencode" => AgentTarget::OpenCode,
        _ => return Err("Agent must be codex, claude or opencode".into()),
    };
    let scope = match selector {
        Some(selector) => InstallScope::Project(project(store, selector)?.path.into()),
        None => InstallScope::User,
    };
    Ok((agent, scope))
}
fn confirm_exact(actual: &str, expected: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("Confirmation must exactly match {expected}"))
    }
}
fn mutates(command: &ControlCommand) -> bool {
    !matches!(
        command,
        ControlCommand::ListProjects
            | ControlCommand::SkillStatus { .. }
            | ControlCommand::ListWorkspaces
            | ControlCommand::ListWorktrees { .. }
            | ControlCommand::ListSessions
            | ControlCommand::ShowSession { .. }
            | ControlCommand::Logs { .. }
            | ControlCommand::ListTasks
            | ControlCommand::ShowTask { .. }
            | ControlCommand::ProjectRemovalStatus { .. }
            | ControlCommand::WorktreeRemovalStatus { .. }
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
        let metas = self.registry.read(cx).control_metas();
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
            let term = self.registry.read(cx).control_terminal(meta.id).cloned();
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
            rows.push(json!({"id":id,"project":meta.project,"worktree":meta.wt_path,"agent":meta.agent,"label":meta.label,"context_roots":meta.context_roots,"backend":if meta.tmux {"tmux"} else {"native"},"status":status,"spawn_error":error,"task_id":task_id}));
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
            ControlCommand::SkillStatus { agent, project: selector } => {
                let (agent,scope) = skill_target(&agent,selector.as_deref(),&cx.global::<SettingsState>().store)?;
                grove_core::skill_install::status(agent,&scope).map(|value|json!(value)).map_err(|e|e.to_string())
            }
            ControlCommand::InstallSkill { agent, project: selector, overwrite } => {
                let (agent,scope) = skill_target(&agent,selector.as_deref(),&cx.global::<SettingsState>().store)?;
                grove_core::skill_install::install(agent,&scope,overwrite).map(|value|json!(value)).map_err(|e|e.to_string())
            }
            ControlCommand::InitProjectGit { project: selector } => {
                let p = project(&cx.global::<SettingsState>().store,&selector)?;
                if self.projects.read(cx).is_removing(&p.path) { return Err("Project is being removed".into()); }
                grove_core::git::init_if_needed(&p.path).map_err(|e|e.to_string())?;
                self.projects.update(cx,crate::project_service::ProjectService::project_git_initialized);
                Ok(json!({"path":p.path,"git_initialized":true}))
            }
            ControlCommand::SkipWorktreeTeardown { path, confirm } => {
                confirm_exact(&confirm,&path)?;
                self.projects.update(cx,|service,cx|service.skip_worktree_teardown_for(&path,cx))?;
                Ok(json!({"path":path,"accepted":true}))
            }
            ControlCommand::AddProject { name, path, workspace } => {
                let workspace = workspace.unwrap_or(cx.global::<SettingsState>().store.workspaces.active);
                let index = self.projects.update(cx, |service, cx| service.register_project_in_workspace(name, path, workspace, cx))?;
                Ok(json!(cx.global::<SettingsState>().store.projects[index]))
            }
            ControlCommand::EditProject { project: selector, name, setup, run, teardown } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                let mut scripts = p.scripts.clone();
                for (target, value) in [(&mut scripts.setup, setup), (&mut scripts.run, run), (&mut scripts.teardown, teardown)] { if let Some(value) = value { *target = if value.trim().is_empty() { None } else { Some(value) }; } }
                self.projects.update(cx, |service, cx| service.update_project(&p.path, name.unwrap_or(p.name), scripts, cx))?;
                Ok(json!({"path":p.path}))
            }
            ControlCommand::MoveProject { project: selector, workspace } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                self.projects.update(cx, |service, cx| service.move_project_to_workspace(&p.path, workspace, false, cx))?;
                Ok(json!({"path":p.path,"workspace":workspace}))
            }
            ControlCommand::ArchiveProject { project: selector } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                self.projects.update(cx, |service,cx| service.archive_project_by_path(&p.path,cx))?;
                Ok(json!({"path":p.path,"archived":true}))
            }
            ControlCommand::RestoreProject { project: selector } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                self.projects.update(cx, |service,cx| service.restore_archived_by_path(&p.path,cx))?;
                Ok(json!({"path":p.path,"archived":false}))
            }
            ControlCommand::DeleteProject { project: selector, confirm } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                confirm_exact(&confirm, &p.name)?;
                self.projects.update(cx, |service, cx| service.delete_archived_by_path(&p.path, cx))?;
                Ok(json!({"path":p.path,"unregistered":true}))
            }
            ControlCommand::RemoveProject { project: selector, confirm, remove_worktrees } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                confirm_exact(&confirm, &p.name)?;
                self.projects.update(cx, |service, cx| service.remove_project_by_path(&p.path, remove_worktrees, cx))?;
                Ok(json!({"path":p.path,"accepted":true,"status_command":"projects removal-status --path"}))
            }
            ControlCommand::ProjectRemovalStatus { path } => {
                let status = self.projects.read(cx).removal_status(&path).ok_or("No removal operation for this exact project path")?;
                Ok(json!({"path":path,"total":status.total,"completed":status.completed,"current_target":status.current_target,"errors":status.errors,"finished":status.finished,"unregistered":status.unregistered}))
            }
            ControlCommand::RemoveWorktree { project: selector, worktree, confirm } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                let path = selected_worktree(&p, &worktree)?;
                confirm_exact(&confirm, &path)?;
                self.projects.update(cx, |service, cx| service.remove_worktree_by_path(&p.path, &path, cx))?;
                Ok(json!({"path":path,"accepted":true,"status_command":"worktrees removal-status --path"}))
            }
            ControlCommand::WorktreeRemovalStatus { path } => {
                let status = self.projects.read(cx).worktree_removal_status(&path).ok_or("No removal operation for this exact worktree path")?;
                Ok(json!({"path":path,"stage":format!("{:?}",status.stage),"error":status.error,"finished":status.stage == crate::project_service::WorktreeRemovalStage::Finished}))
            }
            ControlCommand::ListWorkspaces => Ok(json!(cx.global::<SettingsState>().store.workspaces)),
            ControlCommand::CreateWorkspace { name } => {
                let mut state = cx.global::<SettingsState>().store.workspaces.clone(); state.create(&name)?;
                let id = state.active;
                let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| store.workspaces = state); saved.map_err(|e| e.to_string())?;
                Ok(json!({"id":id}))
            }
            ControlCommand::RenameWorkspace { id, name } => {
                let mut state = cx.global::<SettingsState>().store.workspaces.clone(); state.rename(id,&name)?;
                let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| store.workspaces = state); saved.map_err(|e| e.to_string())?;
                Ok(json!({"id":id}))
            }
            ControlCommand::SelectWorkspace { id } => {
                let mut state = cx.global::<SettingsState>().store.workspaces.clone();
                if !state.rows.iter().any(|row|row.id == id) { return Err("Workspace does not exist".into()); }
                state.select(id);
                let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| store.workspaces = state); saved.map_err(|e| e.to_string())?;
                Ok(json!({"id":id}))
            }
            ControlCommand::DeleteWorkspace { id, confirm } => {
                let store = &cx.global::<SettingsState>().store;
                let mut state = store.workspaces.clone();
                for row in &mut state.rows { row.projects = store.projects.iter().filter(|p|store.project_workspace_id(&p.path) == row.id).count(); }
                state.delete(id,&confirm)?;
                let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| store.workspaces = state); saved.map_err(|e| e.to_string())?;
                Ok(json!({"id":id,"deleted":true}))
            }
            ControlCommand::StartShell { project: selector, worktree } => {
                let before: Vec<_> = sessions.iter().filter_map(|s|s["id"].as_str().map(str::to_owned)).collect();
                match (selector,worktree) {
                    (None,None) => { self.spawn_home_terminal(cx).ok_or("Failed to spawn standalone shell")?; }
                    (Some(selector),Some(worktree)) => {
                        let p = project(&cx.global::<SettingsState>().store,&selector)?;
                        if p.archived || self.projects.read(cx).is_removing(&p.path) { return Err("Project is archived or being removed".into()); }
                        let path = selected_worktree(&p,&worktree)?;
                        if self.projects.read(cx).is_worktree_removing(&path) { return Err("Worktree is being removed".into()); }
                        self.spawn_wt_shell(&path,cx);
                    }
                    _ => return Err("Shell requires project and worktree together".into()),
                }
                self.control_sessions(owner,cx).map_err(|error|format!("Request outcome is uncertain; shell launched: {error}"))?.into_iter().find(|row|row["id"].as_str().is_some_and(|id|!before.iter().any(|old|old == id))).ok_or_else(||"Request outcome is uncertain; shell returned no control row".into())
            }
            ControlCommand::RunScript { project: selector, worktree } => {
                let p = project(&cx.global::<SettingsState>().store, &selector)?;
                let path = selected_worktree(&p, &worktree)?;
                let before: Vec<_> = sessions.iter().filter_map(|s|s["id"].as_str().map(str::to_owned)).collect();
                if !self.spawn_run_script(&p.path, &path, cx) { return Err("Run script launch failed or no run script configured".into()); }
                self.control_sessions(owner,cx).map_err(|error|format!("Request outcome is uncertain; script launched: {error}"))?.into_iter().find(|row|row["id"].as_str().is_some_and(|id|!before.iter().any(|old|old == id))).ok_or_else(||"Request outcome is uncertain; a script was launched without a control row".into())
            }
            ControlCommand::SessionInput { id, text } => {
                if text.len() > 512 * 1024 { return Err("Input exceeds 512 KiB".into()); }
                let local = live_id(owner,&sessions,&id)?;
                let term = self.registry.read(cx).control_terminal(local).cloned().ok_or("Session unavailable")?;
                if term.read(cx).has_exited() { return Err("Session has exited".into()); }
                term.update(cx, |term, cx| {
                    if term.is_pending_attach() { term.attach_now(cx); }
                    term.send_checked(text.as_bytes())
                })?;
                Ok(json!({"id":id,"bytes":text.len()}))
            }
            ControlCommand::ListProjects => Ok(json!(cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .map(|p| json!({"name":p.name,"path":p.path,"archived":p.archived,"scripts":p.scripts,"workspace":cx.global::<SettingsState>().store.project_workspace_id(&p.path)}))
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
                branch,
            } => {
                let project = project(&cx.global::<SettingsState>().store, &selector)?;
                crate::project_service::worktree_prerequisite(&project.path)?;
                let path = self.projects.update(cx, |service, cx| {
                    if let Some(branch) = &branch {
                        service.create_worktree_with_branch(
                            &project,
                            &name,
                            branch,
                            base.as_deref(),
                            cx,
                        )
                    } else {
                        service.create_background_worktree(&project, &name, base.as_deref(), cx)
                    }
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
                roots,
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
                let mut extra_roots = Vec::new();
                let mut context_roots = Vec::new();
                if !roots.is_empty() {
                    context_roots.push(grove_core::session_meta::ContextRoot {
                        project: project.name.clone(),
                        wt_path: cwd.clone(),
                    });
                    for root in roots {
                        let store = &cx.global::<SettingsState>().store;
                        let owner_project =
                            grove_core::session_meta::project_owner(&store.projects, &root)?;
                        let root = selected_worktree(owner_project, &root)?;
                        if owner_project.archived
                            || self.projects.read(cx).is_removing(&owner_project.path)
                            || self.projects.read(cx).is_worktree_removing(&root)
                        {
                            return Err("Context root is archived or being removed".into());
                        }
                        if root == cwd || extra_roots.contains(&root) {
                            return Err("Duplicate context root".into());
                        }
                        context_roots.push(grove_core::session_meta::ContextRoot {
                            project: owner_project.name.clone(),
                            wt_path: root.clone(),
                        });
                        extra_roots.push(root);
                    }
                }
                let temp_bundle_path = if !extra_roots.is_empty() && matches!(agent,Agent::Terminal | Agent::OpenCode) {
                    Some(grove_core::multi_root::SymlinkBundle::create(&extra_roots).map_err(|error|error.to_string())?.into_path().to_string_lossy().into_owned())
                } else { None };
                let task_id = (|| -> Result<Option<String>,String> {
                    if let Some(spec) = task {
                        let id = control::random_id()?;
                        owner.state.create_task(id.clone(),spec)?;
                        if let Err(error) = owner.state.set_launch_context(&id,project.path.clone(),cwd.clone(),if backend { "tmux" } else { "native" }.into()) {
                            let _ = owner.state.set_status(&id,"failed");
                            return Err(error);
                        }
                        Ok(Some(id))
                    } else { Ok(None) }
                })().inspect_err(|_| {
                    if let Some(path) = &temp_bundle_path { grove_core::multi_root::cleanup_path(std::path::Path::new(path)); }
                })?;
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
                if !extra_roots.is_empty() {
                    args = agent
                        .multi_root_launch_args(
                            cx.global::<SettingsState>()
                                .store
                                .dangerously_skip_permissions_enabled
                                .unwrap_or(false),
                            cx.global::<SettingsState>()
                                .store
                                .chrome_enabled
                                .unwrap_or(false),
                            &extra_roots,
                        )
                        .ok_or("Agent does not support context roots")?;
                }
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
                    context_roots,
                    temp_bundle_path.clone(),
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
                    if let Some(path) = &temp_bundle_path { grove_core::multi_root::cleanup_path(std::path::Path::new(path)); }
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
                    .control_terminal(local)
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
                if let Some(index) = self.registry.read(cx).home_terminals().iter().position(|meta|meta.id == local) {
                    let count = self.registry.read(cx).home_terminal_count();
                    self.state.update(cx,|state,cx| { state.select_home_terminal(index,count); cx.notify(); });
                    return Ok(json!({"id":id,"focused":true}));
                }
                if let Some((path,index)) = self.registry.read(cx).panel_shell_location(local) {
                    let store = &cx.global::<SettingsState>().store;
                    let p = grove_core::session_meta::project_owner(&store.projects,&path)?;
                    let proj = store.projects.iter().position(|row|row.path == p.path).ok_or("Shell project is unavailable")?;
                    let worktrees = if grove_core::git::is_repo(&p.path) { grove_core::git::list_worktrees(&p.path) } else { vec![grove_core::git::Worktree { path:p.path.clone(),branch:String::new(),mtime:None,is_main:true }] };
                    self.tree.update(cx,|tree,_|tree.set_active_worktrees(proj,worktrees));
                    let snap = self.snapshot(cx);
                    let wt = snap.projects.iter().find(|p|p.idx == proj).and_then(|p|p.worktrees.iter().position(|wt|wt.path == path)).ok_or("Shell worktree is unavailable")?;
                    let anchor = snap.projects.iter().find(|p|p.idx == proj).and_then(|p|p.worktrees.get(wt)).and_then(|wt|wt.sessions.first().copied()).ok_or("Panel shell needs an open session in its worktree before it can be focused")?;
                    self.registry.update(cx, |registry, cx| { registry.select_wt_shell(&path,index); cx.notify(); });
                    crate::entities::project_tree::ProjectTree::adopt_session_project(&self.tree.clone(),&snap,anchor,self.state.read(cx).proj_idx(),cx);
                    self.state.update(cx,|state,cx| {
                        state.select_session(anchor,&snap);
                        if !state.term_panel_open() { state.toggle_term_panel(true); }
                        state.focus_pane(crate::entities::workspace_state::PtyPane::Panel); cx.notify();
                    });
                    return Ok(json!({"id":id,"focused":true}));
                }
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
                let home = self.registry.read(cx).home_terminals().iter().position(|meta|meta.id == local);
                let panel = self.registry.read(cx).panel_shell_location(local);
                if let Some(index) = home { self.close_home_terminal(index,cx); }
                else if let Some((path,index)) = panel { self.registry.update(cx,|registry,cx| { registry.close_wt_shell(&path,index); cx.notify(); }); }
                else { self.kill_session(local,cx); }
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
        Fixture(path.canonicalize().unwrap())
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
            roots: Vec::new(),
        }
    }
    #[gpui::test]
    fn project_workspace_lifecycle_retries_and_confirmation_guards(cx: &mut gpui::TestAppContext) {
        let f = fixture();
        let runtime = cx.update(|cx| setup(cx, &f.0));
        let call = |cx: &mut gpui::TestAppContext, command, id: &str| {
            cx.update(|cx| {
                runtime.update(cx, |runtime, cx| {
                    runtime.handle_control(request(command, id), cx)
                })
            })
        };
        let created = call(
            cx,
            ControlCommand::CreateWorkspace {
                name: "Tools".into(),
            },
            "create-space",
        );
        assert!(created.ok, "{created:?}");
        let workspace = created.data["id"].as_u64().unwrap();
        let retry = call(
            cx,
            ControlCommand::CreateWorkspace {
                name: "Tools".into(),
            },
            "create-space",
        );
        assert_eq!(created.data, retry.data);
        let moved = call(
            cx,
            ControlCommand::MoveProject {
                project: "control fixture".into(),
                workspace,
            },
            "move",
        );
        assert!(moved.ok, "{moved:?}");
        assert!(
            !call(
                cx,
                ControlCommand::DeleteWorkspace {
                    id: workspace,
                    confirm: "Tools".into()
                },
                "delete-nonempty"
            )
            .ok
        );
        assert!(
            call(
                cx,
                ControlCommand::EditProject {
                    project: "control fixture".into(),
                    name: Some("renamed".into()),
                    setup: None,
                    run: Some("echo hello".into()),
                    teardown: None
                },
                "edit"
            )
            .ok
        );
        assert!(
            call(
                cx,
                ControlCommand::ArchiveProject {
                    project: "renamed".into()
                },
                "archive"
            )
            .ok
        );
        assert!(
            !call(
                cx,
                ControlCommand::DeleteProject {
                    project: "renamed".into(),
                    confirm: "wrong".into()
                },
                "wrong-confirm"
            )
            .ok
        );
        assert!(
            call(
                cx,
                ControlCommand::RestoreProject {
                    project: "renamed".into()
                },
                "restore"
            )
            .ok
        );
        assert!(
            !call(
                cx,
                ControlCommand::DeleteProject {
                    project: "renamed".into(),
                    confirm: "renamed".into()
                },
                "active-delete"
            )
            .ok
        );
        assert!(
            call(
                cx,
                ControlCommand::ArchiveProject {
                    project: "renamed".into()
                },
                "archive-again"
            )
            .ok
        );
        assert!(
            call(
                cx,
                ControlCommand::DeleteProject {
                    project: "renamed".into(),
                    confirm: "renamed".into()
                },
                "delete"
            )
            .ok
        );
        assert!(f.0.is_dir(), "registration deletion must preserve folder");
        assert!(
            call(
                cx,
                ControlCommand::DeleteWorkspace {
                    id: workspace,
                    confirm: "Tools".into()
                },
                "delete-empty"
            )
            .ok
        );
    }

    #[gpui::test]
    fn home_and_panel_shells_are_discoverable_and_can_be_stopped(cx: &mut gpui::TestAppContext) {
        let f = fixture();
        let runtime = cx.update(|cx| setup(cx, &f.0));
        let call = |cx: &mut gpui::TestAppContext, command, id: &str| {
            cx.update(|cx| {
                runtime.update(cx, |runtime, cx| {
                    runtime.handle_control(request(command, id), cx)
                })
            })
        };
        let anchor = call(cx, start(&f.0), "anchor");
        assert!(anchor.ok, "{anchor:?}");
        let home = call(
            cx,
            ControlCommand::StartShell {
                project: None,
                worktree: None,
            },
            "home",
        );
        assert!(home.ok, "{home:?}");
        let panel = call(
            cx,
            ControlCommand::StartShell {
                project: Some("control fixture".into()),
                worktree: Some(f.0.to_string_lossy().into_owned()),
            },
            "panel",
        );
        assert!(panel.ok, "{panel:?}");
        let rows = call(cx, ControlCommand::ListSessions, "list");
        assert_eq!(rows.data.as_array().unwrap().len(), 3);
        for row in [home.data, panel.data] {
            let id = row["id"].as_str().unwrap().to_owned();
            let focused = call(
                cx,
                ControlCommand::Focus { id: id.clone() },
                &format!("focus-{id}"),
            );
            assert!(focused.ok, "{focused:?}");
            assert!(
                call(
                    cx,
                    ControlCommand::Logs {
                        id: id.clone(),
                        lines: 10
                    },
                    &format!("logs-{id}")
                )
                .ok
            );
            assert!(
                call(
                    cx,
                    ControlCommand::Stop { id: id.clone() },
                    &format!("stop-{id}")
                )
                .ok
            );
        }
        let anchor_id = anchor.data["id"].as_str().unwrap().to_owned();
        assert!(call(cx, ControlCommand::Stop { id: anchor_id }, "stop-anchor").ok);
        assert!(call(cx, ControlCommand::ListSessions, "empty")
            .data
            .as_array()
            .unwrap()
            .is_empty());
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
                        roots: Vec::new(),
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
