mod modal;
mod onboarding;
mod spawn;
mod teardown;
mod terminals;
mod theme_picker;
mod util;

pub use modal::{ConfirmKind, Modal, Teardown, TeardownStage};
pub use onboarding::{first_run_modal, onboarding_modal, FirstRunModal, OnboardStep};
pub use theme_picker::ThemePickerScope;
pub(crate) use theme_picker::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
pub use util::{cycle, list_dirs, path_basename};
pub(crate) use util::{discover_sessions, shellexpand_tilde};

use anyhow::Result;
use grove_core::agent::Agent;
use grove_core::git::{self, Worktree};
use grove_core::session::Session;
use grove_core::storage::{self, Project, Store};
use grove_core::theme;
use grove_core::tmux;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub created: std::time::Instant,
}

impl Toast {
    /// How long a toast stays up before auto-dismissing: errors linger
    /// twice as long as informational messages.
    pub fn ttl(kind: ToastKind) -> std::time::Duration {
        match kind {
            ToastKind::Info => std::time::Duration::from_secs(4),
            ToastKind::Error => std::time::Duration::from_secs(8),
        }
    }

    /// Whether the toast should be dismissed as of `now`. Pure so expiry is
    /// unit-testable without waiting.
    pub fn expired_at(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.created) >= Self::ttl(self.kind)
    }
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

pub struct App {
    pub(crate) store: Store,
    pub(crate) worktrees: Vec<Worktree>,
    pub(crate) focus: Pane,
    pub(crate) proj_idx: usize,
    pub(crate) wt_idx: usize,
    pub(crate) modal: Modal,
    pub(crate) sessions: Vec<Session>,
    pub(crate) active_session: Option<usize>,
    /// The shells behind the `terminal` tab, each rooted at `~`. The first is
    /// spawned lazily when the tab is first opened; the user can add more.
    /// These live outside `sessions` so they never show up in the tree /
    /// activity lists and aren't reachable by the session-cycling or kill
    /// machinery. Always non-empty while the terminal tab is in use (closing
    /// the last one immediately respawns a fresh shell).
    pub(crate) home_terminals: Vec<Session>,
    /// Index into `home_terminals` of the terminal the tab is showing.
    pub(crate) active_terminal: Option<usize>,
    /// Monotonic counter behind each terminal's internal label (`terminal 1`,
    /// `terminal 2`, …). The label isn't shown in the UI — rows display only
    /// the icon and the shell's contextual title — but it stays stable and
    /// unique per terminal so it can be stripped from that title and preserved
    /// across a restart.
    pub(crate) home_terminal_seq: usize,
    /// Worktree-scoped terminals for the right-docked slide-over panel, keyed by
    /// absolute worktree path. Each panel can hold several shells (the panel's
    /// tab strip). These live outside `sessions` so they never appear as
    /// sidebar/tree/activity rows — they belong *inside* a session's worktree,
    /// not beside it. Entries are dropped (shells killed) when the worktree is
    /// removed via [`kill_wt_terminals`].
    pub(crate) wt_terminals: HashMap<String, Vec<Session>>,
    /// Active shell index within each worktree's panel, keyed by worktree path.
    pub(crate) wt_active_terminal: HashMap<String, usize>,
    /// Monotonic counter behind each panel terminal's internal label.
    pub(crate) wt_terminal_seq: usize,
    /// Transient top-right notification (e.g. copy confirmation).
    pub(crate) toast: Option<Toast>,
    /// Total worktrees across all projects, cached so the renderer doesn't
    /// shell out to `git` for every project on every frame.
    pub(crate) worktree_count: usize,
    /// Zen mode: when false, the top banner and the sessions sidebar are
    /// hidden on the session page so the PTY can use the full frame.
    pub(crate) chrome_visible: bool,
    /// Whether tmux was available on PATH when Grove started.
    pub(crate) tmux_available: bool,
    /// Agents whose binaries were found on PATH and are executable.
    /// Ordered to match `Agent::ALL`; `sel` in `Modal::AgentPicker` indexes
    /// into this slice. Always contains at least `Terminal`.
    /// Re-scanned each time the picker is opened so newly-installed tools
    /// appear without restarting Grove.
    pub(crate) available_agents: Vec<Agent>,
    /// Completion message from a background job (e.g. `.worktreeinclude`
    /// generation). Set by the worker thread, drained on the GUI tick.
    pub(crate) bg_status: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// In-progress worktree teardown, when `modal` is `Modal::Teardown`.
    pub(crate) teardown: Option<Teardown>,
    /// Whether the active theme should track the OS light/dark setting
    /// (mirrors `store.theme_follow_system`, kept in sync on submit).
    pub(crate) theme_follow_system: bool,
    /// Last-known OS appearance, seeded by an `iced::system::theme()` query
    /// at startup and kept current by the `system::theme_changes()`
    /// subscription. Only consulted while `theme_follow_system` is set.
    pub(crate) system_theme_mode: iced::theme::Mode,
    /// Entries from `themes.json` skipped on the last `theme::load_custom()`
    /// call (bad hex, missing field, name collision, ...), kept for a future
    /// non-blocking error toast/surface (batch 4). Empty on a clean load or
    /// when the file doesn't exist.
    pub(crate) theme_load_errors: Vec<grove_core::theme_file::ThemeLoadError>,
}

impl App {
    pub(crate) fn set_toast(&mut self, message: impl Into<String>) {
        self.toast_with_kind(message, ToastKind::Info);
    }

    pub(crate) fn set_error_toast(&mut self, message: impl Into<String>) {
        self.toast_with_kind(message, ToastKind::Error);
    }

    fn toast_with_kind(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            created: std::time::Instant::now(),
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
    pub(crate) fn new() -> Result<Self> {
        let mut store = storage::load()?;
        let tmux_available = tmux::available();
        // Apply the saved theme before building the initial modal so the
        // onboarding wizard seeds its theme selection from the active theme.
        // In "follow system" mode the real OS appearance arrives later via
        // the seeded `iced::system::theme()` task, but that first frame still
        // needs a concrete theme, so we seed from the saved dark theme (the
        // `SystemThemeChanged` handler corrects this once the query answers).
        // Load user-defined themes before resolving the saved selection below,
        // so a persisted custom theme name (dark/light/plain) still resolves
        // on this first frame instead of silently falling back.
        let theme_load_errors = theme::load_custom();
        // One-time migration: a persisted theme name can go stale (e.g. a
        // builtin dropped from a later curated set, or a custom theme
        // deleted outside the app) — `theme::by_name` fails to resolve it
        // silently. Rewrite any such field to `DEFAULT_DARK_THEME` up front
        // so the dead name doesn't linger in `store.json`, and so the
        // resolution below always lands on a theme that actually exists
        // instead of leaving `theme::ACTIVE` on its static-initializer
        // default (`TOKYONIGHT`, not `DEFAULT_DARK_THEME`) unnoticed.
        if theme_picker::migrate_stale_theme_names(&mut store) {
            storage::persist(&store);
        }
        let theme_follow_system = store.theme_follow_system;
        if theme_follow_system {
            let name = store.theme_dark.as_deref().unwrap_or(DEFAULT_DARK_THEME);
            if !theme::set_by_name(name) {
                theme::set_by_name(DEFAULT_DARK_THEME);
            }
        } else if let Some(name) = store.theme.as_deref() {
            if !theme::set_by_name(name) {
                theme::set_by_name(DEFAULT_DARK_THEME);
            }
        }
        let initial_modal =
            match first_run_modal(store.onboarded, tmux_available, store.tmux_enabled) {
                FirstRunModal::Onboarding => onboarding_modal(),
                FirstRunModal::TmuxChoice => Modal::TmuxChoice,
                FirstRunModal::None => Modal::None,
            };
        // Existing tmux sessions keep their backend even when the saved
        // preference now chooses native sessions for new launches.
        let sessions = if tmux_available {
            discover_sessions()
        } else {
            Vec::new()
        };
        let mut app = App {
            store,
            worktrees: vec![],
            focus: Pane::Projects,
            proj_idx: 0,
            wt_idx: 0,
            modal: initial_modal,
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
            theme_follow_system,
            system_theme_mode: iced::theme::Mode::None,
            theme_load_errors,
        };
        app.refresh_worktrees();
        // Non-blocking notice for any themes.json entries skipped on this
        // startup load (mock E1) — reuses the existing toast mechanism
        // rather than a bespoke banner; auto-dismisses like any other toast.
        if let Some(summary) = grove_core::theme_file::summarize_errors(&app.theme_load_errors) {
            app.set_error_toast(summary);
        }
        Ok(app)
    }

    pub(crate) fn effective_backend(&self) -> EffectiveBackend {
        effective_backend_for(self.tmux_available, self.store.tmux_enabled)
    }

    pub(crate) fn use_tmux(&self) -> bool {
        self.effective_backend() == EffectiveBackend::Tmux
    }

    /// Running sessions that would die with the process: native-backend agent
    /// sessions only. tmux-backed sessions survive a quit and don't count.
    pub(crate) fn native_sessions_running(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.tmux_name().is_none() && s.is_running())
            .count()
    }

    pub(crate) fn set_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled && !self.tmux_available {
            self.set_error_toast("tmux not found; using native sessions");
            return Ok(());
        }
        self.store.tmux_enabled = Some(enabled);
        storage::save(&self.store)?;
        if enabled {
            self.discover_tmux_sessions();
            self.set_toast("tmux enabled for new sessions");
        } else {
            self.set_toast("tmux disabled for new sessions");
        }
        Ok(())
    }

    pub(crate) fn choose_tmux_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_tmux_enabled(enabled)?;
        self.modal = Modal::None;
        Ok(())
    }

    pub(crate) fn skip_permissions_enabled(&self) -> bool {
        self.store
            .dangerously_skip_permissions_enabled
            .unwrap_or(false)
    }

    pub(crate) fn chrome_enabled(&self) -> bool {
        self.store.chrome_enabled.unwrap_or(false)
    }

    pub(crate) fn set_chrome_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.chrome_enabled = Some(enabled);
        storage::save(&self.store)?;
        self.set_toast(if enabled {
            "Chrome control enabled for new Claude sessions"
        } else {
            "Chrome control disabled for new Claude sessions"
        });
        Ok(())
    }

    pub(crate) fn set_skip_permissions_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.dangerously_skip_permissions_enabled = Some(enabled);
        storage::save(&self.store)?;
        self.set_toast(if enabled {
            "permission bypass enabled for new sessions"
        } else {
            "permission bypass disabled for new sessions"
        });
        Ok(())
    }

    pub(crate) fn telemetry_enabled(&self) -> bool {
        self.store.telemetry_enabled.unwrap_or(true)
    }

    pub(crate) fn set_telemetry_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.telemetry_enabled = Some(enabled);
        storage::save(&self.store)?;
        crate::telemetry::set_enabled(enabled);
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
            .filter(|s| s.tmux_name().is_none_or(|name| !known.contains(name)))
            .collect();
        for s in discovered {
            let at = self.session_insert_index(&s);
            if self.active_session.is_some_and(|i| at <= i) {
                self.active_session = self.active_session.map(|i| i + 1);
            }
            self.sessions.insert(at, s);
        }
    }

    /// Recompute the cross-project worktree total. Call after any change that
    /// adds or removes projects or worktrees.
    pub(crate) fn recount_worktrees(&mut self) {
        // `self.worktrees` already holds the selected project's worktrees;
        // reuse it instead of shelling out to `git` for that project again.
        let proj_idx = self.proj_idx;
        let selected = self.worktrees.iter().filter(|w| !w.is_main).count();
        let other_paths: Vec<String> = self
            .store
            .projects
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != proj_idx)
            .map(|(_, p)| p.path.clone())
            .collect();
        let mut scanned = git::list_worktrees_many(&other_paths).into_iter();
        self.worktree_count = (0..self.store.projects.len())
            .map(|i| {
                if i == proj_idx {
                    selected
                } else {
                    // Safe: `other_paths` has exactly one entry per non-`proj_idx`
                    // project, in the same relative order, so this iterator is
                    // never exhausted early.
                    scanned
                        .next()
                        .map_or(0, |w| w.iter().filter(|w| !w.is_main).count())
                }
            })
            .sum();
    }

    pub(crate) fn selected_project(&self) -> Option<&Project> {
        self.store.projects.get(self.proj_idx)
    }

    pub(crate) fn refresh_worktrees(&mut self) {
        self.worktrees = match self.selected_project() {
            Some(p) => git::list_worktrees(&p.path),
            None => vec![],
        };
        if self.wt_idx >= self.worktrees.len() {
            self.wt_idx = self.worktrees.len().saturating_sub(1);
        }
        self.recount_worktrees();
    }

    pub(crate) fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane;
    }

    pub(crate) fn start_add(&mut self) {
        match self.focus {
            // The only reachable caller for the Projects-pane case
            // (`Msg::AddProject`'s open handler in `gui::update`) bypasses
            // this dispatcher and calls `gui::add_project::open` directly —
            // that path needs to seed `Grove::add_project`'s wizard state,
            // which this domain-layer method has no access to. This arm is
            // kept only so the match stays exhaustive over `Pane` (every
            // caller of `start_add` today forces `self.focus` to `Worktrees`
            // before calling when the worktree path is intended).
            Pane::Projects => self.modal = Modal::AddProject,
            Pane::Worktrees => {
                if self.selected_project().is_some() {
                    self.modal = Modal::Input {
                        title: "Worktree name".into(),
                        buffer: String::new(),
                        note: None,
                    };
                }
            }
        }
    }

    pub(crate) fn start_delete(&mut self) {
        match self.focus {
            Pane::Projects => {
                if let Some(p) = self.selected_project() {
                    self.modal = Modal::Confirm {
                        title: "Remove project?".into(),
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
                            Modal::Message("Can't remove the project's main checkout".into());
                        return;
                    }
                    let path = wt.path.clone();
                    self.modal = Modal::Confirm {
                        title: "Remove worktree?".into(),
                        prompt: format!("git worktree remove --force {path}"),
                        destructive: true,
                        kind: ConfirmKind::RemoveWorktree(path),
                    };
                }
            }
        }
    }

    pub(crate) fn picker_move(&mut self, delta: i32) {
        if let Modal::AgentPicker { sel, .. } = &mut self.modal {
            *sel = cycle(*sel, delta, self.available_agents.len());
        }
    }

    pub(crate) fn picker_toggle_default(&mut self) -> Result<()> {
        let Modal::AgentPicker { sel, .. } = &self.modal else {
            return Ok(());
        };
        let Some(agent) = self.available_agents.get(*sel).copied() else {
            return Ok(());
        };
        if self.store.default_agent == Some(agent) {
            self.store.default_agent = None;
            self.set_toast(format!("cleared default agent ({})", agent.label()));
        } else {
            self.store.default_agent = Some(agent);
            self.set_toast(format!("default agent: {}", agent.label()));
        }
        storage::save(&self.store)?;
        Ok(())
    }

    pub(crate) fn picker_submit(&mut self) {
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
        let args = agent.launch_args(self.skip_permissions_enabled(), self.chrome_enabled());
        self.spawn_session(
            label,
            project,
            wt_path.clone(),
            agent,
            args,
            &wt_path,
            false,
        );
    }

    pub(crate) fn open_settings(&mut self) {
        self.modal = Modal::Settings;
    }

    /// Set the global default agent, or clear it when re-selecting the current
    /// default (mirrors `picker_toggle_default`). Only installed tools should
    /// reach here — the Settings UI hides the action on missing tools.
    pub(crate) fn set_default_agent(&mut self, agent: Agent) -> Result<()> {
        if self.store.default_agent == Some(agent) {
            self.store.default_agent = None;
            self.set_toast(format!("cleared default agent ({})", agent.label()));
        } else {
            self.store.default_agent = Some(agent);
            self.set_toast(format!("default agent: {}", agent.label()));
        }
        storage::save(&self.store)?;
        Ok(())
    }

    /// Replace the input-modal buffer from a live `text_input` edit.
    pub(crate) fn set_input_path(&mut self, s: String) {
        if let Modal::Input { buffer, note, .. } = &mut self.modal {
            *buffer = s;
            *note = None;
        }
    }

    pub(crate) fn submit_input(&mut self) -> Result<()> {
        let value = match &self.modal {
            Modal::Input { buffer, .. } => buffer.trim().to_string(),
            _ => return Ok(()),
        };
        if value.is_empty() {
            return Ok(());
        }
        self.modal = Modal::None;
        if !git::valid_worktree_name(&value) {
            self.modal =
                Modal::Message("Invalid name: use letters, digits, '-', '_' or '.'".into());
            return Ok(());
        }
        let Some(p) = self.selected_project().cloned() else {
            return Ok(());
        };
        if !git::is_repo(&p.path) {
            self.modal = Modal::Confirm {
                title: "Initialize Git repo?".into(),
                prompt: format!(
                    "'{}' is not a Git repo. Run `git init`, then create worktree '{}'.",
                    p.path, value
                ),
                destructive: false,
                kind: ConfirmKind::InitAndAddWorktree { name: value },
            };
            return Ok(());
        }
        self.create_worktree(&p, &value);
        Ok(())
    }

    /// Persist a new project and select it. Quiet — no follow-up modals.
    /// `pub(crate)` (rather than private): called from `gui::add_project`'s
    /// submit path now that the wizard's presentation state moved out of
    /// this domain layer — see `Modal::AddProject`'s doc comment.
    pub(crate) fn register_project(&mut self, name: String, path: String) -> Result<usize> {
        // The name becomes a directory under `worktrees_root()`, so it has to
        // stay a single, boring path segment — free text could escape it.
        if !grove_core::git::valid_project_name(&name) {
            anyhow::bail!(
                "'{name}' isn't a valid project name; use letters, digits, '.', '-' or '_'"
            );
        }
        self.store.projects.push(Project {
            name: name.clone(),
            path,
            scripts: grove_core::storage::ProjectScripts::default(),
            theme: None,
        });
        storage::save(&self.store)?;
        self.proj_idx = self.store.projects.len() - 1;
        self.set_toast(format!("added {name}"));
        self.refresh_worktrees();
        Ok(self.proj_idx)
    }

    pub(crate) fn submit_confirm(&mut self, yes: bool) -> Result<()> {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let Modal::Confirm { kind, .. } = modal else {
            return Ok(());
        };
        if !yes {
            return Ok(());
        }
        match kind {
            ConfirmKind::RemoveProject(idx) => {
                // Route through the same teardown as the remove-project modal
                // so the project's sessions are killed, not orphaned.
                let msg = self.finalize_remove_project(idx)?;
                if !msg.is_empty() {
                    self.set_toast(msg);
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
                    self.modal = Modal::Message(format!("Git init failed: {e}"));
                    return Ok(());
                }
                self.create_worktree(&p, &name);
            }
            // Handled at the GUI layer (needs iced::exit); never reaches here.
            ConfirmKind::Quit => {}
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_app(sessions: Vec<Session>) -> App {
    App {
        store: Store::default(),
        worktrees: vec![],
        focus: Pane::Projects,
        proj_idx: 0,
        wt_idx: 0,
        modal: Modal::None,
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
        tmux_available: false,
        available_agents: vec![],
        bg_status: std::sync::Arc::new(std::sync::Mutex::new(None)),
        teardown: None,
        theme_follow_system: false,
        system_theme_mode: iced::theme::Mode::None,
        theme_load_errors: Vec::new(),
    }
}

/// Spawn a cheap real session (a `true` shell script — exits immediately)
/// and force its backend/status to the scenario under test. `Session`'s
/// PTY/child fields are private to `session.rs` and can't be constructed
/// directly, but `backend` and `status` are `pub`, so overwriting them
/// after a real spawn is the lightest way to get a `Session` in an
/// arbitrary (backend, running) state without a real tmux session.
#[cfg(test)]
pub(crate) fn spawn_test_session(
    status: grove_core::session::SessionStatus,
    tmux: bool,
) -> Session {
    use grove_core::session::SessionBackend;
    let mut s = Session::spawn_script("t".into(), "p".into(), ".".into(), "true", ".")
        .expect("spawn test session");
    if tmux {
        s.backend = SessionBackend::Tmux {
            name: "test".into(),
        };
    }
    *s.status.lock().unwrap() = status;
    s
}

#[cfg(test)]
mod tests {
    use super::{
        effective_backend_for, needs_tmux_choice, spawn_test_session, test_app, EffectiveBackend,
    };
    use super::{Toast, ToastKind};
    use grove_core::session::SessionStatus;
    use std::time::{Duration, Instant};

    #[test]
    fn native_sessions_running_counts_only_running_native() {
        let app = test_app(vec![
            spawn_test_session(SessionStatus::Running, false), // native, running: counts
            spawn_test_session(SessionStatus::Exited(Some(0)), false), // native, exited: no
            spawn_test_session(SessionStatus::Running, true),  // tmux, running: no
        ]);
        assert_eq!(app.native_sessions_running(), 1);
    }

    #[test]
    fn toast_ttl_is_kind_dependent() {
        assert_eq!(Toast::ttl(ToastKind::Info), Duration::from_secs(4));
        assert_eq!(Toast::ttl(ToastKind::Error), Duration::from_secs(8));
    }

    #[test]
    fn toast_expiry_follows_ttl() {
        let t0 = Instant::now();
        let info = Toast {
            message: "copied".into(),
            kind: ToastKind::Info,
            created: t0,
        };
        let error = Toast {
            message: "failed".into(),
            kind: ToastKind::Error,
            created: t0,
        };
        assert!(!info.expired_at(t0 + Duration::from_secs(3)));
        assert!(info.expired_at(t0 + Duration::from_secs(4)));
        assert!(!error.expired_at(t0 + Duration::from_secs(7)));
        assert!(error.expired_at(t0 + Duration::from_secs(8)));
    }

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
