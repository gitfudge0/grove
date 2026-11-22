//! UI observable projection of the background Codex actor.
mod restore;
mod skill_clock;

use futures::StreamExt as _;
use gpui::{AppContext as _, Context, Task};
use grove_core::codex::{self, Action, Handle, SavedSession, Snapshot};
use grove_core::composer_catalog::{self, Scope};
use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
    time::Duration,
};

/// Keep launch/recovery boundaries out of worktrees with an in-flight or durable restore.
pub(crate) fn workspace_restore_error(
    paths: &[std::path::PathBuf],
    excluding_owner: Option<&str>,
) -> Option<String> {
    if grove_core::checkpoint::restore_blocked(paths) {
        return Some(
            "Wait for workspace restoration to finish before starting another session here.".into(),
        );
    }
    match codex::workspace_recovery_blocked(paths, excluding_owner) {
        Ok(false) => None,
        Ok(true) => Some("An interrupted rewind needs recovery in this worktree. Reopen that conversation before starting new work here.".into()),
        Err(error) => Some(error),
    }
}

/// UI readers share a stable snapshot. Any local mutation forks it immediately,
/// including writes made before GPUI delivers observer notifications.
pub struct SessionSnapshot(Rc<Snapshot>);
impl SessionSnapshot {
    pub fn shared(&self) -> Rc<Snapshot> {
        self.0.clone()
    }
}
impl From<Snapshot> for SessionSnapshot {
    fn from(state: Snapshot) -> Self {
        Self(Rc::new(state))
    }
}
impl Deref for SessionSnapshot {
    type Target = Snapshot;
    fn deref(&self) -> &Snapshot {
        &self.0
    }
}
impl DerefMut for SessionSnapshot {
    fn deref_mut(&mut self) -> &mut Snapshot {
        Rc::make_mut(&mut self.0)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DashboardLaunch {
    Starting,
    Ready,
    Submitted,
}

struct DormantRestore {
    connection: codex::DeferredConnection,
    validation: codex::StartupValidation,
    args: Vec<String>,
    loading: bool,
    draft_changed: bool,
}

pub struct CodexSession {
    has_transport: bool,
    skill_startup_ready: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    restored: Option<DormantRestore>,
    restore_load: Option<Task<()>>,
    pub state: SessionSnapshot,
    pub catalog_loading: bool,
    pub catalog_error: Option<String>,
    pub(crate) catalog_missing_executable: Option<grove_core::agent::MissingExecutable>,
    pub handle: Handle,
    updates_task: Task<()>,
    skill_watch: Option<Task<()>>,
    refreshed_skills: Option<Vec<serde_json::Value>>,
    deferred: Option<std::sync::mpsc::Receiver<Action>>,
    catalog_generation: u64,
    catalog_scope: Option<Scope>,
    catalog_selection_error: bool,
    dashboard_title_waiting: Option<String>,
    dashboard_launch: Option<DashboardLaunch>,
}
impl CodexSession {
    pub fn has_transport(&self) -> bool {
        self.has_transport
    }

    /// A composer-owned draft with no conversation, persistence, or registry entry.
    /// Capability discovery uses a temporary metadata-only provider handshake.
    pub fn draft(saved: SavedSession, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        let mut session = Self::draft_pending(saved, cx);
        session.refresh_catalog(session.state.saved.cwd.clone(), args, cx);
        session
    }

    pub(crate) fn draft_pending(saved: SavedSession, cx: &mut Context<Self>) -> Self {
        let (wake, mut wakes) = futures::channel::mpsc::channel(1);
        let wake = std::sync::Mutex::new(wake);
        let (handle, receiver) = Handle::channel_with_wake(std::sync::Arc::new(move || {
            // Only a notification is bounded; commands remain in the actor queue.
            let _ = wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_send(());
        }));
        let mut state = Snapshot::new(saved);
        state.status = codex::Status::Idle;
        let poll = cx.spawn(async move |this, cx| {
            while wakes.next().await.is_some() {
                if this.update(cx, CodexSession::drain_deferred).is_err() {
                    break;
                }
            }
        });
        Self {
            has_transport: false,
            skill_startup_ready: None,
            restored: None,
            restore_load: None,
            state: state.into(),
            handle,
            updates_task: poll,
            skill_watch: None,
            refreshed_skills: None,
            deferred: Some(receiver),
            catalog_generation: 0,
            catalog_scope: None,
            catalog_loading: true,
            catalog_error: None,
            catalog_missing_executable: None,
            catalog_selection_error: false,
            dashboard_title_waiting: None,
            dashboard_launch: None,
        }
    }
    /// Refresh checkout-specific skills/config and provider-advertised models.
    /// A late response cannot replace a newer destination or an activated session.
    pub fn refresh_catalog(&mut self, cwd: String, args: Vec<String>, cx: &mut Context<Self>) {
        self.refresh_catalog_inner(cwd, args, false, cx);
    }
    fn refresh_catalog_inner(
        &mut self,
        cwd: String,
        args: Vec<String>,
        retry: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.is_deferred() {
            return;
        }
        let mut request = self.state.saved.clone();
        request.cwd.clone_from(&cwd);
        let same_scope = self.catalog_scope.as_ref() == Some(&Scope::new(&request, &args));
        self.start_skill_watch(args.clone(), cx);
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.catalog_loading = true;
        if self.catalog_error.is_some() && self.state.error == self.catalog_error {
            self.state.error = None;
        }
        self.catalog_error = None;
        self.catalog_missing_executable = None;
        self.catalog_selection_error = false;
        // Skills and resolved defaults belong to a checkout/provider pair. Do
        // not expose the previous destination's configuration during refresh.
        self.state.skills.clear();
        if !same_scope {
            self.state.models.clear();
        }
        self.state.permission_modes.clear();
        self.state.resolved_model = None;
        self.state.resolved_effort = None;
        self.state.resolved_access = None;
        self.state.sandbox = None;
        self.state.approval_policy = None;
        self.state.reported_speed = None;
        self.state.speed_disabled_reason = None;
        cx.notify();
        if !std::path::Path::new(&cwd).is_dir() {
            self.catalog_loading = false;
            self.catalog_error = Some("The selected checkout is no longer available.".into());
            return;
        }
        let generation = self.catalog_generation;
        let mut saved = self.state.saved.clone();
        saved.cwd = cwd;
        let scope = Scope::new(&saved, &args);
        self.catalog_scope = Some(scope);
        if let Err(missing) = saved.provider.agent().check_executable() {
            self.complete_catalog(generation, Err(missing.to_string()), cx);
            return;
        }
        if !retry {
            if let Some(catalog) = composer_catalog::cached(&saved, &args) {
                self.complete_catalog(generation, Ok(catalog), cx);
                return;
            }
        }
        let discovery = cx.background_spawn(async move {
            grove_core::env_path::wait_for_initial_environment();
            saved
                .provider
                .agent()
                .check_executable()
                .map_err(|missing| missing.to_string())?;
            if retry {
                composer_catalog::invalidate(&saved, &args);
            }
            composer_catalog::discover(saved, args)
        });
        cx.spawn(async move |this, cx| {
            let result = discovery.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_catalog(generation, result, cx);
            });
        })
        .detach();
    }
    /// Model refresh changes advertised capabilities without reconfiguring the conversation.
    pub fn update_models(&mut self, models: Vec<serde_json::Value>, cx: &mut Context<Self>) {
        self.state.models = models.clone().into();
        if self.has_transport {
            let _ = self.handle.send(Action::UpdateModels(models));
        } else if self.catalog_selection_error {
            self.revalidate_catalog_selection();
        }
        cx.notify();
    }
    pub fn retry_catalog(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        self.refresh_catalog_inner(self.state.saved.cwd.clone(), args, true, cx);
    }
    /// Geometry fixtures must not depend on installed CLIs or an asynchronous handshake.
    #[cfg(test)]
    pub(crate) fn settle_draft_catalog_for_test(&mut self, cx: &mut Context<Self>) {
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.catalog_loading = false;
        self.catalog_error = None;
        self.catalog_missing_executable = None;
        self.catalog_selection_error = false;
        self.state.error = None;
        cx.notify();
    }

    fn complete_catalog(
        &mut self,
        generation: u64,
        result: Result<Snapshot, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_deferred() || self.catalog_generation != generation {
            return;
        }
        self.catalog_loading = false;
        let result = match self.state.saved.provider.agent().check_executable() {
            Ok(()) => result,
            Err(missing) => {
                let error = missing.to_string();
                self.catalog_missing_executable = Some(missing);
                self.state.models.clear();
                Err(error)
            }
        };
        match result {
            Ok(catalog) => {
                if self.catalog_missing_executable.take().is_some()
                    && self.state.error == self.catalog_error
                {
                    self.state.error = None;
                }
                self.state.models = catalog.models;
                self.state.skills = catalog.skills;
                self.state.permission_modes = catalog.permission_modes;
                self.state.resolved_model = catalog.resolved_model;
                self.state.resolved_effort = catalog.resolved_effort;
                self.state.resolved_access = catalog.resolved_access;
                self.state.sandbox = catalog.sandbox;
                self.state.approval_policy = catalog.approval_policy;
                self.state.reported_speed = catalog.reported_speed;
                self.state.speed_disabled_reason = catalog.speed_disabled_reason;
                self.catalog_error = None;
                self.catalog_error = self.validate_draft_catalog().err();
                self.catalog_selection_error = self.catalog_error.is_some();
                if let Some(error) = self.catalog_error.clone() {
                    self.state.error = Some(error);
                }
            }
            Err(error) => {
                self.catalog_selection_error = false;
                self.catalog_error = Some(error.clone());
                self.state.error = Some(error);
            }
        }
        cx.notify();
    }
    fn validate_draft_catalog(&self) -> Result<(), String> {
        if self.catalog_loading {
            return Err("Wait for the selected provider's model options to finish loading.".into());
        }
        if let Some(error) = &self.catalog_error {
            return Err(error.clone());
        }
        if let Some(selected) = &self.state.saved.composer.model {
            if !self
                .state
                .models
                .iter()
                .any(|model| model["model"].as_str() == Some(selected.as_str()))
            {
                return Err("The selected model is no longer advertised by this provider. Choose another model.".into());
            }
        }
        Ok(())
    }
    /// Changing an explicit model can recover its validation error. It cannot
    /// recover a failed metadata handshake; that still needs a catalog retry.
    fn revalidate_catalog_selection(&mut self) {
        if !self.catalog_selection_error {
            return;
        }
        let previous = self.catalog_error.take();
        self.catalog_error = self.validate_draft_catalog().err();
        self.catalog_selection_error = self.catalog_error.is_some();
        if self.state.error == previous {
            self.state.error.clone_from(&self.catalog_error);
        }
    }
    pub fn is_deferred(&self) -> bool {
        self.deferred.is_some()
    }
    /// Provider identity may change only before the dashboard draft is launched.
    /// Browse-time metadata discovery never calls this method.
    pub(crate) fn select_draft_model(
        &mut self,
        provider: codex::Provider,
        model: String,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.is_deferred() {
            return Err("A running conversation cannot switch providers.".into());
        }
        if model.is_empty() {
            return Err("Choose an advertised model before switching providers.".into());
        }
        provider
            .agent()
            .check_executable()
            .map_err(|missing| missing.to_string())?;
        provider.launch_options(&args)?;
        self.drain_deferred(cx);
        let provider_changed = self.state.saved.provider != provider;
        if provider_changed {
            self.state.saved.provider = provider;
            self.state.saved.label = provider.label().into();
            self.state.saved.composer = codex::ComposerSettings::default();
            self.state.model = None;
            self.state.effort = None;
            self.state.access = None;
        }
        if self.state.model.as_deref() != Some(model.as_str()) {
            self.state.effort = None;
            self.state.saved.composer.effort = None;
        }
        self.state.error = None;
        self.state.saved.composer.reset_to_configured = false;
        self.state.saved.composer.reset_model_effort_only = false;
        self.send(
            Action::Configure {
                model: Some(model),
                effort: None,
                access: None,
            },
            cx,
        );
        // Models already include effort capabilities. Selecting another advertised
        // model in the same scope is local, not another provider initialization.
        let ready = !provider_changed
            && !self.catalog_loading
            && self.catalog_error.is_none()
            && self.catalog_scope.as_ref() == Some(&Scope::new(&self.state.saved, &args))
            && std::path::Path::new(&self.state.saved.cwd).is_dir()
            && self.validate_draft_catalog().is_ok();
        if !ready {
            self.refresh_catalog(self.state.saved.cwd.clone(), args, cx);
        }
        Ok(())
    }
    pub(crate) fn drain_deferred(&mut self, cx: &mut Context<Self>) {
        let actions: Vec<_> = self
            .deferred
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        for action in actions {
            self.send(action, cx);
        }
    }
    /// Connect only after the host has validated a concrete launch destination.
    pub fn activate(
        &mut self,
        project: String,
        cwd: String,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.is_deferred() {
            return Err("This session has already been launched.".into());
        }
        if let Err(missing) = self.state.saved.provider.agent().check_executable() {
            let error = missing.to_string();
            self.complete_catalog(self.catalog_generation, Err(error.clone()), cx);
            return Err(error);
        }
        self.state.saved.provider.launch_options(&args)?;
        let paths = std::iter::once(std::path::PathBuf::from(&cwd))
            .chain(
                self.state
                    .saved
                    .roots
                    .iter()
                    .map(|root| root.wt_path.clone().into()),
            )
            .collect::<Vec<_>>();
        let validation_cwd = cwd.clone();
        let validation = std::sync::Arc::new(move || {
            if let Some(error) = workspace_restore_error(&paths, None) {
                return Err(error);
            }
            if !std::path::Path::new(&validation_cwd).is_dir() {
                return Err("The selected checkout is no longer available.".into());
            }
            Ok(())
        });
        self.drain_deferred(cx);
        self.validate_draft_catalog()?;
        let mut saved = self.state.saved.clone();
        saved.project = project;
        saved.cwd = cwd;
        let pending_title = saved
            .draft
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(80).collect::<String>());
        *self = Self::new_checked(saved, args, validation, cx);
        self.dashboard_launch = Some(DashboardLaunch::Starting);
        self.dashboard_title_waiting = pending_title;
        if let Some(title) = &self.dashboard_title_waiting {
            self.state.saved.title = Some(title.clone());
            self.state.title_generation_pending = true;
        }
        cx.notify();
        Ok(())
    }
    pub fn new(saved: SavedSession, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        Self::new_checked(saved, args, std::sync::Arc::new(|| Ok(())), cx)
    }
    /// Return the pending session immediately; fresh checkout validation runs
    /// in the provider actor before any CLI or metadata discovery can start.
    pub fn new_checked(
        saved: SavedSession,
        args: Vec<String>,
        validation: codex::StartupValidation,
        cx: &mut Context<Self>,
    ) -> Self {
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let validated = ready.clone();
        let validation = std::sync::Arc::new(move || {
            validated.store(false, std::sync::atomic::Ordering::Release);
            let result = validation();
            validated.store(result.is_ok(), std::sync::atomic::Ordering::Release);
            result
        });
        let state = Snapshot::new(saved.clone());
        let connection = codex::connect_checked(saved, args.clone(), validation);
        let mut session = Self::from_connection(state, connection, cx);
        session.skill_startup_ready = Some(ready);
        session.start_skill_watch(args, cx);
        session
    }
    pub(crate) fn from_connection(
        state: Snapshot,
        connection: codex::Connection,
        cx: &mut Context<Self>,
    ) -> Self {
        let handle = connection.handle;
        let (wake, mut wakes) = futures::channel::mpsc::channel(1);
        let wake = std::sync::Mutex::new(wake);
        let event_driven = connection.updates.set_wake(std::sync::Arc::new(move || {
            let _ = wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_send(());
        }));
        let poll = cx.spawn(async move |this, cx| loop {
            if event_driven && wakes.next().await.is_none() {
                break;
            }
            // Coalesce bursts after a wake. Idle transports schedule no timer.
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
            let mut latest = None;
            while let Ok(state) = connection.updates.try_recv() {
                latest = Some(state);
            }
            if let Some(state) = latest {
                if this
                    .update(cx, |this, cx| {
                        this.accept_snapshot(state);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            has_transport: true,
            skill_startup_ready: None,
            restored: None,
            restore_load: None,
            state: state.into(),
            handle,
            updates_task: poll,
            skill_watch: None,
            refreshed_skills: None,
            deferred: None,
            catalog_generation: 0,
            catalog_scope: None,
            catalog_loading: false,
            catalog_error: None,
            catalog_missing_executable: None,
            catalog_selection_error: false,
            dashboard_title_waiting: None,
            dashboard_launch: None,
        }
    }
    /// Poll only source metadata in the background; changed sources trigger one
    /// shared metadata handshake, without interrupting the conversation transport.
    fn start_skill_watch(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        self.skill_watch = Some(cx.spawn(async move |this, cx| {
            let mut previous = None;
            loop {
                let Ok(ready) = this.read_with(cx, |this, _| {
                    (!this.is_deferred() || !this.catalog_loading)
                        && this
                            .skill_startup_ready
                            .as_ref()
                            .is_none_or(|ready| ready.load(std::sync::atomic::Ordering::Acquire))
                }) else {
                    return;
                };
                if !ready {
                    if cx.update(skill_clock::wait).await.is_err() {
                        return;
                    }
                    continue;
                }
                let Ok((request, draft_generation)) = this.read_with(cx, |this, _| {
                    let saved = &this.state.saved;
                    // Discovery needs launch configuration, never a transcript,
                    // recovery checkpoints or private composer attachments.
                    let mut request = SavedSession::empty(
                        saved.key.clone(),
                        saved.provider,
                        saved.project.clone(),
                        saved.cwd.clone(),
                    );
                    request.roots.clone_from(&saved.roots);
                    request.config_args.clone_from(&saved.config_args);
                    request.native_config_version = saved.native_config_version;
                    (
                        request,
                        this.is_deferred().then_some(this.catalog_generation),
                    )
                }) else {
                    break;
                };
                let options = args.clone();
                let old = previous.clone();
                let (revision, catalog) = cx
                    .background_spawn(async move {
                        if draft_generation.is_some() {
                            grove_core::env_path::wait_for_initial_environment();
                        }
                        let inputs = composer_catalog::inputs(&request, &options);
                        let key = (Scope::new(&request, &options), inputs.revision());
                        if draft_generation.is_some() {
                            if let Err(missing) = request.provider.agent().check_executable() {
                                return (key, Some(Err(missing.to_string())));
                            }
                        }
                        let catalog =
                            if old.as_ref() != Some(&key) || inputs.models_require_refresh() {
                                Some(composer_catalog::discover_automatically_with_inputs(
                                    request, options, inputs,
                                ))
                            } else {
                                None
                            };
                        (key, catalog)
                    })
                    .await;
                let successful = catalog.as_ref().is_none_or(std::result::Result::is_ok);
                if let Some(result) = catalog {
                    if this
                        .update(cx, |this, cx| {
                            if Scope::new(&this.state.saved, &args) != revision.0 {
                                return;
                            }
                            if let Some(generation) = draft_generation {
                                if result.is_ok()
                                    || this
                                        .state
                                        .saved
                                        .provider
                                        .agent()
                                        .check_executable()
                                        .is_err()
                                {
                                    this.complete_catalog(generation, result, cx);
                                }
                                return;
                            }
                            let Ok(catalog) = result else { return };
                            if this.state.models != catalog.models {
                                this.update_models(catalog.models.to_vec(), cx);
                            }
                            if this.state.skills != catalog.skills {
                                this.state.skills.clone_from(&catalog.skills);
                                this.refreshed_skills = Some(catalog.skills.to_vec());
                                let _ = this
                                    .handle
                                    .send(Action::UpdateSkills(catalog.skills.to_vec()));
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                if successful {
                    previous = Some(revision);
                } else if draft_generation.is_some() {
                    previous = None;
                }
                let periods = if successful { 1 } else { 6 };
                for _ in 0..periods {
                    let tick = cx.update(skill_clock::wait);
                    if tick.await.is_err() {
                        return;
                    }
                }
            }
        }));
    }
    pub(crate) fn dashboard_launch_pending(&self) -> bool {
        self.dashboard_launch.is_some()
    }
    #[cfg(test)]
    pub(crate) fn begin_dashboard_handoff_for_test(&mut self) {
        self.dashboard_launch = Some(DashboardLaunch::Starting);
    }
    pub(crate) fn dashboard_submission_ready(&self) -> bool {
        self.dashboard_launch == Some(DashboardLaunch::Ready)
    }
    fn accept_snapshot(&mut self, mut state: Snapshot) {
        if let Some(skills) = &self.refreshed_skills {
            if &state.skills == skills {
                self.refreshed_skills = None;
            } else {
                state.skills = skills.clone().into();
            }
        }
        if let Some(launch) = self.dashboard_launch {
            match state.status {
                codex::Status::Idle
                    if state.error.is_none() && state.rejected_submission.is_none() =>
                {
                    // Readiness is an internal handoff, not a user-visible idle period.
                    if launch == DashboardLaunch::Starting {
                        self.dashboard_launch = Some(DashboardLaunch::Ready);
                    }
                    state.status = codex::Status::Working;
                }
                codex::Status::Starting => {}
                _ => self.dashboard_launch = None,
            }
        }
        if let Some(title) = &self.dashboard_title_waiting {
            if (state.title_generation_pending && state.saved.title.is_some())
                || state.saved.title_generation.is_some()
                || state.rejected_submission.is_some()
                || matches!(
                    state.status,
                    codex::Status::Failed
                        | codex::Status::Disconnected
                        | codex::Status::Interrupted
                )
            {
                self.dashboard_title_waiting = None;
            } else {
                state.title_generation_pending = true;
                if state.saved.title.is_none() {
                    state.saved.title = Some(title.clone());
                }
            }
        }
        self.state = state.into();
    }
    pub(crate) fn cancel_dashboard_title_waiting(&mut self) {
        if self.dashboard_launch.take() == Some(DashboardLaunch::Ready) {
            self.state.status = codex::Status::Idle;
        }
        if self.dashboard_title_waiting.take().is_some() {
            self.state.title_generation_pending = false;
        }
    }
    pub fn send(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.restored.is_some() {
            if matches!(action, Action::Reconnect) && self.state.status == codex::Status::Failed {
                if let Some(restore) = self.restored.as_mut() {
                    restore.loading = false;
                }
            }
            self.ensure_connected(cx);
        }
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent())
            && matches!(
                &action,
                Action::Prompt(_)
                    | Action::PromptWithImages { .. }
                    | Action::PromptInline { .. }
                    | Action::Submit { .. }
                    | Action::SubmitWithMode { .. }
                    | Action::ProjectPrompt { .. }
                    | Action::ActivityPrompt { .. }
                    | Action::ResumeQueue
                    | Action::SteerQueued(_)
                    | Action::Compact
                    | Action::Reconnect
                    | Action::ClearConversation
                    | Action::ApprovePlan { .. }
                    | Action::SendPlanResponse { .. }
            )
        {
            self.state.error=Some("Wait for this provider’s authentication action to finish. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        if self.is_deferred() {
            match action {
                Action::SetTitle(title) => {
                    grove_core::session_title::set_manual_title(&mut self.state.saved, &title);
                    self.state.title_generation_pending = false;
                    self.dashboard_title_waiting = None;
                }
                Action::RegenerateTitle => {
                    grove_core::session_title::request_regeneration(&mut self.state.saved);
                }
                Action::Draft(text) => self.state.saved.draft = text,
                Action::SideChat { checkout, chat } => {
                    self.state.saved.side_chats.insert(checkout, chat);
                }
                Action::SideChatDraft { checkout, draft } => {
                    self.state
                        .saved
                        .side_chats
                        .entry(checkout)
                        .or_default()
                        .draft = draft;
                }
                Action::DraftImages(images) => self.state.saved.draft_images = images,
                Action::DraftReference(reference) => self.state.saved.draft_reference = reference,
                Action::Configure {
                    model,
                    effort,
                    access,
                } => {
                    if let Some(model) = model {
                        self.state.model = Some(model);
                    }
                    if let Some(effort) = effort {
                        self.state.effort = Some(effort);
                    }
                    if let Some(access) = access {
                        self.state.access = Some(access);
                    }
                    let state = &mut *self.state;
                    state.saved.composer.model.clone_from(&state.model);
                    state.saved.composer.effort.clone_from(&state.effort);
                    state.saved.composer.access.clone_from(&state.access);
                }
                Action::SetSpeed(speed) => self.state.saved.composer.speed = Some(speed),
                Action::ResetModelEffort => {
                    self.state.model = None;
                    self.state.effort = None;
                    self.state.saved.composer.model = None;
                    self.state.saved.composer.effort = None;
                    self.state.saved.composer.reset_to_configured = true;
                    self.state.saved.composer.reset_model_effort_only = true;
                }
                Action::ResetConfiguration => {
                    self.state.model = None;
                    self.state.effort = None;
                    self.state.access = None;
                    self.state.saved.composer = codex::ComposerSettings {
                        reset_to_configured: true,
                        ..Default::default()
                    };
                }
                Action::Metadata { project, roots } => {
                    self.state.saved.project = project;
                    self.state.saved.roots = roots;
                }
                _ => {
                    self.state.error = Some(
                        "Launch this Dashboard draft before using conversation actions.".into(),
                    );
                }
            }
            self.revalidate_catalog_selection();
            cx.notify();
            return;
        }
        if let Err(e) = self.handle.send(action) {
            self.state.error = Some(e);
            cx.notify();
        }
    }
    /// Project actions use the provider's visible queue and leave all composer data intact.
    pub fn project_prompt(&mut self, text: String, cx: &mut Context<Self>) -> Result<u64, String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish.".into());
        }
        codex::validate_prompt(&text, &[])?;
        if self.state.status == codex::Status::Disconnected
            || (self.state.saved.provider == codex::Provider::Codex
                && self.state.saved.thread_id.is_none())
        {
            return Err("Reconnect before sending a project prompt".into());
        }
        let id = self.handle.project_prompt(text)?;
        self.state.clear_error();
        cx.notify();
        Ok(id)
    }
    /// Independent Activity input never writes the full conversation's composer.
    pub fn activity_prompt(
        &mut self,
        text: String,
        mode: codex::SubmissionMode,
        expected_turn: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish.".into());
        }
        codex::validate_prompt(&text, &[])?;
        if self.is_deferred()
            || self.state.status == codex::Status::Disconnected
            || self.state.status == codex::Status::Starting
            || self.state.compacting
            || self.state.saved.pending_edit.is_some()
        {
            return Err("Open the session and finish recovery before replying.".into());
        }
        let id = self.handle.activity_prompt(text, mode, expected_turn)?;
        self.state.clear_error();
        cx.notify();
        Ok(id)
    }
    pub fn prompt(&mut self, text: String, cx: &mut Context<Self>) -> Result<(), String> {
        self.queue_prompt(text, Vec::new(), false, cx).map(|_| ())
    }
    pub fn prompt_with_images(
        &mut self,
        text: String,
        images: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        self.queue_prompt(text, images, true, cx)
    }
    pub fn submit_with_mode(
        &mut self,
        text: String,
        images: Vec<String>,
        mode: codex::SubmissionMode,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if mode == codex::SubmissionMode::Send {
            return self.prompt_with_images(text, images, cx);
        }
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish. Your draft is preserved.".into());
        }
        codex::validate_prompt(&text, &images)?;
        if (self.state.saved.provider == codex::Provider::Codex
            && self.state.saved.thread_id.is_none())
            || self.state.status == codex::Status::Disconnected
            || (self.state.status == codex::Status::Starting
                && !(self.state.saved.provider == codex::Provider::Claude
                    && mode == codex::SubmissionMode::Queue))
        {
            return Err("Reconnect before sending a prompt".into());
        }
        if mode == codex::SubmissionMode::Steer && self.state.turn_id.is_none() {
            return Err(format!(
                "Wait for an active {} turn before steering",
                self.state.saved.provider.label()
            ));
        }
        let id = self.handle.submit_with_mode(text, images, mode)?;
        self.state.clear_error();
        cx.notify();
        Ok(id)
    }
    /// Start a fresh provider conversation in the existing pane.
    pub fn clear_conversation(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish.".into());
        }
        if self.state.status == codex::Status::Disconnected {
            return Err(codex::CLEAR_DISCONNECTED.into());
        }
        if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting {
            return Err(codex::CLEAR_BUSY.into());
        }
        self.handle.send(Action::ClearConversation)?;
        self.state.clear_error();
        cx.notify();
        Ok(())
    }
    pub fn resume_queue(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish.".into());
        }
        if self.state.status == codex::Status::Disconnected {
            return Err("Reconnect before resuming queued inputs".into());
        }
        if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting {
            return Err("Wait for the current turn before resuming queued inputs".into());
        }
        if self.state.queued_prompts.is_empty() {
            return Err("There are no queued prompts to send.".into());
        }
        self.handle.send(Action::ResumeQueue)?;
        self.state.clear_error();
        cx.notify();
        Ok(())
    }
    /// Host `/compact`. Rejected inline, never sent, while a turn is active.
    pub fn compact(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish.".into());
        }
        if self.state.status == codex::Status::Disconnected {
            return Err(codex::COMPACT_DISCONNECTED.into());
        }
        if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting {
            return Err(codex::COMPACT_BUSY.into());
        }
        if self.state.saved.thread_id.is_none() || !codex::has_conversation(&self.state.saved.items)
        {
            return Err(codex::NOTHING_TO_COMPACT.into());
        }
        self.handle.send(Action::Compact)?;
        self.state.clear_error();
        cx.notify();
        Ok(())
    }
    fn queue_prompt(
        &mut self,
        text: String,
        images: Vec<String>,
        tracked: bool,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if grove_core::connections::credentials_busy(self.state.saved.provider.agent()) {
            return Err("Wait for this provider’s authentication action to finish. Your draft is preserved.".into());
        }
        codex::validate_prompt(&text, &images)?;
        let dashboard_ready = self.dashboard_submission_ready();
        if (self.state.saved.provider == codex::Provider::Codex
            && self.state.saved.thread_id.is_none())
            || matches!(
                self.state.status,
                codex::Status::Starting | codex::Status::Disconnected
            )
        {
            return Err("Reconnect before sending a prompt".into());
        }
        if (!dashboard_ready && self.state.status.busy()) || self.state.turn_id.is_some() {
            return Err(format!(
                "Wait for or interrupt the current {} turn",
                self.state.saved.provider.label()
            ));
        }
        // Reference sends use a serialized prompt while the saved composer
        // keeps its editable raw text and separate quote for retry/reconnect.
        if self
            .state
            .saved
            .draft_reference
            .as_ref()
            .is_none_or(|reference| reference.serialize(&self.state.saved.draft) != text)
        {
            self.state.saved.draft.clone_from(&text);
        }
        self.state.saved.draft_images.clone_from(&images);
        let submission = if tracked {
            self.handle.submit(text, images)?
        } else {
            self.handle.send(Action::Prompt(text))?;
            0
        };
        self.state.clear_error();
        if dashboard_ready {
            self.dashboard_launch = Some(DashboardLaunch::Submitted);
        }
        self.state.status = codex::Status::Working;
        cx.notify();
        Ok(submission)
    }
}
impl Drop for CodexSession {
    fn drop(&mut self) {
        // Preserve an explicit queued archive request ahead of shutdown's
        // preservation request. A dormant actor must drain both without a CLI.
        self.handle.stop(false);
        if let Some(restore) = self.restored.take() {
            let key = self.state.saved.key.clone();
            let project = self.state.saved.project.clone();
            let roots = self.state.saved.roots.clone();
            let draft = restore.draft_changed.then(|| {
                (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                    self.state.saved.draft_reference.clone(),
                )
            });
            std::thread::spawn(move || match codex::load_active_session(&key) {
                Ok(mut saved) => {
                    saved.project = project;
                    saved.roots = roots;
                    if let Some((text, images, reference)) = draft {
                        saved.draft = text;
                        saved.draft_images = images;
                        saved.draft_reference = reference;
                    }
                    let _connection = codex::connect_deferred_checked(
                        saved,
                        restore.args,
                        restore.connection,
                        restore.validation,
                    );
                }
                Err(error) => restore
                    .connection
                    .reject_pending(format!("Could not close restored session: {error}")),
            });
        }
    }
}

#[cfg(test)]
mod dashboard_tests;
