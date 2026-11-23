//! Dormant session restoration and activation without replacing its entity.

use super::*;

impl CodexSession {
    /// A retained transcript for read-only History. No actor, capability
    /// discovery, transport polling, or disk writes are started.
    pub fn history(saved: SavedSession, _: &mut Context<Self>) -> Self {
        let (handle, receiver) = Handle::channel();
        drop(receiver);
        let mut state = Snapshot::new(saved);
        state.status = codex::Status::Done;
        Self {
            has_transport: false,
            skill_startup_ready: None,
            restored: None,
            restore_load: None,
            state: state.into(),
            handle,
            updates_task: Task::ready(()),
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

    /// A restored registration retains its command identity and metadata but
    /// starts neither a provider process nor transcript hydration while hidden.
    #[cfg(test)]
    pub(super) fn restored(saved: SavedSession, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        Self::restored_checked(saved, args, std::sync::Arc::new(|| Ok(())), cx)
    }

    pub fn restored_checked(
        saved: SavedSession,
        args: Vec<String>,
        validation: codex::StartupValidation,
        cx: &mut Context<Self>,
    ) -> Self {
        let (wake, mut wakes) = futures::channel::mpsc::channel(1);
        let wake = std::sync::Mutex::new(wake);
        let connection = codex::deferred_connection(std::sync::Arc::new(move || {
            let _ = wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_send(());
        }));
        let mut session = Self::history(saved, cx);
        session.state.status = codex::Status::Idle;
        session.handle = connection.handle.clone();
        session.restored = Some(DormantRestore {
            connection,
            validation,
            args,
            loading: false,
            draft_changed: false,
        });
        session.updates_task = cx.spawn(async move |this, cx| {
            while wakes.next().await.is_some() {
                if this.update(cx, Self::ensure_connected).is_err() {
                    break;
                }
            }
        });
        session
    }

    pub fn ensure_connected(&mut self, cx: &mut Context<Self>) {
        let Some(restore) = self.restored.as_mut() else {
            return;
        };
        if restore.loading {
            return;
        }
        restore.loading = true;
        self.state.status = codex::Status::Starting;
        let key = self.state.saved.key.clone();
        let load = cx.background_spawn(async move { codex::load_active_session(&key) });
        self.restore_load = Some(cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(mut saved) => {
                        let Some(restore) = this.restored.take() else {
                            return;
                        };
                        // Startup migration may have repaired ownership after the
                        // persisted head was written; retain the current mapping.
                        saved.project.clone_from(&this.state.saved.project);
                        saved.roots.clone_from(&this.state.saved.roots);
                        if restore.draft_changed {
                            saved.draft.clone_from(&this.state.saved.draft);
                            saved
                                .draft_images
                                .clone_from(&this.state.saved.draft_images);
                            saved
                                .draft_reference
                                .clone_from(&this.state.saved.draft_reference);
                        }
                        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let validated = ready.clone();
                        let validation = std::sync::Arc::new(move || {
                            validated.store(false, std::sync::atomic::Ordering::Release);
                            let result = (restore.validation)();
                            validated.store(result.is_ok(), std::sync::atomic::Ordering::Release);
                            result
                        });
                        let connection = codex::connect_deferred_checked(
                            saved.clone(),
                            restore.args.clone(),
                            restore.connection,
                            validation,
                        );
                        let replacement =
                            Self::from_connection(Snapshot::new(saved), connection, cx);
                        this.replace_restored(replacement);
                        this.skill_startup_ready = Some(ready);
                        this.start_skill_watch(restore.args, cx);
                    }
                    Err(error) => this.restore_failed(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn replace_restored(&mut self, replacement: Self) {
        // The replacement intentionally owns the same command identity. Drop
        // the old view's subscriptions/tasks without stopping its new actor.
        let (inert, receiver) = Handle::channel();
        drop(receiver);
        self.handle = inert;
        *self = replacement;
    }

    pub(super) fn restore_failed(&mut self, error: String) {
        let Some(restore) = self.restored.as_mut() else {
            return;
        };
        restore.loading = false;
        for action in restore.connection.take_pending_actions() {
            match action {
                Action::Draft(text) => {
                    self.state.saved.draft = text;
                    restore.draft_changed = true;
                }
                Action::DraftImages(images) => {
                    self.state.saved.draft_images = images;
                    restore.draft_changed = true;
                }
                Action::DraftReference(reference) => {
                    self.state.saved.draft_reference = reference;
                    restore.draft_changed = true;
                }
                Action::Submit { id, .. }
                | Action::SubmitWithMode { id, .. }
                | Action::ProjectPrompt { id, .. }
                | Action::ActivityPrompt { id, .. } => self.state.rejected_submission = Some(id),
                Action::Stop {
                    done,
                    archive_outcome,
                    ..
                } => {
                    if let Some(outcome) = archive_outcome {
                        let _ = outcome.send(Err(error.clone()));
                    }
                    let _ = done.send(());
                }
                _ => {}
            }
        }
        self.state.status = codex::Status::Failed;
        self.state.error = Some(format!("Could not restore this session: {error}. Queued requests were not sent; try again after resolving recovery."));
    }
}
