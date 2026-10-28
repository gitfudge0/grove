//! UI observable projection of the background Codex actor.
use gpui::{AppContext as _, Context, Task};
use grove_core::codex::{self, Action, Handle, SavedSession, Snapshot};
use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
    time::Duration,
};

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
pub struct CodexSession {
    pub state: SessionSnapshot,
    pub handle: Handle,
    _poll: Task<()>,
    deferred: Option<std::sync::mpsc::Receiver<Action>>,
    catalog_generation: u64,
}
impl CodexSession {
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
            state: state.into(),
            handle,
            _poll: poll,
            deferred: Some(receiver),
            catalog_generation: 0,
        };
        session.refresh_catalog(cwd, args, cx);
        session
    }
    /// Refresh checkout-specific skills/config and provider-advertised models.
    /// A late response cannot replace a newer destination or an activated session.
    pub fn refresh_catalog(&mut self, cwd: String, args: Vec<String>, cx: &mut Context<Self>) {
        if !self.is_deferred() {
            return;
        }
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        // Skills and resolved defaults belong to a checkout/provider pair. Do
        // not expose the previous destination's configuration during refresh.
        self.state.skills.clear();
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
            return;
        }
        let generation = self.catalog_generation;
        let mut saved = self.state.saved.clone();
        saved.cwd = cwd;
        let discovery = cx.background_spawn(async move { codex::discover_composer(saved, args) });
        cx.spawn(async move |this, cx| {
            let result = discovery.await;
            let _ = this.update(cx, |this, cx| {
                if !this.is_deferred() || this.catalog_generation != generation {
                    return;
                }
                match result {
                    Ok(catalog) => {
                        this.state.models = catalog.models;
                        this.state.skills = catalog.skills;
                        this.state.permission_modes = catalog.permission_modes;
                        this.state.resolved_model = catalog.resolved_model;
                        this.state.resolved_effort = catalog.resolved_effort;
                        this.state.resolved_access = catalog.resolved_access;
                        this.state.sandbox = catalog.sandbox;
                        this.state.approval_policy = catalog.approval_policy;
                        this.state.reported_speed = catalog.reported_speed;
                        this.state.speed_disabled_reason = catalog.speed_disabled_reason;
                    }
                    Err(error) => this.state.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn is_deferred(&self) -> bool {
        self.deferred.is_some()
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
        if !std::path::Path::new(&cwd).is_dir() {
            return Err("The selected checkout is no longer available.".into());
        }
        self.drain_deferred(cx);
        let mut saved = self.state.saved.clone();
        saved.project = project;
        saved.cwd = cwd;
        let connection = codex::connect(saved.clone(), args);
        *self = Self::from_connection(Snapshot::new(saved), connection, cx);
        cx.notify();
        Ok(())
    }
    pub fn new(saved: SavedSession, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        let state = Snapshot::new(saved.clone());
        let connection = codex::connect(saved, args);
        Self::from_connection(state, connection, cx)
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
                        this.state = state.into();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            state: state.into(),
            handle,
            _poll: poll,
            deferred: None,
            catalog_generation: 0,
        }
    }
    pub fn send(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.is_deferred() {
            match action {
                Action::Draft(text) => self.state.saved.draft = text,
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
        if (self.state.saved.provider == codex::Provider::Codex
            && self.state.saved.thread_id.is_none())
            || matches!(
                self.state.status,
                codex::Status::Starting | codex::Status::Disconnected
            )
        {
            return Err("Reconnect before sending a prompt".into());
        }
        if self.state.status.busy() || self.state.turn_id.is_some() {
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

    fn saved(provider: codex::Provider) -> SavedSession {
        SavedSession {
            claude_subagents: std::collections::HashMap::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            claude_resume: None,
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            provider,
            key: "dashboard-deferred-test".into(),
            project: "before".into(),
            cwd: "/unused".into(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: Vec::new(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            composer: codex::ComposerSettings::default(),
            context: None,
        }
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
