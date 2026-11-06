//! Ask-specific navigation/adaptation; generation and persistence live in ChangeSession/Store.
use super::*;
use grove_core::{
    change_result_store::{Source, Store},
    codex::SavedSession,
};
impl Sidebar {
    fn question_store(&self) -> Result<Store<Answer>, String> {
        #[cfg(test)]
        if let Some(root) = &self.questions.store_root {
            return Ok(Store::new(root.clone()));
        }
        Store::open()
    }
    fn question_session(
        &self,
        id: SessionId,
        cx: &App,
    ) -> Option<Entity<crate::entities::codex_session::CodexSession>> {
        self.runtime.read(cx).registry.read(cx).codex(id).cloned()
    }
    pub(in crate::views::sidebar) fn linked_question_key(&self) -> Option<Key> {
        if self.dashboard_open {
            return None;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return None;
        };
        self.questions.linked.get(&id).cloned()
    }
    pub(in crate::views::sidebar) fn is_question_session(&self, id: SessionId) -> bool {
        self.questions.linked.contains_key(&id)
    }
    pub(super) fn sync_question_identity(&mut self, id: SessionId, session_key: &str) {
        let previous = self
            .questions
            .identities
            .insert(id, session_key.into())
            .or_else(|| {
                self.questions
                    .linked
                    .get(&id)
                    .and_then(|target| self.questions.entries.get(target))
                    .and_then(|entry| entry.session_key.clone())
            });
        let Some(previous) = previous.filter(|previous| previous != session_key) else {
            return;
        };
        let previous_owner = owner(&Some(Selection::Session(id)));
        self.questions.linked.remove(&id);
        let keys: Vec<_> = self
            .questions
            .entries
            .keys()
            .filter(|target| target.owner == previous_owner)
            .cloned()
            .collect();
        for target in keys {
            if let Some(mut entry) = self.questions.entries.remove(&target) {
                entry.subscription = None;
                entry.controller = None;
                entry.phase = SessionPhase::Idle;
                entry.session = None;
                entry.run = entry.run.wrapping_add(1);
                entry.checking = false;
                entry.checked_at = None;
                // Retain unsaved native input with its old durable conversation identity.
                self.questions
                    .retired_entries
                    .insert((previous.clone(), target.path, target.scope), entry);
            }
        }
        if self
            .questions
            .open
            .as_ref()
            .is_some_and(|target| target.owner == previous_owner)
        {
            self.questions.open = None;
            self.questions.source = None;
        }
        self.questions
            .hydrated
            .retain(|(session, _)| *session != id);
    }
    fn attach_question_controller(
        &mut self,
        id: SessionId,
        session_key: String,
        source: Source,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let session = self.question_session(id, cx)?;
        if session.read(cx).state.saved.key != session_key {
            return None;
        }
        self.sync_question_identity(id, &session_key);
        let store = match self.question_store() {
            Ok(store) => store,
            Err(error) => {
                self.question_error(error, cx);
                return None;
            }
        };
        let target = key(
            &Some(Selection::Session(id)),
            &source.checkout,
            &source.scope,
        );
        if self.questions.entries.get(&target).is_some_and(|e| {
            e.controller.is_some() && e.session_key.as_deref() == Some(session_key.as_str())
        }) {
            return Some(target);
        }
        let controller =
            cx.new(|cx| ChangeSession::<Answer>::new(session, store, session_key.clone(), cx));
        let observed_target = target.clone();
        let subscription = cx.observe(&controller, move |this, controller, cx| {
            let state = controller.read(cx);
            let result = state.result.clone();
            let phase = state.phase;
            let error = state.error.clone();
            if let Some(entry) = this.questions.entries.get_mut(&observed_target) {
                if entry.phase == SessionPhase::Idle && phase.busy() {
                    entry.run = entry.run.wrapping_add(1);
                    entry.checking = false;
                    entry.checked_at = None;
                    entry.freshness = None;
                }
                entry.phase = phase;
                entry.error = error;
                if let Some(answer) =
                    result.filter(|answer| entry.result.as_deref() != Some(answer.as_ref()))
                {
                    if entry.input.is_none() && entry.pending_question.is_none() {
                        entry.pending_question = Some(answer.question.clone());
                    }
                    entry.text = Some(cx.new(|cx| TextViewState::markdown(&answer.markdown(), cx)));
                    entry.result = Some(answer);
                    entry.checked_at = None;
                    entry.checking = false;
                    entry.freshness = None;
                    entry.diff = None;
                }
            }
            cx.notify();
        });
        let preserved = self.questions.retired_entries.remove(&(
            session_key.clone(),
            target.path.clone(),
            target.scope.clone(),
        ));
        let entry = self
            .questions
            .entries
            .entry(target.clone())
            .or_insert_with(|| preserved.unwrap_or_default());
        entry.scope = Some(source.scope);
        entry.session = Some(id);
        entry.session_key = Some(session_key);
        entry.controller = Some(controller);
        entry.subscription = Some(subscription);
        self.questions.linked.insert(id, target.clone());
        Some(target)
    }
    pub(super) fn start_question_conversation(
        &mut self,
        path: String,
        scope: Scope,
        question: String,
        cx: &mut Context<Self>,
    ) {
        let result = (|| -> Result<_, String> {
            let source = Source {
                checkout: path,
                scope,
            }
            .normalized()?;
            let provider = self.checkout_commit_provider(cx)?;
            let template = if self.dashboard_open {
                self.dashboard
                    .as_ref()
                    .map(|d| d.session.read(cx).state.saved.clone())
            } else if let Some(Selection::Session(id)) = self.selection {
                self.question_session(id, cx)
                    .map(|s| s.read(cx).state.saved.clone())
            } else {
                None
            };
            let new_key = grove_core::identity::random_id()?;
            let project = grove_core::session_meta::project_owner(
                &cx.global::<SettingsState>().store.projects,
                &source.checkout,
            )
            .ok()
            .map(|p| p.name.clone())
            .unwrap_or_default();
            let mut saved = template.map_or_else(
                || {
                    SavedSession::empty(
                        new_key.clone(),
                        provider,
                        project.clone(),
                        source.checkout.clone(),
                    )
                },
                |seed| seed.independent_conversation(new_key.clone()),
            );
            saved.cwd.clone_from(&source.checkout);
            saved.project = project;
            saved.roots.clear();
            grove_core::session_title::set_manual_title(
                &mut saved,
                &format!("Ask about {}", question_source_label(&source.scope)),
            );
            self.question_store()?.bind(&saved.key, &source)?;
            let id = self
                .runtime
                .update(cx, |runtime, cx| runtime.start_change_session(saved, cx))?;
            Ok((id, new_key, source))
        })();
        let (id, session_key, source) = match result {
            Ok(v) => v,
            Err(error) => {
                self.question_error(error, cx);
                return;
            }
        };
        self.dashboard_open = false;
        self.snapshot = self.runtime.update(cx, |runtime, cx| runtime.snapshot(cx));
        self.select(Selection::Session(id), cx);
        self.checkout_panel.chosen_root = Some(source.checkout.clone());
        let Some(target) =
            self.attach_question_controller(id, session_key.clone(), source.clone(), cx)
        else {
            return;
        };
        self.questions.hydrated.insert((id, session_key));
        let entry = self.questions.entries.get_mut(&target);
        if let Some(entry) = entry {
            entry.pending_question = Some(question.clone());
        }
        self.questions.open = Some(target.clone());
        self.questions.source = None;
        self.questions.focus_pending = true;
        self.files_active = false;
        self.dismiss_description();
        if let Some(controller) = self
            .questions
            .entries
            .get(&target)
            .and_then(|e| e.controller.clone())
        {
            controller.update(cx, |controller, cx| {
                controller.start(source.checkout, source.scope, question, cx);
            });
        }
        cx.notify();
    }
    pub(in crate::views::sidebar) fn restore_session_questions(&mut self, cx: &mut Context<Self>) {
        if self.dashboard_open {
            return;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return;
        };
        let Some(session) = self.question_session(id, cx) else {
            return;
        };
        let session_key = session.read(cx).state.saved.key.clone();
        self.sync_question_identity(id, &session_key);
        if !self.questions.hydrated.insert((id, session_key.clone())) {
            return;
        }
        let store = self.question_store();
        let expected_key = session_key.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let store = store?;
                    let source = store.source_for_session(&session_key)?;
                    let input = if source.is_some() {
                        store
                            .pending_for_session(&session_key)?
                            .map(|request| request.input)
                            .or(store.input_for_session(&session_key)?)
                    } else {
                        None
                    };
                    Ok::<_, String>((source, session_key, input))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this
                    .question_session(id, cx)
                    .is_some_and(|session| session.read(cx).state.saved.key == expected_key)
                {
                    return;
                }
                match result {
                    Ok((Some(source), session_key, input)) => {
                        if let Some(target) =
                            this.attach_question_controller(id, session_key, source, cx)
                        {
                            if let Some(input) = input {
                                if let Some(entry) =
                                    this.questions.entries.get_mut(&target).filter(|entry| {
                                        entry.input.is_none() && entry.pending_question.is_none()
                                    })
                                {
                                    entry.pending_question = Some(input);
                                }
                            }
                            if let Some(controller) = this
                                .questions
                                .entries
                                .get(&target)
                                .and_then(|e| e.controller.clone())
                            {
                                controller.update(cx, ChangeSession::hydrate);
                            }
                        }
                    }
                    Ok((None, _, _)) => {}
                    Err(error) => this.question_error(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn reload_question(&mut self, target: &Key, cx: &mut Context<Self>) {
        if let Some(controller) = self
            .questions
            .entries
            .get(target)
            .and_then(|e| e.controller.clone())
        {
            controller.update(cx, ChangeSession::reload);
        }
    }
}
