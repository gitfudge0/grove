use crate::agent::Agent;
use crate::git::{self, Worktree};
use crate::launch;
use crate::storage::{self, Project, Store};
use anyhow::Result;

#[derive(Copy, Clone, PartialEq)]
pub enum Pane {
    Projects,
    Worktrees,
}

#[derive(Clone)]
pub enum Modal {
    None,
    Input {
        title: String,
        buffer: String,
        kind: InputKind,
        dir_sel: usize,
    },
    Confirm {
        prompt: String,
        kind: ConfirmKind,
    },
    Message(String),
    Help,
    AgentPicker {
        wt_path: String,
        sel: usize,
    },
}

#[derive(Clone)]
pub enum InputKind {
    AddProjectPath,
    AddProjectName { path: String },
    AddWorktreeName,
}

#[derive(Clone)]
pub enum ConfirmKind {
    RemoveProject(usize),
    RemoveWorktree(String), // wt path
    InitRepo { path: String, name: String },
    GenerateInclude { path: String },
}

pub struct App {
    pub store: Store,
    pub worktrees: Vec<Worktree>,
    pub focus: Pane,
    pub proj_idx: usize,
    pub wt_idx: usize,
    pub modal: Modal,
    pub status: String,
    pub should_quit: bool,
    pub pending_exec: Option<PendingExec>,
}

#[derive(Clone)]
pub struct PendingExec {
    pub program: String,
    pub cwd: String,
    pub args: Vec<String>,
}

impl App {
    pub fn new() -> Result<Self> {
        let store = storage::load()?;
        let mut app = App {
            store,
            worktrees: vec![],
            focus: Pane::Projects,
            proj_idx: 0,
            wt_idx: 0,
            modal: Modal::None,
            status: String::new(),
            should_quit: false,
            pending_exec: None,
        };
        app.refresh_worktrees();
        Ok(app)
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.store.projects.get(self.proj_idx)
    }

    pub fn refresh_worktrees(&mut self) {
        self.worktrees = match self.selected_project() {
            Some(p) => git::list_worktrees(&p.path),
            None => vec![],
        };
        if self.wt_idx >= self.worktrees.len() {
            self.wt_idx = self.worktrees.len().saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Pane::Projects => {
                if self.proj_idx > 0 {
                    self.proj_idx -= 1;
                    self.wt_idx = 0;
                    self.refresh_worktrees();
                }
            }
            Pane::Worktrees => {
                if self.wt_idx > 0 {
                    self.wt_idx -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            Pane::Projects => {
                if self.proj_idx + 1 < self.store.projects.len() {
                    self.proj_idx += 1;
                    self.wt_idx = 0;
                    self.refresh_worktrees();
                }
            }
            Pane::Worktrees => {
                if self.wt_idx + 1 < self.worktrees.len() {
                    self.wt_idx += 1;
                }
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Pane::Projects => Pane::Worktrees,
            Pane::Worktrees => Pane::Projects,
        };
    }

    pub fn start_add(&mut self) {
        match self.focus {
            Pane::Projects => {
                self.modal = Modal::Input {
                    title: "Project directory path".into(),
                    buffer: "~/".into(),
                    kind: InputKind::AddProjectPath,
                    dir_sel: 0,
                };
            }
            Pane::Worktrees => {
                if self.selected_project().is_some() {
                    self.modal = Modal::Input {
                        title: "Worktree name".into(),
                        buffer: String::new(),
                        kind: InputKind::AddWorktreeName,
                        dir_sel: 0,
                    };
                }
            }
        }
    }

    pub fn start_delete(&mut self) {
        match self.focus {
            Pane::Projects => {
                if let Some(p) = self.selected_project() {
                    self.modal = Modal::Confirm {
                        prompt: format!("Remove project '{}' from manager? (files untouched)", p.name),
                        kind: ConfirmKind::RemoveProject(self.proj_idx),
                    };
                }
            }
            Pane::Worktrees => {
                if let Some(wt) = self.worktrees.get(self.wt_idx) {
                    let path = wt.path.clone();
                    self.modal = Modal::Confirm {
                        prompt: format!("git worktree remove --force {}?", path),
                        kind: ConfirmKind::RemoveWorktree(path),
                    };
                }
            }
        }
    }

    pub fn on_enter(&mut self) -> Result<()> {
        match self.focus {
            Pane::Projects => {
                if !self.worktrees.is_empty() || self.selected_project().is_some() {
                    self.focus = Pane::Worktrees;
                }
            }
            Pane::Worktrees => self.open_worktree(false),
        }
        Ok(())
    }

    pub fn open_worktree(&mut self, force_pick: bool) {
        let Some(wt) = self.worktrees.get(self.wt_idx).cloned() else { return };
        if !force_pick {
            if let Some(agent) = self.store.default_agent {
                self.launch_agent(agent, wt.path);
                return;
            }
        }
        let sel = self
            .store
            .default_agent
            .and_then(|a| Agent::ALL.iter().position(|x| *x == a))
            .unwrap_or(0);
        self.modal = Modal::AgentPicker { wt_path: wt.path, sel };
    }

    pub fn launch_agent(&mut self, agent: Agent, cwd: String) {
        self.pending_exec = Some(PendingExec {
            program: agent.program().to_string(),
            cwd,
            args: agent.launch_args(),
        });
        self.should_quit = true;
    }

    pub fn picker_move(&mut self, delta: i32) {
        if let Modal::AgentPicker { sel, .. } = &mut self.modal {
            let len = Agent::ALL.len() as i32;
            let next = (*sel as i32 + delta).rem_euclid(len);
            *sel = next as usize;
        }
    }

    pub fn picker_toggle_default(&mut self) -> Result<()> {
        let Modal::AgentPicker { sel, .. } = &self.modal else { return Ok(()) };
        let agent = Agent::ALL[*sel];
        if self.store.default_agent == Some(agent) {
            self.store.default_agent = None;
            self.status = format!("cleared default agent ({})", agent.label());
        } else {
            self.store.default_agent = Some(agent);
            self.status = format!("default agent: {}", agent.label());
        }
        storage::save(&self.store)?;
        Ok(())
    }

    pub fn picker_submit(&mut self) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::AgentPicker { wt_path, sel } = modal else { return };
        self.launch_agent(Agent::ALL[sel], wt_path);
    }

    pub fn input_dir_move(&mut self, delta: i32) {
        let Modal::Input { buffer, kind, dir_sel, .. } = &mut self.modal else { return };
        if !matches!(kind, InputKind::AddProjectPath) { return }
        let entries = list_dirs(buffer);
        if entries.is_empty() { *dir_sel = 0; return }
        let len = entries.len() as i32;
        *dir_sel = (*dir_sel as i32 + delta).rem_euclid(len) as usize;
    }

    pub fn input_dir_pick(&mut self) {
        let Modal::Input { buffer, kind, dir_sel, .. } = &mut self.modal else { return };
        if !matches!(kind, InputKind::AddProjectPath) { return }
        let entries = list_dirs(buffer);
        if let Some(pick) = entries.get(*dir_sel) {
            *buffer = format!("{}/", pick);
            *dir_sel = 0;
        }
    }

    pub fn input_buffer_edit<F: FnOnce(&mut String)>(&mut self, f: F) {
        if let Modal::Input { buffer, dir_sel, .. } = &mut self.modal {
            f(buffer);
            *dir_sel = 0;
        }
    }

    pub fn submit_input(&mut self) -> Result<()> {
        if let Modal::Input { buffer, kind: InputKind::AddProjectPath, .. } = &self.modal {
            let expanded = shellexpand_tilde(buffer.trim());
            let is_dir = std::path::PathBuf::from(&expanded).is_dir();
            if !is_dir {
                if !list_dirs(buffer).is_empty() {
                    self.input_dir_pick();
                }
                return Ok(());
            }
        }
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::Input { buffer, kind, .. } = modal else { return Ok(()); };
        let value = buffer.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        match kind {
            InputKind::AddProjectPath => {
                let path = shellexpand_tilde(&value);
                let pb = std::path::PathBuf::from(&path);
                let abs = std::fs::canonicalize(&pb)?
                    .to_string_lossy()
                    .to_string();
                let default_name = pb
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                self.modal = Modal::Input {
                    title: "Project name".into(),
                    buffer: default_name,
                    kind: InputKind::AddProjectName { path: abs },
                    dir_sel: 0,
                };
            }
            InputKind::AddProjectName { path } => {
                if self.store.projects.iter().any(|p| p.name == value) {
                    self.modal = Modal::Message(format!("Project '{value}' already exists"));
                    return Ok(());
                }
                self.store.projects.push(Project {
                    name: value.clone(),
                    path: path.clone(),
                });
                storage::save(&self.store)?;
                self.proj_idx = self.store.projects.len() - 1;
                self.status = format!("added {value}");

                let needs_init = !std::path::Path::new(&path).join(".git").exists();
                let needs_include =
                    !std::path::Path::new(&path).join(".worktreeinclude").exists();

                if needs_init {
                    self.modal = Modal::Confirm {
                        prompt: format!("'{path}' is not a git repo. Run `git init`?"),
                        kind: ConfirmKind::InitRepo { path, name: value },
                    };
                } else if needs_include {
                    self.modal = Modal::Confirm {
                        prompt: "Generate .worktreeinclude with Claude (haiku)?".into(),
                        kind: ConfirmKind::GenerateInclude { path },
                    };
                }
                self.refresh_worktrees();
            }
            InputKind::AddWorktreeName => {
                if value.contains('/') {
                    self.modal = Modal::Message("name cannot contain '/'".into());
                    return Ok(());
                }
                let Some(p) = self.selected_project().cloned() else { return Ok(()); };
                self.pending_exec = Some(PendingExec {
                    program: "claude".into(),
                    cwd: p.path,
                    args: vec!["--worktree".into(), value],
                });
                self.should_quit = true;
            }
        }
        Ok(())
    }

    pub fn submit_confirm(&mut self, yes: bool) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::Confirm { kind, .. } = modal else { return Ok(()); };
        if !yes {
            return Ok(());
        }
        match kind {
            ConfirmKind::RemoveProject(idx) => {
                if idx < self.store.projects.len() {
                    let removed = self.store.projects.remove(idx);
                    storage::save(&self.store)?;
                    if self.proj_idx >= self.store.projects.len() {
                        self.proj_idx = self.store.projects.len().saturating_sub(1);
                    }
                    self.status = format!("removed project {}", removed.name);
                    self.refresh_worktrees();
                }
            }
            ConfirmKind::RemoveWorktree(path) => {
                if let Some(p) = self.selected_project().cloned() {
                    if let Err(e) = git::remove_worktree(&p.path, &path) {
                        self.status = format!("err: {e}");
                    } else {
                        self.status = format!("removed worktree {path}");
                    }
                    self.refresh_worktrees();
                }
            }
            ConfirmKind::InitRepo { path, name } => {
                git::init_if_needed(&path)?;
                let needs_include =
                    !std::path::Path::new(&path).join(".worktreeinclude").exists();
                if needs_include {
                    self.modal = Modal::Confirm {
                        prompt: "Generate .worktreeinclude with Claude (haiku)?".into(),
                        kind: ConfirmKind::GenerateInclude { path },
                    };
                }
                let _ = name;
            }
            ConfirmKind::GenerateInclude { path } => {
                let prompt = "Inspect this project directory and write a .worktreeinclude file at its root. \
                    It uses .gitignore syntax — list patterns matching gitignored files that should be copied \
                    into a fresh git worktree so the worktree can run immediately (.env, .env.local, local config, \
                    secrets, build/IDE state that's gitignored but needed). Look at .gitignore, package.json, \
                    pyproject.toml, Gemfile, go.mod, etc. Only write the file. No commentary.";
                let res = launch::run_inline(|| {
                    launch::run_claude_inline(
                        &path,
                        &[
                            "--model",
                            "haiku",
                            "--dangerously-skip-permissions",
                            "-p",
                            prompt,
                        ],
                    )
                });
                self.status = match res {
                    Ok(()) => ".worktreeinclude generated".into(),
                    Err(e) => format!("generate failed: {e}"),
                };
                self.refresh_worktrees();
            }
        }
        Ok(())
    }
}

pub fn list_dirs(buffer: &str) -> Vec<String> {
    let expanded = shellexpand_tilde(buffer);
    let (dir, prefix) = if expanded.is_empty() {
        (std::path::PathBuf::from("."), String::new())
    } else if expanded.ends_with('/') {
        (std::path::PathBuf::from(expanded.trim_end_matches('/')), String::new())
    } else {
        let pb = std::path::PathBuf::from(&expanded);
        let parent = pb.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = pb.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let parent = if parent.as_os_str().is_empty() { std::path::PathBuf::from(".") } else { parent };
        (parent, name)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![]; };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') || prefix.starts_with('.'))
        .filter(|n| n.starts_with(&prefix))
        .map(|n| format!("{}/{}", dir.display(), n))
        .collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out
}

fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    s.to_string()
}
