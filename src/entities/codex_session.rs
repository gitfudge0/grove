//! UI observable projection of the background Codex actor.
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

pub struct CodexSession {
    has_transport: bool,
    pub state: SessionSnapshot,
    pub catalog_loading: bool,
    pub catalog_error: Option<String>,
    pub handle: Handle,
    _poll: Task<()>,
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

    /// A retained transcript for read-only History. No actor, capability
    /// discovery, transport polling, or disk writes are started.
    pub fn history(saved: SavedSession, _: &mut Context<Self>) -> Self {
        let (handle, receiver) = Handle::channel();
        drop(receiver);
        let mut state = Snapshot::new(saved);
        state.status = codex::Status::Done;
        Self {
            has_transport: false,
            state: state.into(),
            handle,
            _poll: Task::ready(()),
            skill_watch: None,
            refreshed_skills: None,
            deferred: None,
            catalog_generation: 0,
            catalog_scope: None,
            catalog_loading: false,
            catalog_error: None,
            catalog_selection_error: false,
            dashboard_title_waiting: None,
            dashboard_launch: None,
        }
    }

    /// A composer-owned draft with no conversation, persistence, or registry entry.
    /// Capability discovery uses a temporary metadata-only provider handshake.
    pub fn draft(saved: SavedSession, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        let cwd = saved.cwd.clone();
        let (handle, receiver) = Handle::channel();
        let mut state = Snapshot::new(saved);
        state.status = codex::Status::Idle;
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
            if this.update(cx, CodexSession::drain_deferred).is_err() {
                break;
            }
        });
        let mut session = Self {
            has_transport: false,
            state: state.into(),
            handle,
            _poll: poll,
            skill_watch: None,
            refreshed_skills: None,
            deferred: Some(receiver),
            catalog_generation: 0,
            catalog_scope: None,
            catalog_loading: false,
            catalog_error: None,
            catalog_selection_error: false,
            dashboard_title_waiting: None,
            dashboard_launch: None,
        };
        // The startup dashboard is created before model browsing. Warm the other
        // provider concurrently; the selected provider is loaded by refresh below.
        if let Some(settings) = cx.try_global::<crate::settings::SettingsState>() {
            let mut other = session.state.saved.clone();
            other.provider = match other.provider {
                codex::Provider::Codex => codex::Provider::Claude,
                codex::Provider::Claude => codex::Provider::Codex,
            };
            let other_args = other.provider.agent().launch_args_for_backend(
                true,
                settings.store.yolo_enabled,
                settings.store.chrome_enabled.unwrap_or(false),
            );
            if std::path::Path::new(&cwd).is_dir() {
                cx.background_spawn(async move { composer_catalog::discover(other, other_args) })
                    .detach();
            }
        }
        session.refresh_catalog(cwd, args, cx);
        session
    }
    /// Refresh checkout-specific skills/config and provider-advertised models.
    /// A late response cannot replace a newer destination or an activated session.
    pub fn refresh_catalog(&mut self, cwd: String, args: Vec<String>, cx: &mut Context<Self>) {
        if !self.is_deferred() {
            return;
        }
        self.start_skill_watch(args.clone(), cx);
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.catalog_loading = true;
        if self.catalog_error.is_some() && self.state.error == self.catalog_error {
            self.state.error = None;
        }
        self.catalog_error = None;
        self.catalog_selection_error = false;
        // Skills and resolved defaults belong to a checkout/provider pair. Do
        // not expose the previous destination's configuration during refresh.
        self.state.skills.clear();
        self.state.models.clear();
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
        self.catalog_scope = Some(Scope::new(&saved, &args));
        if let Some(catalog) = composer_catalog::cached(&saved, &args) {
            self.complete_catalog(generation, Ok(catalog), cx);
            return;
        }
        let discovery = cx.background_spawn(async move { composer_catalog::discover(saved, args) });
        cx.spawn(async move |this, cx| {
            let result = discovery.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_catalog(generation, result, cx);
            });
        })
        .detach();
    }
    pub fn retry_catalog(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        composer_catalog::invalidate(&self.state.saved, &args);
        self.refresh_catalog(self.state.saved.cwd.clone(), args, cx);
    }
    /// Geometry fixtures must not depend on installed CLIs or an asynchronous handshake.
    #[cfg(test)]
    pub(crate) fn settle_draft_catalog_for_test(&mut self, cx: &mut Context<Self>) {
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.catalog_loading = false;
        self.catalog_error = None;
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
        match result {
            Ok(catalog) => {
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
        if let Some(error) = workspace_restore_error(&paths, None) {
            return Err(error);
        }
        if !std::path::Path::new(&cwd).is_dir() {
            return Err("The selected checkout is no longer available.".into());
        }
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
        let connection = codex::connect(saved.clone(), args.clone());
        *self = Self::from_connection(Snapshot::new(saved), connection, cx);
        self.start_skill_watch(args, cx);
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
        let state = Snapshot::new(saved.clone());
        let connection = codex::connect(saved, args.clone());
        let mut session = Self::from_connection(state, connection, cx);
        session.start_skill_watch(args, cx);
        session
    }
    pub(crate) fn from_connection(
        state: Snapshot,
        connection: codex::Connection,
        cx: &mut Context<Self>,
    ) -> Self {
        let handle = connection.handle;
        let poll = cx.spawn(async move |this, cx| loop {
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
            state: state.into(),
            handle,
            _poll: poll,
            skill_watch: None,
            refreshed_skills: None,
            deferred: None,
            catalog_generation: 0,
            catalog_scope: None,
            catalog_loading: false,
            catalog_error: None,
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
                let Ok(saved) = this.read_with(cx, |this, _| this.state.saved.clone()) else {
                    break;
                };
                let request = saved.clone();
                let options = args.clone();
                let old = previous.clone();
                let (revision, catalog) = cx
                    .background_spawn(async move {
                        let revision = composer_catalog::skill_revision_with_args(
                            &request.cwd,
                            &options,
                            &request.config_args,
                        );
                        let key = (Scope::new(&request, &options), revision);
                        let catalog = if old.as_ref() != Some(&key) {
                            Some(composer_catalog::discover(request, options))
                        } else {
                            None
                        };
                        (key, catalog)
                    })
                    .await;
                let successful = catalog.as_ref().is_none_or(std::result::Result::is_ok);
                if let Some(Ok(catalog)) = catalog {
                    if this
                        .update(cx, |this, cx| {
                            if this.state.saved.cwd != saved.cwd
                                || this.state.saved.provider != saved.provider
                            {
                                return;
                            }
                            if this.state.skills != catalog.skills {
                                this.state.skills.clone_from(&catalog.skills);
                                if !this.is_deferred() {
                                    this.refreshed_skills = Some(catalog.skills.clone());
                                    let _ = this.handle.send(Action::UpdateSkills(catalog.skills));
                                }
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
                }
                cx.background_executor()
                    .timer(Duration::from_secs(if successful { 2 } else { 30 }))
                    .await;
            }
        }));
    }
    pub(crate) fn dashboard_submission_ready(&self) -> bool {
        self.dashboard_launch == Some(DashboardLaunch::Ready)
    }
    fn accept_snapshot(&mut self, mut state: Snapshot) {
        if let Some(skills) = &self.refreshed_skills {
            if &state.skills == skills {
                self.refreshed_skills = None;
            } else {
                state.skills.clone_from(skills);
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
        self.handle.stop(false);
    }
}

#[cfg(test)]
mod dashboard_tests {
    use super::*;

    #[gpui::test]
    fn dashboard_view_sends_ready_launch_as_first_turn(cx: &mut gpui::TestAppContext) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|cx| {
                gpui_component::init(cx);
                cx.set_global(crate::settings::SettingsState::new(
                    grove_core::storage::Store::default(),
                ));
            });
            let (handle, actions) = Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let mut initial = Snapshot::new(saved(provider));
            initial.saved.draft = "First dashboard prompt".into();
            let mut session_entity = None;
            let (view, window_cx) = cx.add_window_view(|window, cx| {
                let session = cx.new(|cx| {
                    let mut session = CodexSession::from_connection(
                        initial,
                        codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    );
                    session.dashboard_launch = Some(DashboardLaunch::Starting);
                    session
                });
                session_entity = Some(session.clone());
                crate::views::CodexView::new(session, window, cx)
            });
            window_cx
                .update(|window, cx| view.update(cx, |view, cx| view.launch_dashboard(window, cx)));
            let session = session_entity.unwrap();
            window_cx.update(|_, cx| {
                session.update(cx, |session, cx| {
                    let mut ready = (*session.state).clone();
                    ready.status = codex::Status::Idle;
                    ready.saved.thread_id = Some("ready-thread".into());
                    session.accept_snapshot(ready);
                    assert_eq!(session.state.session_status(), "Working");
                    cx.notify();
                });
            });
            window_cx.run_until_parked();
            let dispatched = actions.try_iter().collect::<Vec<_>>();
            assert_eq!(
                dispatched
                    .iter()
                    .filter(|action| matches!(action, Action::Submit { .. }))
                    .count(),
                1,
                "{provider:?} must send, not enqueue"
            );
            assert_eq!(
                session.read_with(window_cx, |s, _| s.state.session_status()),
                "Working"
            );
        }
    }

    #[gpui::test]
    fn dashboard_launch_never_publishes_ready_before_first_prompt(cx: &mut gpui::TestAppContext) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|cx| {
                let (handle, actions) = Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
                let initial = Snapshot::new(saved(provider));
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        initial.clone(),
                        codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                session.update(cx, |session, cx| {
                    session.dashboard_launch = Some(DashboardLaunch::Starting);
                    let mut ready = initial;
                    ready.status = codex::Status::Idle;
                    ready.saved.thread_id = Some("ready-thread".into());
                    session.accept_snapshot(ready.clone());
                    assert_eq!(session.state.session_status(), "Working", "{provider:?}");
                    assert!(session.dashboard_submission_ready());
                    session
                        .queue_prompt("First prompt".into(), Vec::new(), true, cx)
                        .unwrap();
                    assert!(actions
                        .try_iter()
                        .any(|action| matches!(action, Action::Submit { .. })));
                    // A delayed idle snapshot must not undo the optimistic send state.
                    session.accept_snapshot(ready.clone());
                    assert_eq!(session.state.session_status(), "Working");
                    assert!(!session.dashboard_submission_ready());
                    let mut working = ready.clone();
                    working.status = codex::Status::Working;
                    session.accept_snapshot(working);
                    session.accept_snapshot(ready.clone());
                    assert_eq!(
                        session.state.session_status(),
                        "Ready",
                        "later idle is real"
                    );
                    for status in [
                        codex::Status::Failed,
                        codex::Status::Disconnected,
                        codex::Status::Interrupted,
                    ] {
                        session.dashboard_launch = Some(DashboardLaunch::Starting);
                        let mut failed = ready.clone();
                        failed.status = status;
                        session.accept_snapshot(failed);
                        assert_eq!(session.state.status, status);
                        assert!(session.dashboard_launch.is_none());
                    }
                    session.dashboard_launch = Some(DashboardLaunch::Starting);
                    session.accept_snapshot(ready);
                    session.cancel_dashboard_title_waiting();
                    assert_eq!(session.state.session_status(), "Ready");
                });
            });
        }
    }

    #[gpui::test]
    fn history_snapshot_has_no_transport_or_catalog_for_codex_and_claude(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let record = saved(provider);
                let expected = serde_json::to_value(&record).unwrap();
                let session = cx.new(|cx| CodexSession::history(record, cx));
                let session = session.read(cx);
                assert_eq!(
                    serde_json::to_value(&session.state.saved).unwrap(),
                    expected
                );
                assert_eq!(session.state.status, codex::Status::Done);
                assert!(!session.catalog_loading);
                assert!(session.deferred.is_none());
                assert!(session.handle.send(Action::Reconnect).is_err());
            }
        });
    }

    #[gpui::test]
    fn dashboard_title_remains_pending_through_startup_and_hands_off_or_fails(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|cx| {
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
                let mut starting = Snapshot::new(saved(provider));
                starting.status = codex::Status::Starting;
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        starting.clone(),
                        codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                session.update(cx, |session, _| {
                    session.dashboard_title_waiting = Some("First prompt".into());
                    session.accept_snapshot(starting.clone());
                    assert!(session.state.title_generation_pending);
                    assert_eq!(session.state.saved.title.as_deref(), Some("First prompt"));
                    let mut failed = starting.clone();
                    failed.status = codex::Status::Disconnected;
                    session.accept_snapshot(failed);
                    assert!(!session.state.title_generation_pending);
                    assert!(session.dashboard_title_waiting.is_none());
                    session.dashboard_title_waiting = Some("First prompt".into());
                    let mut actor_pending = starting.clone();
                    actor_pending.title_generation_pending = true;
                    session.accept_snapshot(actor_pending.clone());
                    assert!(session.state.title_generation_pending);
                    assert!(session.dashboard_title_waiting.is_some());
                    let mut rejected = actor_pending.clone();
                    rejected.rejected_submission = Some(42);
                    rejected.title_generation_pending = false;
                    session.accept_snapshot(rejected);
                    assert!(!session.state.title_generation_pending);
                    assert!(session.dashboard_title_waiting.is_none());
                    session.dashboard_title_waiting = Some("First prompt".into());
                    session.accept_snapshot(starting.clone());
                    session.cancel_dashboard_title_waiting();
                    assert!(!session.state.title_generation_pending);
                    session.dashboard_title_waiting = Some("First prompt".into());
                    let mut actor_pending = starting;
                    actor_pending.title_generation_pending = true;
                    actor_pending.saved.title = Some("First prompt".into());
                    session.accept_snapshot(actor_pending.clone());
                    assert!(session.state.title_generation_pending);
                    assert!(session.dashboard_title_waiting.is_none());
                    actor_pending.title_generation_pending = false;
                    session.accept_snapshot(actor_pending);
                    assert!(!session.state.title_generation_pending);
                });
            });
        }
    }

    fn saved(provider: codex::Provider) -> SavedSession {
        SavedSession {
            claude_subagents: std::collections::HashMap::default(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            key: "dashboard-deferred-test".into(),
            project: "before".into(),
            cwd: "/unused".into(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
            composer: codex::ComposerSettings::default(),
            context: None,
            context_details: None,
        }
    }

    #[gpui::test]
    fn refreshed_skills_survive_queued_snapshots_until_actor_acknowledges(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
                let mut snapshot = Snapshot::new(saved(provider));
                snapshot.saved.draft = "keep typing".into();
                snapshot.saved.thread_id = Some("same-thread".into());
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        snapshot.clone(),
                        codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                session.update(cx, |session, _| {
                    let skills = vec![serde_json::json!({"name":"new-skill"})];
                    session.refreshed_skills = Some(skills.clone());
                    session.accept_snapshot(snapshot.clone());
                    assert_eq!(session.state.skills, skills);
                    assert_eq!(session.state.saved.draft, "keep typing");
                    assert_eq!(
                        session.state.saved.thread_id.as_deref(),
                        Some("same-thread")
                    );
                    assert!(session.refreshed_skills.is_some());
                    snapshot.skills = skills;
                    session.accept_snapshot(snapshot.clone());
                    assert!(session.refreshed_skills.is_none());
                    snapshot.skills.clear();
                    session.accept_snapshot(snapshot.clone());
                    assert!(session.state.skills.is_empty());
                });
            }
        });
    }

    #[gpui::test]
    fn deferred_title_edits_preserve_manual_ownership_and_composer(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    session.send(Action::Draft("Keep my draft".into()), cx);
                    session.send(Action::SetTitle("  My session title  ".into()), cx);
                    assert_eq!(
                        session.state.saved.title.as_deref(),
                        Some("My session title")
                    );
                    assert!(session.state.saved.title_generation.is_some());
                    assert_eq!(session.state.saved.draft, "Keep my draft");
                    assert!(session.is_deferred());
                    assert!(!session.state.title_generation_pending);
                    session.send(Action::SetTitle("  ".into()), cx);
                    assert_eq!(
                        session.state.saved.title.as_deref(),
                        Some("My session title")
                    );
                    let restored = serde_json::from_value::<SavedSession>(
                        serde_json::to_value(&session.state.saved).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(restored.title.as_deref(), Some("My session title"));
                    assert_eq!(restored.provider, provider);
                });
            }
        });
    }

    #[gpui::test]
    fn draft_model_selection_keeps_ready_efforts_without_another_handshake(
        cx: &mut gpui::TestAppContext,
    ) {
        let checkout = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                // Start outside a real checkout so this fixture never launches a CLI.
                let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    session.state.saved.cwd = checkout.path().to_string_lossy().into_owned();
                    session.catalog_scope = Some(Scope::new(&session.state.saved, &[]));
                    session.catalog_loading = false;
                    session.catalog_error = None;
                    session.state.models = vec![
                        serde_json::json!({"model":"first", "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}),
                        serde_json::json!({"model":"second", "supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"max"}]}),
                    ];
                    session.state.skills = vec![serde_json::json!({"name":"checkout-skill"})];
                    session.state.resolved_access = Some("read-only".into());
                    session.state.saved.draft = "keep my draft".into();
                    session.state.effort = Some("low".into());
                    session.state.saved.composer.effort = Some("low".into());
                    let generation = session.catalog_generation;
                    session.select_draft_model(provider, "second".into(), Vec::new(), cx).unwrap();
                    assert_eq!(session.catalog_generation, generation);
                    assert!(!session.catalog_loading);
                    assert!(session.catalog_error.is_none());
                    assert_eq!(session.state.models[1]["supportedReasoningEfforts"][1]["reasoningEffort"], "max");
                    assert_eq!(session.state.skills[0]["name"], "checkout-skill");
                    assert_eq!(session.state.resolved_access.as_deref(), Some("read-only"));
                    assert_eq!(session.state.saved.composer.model.as_deref(), Some("second"));
                    assert!(session.state.saved.composer.effort.is_none());
                    assert_eq!(session.state.saved.draft, "keep my draft");
                });
            }
        });
    }

    #[gpui::test]
    fn draft_model_switch_preserves_content_and_resets_provider_configuration(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            for (before, after) in [
                (codex::Provider::Codex, codex::Provider::Claude),
                (codex::Provider::Claude, codex::Provider::Codex),
            ] {
                let session = cx.new(|cx| CodexSession::draft(saved(before), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    let reference = codex::ReferenceDraft {
                        id: "quote".into(),
                        thread_id: Some("source".into()),
                        sources: Vec::new(),
                        text: "keep this quote".into(),
                    };
                    session.send(Action::DraftImages(vec!["image-data".into()]), cx);
                    session.send(Action::DraftReference(Some(reference.clone())), cx);
                    session.send(
                        Action::Configure {
                            model: Some("old-model".into()),
                            effort: Some("high".into()),
                            access: Some("full-access".into()),
                        },
                        cx,
                    );
                    session.send(Action::SetSpeed(codex::Speed::Ultrafast), cx);
                    session
                        .handle
                        .send(Action::Draft("newest prompt".into()))
                        .unwrap();
                    let generation = session.catalog_generation;
                    session
                        .select_draft_model(after, "chosen-model".into(), Vec::new(), cx)
                        .unwrap();
                    assert!(session.is_deferred());
                    assert_eq!(session.state.saved.provider, after);
                    assert_eq!(session.state.saved.label, after.label());
                    assert_eq!(session.state.saved.project, "before");
                    assert_eq!(session.state.saved.cwd, "/unused");
                    assert_eq!(session.state.saved.draft, "newest prompt");
                    assert_eq!(session.state.saved.draft_images, vec!["image-data"]);
                    assert_eq!(session.state.saved.draft_reference, Some(reference));
                    assert_eq!(
                        session.state.saved.composer.model.as_deref(),
                        Some("chosen-model")
                    );
                    assert!(session.state.saved.composer.effort.is_none());
                    assert!(session.state.saved.composer.access.is_none());
                    assert_eq!(
                        session.state.saved.composer.speed,
                        Some(codex::Speed::Standard)
                    );
                    assert_ne!(session.catalog_generation, generation);
                    assert!(session.state.saved.thread_id.is_none());
                });
            }
        });
    }

    #[gpui::test]
    fn draft_model_selection_rejects_running_session_without_changing_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let session =
                cx.new(|cx| CodexSession::draft(saved(codex::Provider::Codex), Vec::new(), cx));
            session.update(cx, |session, cx| {
                // Simulate activation's invariant without opening a provider process.
                session.deferred = None;
                session.state.saved.draft = "keep my task".into();
                let generation = session.catalog_generation;
                let error = session
                    .select_draft_model(codex::Provider::Claude, "other".into(), Vec::new(), cx)
                    .unwrap_err();
                assert!(error.contains("cannot switch providers"));
                assert_eq!(session.state.saved.provider, codex::Provider::Codex);
                assert_eq!(session.state.saved.draft, "keep my task");
                assert!(session.state.saved.composer.model.is_none());
                assert_eq!(session.catalog_generation, generation);
            });
        });
    }

    #[gpui::test]
    fn draft_activation_rejects_pending_failed_and_unadvertised_models(
        cx: &mut gpui::TestAppContext,
    ) {
        let checkout = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    let cwd = checkout.path().to_string_lossy().into_owned();
                    session.state.saved.cwd.clone_from(&cwd);
                    session.state.saved.draft = "retained task".into();
                    session.catalog_error = None;
                    session.catalog_loading = true;
                    let error = session
                        .activate("project".into(), cwd.clone(), Vec::new(), cx)
                        .unwrap_err();
                    assert!(error.contains("finish loading"));
                    session.catalog_loading = false;
                    session.catalog_error = Some("Provider metadata failed".into());
                    let error = session
                        .activate("project".into(), cwd.clone(), Vec::new(), cx)
                        .unwrap_err();
                    assert_eq!(error, "Provider metadata failed");
                    session.catalog_error = None;
                    session
                        .handle
                        .send(Action::Configure {
                            model: Some("missing-model".into()),
                            effort: None,
                            access: None,
                        })
                        .unwrap();
                    session.state.models = vec![serde_json::json!({"model":"available-model"})];
                    let error = session
                        .activate("project".into(), cwd, Vec::new(), cx)
                        .unwrap_err();
                    assert!(error.contains("no longer advertised"));
                    assert!(session.is_deferred());
                    assert_eq!(session.state.saved.provider, provider);
                    assert_eq!(session.state.saved.draft, "retained task");
                    // Ready defaults need no explicit model. An advertised explicit model is valid.
                    session.state.saved.composer.model = None;
                    assert!(session.validate_draft_catalog().is_ok());
                    session.state.saved.composer.model = Some("available-model".into());
                    assert!(session.validate_draft_catalog().is_ok());
                });
            }
        });
    }

    #[gpui::test]
    fn draft_catalog_ignores_superseded_results_and_rejects_disappeared_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let session =
                cx.new(|cx| CodexSession::draft(saved(codex::Provider::Codex), Vec::new(), cx));
            session.update(cx, |session, cx| {
                let old_generation = session.catalog_generation;
                session
                    .select_draft_model(
                        codex::Provider::Claude,
                        "chosen-model".into(),
                        Vec::new(),
                        cx,
                    )
                    .unwrap();
                session.catalog_loading = true;
                session.catalog_error = None;
                let mut stale = Snapshot::new(saved(codex::Provider::Codex));
                stale.models = vec![serde_json::json!({"model":"stale-codex-model"})];
                session.complete_catalog(old_generation, Ok(stale), cx);
                assert!(session.catalog_loading);
                assert!(session.state.models.is_empty());
                assert_eq!(session.state.saved.provider, codex::Provider::Claude);
                session.complete_catalog(old_generation, Err("old failure".into()), cx);
                assert!(session.catalog_error.is_none());
                let mut current = Snapshot::new(saved(codex::Provider::Claude));
                current.models = vec![serde_json::json!({"model":"different-model"})];
                session.complete_catalog(session.catalog_generation, Ok(current), cx);
                assert!(!session.catalog_loading);
                assert!(session
                    .catalog_error
                    .as_deref()
                    .unwrap()
                    .contains("no longer advertised"));
                assert_eq!(
                    session.state.saved.composer.model.as_deref(),
                    Some("chosen-model")
                );
            });
        });
    }

    #[gpui::test]
    fn draft_catalog_selection_recovers_on_reset_but_discovery_failure_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    for action in [
                        Action::ResetModelEffort,
                        Action::ResetConfiguration,
                        Action::Configure {
                            model: Some("available".into()),
                            effort: None,
                            access: None,
                        },
                    ] {
                        session.state.model = Some("removed".into());
                        session.state.saved.composer.model = Some("removed".into());
                        let mut catalog = Snapshot::new(saved(provider));
                        catalog.models = vec![serde_json::json!({"model":"available"})];
                        session.complete_catalog(session.catalog_generation, Ok(catalog), cx);
                        assert!(session.catalog_error.is_some());
                        assert!(session.state.error.is_some());
                        session.send(action, cx);
                        assert!(session.catalog_error.is_none());
                        assert!(session.state.error.is_none());
                        assert!(session.validate_draft_catalog().is_ok());
                    }
                    for action in [Action::ResetModelEffort, Action::ResetConfiguration] {
                        session.complete_catalog(
                            session.catalog_generation,
                            Err("provider discovery failed".into()),
                            cx,
                        );
                        session.send(action, cx);
                        assert_eq!(
                            session.catalog_error.as_deref(),
                            Some("provider discovery failed")
                        );
                        assert_eq!(
                            session.state.error.as_deref(),
                            Some("provider discovery failed")
                        );
                        assert!(session.validate_draft_catalog().is_err());
                    }
                });
            }
        });
    }

    #[gpui::test]
    fn deferred_composer_preserves_provider_options_and_rejects_stale_checkout(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
                session.update(cx, |session, cx| {
                    session.send(Action::Draft("retained task".into()), cx);
                    session.send(
                        Action::DraftImages(vec!["data:image/png;base64,fixture".into()]),
                        cx,
                    );
                    session.send(
                        Action::Configure {
                            model: Some("selected-model".into()),
                            effort: Some("high".into()),
                            access: Some("full-access".into()),
                        },
                        cx,
                    );
                    // The composer also sends directly through Handle. Activation must
                    // flush those commands before reading the launch draft.
                    session
                        .handle
                        .send(Action::Draft("latest direct draft".into()))
                        .unwrap();
                    session.drain_deferred(cx);
                    session.state.skills = vec![serde_json::json!({"name":"old-checkout-skill"})];
                    session.state.resolved_model = Some("old-checkout-model".into());
                    session.state.resolved_access = Some("full-access".into());
                    session.refresh_catalog(
                        "/nonexistent/grove-dashboard-checkout".into(),
                        Vec::new(),
                        cx,
                    );
                    assert!(session.state.skills.is_empty());
                    assert!(session.state.resolved_model.is_none());
                    assert!(session.state.resolved_access.is_none());
                    let error = session
                        .activate(
                            "chosen project".into(),
                            "/nonexistent/grove-dashboard-checkout".into(),
                            Vec::new(),
                            cx,
                        )
                        .unwrap_err();
                    assert!(error.contains("checkout"));
                    assert!(session.is_deferred());
                    assert_eq!(session.state.saved.project, "before");
                    assert_eq!(session.state.saved.draft, "latest direct draft");
                    assert_eq!(session.state.saved.draft_images.len(), 1);
                    assert_eq!(session.state.saved.provider, provider);
                    assert_eq!(
                        session.state.saved.composer.model.as_deref(),
                        Some("selected-model")
                    );
                    assert_eq!(session.state.saved.composer.effort.as_deref(), Some("high"));
                    assert_eq!(
                        session.state.saved.composer.access.as_deref(),
                        Some("full-access")
                    );
                    assert!(session.state.saved.thread_id.is_none());
                });
            }
        });
    }
}
