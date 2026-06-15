use crate::agent::Agent;
use crate::git::{self, Worktree};
use crate::session::Session;
use crate::storage::{self, Project, Store};
use crate::theme;
use crate::tmux;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::process::Command;

pub fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i32 + delta).rem_euclid(len as i32) as usize
}

pub struct Toast {
    pub message: String,
}

#[derive(Copy, Clone, PartialEq)]
pub enum Pane {
    Projects,
    Worktrees,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveBackend {
    Native,
    Tmux,
}

pub fn effective_backend_for(tmux_available: bool, tmux_enabled: Option<bool>) -> EffectiveBackend {
    if tmux_available && tmux_enabled == Some(true) {
        EffectiveBackend::Tmux
    } else {
        EffectiveBackend::Native
    }
}

pub fn needs_tmux_choice(tmux_available: bool, tmux_enabled: Option<bool>) -> bool {
    tmux_available && tmux_enabled.is_none()
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
        title: String,
        prompt: String,
        destructive: bool,
        kind: ConfirmKind,
    },
    /// Two-stage project removal: confirmation (with an optional checkbox to
    /// also delete worktrees on disk) followed by a progress view while the
    /// worktrees are torn down.
    RemoveProject {
        idx: usize,
        name: String,
        project_path: String,
        /// Non-main worktree paths discovered when the modal opened.
        worktrees: Vec<String>,
        also_remove_worktrees: bool,
        in_progress: bool,
        done: usize,
        current: String,
        errors: Vec<String>,
    },
    Message(String),
    TmuxChoice,
    AgentPicker {
        project: String,
        wt_path: String,
        sel: usize,
    },
    ThemePicker {
        sel_dark: usize,
        sel_light: usize,
        tab: crate::theme::ThemeKind,
        original: crate::theme::Theme,
    },
    /// Worktree teardown: runs the project's teardown script (if any) in a
    /// modal-embedded PTY, then performs `git worktree remove`. The live PTY
    /// session and stage live in `App::teardown`.
    Teardown,
    /// Per-project lifecycle-scripts editor. The editable buffers and target
    /// project live in the GUI model (`Grove::scripts_editor`); this just marks
    /// the modal open.
    ScriptsEditor,
}

/// Stage of an in-progress worktree teardown.
#[derive(Clone, Copy, PartialEq)]
pub enum TeardownStage {
    /// The teardown script is running in `session`.
    RunningScript,
    /// Script finished; `git worktree remove` is executing.
    Removing,
    /// Done — `error` is `Some` if removal failed.
    Done { failed: bool },
}

/// State for a worktree deletion in progress. Holds the live teardown PTY (if a
/// teardown script is configured) so the modal can render it; kept out of the
/// cloneable `Modal` because `Session` isn't `Clone`.
pub struct Teardown {
    pub wt_path: String,
    pub project_path: String,
    pub session: Option<Session>,
    pub stage: TeardownStage,
    pub message: String,
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
    InitAndAddWorktree { name: String },
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
    pub sessions: Vec<Session>,
    pub active_session: Option<usize>,
    /// The shells behind the `terminal` tab, each rooted at `~`. The first is
    /// spawned lazily when the tab is first opened; the user can add more.
    /// These live outside `sessions` so they never show up in the tree /
    /// activity lists and aren't reachable by the session-cycling or kill
    /// machinery. Always non-empty while the terminal tab is in use (closing
    /// the last one immediately respawns a fresh shell).
    pub home_terminals: Vec<Session>,
    /// Index into `home_terminals` of the terminal the tab is showing.
    pub active_terminal: Option<usize>,
    /// Monotonic counter behind each terminal's internal label (`terminal 1`,
    /// `terminal 2`, …). The label isn't shown in the UI — rows display only
    /// the icon and the shell's contextual title — but it stays stable and
    /// unique per terminal so it can be stripped from that title and preserved
    /// across a restart.
    pub home_terminal_seq: usize,
    /// Worktree-scoped terminals for the right-docked slide-over panel, keyed by
    /// absolute worktree path. Each panel can hold several shells (the panel's
    /// tab strip). These live outside `sessions` so they never appear as
    /// sidebar/tree/activity rows — they belong *inside* a session's worktree,
    /// not beside it. Entries are dropped (shells killed) when the worktree is
    /// removed via [`kill_wt_terminals`].
    pub wt_terminals: HashMap<String, Vec<Session>>,
    /// Active shell index within each worktree's panel, keyed by worktree path.
    pub wt_active_terminal: HashMap<String, usize>,
    /// Monotonic counter behind each panel terminal's internal label.
    pub wt_terminal_seq: usize,
    /// Transient top-right notification (e.g. copy confirmation).
    pub toast: Option<Toast>,
    /// Total worktrees across all projects, cached so the renderer doesn't
    /// shell out to `git` for every project on every frame.
    pub worktree_count: usize,
    /// Zen mode: when false, the top banner and the sessions sidebar are
    /// hidden on the session page so the PTY can use the full frame.
    pub chrome_visible: bool,
    /// Whether tmux was available on PATH when Grove started.
    pub tmux_available: bool,
    /// Agents whose binaries were found on PATH and are executable.
    /// Ordered to match `Agent::ALL`; `sel` in `Modal::AgentPicker` indexes
    /// into this slice. Always contains at least `Terminal`.
    /// Re-scanned each time the picker is opened so newly-installed tools
    /// appear without restarting Grove.
    pub(crate) available_agents: Vec<Agent>,
    /// Completion message from a background job (e.g. `.worktreeinclude`
    /// generation). Set by the worker thread, drained on the GUI tick.
    pub bg_status: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// In-progress worktree teardown, when `modal` is `Modal::Teardown`.
    pub teardown: Option<Teardown>,
}

impl App {
    pub fn set_toast(&mut self, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
        });
    }

    /// Re-scan PATH and update `available_agents`. Called before opening the
    /// agent picker so that tools installed while Grove is running are visible.
    pub(crate) fn refresh_available_agents(&mut self) {
        self.available_agents = Agent::ALL
            .iter()
            .copied()
            .filter(|a| a.available())
            .collect();
    }
}

impl App {
    pub fn new() -> Result<Self> {
        let store = storage::load()?;
        let tmux_available = tmux::available();
        let initial_modal = if needs_tmux_choice(tmux_available, store.tmux_enabled) {
            Modal::TmuxChoice
        } else {
            Modal::None
        };
        // Existing tmux sessions keep their backend even when the saved
        // preference now chooses native sessions for new launches.
        let sessions = if tmux_available {
            discover_sessions()
        } else {
            Vec::new()
        };
        if let Some(name) = store.theme.as_deref() {
            theme::set_by_name(name);
        }
        let mut app = App {
            store,
            worktrees: vec![],
            focus: Pane::Projects,
            proj_idx: 0,
            wt_idx: 0,
            modal: initial_modal,
            status: String::new(),
            sessions,
            active_session: None,
            home_terminals: Vec::new(),
            active_terminal: None,
            home_terminal_seq: 0,
            wt_terminals: HashMap::new(),
            wt_active_terminal: HashMap::new(),
            wt_terminal_seq: 0,
            toast: None,
            worktree_count: 0,
            chrome_visible: true,
            tmux_available,
            available_agents: Agent::ALL
                .iter()
                .copied()
                .filter(|a| a.available())
                .collect(),
            bg_status: std::sync::Arc::new(std::sync::Mutex::new(None)),
            teardown: None,
        };
        app.refresh_worktrees();
        Ok(app)
    }

    pub fn effective_backend(&self) -> EffectiveBackend {
        effective_backend_for(self.tmux_available, self.store.tmux_enabled)
    }

    pub fn use_tmux(&self) -> bool {
        self.effective_backend() == EffectiveBackend::Tmux
    }

    pub fn set_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled && !self.tmux_available {
            self.status = "tmux not found; using native sessions".into();
            self.set_toast("tmux not found");
            return Ok(());
        }
        self.store.tmux_enabled = Some(enabled);
        storage::save(&self.store)?;
        if enabled {
            self.discover_tmux_sessions();
            self.status = "tmux enabled for new sessions".into();
        } else {
            self.status = "tmux disabled for new sessions".into();
        }
        Ok(())
    }

    pub fn choose_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_tmux_enabled(enabled)?;
        self.modal = Modal::None;
        Ok(())
    }

    fn discover_tmux_sessions(&mut self) {
        if !self.tmux_available {
            return;
        }
        let known: HashSet<String> = self
            .sessions
            .iter()
            .filter_map(|s| s.tmux_name().map(str::to_string))
            .collect();
        let discovered: Vec<Session> = discover_sessions()
            .into_iter()
            .filter(|s| s.tmux_name().map_or(true, |name| !known.contains(name)))
            .collect();
        for s in discovered {
            let at = self.session_insert_index(&s);
            if self.active_session.map_or(false, |i| at <= i) {
                self.active_session = self.active_session.map(|i| i + 1);
            }
            self.sessions.insert(at, s);
        }
    }

    /// Recompute the cross-project worktree total. Call after any change that
    /// adds or removes projects or worktrees.
    pub fn recount_worktrees(&mut self) {
        // `self.worktrees` already holds the selected project's worktrees;
        // reuse it instead of shelling out to `git` for that project again.
        let proj_idx = self.proj_idx;
        let selected = self.worktrees.iter().filter(|w| !w.is_main).count();
        self.worktree_count = self
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == proj_idx {
                    selected
                } else {
                    git::list_worktrees(&p.path)
                        .iter()
                        .filter(|w| !w.is_main)
                        .count()
                }
            })
            .sum();
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
        self.recount_worktrees();
    }

    pub fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane;
    }

    pub fn start_add(&mut self) {
        match self.focus {
            Pane::Projects => {
                self.modal = Modal::Input {
                    title: "project directory path".into(),
                    buffer: "~/".into(),
                    kind: InputKind::AddProjectPath,
                    dir_sel: 0,
                };
            }
            Pane::Worktrees => {
                if self.selected_project().is_some() {
                    self.modal = Modal::Input {
                        title: "worktree name".into(),
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
                        title: "remove project?".into(),
                        prompt: format!(
                            "'{}' will be unregistered. Files on disk stay put.",
                            p.name
                        ),
                        destructive: true,
                        kind: ConfirmKind::RemoveProject(self.proj_idx),
                    };
                }
            }
            Pane::Worktrees => {
                if let Some(wt) = self.worktrees.get(self.wt_idx) {
                    if wt.is_main {
                        self.modal =
                            Modal::Message("can't remove the project's main checkout".into());
                        return;
                    }
                    let path = wt.path.clone();
                    self.modal = Modal::Confirm {
                        title: "remove worktree?".into(),
                        prompt: format!("git worktree remove --force {}", path),
                        destructive: true,
                        kind: ConfirmKind::RemoveWorktree(path),
                    };
                }
            }
        }
    }

    /// Index of the default agent in the picker, or 0 if none is set.
    fn picker_sel(&self) -> usize {
        self.store
            .default_agent
            .and_then(|a| self.available_agents.iter().position(|&x| x == a))
            .unwrap_or(0)
    }

    /// Launch the default agent for `wt_path`, or open the picker if no
    /// default is configured or the saved default is no longer available.
    fn launch_or_pick(&mut self, project: String, wt_path: String) {
        self.refresh_available_agents();
        let default = self
            .store
            .default_agent
            .filter(|a| self.available_agents.contains(a));
        if let Some(agent) = default {
            let label = path_basename(&wt_path);
            let args = agent.launch_args();
            self.spawn_session(label, project, wt_path.clone(), agent, args, &wt_path);
        } else {
            if let Some(saved) = self.store.default_agent {
                if !self.available_agents.contains(&saved) {
                    self.set_toast(format!("{} not found; pick an agent", saved.label()));
                }
            }
            self.modal = Modal::AgentPicker {
                project,
                wt_path,
                sel: self.picker_sel(),
            };
        }
    }

    /// Compute the index at which a newly spawned session should be inserted
    /// so the sessions list stays grouped by project, and worktrees within a
    /// project follow the project's actual worktree order (rather than the
    /// order sessions happened to be created in).
    fn session_insert_index(&self, s: &Session) -> usize {
        let proj_path = self
            .store
            .projects
            .iter()
            .find(|p| p.name == s.project)
            .map(|p| p.path.clone());
        let wt_order: Vec<String> = match proj_path {
            Some(p) => git::list_worktrees(&p)
                .into_iter()
                .map(|w| w.path)
                .collect(),
            None => Vec::new(),
        };
        let new_pos = wt_order.iter().position(|p| p == &s.wt_path);

        let proj_block: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, x)| x.project == s.project)
            .map(|(i, _)| i)
            .collect();
        if proj_block.is_empty() {
            return self.sessions.len();
        }
        if let Some(new_pos) = new_pos {
            for &i in &proj_block {
                let other_pos = wt_order.iter().position(|p| p == &self.sessions[i].wt_path);
                if other_pos.map_or(false, |o| o > new_pos) {
                    return i;
                }
            }
        }
        proj_block.last().unwrap() + 1
    }

    /// Spawn an agent in a new embedded PTY session and focus it.
    pub fn spawn_session(
        &mut self,
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: Vec<String>,
        cwd: &str,
    ) {
        match Session::spawn(
            label.clone(),
            project,
            wt_path,
            agent,
            &args,
            cwd,
            self.use_tmux(),
        ) {
            Ok(s) => {
                let at = self.session_insert_index(&s);
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.status = format!("started {label}");
            }
            Err(e) => {
                self.modal = Modal::Message(format!("failed to start agent: {e}"));
            }
        }
    }

    /// Spawn a lifecycle script (`setup`/`run`) as a focused session tab under
    /// `wt_path`. No-op when the snippet is empty/whitespace.
    pub fn spawn_script_session(
        &mut self,
        stage: &str,
        project: String,
        wt_path: String,
        script: &str,
    ) {
        let script = script.trim();
        if script.is_empty() {
            return;
        }
        match Session::spawn_script(stage.to_string(), project, wt_path.clone(), script, &wt_path) {
            Ok(s) => {
                let at = self.session_insert_index(&s);
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.status = format!("running {stage} script");
            }
            Err(e) => {
                self.set_toast(format!("{stage} script failed: {e}"));
            }
        }
    }

    /// Run the project's `run` script in the given worktree, if configured.
    pub fn run_worktree_script(&mut self, wt_path: &str) {
        let Some(p) = self.selected_project().cloned() else {
            return;
        };
        match p.scripts.run.as_deref() {
            Some(script) if !script.trim().is_empty() => {
                self.spawn_script_session("run", p.name.clone(), wt_path.to_string(), script);
            }
            _ => self.set_toast("no run script configured for this project"),
        }
    }

    fn create_worktree(&mut self, p: &Project, name: &str) {
        let wt_path = match git::add_worktree(&p.path, &p.name, name) {
            Ok(path) => path,
            Err(e) => {
                self.modal = Modal::Message(format!("add worktree failed: {e}"));
                return;
            }
        };
        if let Err(e) = git::copy_worktree_includes(&p.path, &wt_path) {
            self.status = format!("worktreeinclude: {e}");
        }
        self.refresh_worktrees();
        // Launch the agent first, then the setup script (if any) so the setup
        // tab is spawned last and is the one focused by default — when a setup
        // script exists, the user wants to watch it run before touching the
        // agent. Both tabs coexist.
        self.launch_or_pick(p.name.clone(), wt_path.clone());
        if let Some(setup) = p.scripts.setup.clone() {
            self.spawn_script_session("setup", p.name.clone(), wt_path, &setup);
        }
    }

    /// Absolute path of the home directory, falling back to `/` if it can't be
    /// resolved. Used as the home terminal's working directory.
    fn home_dir() -> String {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into())
    }

    /// The home terminal the tab is currently showing, if any.
    pub fn active_home_terminal(&self) -> Option<&Session> {
        self.active_terminal
            .and_then(|i| self.home_terminals.get(i))
    }

    /// Ensure at least one home terminal exists (spawning the first on demand),
    /// resize them all to the current pane, and make sure something is selected.
    pub fn ensure_home_terminal(&mut self, rows: u16, cols: u16) {
        if self.home_terminals.is_empty() {
            self.spawn_home_terminal(rows, cols);
        } else {
            for s in &mut self.home_terminals {
                s.resize(rows, cols);
            }
        }
        if self.active_terminal.is_none() && !self.home_terminals.is_empty() {
            self.active_terminal = Some(0);
        }
    }

    /// Spawn an additional home terminal and focus it.
    pub fn new_home_terminal(&mut self, rows: u16, cols: u16) {
        self.spawn_home_terminal(rows, cols);
    }

    /// Replace the active terminal's shell in place with a fresh one at `~`,
    /// keeping its slot and label. Used to recover an exited terminal.
    pub fn restart_active_terminal(&mut self, rows: u16, cols: u16) {
        let Some(i) = self.active_terminal else {
            return;
        };
        if i >= self.home_terminals.len() {
            return;
        }
        let label = self.home_terminals[i].label.clone();
        // Only swap once the replacement is live: on spawn failure
        // `build_home_terminal` toasts and we keep the (usually exited)
        // terminal in place rather than leaving an empty slot.
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            let mut old = std::mem::replace(&mut self.home_terminals[i], s);
            old.kill();
        }
    }

    /// Close the terminal at `idx`. The terminal tab always keeps at least one
    /// shell, so closing the last one immediately spawns a replacement.
    pub fn close_home_terminal(&mut self, idx: usize, rows: u16, cols: u16) {
        if idx >= self.home_terminals.len() {
            return;
        }
        let mut s = self.home_terminals.remove(idx);
        s.kill();
        self.active_terminal = match self.active_terminal {
            Some(a) if a == idx => {
                if self.home_terminals.is_empty() {
                    None
                } else {
                    Some(idx.min(self.home_terminals.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        if self.home_terminals.is_empty() {
            self.spawn_home_terminal(rows, cols);
        }
    }

    fn spawn_home_terminal(&mut self, rows: u16, cols: u16) {
        self.home_terminal_seq += 1;
        let label = format!("terminal {}", self.home_terminal_seq);
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            self.home_terminals.push(s);
            self.active_terminal = Some(self.home_terminals.len() - 1);
        }
    }

    /// Build a native home-terminal session at `~`, sized to the pane. Always
    /// native: a local convenience shell, not a worktree-backed agent that
    /// needs to survive grove restarts via tmux. Returns `None` (and toasts) on
    /// spawn failure.
    fn build_home_terminal(&mut self, label: String, rows: u16, cols: u16) -> Option<Session> {
        let home = Self::home_dir();
        let args = Agent::Terminal.launch_args();
        match Session::spawn(
            label,
            String::new(),
            home.clone(),
            Agent::Terminal,
            &args,
            &home,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                Some(s)
            }
            Err(e) => {
                self.set_toast(format!("terminal failed: {e}"));
                None
            }
        }
    }

    // ── per-worktree terminal panel ────────────────────────────────────────

    /// The shells of the panel for `wt_path` (empty if none spawned yet).
    pub fn wt_terminals_for(&self, wt_path: &str) -> &[Session] {
        self.wt_terminals
            .get(wt_path)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The active shell of the panel for `wt_path`, if any.
    pub fn active_wt_terminal(&self, wt_path: &str) -> Option<&Session> {
        let i = *self.wt_active_terminal.get(wt_path)?;
        self.wt_terminals.get(wt_path)?.get(i)
    }

    /// Active shell index within the panel for `wt_path`.
    pub fn active_wt_terminal_idx(&self, wt_path: &str) -> Option<usize> {
        self.wt_active_terminal.get(wt_path).copied()
    }

    /// Ensure the panel for `wt_path` has at least one shell, resize them all,
    /// and select something. Mirrors `ensure_home_terminal` but rooted in the
    /// worktree rather than `~`.
    pub fn ensure_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        match self.wt_terminals.get_mut(wt_path) {
            Some(v) if !v.is_empty() => {
                for s in v {
                    s.resize(rows, cols);
                }
            }
            // Missing or present-but-empty: spawn the first shell (which also
            // sets the active index).
            _ => self.spawn_wt_terminal(wt_path, rows, cols),
        }
        if self.wt_active_terminal.get(wt_path).is_none()
            && !self.wt_terminals_for(wt_path).is_empty()
        {
            self.wt_active_terminal.insert(wt_path.to_string(), 0);
        }
    }

    /// Spawn an additional panel shell for `wt_path` and focus it.
    pub fn new_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.spawn_wt_terminal(wt_path, rows, cols);
    }

    /// Focus the panel shell at `idx` for `wt_path`.
    pub fn select_wt_terminal(&mut self, wt_path: &str, idx: usize, rows: u16, cols: u16) {
        if let Some(v) = self.wt_terminals.get_mut(wt_path) {
            if idx < v.len() {
                v[idx].resize(rows, cols);
                self.wt_active_terminal.insert(wt_path.to_string(), idx);
            }
        }
    }

    /// Close the panel shell at `idx` for `wt_path`. Unlike the home terminal
    /// this does *not* respawn when the last one closes — an empty panel is a
    /// valid state (the panel shows its empty/start affordance).
    pub fn close_wt_terminal(&mut self, wt_path: &str, idx: usize) {
        let Some(v) = self.wt_terminals.get_mut(wt_path) else {
            return;
        };
        if idx >= v.len() {
            return;
        }
        let mut s = v.remove(idx);
        s.kill();
        let new_active = match self.wt_active_terminal.get(wt_path).copied() {
            Some(a) if a == idx => {
                if v.is_empty() {
                    None
                } else {
                    Some(idx.min(v.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        match new_active {
            Some(a) => {
                self.wt_active_terminal.insert(wt_path.to_string(), a);
            }
            None => {
                self.wt_active_terminal.remove(wt_path);
            }
        }
    }

    /// Kill and drop every panel shell for `wt_path`. Called when the owning
    /// worktree/session is removed so no orphaned shells survive.
    pub fn kill_wt_terminals(&mut self, wt_path: &str) {
        if let Some(mut v) = self.wt_terminals.remove(wt_path) {
            for s in &mut v {
                s.kill();
            }
        }
        self.wt_active_terminal.remove(wt_path);
    }

    fn spawn_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.wt_terminal_seq += 1;
        let label = format!("wt-terminal {}", self.wt_terminal_seq);
        let args = Agent::Terminal.launch_args();
        match Session::spawn(
            label,
            String::new(),
            wt_path.to_string(),
            Agent::Terminal,
            &args,
            wt_path,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                let v = self.wt_terminals.entry(wt_path.to_string()).or_default();
                v.push(s);
                self.wt_active_terminal
                    .insert(wt_path.to_string(), v.len() - 1);
            }
            Err(e) => {
                self.set_toast(format!("terminal failed: {e}"));
            }
        }
    }

    /// Open the project-removal modal for the project at `idx`. Discovers
    /// the project's non-main worktrees up front so the modal can show
    /// "Also delete N worktrees on disk" without re-shelling-out per frame.
    pub fn open_remove_project_modal(&mut self, idx: usize) {
        let Some(p) = self.store.projects.get(idx).cloned() else {
            return;
        };
        let worktrees: Vec<String> = git::list_worktrees(&p.path)
            .into_iter()
            .filter(|w| !w.is_main)
            .map(|w| w.path)
            .collect();
        self.modal = Modal::RemoveProject {
            idx,
            name: p.name,
            project_path: p.path,
            worktrees,
            also_remove_worktrees: false,
            in_progress: false,
            done: 0,
            current: String::new(),
            errors: Vec::new(),
        };
    }

    /// Kill any sessions belonging to the named project. Used when removing
    /// a project so its sessions don't linger in the sidebar as orphans.
    pub fn kill_sessions_for_project(&mut self, project: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].project == project {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
    }

    /// Finalize project removal after any worktree teardown has completed.
    /// Kills lingering sessions, drops the project from the store, and
    /// persists. Returns a status string for the caller to display.
    pub fn finalize_remove_project(&mut self, idx: usize) -> Result<String> {
        if idx >= self.store.projects.len() {
            return Ok(String::new());
        }
        let name = self.store.projects[idx].name.clone();
        self.kill_sessions_for_project(&name);
        let removed = self.store.projects.remove(idx);
        storage::save(&self.store)?;
        if self.proj_idx >= self.store.projects.len() {
            self.proj_idx = self.store.projects.len().saturating_sub(1);
        }
        self.refresh_worktrees();
        Ok(format!("removed project {}", removed.name))
    }

    /// Kill any sessions whose worktree path matches `wt_path` and remove them
    /// from the sessions list. Adjusts `active_session` so it still points at a
    /// valid session (or `None` when the list is empty).
    pub fn kill_sessions_for_wt(&mut self, wt_path: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].wt_path == wt_path {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
        // The worktree is going away — drop its panel shells too.
        self.kill_wt_terminals(wt_path);
    }

    /// Begin tearing down `path`: kill its sessions, then either run the
    /// project's teardown script in a modal PTY (advancing to removal when it
    /// exits) or remove the worktree immediately. Opens `Modal::Teardown`.
    fn start_teardown(&mut self, p: &Project, path: String) {
        self.kill_sessions_for_wt(&path);
        let script = p
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let session = script.and_then(|s| {
            Session::spawn_script("teardown".into(), p.name.clone(), path.clone(), s, &path).ok()
        });
        self.modal = Modal::Teardown;
        if session.is_some() {
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session,
                stage: TeardownStage::RunningScript,
                message: "running teardown script…".into(),
            });
        } else {
            // No teardown script (or it failed to spawn): remove right away.
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session: None,
                stage: TeardownStage::Removing,
                message: "removing worktree…".into(),
            });
            self.do_teardown_removal();
        }
    }

    /// Drive an in-progress teardown forward. Called every GUI tick. When the
    /// teardown script's PTY exits, performs the git removal.
    pub fn poll_teardown(&mut self) {
        let advance = matches!(
            self.teardown.as_ref(),
            Some(td) if td.stage == TeardownStage::RunningScript
                && td.session.as_ref().is_none_or(|s| !s.is_running())
        );
        if advance {
            self.do_teardown_removal();
        }
    }

    /// Run `git worktree remove` for the active teardown and transition to
    /// `Done`. Drops the (exited) teardown PTY session.
    fn do_teardown_removal(&mut self) {
        let Some(td) = self.teardown.as_mut() else {
            return;
        };
        td.stage = TeardownStage::Removing;
        td.session = None;
        let wt_path = td.wt_path.clone();
        let project_path = td.project_path.clone();
        let err = git::remove_worktree(&project_path, &wt_path)
            .err()
            .map(|e| e.to_string());
        if let Some(td) = self.teardown.as_mut() {
            td.stage = TeardownStage::Done {
                failed: err.is_some(),
            };
            td.message = match &err {
                Some(e) => format!("removal failed: {e}"),
                None => "worktree deleted".into(),
            };
        }
        self.status = match &err {
            Some(e) => format!("teardown err: {e}"),
            None => format!("removed worktree {wt_path}"),
        };
        self.refresh_worktrees();
    }

    /// Skip a still-running teardown script: kill it and proceed to removal.
    pub fn skip_teardown_script(&mut self) {
        if let Some(td) = self.teardown.as_mut() {
            if td.stage == TeardownStage::RunningScript {
                if let Some(s) = td.session.as_mut() {
                    s.kill();
                }
                self.do_teardown_removal();
            }
        }
    }

    /// Dismiss the teardown modal. Only meaningful once removal has finished.
    pub fn close_teardown(&mut self) {
        self.teardown = None;
        self.modal = Modal::None;
    }

    pub fn picker_move(&mut self, delta: i32) {
        if let Modal::AgentPicker { sel, .. } = &mut self.modal {
            *sel = cycle(*sel, delta, self.available_agents.len());
        }
    }

    pub fn picker_toggle_default(&mut self) -> Result<()> {
        let Modal::AgentPicker { sel, .. } = &self.modal else {
            return Ok(());
        };
        let Some(agent) = self.available_agents.get(*sel).copied() else {
            return Ok(());
        };
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
        let Modal::AgentPicker {
            project,
            wt_path,
            sel,
        } = modal
        else {
            return;
        };
        let Some(agent) = self.available_agents.get(sel).copied() else {
            return;
        };
        let label = path_basename(&wt_path);
        let args = agent.launch_args();
        self.spawn_session(label, project, wt_path.clone(), agent, args, &wt_path);
    }

    pub fn open_theme_picker(&mut self) {
        let original = theme::current();
        let tab = original.kind;
        let sel = theme::themes_of(tab)
            .iter()
            .position(|t| t.name == original.name)
            .unwrap_or(0);
        let (sel_dark, sel_light) = match tab {
            theme::ThemeKind::Dark => (sel, 0),
            theme::ThemeKind::Light => (0, sel),
        };
        self.modal = Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            original,
        };
    }

    pub fn theme_picker_move(&mut self, delta: i32) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &mut self.modal
        else {
            return;
        };
        let themes = theme::themes_of(*tab);
        if themes.is_empty() {
            return;
        }
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        *sel = cycle(*sel, delta, themes.len());
        theme::set(themes[*sel]);
    }

    pub fn theme_picker_switch_tab(&mut self) {
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = &mut self.modal
        else {
            return;
        };
        *tab = match *tab {
            theme::ThemeKind::Dark => theme::ThemeKind::Light,
            theme::ThemeKind::Light => theme::ThemeKind::Dark,
        };
        let themes = theme::themes_of(*tab);
        let sel = match tab {
            theme::ThemeKind::Dark => *sel_dark,
            theme::ThemeKind::Light => *sel_light,
        };
        if let Some(t) = themes.get(sel) {
            theme::set(*t);
        }
    }

    pub fn theme_picker_submit(&mut self) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::ThemePicker {
            sel_dark,
            sel_light,
            tab,
            ..
        } = modal
        else {
            return Ok(());
        };
        let themes = theme::themes_of(tab);
        let sel = match tab {
            theme::ThemeKind::Dark => sel_dark,
            theme::ThemeKind::Light => sel_light,
        };
        let Some(chosen) = themes.get(sel).copied() else {
            return Ok(());
        };
        theme::set(chosen);
        self.store.theme = Some(chosen.name.to_string());
        storage::save(&self.store)?;
        self.status = format!("theme: {}", chosen.name);
        Ok(())
    }

    pub fn theme_picker_cancel(&mut self) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        if let Modal::ThemePicker { original, .. } = modal {
            theme::set(original);
        }
    }

    pub fn input_dir_move(&mut self, delta: i32) {
        let Modal::Input {
            buffer,
            kind,
            dir_sel,
            ..
        } = &mut self.modal
        else {
            return;
        };
        if !matches!(kind, InputKind::AddProjectPath) {
            return;
        }
        let entries = list_dirs(buffer);
        if entries.is_empty() {
            *dir_sel = 0;
            return;
        }
        *dir_sel = cycle(*dir_sel, delta, entries.len());
    }

    pub fn input_dir_pick(&mut self) {
        let Modal::Input {
            buffer,
            kind,
            dir_sel,
            ..
        } = &mut self.modal
        else {
            return;
        };
        if !matches!(kind, InputKind::AddProjectPath) {
            return;
        }
        let entries = list_dirs(buffer);
        if let Some(pick) = entries.get(*dir_sel) {
            *buffer = format!("{}/", pick);
            *dir_sel = 0;
        }
    }

    pub fn input_buffer_edit<F: FnOnce(&mut String)>(&mut self, f: F) {
        if let Modal::Input {
            buffer, dir_sel, ..
        } = &mut self.modal
        {
            f(buffer);
            *dir_sel = 0;
        }
    }

    pub fn submit_input(&mut self) -> Result<()> {
        if let Modal::Input {
            buffer,
            kind: InputKind::AddProjectPath,
            ..
        } = &self.modal
        {
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
        let Modal::Input { buffer, kind, .. } = modal else {
            return Ok(());
        };
        let value = buffer.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        match kind {
            InputKind::AddProjectPath => {
                let path = shellexpand_tilde(&value);
                let pb = std::path::PathBuf::from(&path);
                let abs = std::fs::canonicalize(&pb)?.to_string_lossy().to_string();
                let default_name = pb
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                self.modal = Modal::Input {
                    title: "project name".into(),
                    buffer: default_name,
                    kind: InputKind::AddProjectName { path: abs },
                    dir_sel: 0,
                };
            }
            InputKind::AddProjectName { path } => {
                if self.store.projects.iter().any(|p| p.name == value) {
                    self.modal = Modal::Message(format!("project '{value}' already exists"));
                    return Ok(());
                }
                self.store.projects.push(Project {
                    name: value.clone(),
                    path: path.clone(),
                    scripts: Default::default(),
                });
                storage::save(&self.store)?;
                self.proj_idx = self.store.projects.len() - 1;
                self.status = format!("added {value}");

                let needs_init = !std::path::Path::new(&path).join(".git").exists();
                let needs_include = !std::path::Path::new(&path)
                    .join(".worktreeinclude")
                    .exists();

                if needs_init {
                    self.modal = Modal::Confirm {
                        title: "initialize git repo?".into(),
                        prompt: format!("'{path}' is not a git repo. Run `git init`."),
                        destructive: false,
                        kind: ConfirmKind::InitRepo { path, name: value },
                    };
                } else if needs_include {
                    self.modal = Modal::Confirm {
                        title: "generate .worktreeinclude?".into(),
                        prompt: "Use Claude (haiku) to draft a .worktreeinclude for this repo."
                            .into(),
                        destructive: false,
                        kind: ConfirmKind::GenerateInclude { path },
                    };
                }
                self.refresh_worktrees();
            }
            InputKind::AddWorktreeName => {
                if !git::valid_worktree_name(&value) {
                    self.modal =
                        Modal::Message("invalid name: use letters, digits, '-', '_' or '.'".into());
                    return Ok(());
                }
                let Some(p) = self.selected_project().cloned() else {
                    return Ok(());
                };
                if !std::path::Path::new(&p.path).join(".git").exists() {
                    self.modal = Modal::Confirm {
                        title: "initialize git repo?".into(),
                        prompt: format!(
                            "'{}' is not a git repo. Run `git init`, then create worktree '{}'.",
                            p.path, value
                        ),
                        destructive: false,
                        kind: ConfirmKind::InitAndAddWorktree { name: value },
                    };
                    return Ok(());
                }
                self.create_worktree(&p, &value);
            }
        }
        Ok(())
    }

    pub fn submit_confirm(&mut self, yes: bool) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::Confirm { kind, .. } = modal else {
            return Ok(());
        };
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
                    self.start_teardown(&p, path);
                }
            }
            ConfirmKind::InitAndAddWorktree { name } => {
                let Some(p) = self.selected_project().cloned() else {
                    return Ok(());
                };
                if let Err(e) = git::init_if_needed(&p.path) {
                    self.modal = Modal::Message(format!("git init failed: {e}"));
                    return Ok(());
                }
                self.create_worktree(&p, &name);
            }
            ConfirmKind::InitRepo { path, name } => {
                if let Err(e) = git::init_if_needed(&path) {
                    self.modal = Modal::Message(format!("git init failed: {e}"));
                    return Ok(());
                }
                let needs_include = !std::path::Path::new(&path)
                    .join(".worktreeinclude")
                    .exists();
                if needs_include {
                    self.modal = Modal::Confirm {
                        title: "generate .worktreeinclude?".into(),
                        prompt: "Use Claude (haiku) to draft a .worktreeinclude for this repo."
                            .into(),
                        destructive: false,
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
                // Run in the background — the claude CLI can take minutes and
                // must not block the UI thread. The tick handler drains
                // `bg_status` when the job finishes.
                let slot = self.bg_status.clone();
                self.status = "generating .worktreeinclude…".into();
                std::thread::spawn(move || {
                    let res = Command::new("claude")
                        .args([
                            "--model",
                            "haiku",
                            "--dangerously-skip-permissions",
                            "-p",
                            prompt,
                        ])
                        .current_dir(&path)
                        .stdin(std::process::Stdio::null())
                        .status();
                    let msg = match res {
                        Ok(s) if s.success() => ".worktreeinclude generated".into(),
                        Ok(s) => format!("generate failed: claude exited with {:?}", s.code()),
                        Err(e) => format!("generate failed: {e}"),
                    };
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(msg);
                    }
                });
            }
        }
        Ok(())
    }
}

/// Re-attach to any tmux sessions grove left running from a previous launch.
/// Sessions that fail to attach are silently dropped.
fn discover_sessions() -> Vec<Session> {
    tmux::list_grove_sessions()
        .into_iter()
        .filter(|d| tmux::has_session(&d.name))
        .filter_map(|d| Session::attach_existing(d).ok())
        .collect()
}

/// Last path component, or the whole string if there is no separator.
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

pub fn list_dirs(buffer: &str) -> Vec<String> {
    let expanded = shellexpand_tilde(buffer);
    let (dir, prefix) = if expanded.is_empty() {
        (std::path::PathBuf::from("."), String::new())
    } else if expanded.ends_with('/') {
        (
            std::path::PathBuf::from(expanded.trim_end_matches('/')),
            String::new(),
        )
    } else {
        let pb = std::path::PathBuf::from(&expanded);
        let parent = pb
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = pb
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let parent = if parent.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            parent
        };
        (parent, name)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
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

#[cfg(test)]
mod tests {
    use super::{effective_backend_for, needs_tmux_choice, EffectiveBackend};

    #[test]
    fn tmux_unavailable_uses_native_for_any_preference() {
        assert_eq!(effective_backend_for(false, None), EffectiveBackend::Native);
        assert_eq!(
            effective_backend_for(false, Some(true)),
            EffectiveBackend::Native
        );
        assert_eq!(
            effective_backend_for(false, Some(false)),
            EffectiveBackend::Native
        );
    }

    #[test]
    fn tmux_available_uses_saved_preference() {
        assert_eq!(
            effective_backend_for(true, Some(true)),
            EffectiveBackend::Tmux
        );
        assert_eq!(
            effective_backend_for(true, Some(false)),
            EffectiveBackend::Native
        );
    }

    #[test]
    fn tmux_available_without_preference_requires_choice() {
        assert_eq!(effective_backend_for(true, None), EffectiveBackend::Native);
        assert!(needs_tmux_choice(true, None));
        assert!(!needs_tmux_choice(true, Some(true)));
        assert!(!needs_tmux_choice(true, Some(false)));
        assert!(!needs_tmux_choice(false, None));
    }
}
