//! Managed launch, adoption, restoration, and provider session identity.

use super::*;

impl Runtime {
    /// Spawn by concrete project name + worktree path; the palette's target may live in a project with no cached snapshot position.
    pub fn spawn_session_in(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        let args = {
            let store = &cx.global::<SettingsState>().store;
            agent.launch_args_for_backend(
                native_codex_enabled(store, None),
                store.yolo_enabled,
                store.chrome_enabled.unwrap_or(false),
            )
        };
        self.spawn_session_in_with_args(name, cwd, agent, args, cx)
    }

    /// Spawn one agent session with a primary worktree and agent-specific extra arguments.
    pub fn spawn_session_in_with_args(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.spawn_session_in_with_context(name, cwd, agent, args, Vec::new(), None, cx)
    }

    /// Spawn one agent session with its ordered writable worktree context.
    #[allow(clippy::too_many_arguments)] // Preserve the existing launch API during UI extraction.
    pub fn spawn_session_in_with_context(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        context_roots: Vec<grove_core::session_meta::ContextRoot>,
        temp_bundle_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.provider_updates.read(cx).is_updating(agent)
            || grove_core::connections::credentials_busy(agent)
        {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            self.toast.update(cx, |toast, cx| {
                toast.set_error(
                    "This provider is updating or changing credentials. Try again after verification completes.",
                    cx,
                );
            });
            return false;
        }
        if self.pending_managed_launch.is_some() {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            return false;
        }
        let launch_error = std::iter::once(cwd.as_str())
            .chain(context_roots.iter().map(|root| root.wt_path.as_str()))
            .find_map(|wt| match self.cached_checkout_owner(wt, cx) {
                Ok(project) if self.projects.read(cx).is_removing(&project.path) => {
                    Some("Cannot launch a session while this project is being removed.".to_string())
                }
                Ok(_) if self.projects.read(cx).is_worktree_removing(wt) => Some(
                    "Cannot launch a session while this worktree is being removed.".to_string(),
                ),
                Ok(_) => None,
                Err(error) => Some(error),
            });
        if let Some(error) = launch_error {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
            return false;
        }
        if matches!(agent, Agent::Codex | Agent::Claude)
            && native_codex_enabled(&cx.global::<SettingsState>().store, None)
        {
            return self.spawn_native_codex(
                PendingManagedLaunch {
                    name,
                    cwd,
                    agent,
                    args,
                    context_roots,
                    temp_bundle_path,
                    record_recent: false,
                },
                cx,
            );
        }
        if should_choose_backend(
            cx.global::<SettingsState>().store.tmux_enabled,
            grove_core::tmux::available,
        ) {
            self.queue_backend_choice(
                PendingManagedLaunch {
                    name,
                    cwd,
                    agent,
                    args,
                    context_roots,
                    temp_bundle_path,
                    record_recent: false,
                },
                cx,
            );
            return false;
        }
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
        let use_tmux = managed_session_uses_tmux(cx.global::<SettingsState>().store.tmux_enabled);
        let (id, extra_args, state_file, target) = self.registry.update(cx, |r, cx| {
            let id = r.insert_meta_with_context(
                name.clone(),
                cwd.clone(),
                agent,
                context_roots.clone(),
                temp_bundle_path.clone(),
            );
            let label = r.meta(id).map_or_else(String::new, |m| m.label.clone());
            let extra_args = r.take_attention_args(id);
            let state_file = r.attention_files(id).map(|f| f.state_file.clone());
            cx.notify();
            (
                id,
                extra_args,
                state_file,
                crate::entities::session_registry::SpawnTarget {
                    cwd,
                    agent,
                    project: name,
                    label,
                    args,
                    context_roots,
                    temp_bundle_path: temp_bundle_path.clone(),
                    use_tmux,
                },
            )
        });
        let validation_policy = if target.context_roots.is_empty() {
            CheckoutValidation::RegisteredFolder
        } else {
            CheckoutValidation::GitInventory
        };
        let validation_projects = cx.global::<SettingsState>().store.projects.clone();
        let validation_expected = expected_project_paths(
            &validation_projects,
            &std::iter::once(target.project.as_str())
                .chain(
                    target
                        .context_roots
                        .iter()
                        .map(|root| root.project.as_str()),
                )
                .collect::<Vec<_>>(),
            validation_policy,
        );
        let validation_paths: Vec<_> = std::iter::once(std::path::PathBuf::from(&target.cwd))
            .chain(
                target
                    .context_roots
                    .iter()
                    .map(|root| root.wt_path.clone().into()),
            )
            .collect();
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_checked(
                &target,
                &extra_args,
                state_file.as_deref(),
                Box::new(move || {
                    let expected = validation_expected.as_ref().map_err(Clone::clone)?;
                    validate_checkout_paths(
                        &validation_projects,
                        &validation_paths,
                        validation_policy,
                        expected,
                    )
                }),
                cx,
            )
        });
        let tmux_name = match session.read(cx).backend() {
            crate::entities::terminal_session::Backend::Tmux { name } => Some(name.clone()),
            crate::entities::terminal_session::Backend::Native => None,
        };
        let tmux_backed = tmux_name.is_some();
        let spawn_error = session.read(cx).spawn_error().map(str::to_string);
        if let Some(e) = spawn_error.as_deref() {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            crate::telemetry::track("error", vec![("kind", "spawn_failed".into())]);
            let msg = format!("failed to start session: {e}");
            self.toast.update(cx, |t, cx| t.set_error(msg, cx));
        }
        self.registry.update(cx, |r, cx| {
            r.attach_observed(id, session, tmux_name, cx);
            cx.notify();
        });
        if spawn_error.is_none() {
            let (open, open_tmux) = {
                let r = self.registry.read(cx);
                (
                    r.len() as u64,
                    r.all().iter().filter(|m| m.tmux).count() as u64,
                )
            };
            crate::telemetry::track(
                "session_created",
                vec![
                    ("agent", agent.label().into()),
                    ("tmux", tmux_backed.into()),
                    ("open_sessions", open.into()),
                    ("open_native", (open - open_tmux).into()),
                    ("open_tmux", open_tmux.into()),
                ],
            );
        }
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
        spawn_error.is_none()
    }

    pub(crate) fn fork_session(
        &mut self,
        source: SessionId,
        boundary: usize,
        restore_user: bool,
        cx: &mut Context<Self>,
    ) -> Result<SessionId, String> {
        let session = self
            .registry
            .read(cx)
            .codex(source)
            .ok_or("Session is no longer available.")?;
        let state = &session.read(cx).state;
        if !state.can_fork_at(boundary, restore_user) {
            return Err("This conversation boundary is unavailable for forking.".into());
        }
        let saved = state.saved.fork_at(boundary, restore_user)?;
        self.reopen_session_history(saved, cx)
    }

    fn attach_native_codex(
        &mut self,
        saved: grove_core::codex::SavedSession,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> SessionId {
        self.attach_native_codex_inner(saved, args, false, cx)
    }

    fn attach_native_codex_inner(
        &mut self,
        saved: grove_core::codex::SavedSession,
        args: Vec<String>,
        restored: bool,
        cx: &mut Context<Self>,
    ) -> SessionId {
        let id = self.registry.update(cx, |registry, _| {
            registry.insert_meta_with_context(
                saved.project.clone(),
                saved.cwd.clone(),
                saved.provider.agent(),
                saved.roots.clone(),
                None,
            )
        });
        let validation_projects = cx.global::<SettingsState>().store.projects.clone();
        let validation_paths: Vec<_> = std::iter::once(std::path::PathBuf::from(&saved.cwd))
            .chain(saved.roots.iter().map(|root| root.wt_path.clone().into()))
            .collect();
        let validation_policy = if saved.project.is_empty() {
            CheckoutValidation::ProjectlessDirectory
        } else if saved.roots.is_empty() {
            CheckoutValidation::RegisteredFolder
        } else {
            CheckoutValidation::GitInventory
        };
        let validation_expected = expected_project_paths(
            &validation_projects,
            &std::iter::once(saved.project.as_str())
                .chain(saved.roots.iter().map(|root| root.project.as_str()))
                .collect::<Vec<_>>(),
            validation_policy,
        );
        let validation = std::sync::Arc::new(move || {
            let expected = validation_expected.as_ref().map_err(Clone::clone)?;
            validate_checkout_paths(
                &validation_projects,
                &validation_paths,
                validation_policy,
                expected,
            )
        });
        let session = cx.new(|cx| {
            if restored {
                crate::entities::codex_session::CodexSession::restored_checked(
                    saved, args, validation, cx,
                )
            } else {
                crate::entities::codex_session::CodexSession::new_checked(
                    saved, args, validation, cx,
                )
            }
        });
        let handle = session.read(cx).handle.clone();
        self.registry.update(cx, |registry, cx| {
            registry.take_attention_args(id);
            registry.attach_codex(id, session.clone(), handle);
            cx.notify();
        });
        self.observe_native_title(id, &session, cx);
        id
    }
    /// Reopen a retained native transcript without changing its provider,
    /// permissions, draft, or checkout identity. Browsing history never calls this.
    pub(crate) fn reopen_session_history(
        &mut self,
        mut saved: grove_core::codex::SavedSession,
        cx: &mut Context<Self>,
    ) -> Result<SessionId, String> {
        if !native_codex_enabled(&cx.global::<SettingsState>().store, None) {
            return Err("Enable native agents before reopening this session.".into());
        }
        if self.registry.read(cx).has_stopping_provider_sessions() {
            return Err("Wait for the closing session to finish, then try again.".into());
        }
        if let Some(id) = self.registry.read(cx).all().iter().find_map(|meta| {
            self.registry
                .read(cx)
                .codex(meta.id)
                .filter(|session| session.read(cx).state.saved.key == saved.key)
                .map(|_| meta.id)
        }) {
            return Ok(id);
        }
        let projects = &cx.global::<SettingsState>().store.projects;
        let owner = if saved.project.is_empty() {
            None
        } else {
            Some(grove_core::session_meta::project_owner(
                projects, &saved.cwd,
            )?)
        };
        if !std::path::Path::new(&saved.cwd).is_dir() {
            return Err("Restore the session directory before reopening the session.".into());
        }
        for wt in owner
            .iter()
            .map(|_| saved.cwd.as_str())
            .chain(saved.roots.iter().map(|root| root.wt_path.as_str()))
        {
            let project = grove_core::session_meta::project_owner(projects, wt)?;
            if project.archived || !std::path::Path::new(wt).is_dir() {
                return Err(
                    "Restore every project and checkout before reopening the session.".into(),
                );
            }
            if self.projects.read(cx).is_removing(&project.path) {
                return Err("Cannot reopen a session while its project is being removed.".into());
            }
            if self.projects.read(cx).is_worktree_removing(wt) {
                return Err("Cannot reopen a session while its worktree is being removed.".into());
            }
        }
        if self
            .provider_updates
            .read(cx)
            .is_updating(saved.provider.agent())
            || grove_core::connections::credentials_busy(saved.provider.agent())
        {
            return Err("This provider is updating or changing credentials. Try again when verification finishes.".into());
        }
        if let Some(error) = crate::entities::codex_session::workspace_restore_error(
            &std::iter::once(std::path::PathBuf::from(&saved.cwd))
                .chain(saved.roots.iter().map(|root| root.wt_path.clone().into()))
                .collect::<Vec<_>>(),
            Some(&saved.key),
        ) {
            return Err(error);
        }
        if let Some(owner) = owner {
            saved.project.clone_from(&owner.name);
        }
        grove_core::codex::prepare_history_reopen(&saved)?;
        let key = saved.key.clone();
        let id = self.attach_native_codex(saved, Vec::new(), cx);
        if let Err(error) = grove_core::codex::finish_history_reopen(&key) {
            // A duplicate archive is recoverable; the active key remains authoritative.
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
        }
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |state, cx| {
            state.select_session(id, &snap);
            cx.notify();
        });
        Ok(id)
    }
    /// Register the exact Dashboard session entity so its composer and process retain ownership.
    /// A description is a normal retained conversation with its own stable key.
    pub(crate) fn start_change_session(
        &mut self,
        saved: grove_core::codex::SavedSession,
        cx: &mut Context<Self>,
    ) -> Result<SessionId, String> {
        self.start_description_session(saved, cx)
    }

    pub(crate) fn start_description_session(
        &mut self,
        saved: grove_core::codex::SavedSession,
        cx: &mut Context<Self>,
    ) -> Result<SessionId, String> {
        if !native_codex_enabled(&cx.global::<SettingsState>().store, None) {
            return Err("Enable native agents before describing changes.".into());
        }
        if self
            .provider_updates
            .read(cx)
            .is_updating(saved.provider.agent())
            || grove_core::connections::credentials_busy(saved.provider.agent())
        {
            return Err(
                "This provider is updating or changing credentials. Try again when it finishes."
                    .into(),
            );
        }
        if !std::path::Path::new(&saved.cwd).is_dir() {
            return Err("The selected checkout is no longer available.".into());
        }
        if let Ok(project) = grove_core::session_meta::project_owner(
            &cx.global::<SettingsState>().store.projects,
            &saved.cwd,
        ) {
            if self.projects.read(cx).is_removing(&project.path)
                || self.projects.read(cx).is_worktree_removing(&saved.cwd)
            {
                return Err(
                    "Cannot start a description while its checkout is being removed.".into(),
                );
            }
        }
        let settings = &cx.global::<SettingsState>().store;
        let args = saved.provider.agent().launch_args_for_backend(
            true,
            settings.yolo_enabled,
            settings.chrome_enabled.unwrap_or(false),
        );
        saved.provider.launch_options(&args)?;
        saved.persist_recovery()?;
        let id = self.attach_native_codex(saved, args, cx);
        Ok(id)
    }

    pub(crate) fn adopt_dashboard_session(
        &mut self,
        session: Entity<crate::entities::codex_session::CodexSession>,
        cx: &mut Context<Self>,
    ) -> SessionId {
        let saved = session.read(cx).state.saved.clone();
        let handle = session.read(cx).handle.clone();
        let id = self.registry.update(cx, |registry, cx| {
            let id = registry.insert_meta_with_context(
                saved.project,
                saved.cwd,
                saved.provider.agent(),
                saved.roots,
                None,
            );
            registry.take_attention_args(id);
            registry.attach_codex(id, session.clone(), handle);
            cx.notify();
            id
        });
        self.observe_native_title(id, &session, cx);
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |state, cx| {
            state.select_session(id, &snap);
            cx.notify();
        });
        id
    }
    pub(super) fn observe_native_title(
        &mut self,
        id: SessionId,
        session: &Entity<crate::entities::codex_session::CodexSession>,
        cx: &mut Context<Self>,
    ) {
        let title = session.read(cx).state.saved.title.clone();
        self.registry.update(cx, |registry, cx| {
            if registry.set_restored_title(id, title) {
                cx.notify();
            }
        });
        let observer = cx.observe(session, move |runtime, session, cx| {
            let title = session.read(cx).state.saved.title.clone();
            runtime.registry.update(cx, |registry, cx| {
                if registry.set_restored_title(id, title) {
                    cx.notify();
                }
            });
        });
        self.native_title_observers.insert(id, observer);
    }
    fn spawn_native_codex(&mut self, launch: PendingManagedLaunch, cx: &mut Context<Self>) -> bool {
        let PendingManagedLaunch {
            name: project,
            cwd,
            args,
            context_roots: roots,
            agent,
            ..
        } = launch;
        if self.provider_updates.read(cx).is_updating(agent)
            || grove_core::connections::credentials_busy(agent)
        {
            self.toast.update(cx, |toast, cx| {
                toast.set_error(
                    "This provider is updating or changing credentials. Try again after verification completes.",
                    cx,
                );
            });
            return false;
        }
        let provider = if agent == Agent::Claude {
            grove_core::codex::Provider::Claude
        } else {
            grove_core::codex::Provider::Codex
        };
        if let Err(error) = provider.launch_options(&args) {
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
            return false;
        }
        let key = match grove_core::identity::random_id() {
            Ok(key) => key,
            Err(error) => {
                self.toast
                    .update(cx, |toast, cx| toast.set_error(error, cx));
                return false;
            }
        };
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Explicit,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key,
            project,
            cwd,
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots,
            thread_id: None,
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let id = self.attach_native_codex(saved, args, cx);
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |state, cx| {
            state.select_session(id, &snap);
            cx.notify();
        });
        true
    }
    pub(super) fn restore_native_codex(&mut self, cx: &mut Context<Self>) {
        let ready = crate::app::startup_ready(cx);
        let load = cx.background_spawn(async move {
            ready.await;
            grove_core::codex::load_sessions_metadata()
        });
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let _ = this.update(cx, |runtime, cx| {
                runtime.restoring_codex = false;
                runtime
                    .provider_updates
                    .update(cx, |updates, cx| updates.set_restoring(false, cx));
                if !native_codex_enabled(&cx.global::<SettingsState>().store, None) {
                    return;
                }
                match loaded {
                    Ok(recovery) => {
                        if !recovery.warnings.is_empty() {
                            runtime.toast.update(cx, |toast, cx| {
                                toast.set_error(recovery.warnings.join("\n"), cx);
                            });
                        }
                        for mut saved in recovery.sessions {
                            let projects = &cx.global::<SettingsState>().store.projects;
                            if restore_session_project(&mut saved, projects) {
                                runtime.attach_native_codex_inner(saved, Vec::new(), true, cx);
                            }
                        }
                    }
                    Err(error) => runtime
                        .toast
                        .update(cx, |toast, cx| toast.set_error(error, cx)),
                }
            });
        })
        .detach();
    }

    pub fn has_pending_managed_launch(&self) -> bool {
        self.pending_managed_launch.is_some()
    }

    pub(super) fn queue_backend_choice(
        &mut self,
        launch: PendingManagedLaunch,
        cx: &mut Context<Self>,
    ) {
        self.pending_managed_launch = Some(launch);
        cx.emit(RuntimeEvent::BackendChoiceRequested);
    }

    pub fn cancel_pending_managed_launch(&mut self) {
        if let Some(pending) = self.pending_managed_launch.take() {
            if let Some(path) = pending.temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
        }
    }

    /// The caller saves the backend preference first. Taking the request makes
    /// repeated confirmation clicks harmless and preserves its original args.
    pub fn resume_pending_managed_launch(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_managed_launch.take() else {
            return false;
        };
        let (name, cwd, agent, record_recent) = (
            pending.name.clone(),
            pending.cwd.clone(),
            pending.agent,
            pending.record_recent,
        );
        let launched = self.spawn_session_in_with_context(
            pending.name,
            pending.cwd,
            pending.agent,
            pending.args,
            pending.context_roots,
            pending.temp_bundle_path,
            cx,
        );
        if launched && record_recent {
            self.record_recent_launch(name, cwd, agent, cx);
        }
        launched
    }
}
