//! Session lifecycle and durable imports for the native description reader.
use super::*;
use grove_core::{
    change_description_store::{Source, Store},
    codex::{Action, SavedSession, Status},
};

pub(super) struct Pending {
    snapshot: domain::Snapshot,
    start_item: usize,
    prompt: String,
    recovered: bool,
    sent: bool,
    pub(super) generation: String,
}

fn install_result(entry: &mut Entry, description: Description, cx: &mut Context<Sidebar>) {
    let summary = format!(
        "# {}\n\n{}\n\n## Description\n\n{}",
        description.title,
        description.types.join(" · "),
        description
            .summary
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    entry.text = Some(cx.new(|cx| TextViewState::markdown(&summary, cx)));
    entry.file_text.clear();
    for file in &description.files {
        let text = format!(
            "**{}**\n\n{}",
            file.title,
            file.summary
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        entry.file_text.insert(
            file.path.clone(),
            cx.new(|cx| TextViewState::markdown(&text, cx)),
        );
    }
    entry.diagram = description
        .diagram
        .as_ref()
        .map(|d| cx.new(|cx| TextViewState::markdown(&format!("```mermaid\n{d}\n```"), cx)));
    entry.result = Some(Arc::new(description));
    entry.freshness = None;
    entry.checked_at = None;
    entry.checking = false;
    entry.expanded.clear();
    entry.diff = None;
    entry.scroll = ScrollHandle::default();
}

fn request_prompt(snapshot: &domain::Snapshot, generation: &str) -> Result<String, String> {
    Ok(format!("{}\n\nGrove request identity: {generation}. This identifies this generation attempt; do not include it in the result.", domain::conversation_prompt(snapshot)?))
}

// Match the captured request, not the latest agent message. A queued follow-up or
// restart must never import an unrelated answer against an older snapshot.
fn completed_description(
    state: &grove_core::codex::Snapshot,
    pending: &Pending,
) -> Option<Result<Description, String>> {
    let request = state
        .saved
        .items
        .iter()
        .enumerate()
        .skip(pending.start_item)
        .rev()
        .find(|(_, item)| {
            item["type"] == "userMessage"
                && item["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|part| part["text"].as_str() == Some(pending.prompt.as_str()))
                })
        })
        .map(|(index, _)| index);
    let complete = request.is_some_and(|index| {
        state.saved.work_periods.iter().any(|period| {
            period.start_item <= index && period.end_item.is_some_and(|end| end > index)
        })
    });
    if !complete && (state.status.busy() || state.turn_id.is_some()) {
        return None;
    }
    let recovered_idle = pending.recovered
        && state.saved.thread_id.is_some()
        && matches!(state.status, Status::Idle | Status::Done);
    if !complete && !recovered_idle && state.error.is_none() && state.status != Status::Disconnected
    {
        return None;
    }
    if !complete {
        if let Some(error) = &state.error {
            return Some(Err(error.clone()));
        }
    }
    let Some(request) = request else {
        return Some(Err("Description generation was interrupted before the captured request completed. Regenerate to try again.".into()));
    };
    let start = request + 1;
    let end = state
        .saved
        .items
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, item)| item["type"] == "userMessage")
        .map_or(state.saved.items.len(), |(index, _)| index);
    let text = state
        .saved
        .items
        .iter()
        .take(end)
        .skip(start)
        .rev()
        .find(|item| item["type"] == "agentMessage" && item["phase"].as_str() != Some("commentary"))
        .and_then(|item| item["text"].as_str())
        .unwrap_or("");
    Some(domain::from_conversation(pending.snapshot.clone(), text))
}

impl Sidebar {
    pub(super) fn description_store(&self) -> Result<Store, String> {
        #[cfg(test)]
        if let Some(root) = &self.descriptions.store_root {
            return Ok(Store::new(root.clone()));
        }
        Store::open()
    }

    fn description_session(
        &self,
        id: SessionId,
        cx: &App,
    ) -> Option<Entity<crate::entities::codex_session::CodexSession>> {
        self.runtime.read(cx).registry.read(cx).codex(id).cloned()
    }

    fn active_description_for_source(&self, source: &Source, cx: &App) -> Option<(SessionId, Key)> {
        self.descriptions
            .entries
            .iter()
            .find_map(|(target, entry)| {
                if entry.cancel.is_none() && !entry.publishing {
                    return None;
                }
                let candidate = Source {
                    checkout: target.path.clone(),
                    scope: entry.scope.clone()?,
                };
                self.description_session(entry.session?, cx)?;
                let equal = match (&source.scope, &candidate.scope) {
                    (Scope::PullRequest { .. }, Scope::PullRequest { .. }) => candidate
                        .normalized()
                        .ok()
                        .is_some_and(|candidate| candidate.scope == source.scope),
                    (Scope::Local, Scope::Local) => candidate
                        .normalized()
                        .ok()
                        .is_some_and(|candidate| candidate.checkout == source.checkout),
                    _ => false,
                };
                equal.then_some((entry.session?, target.clone()))
            })
    }
    fn open_description_session(&mut self, id: SessionId, target: Key, cx: &mut Context<Self>) {
        self.dashboard_open = false;
        self.select(Selection::Session(id), cx);
        self.checkout_panel.chosen_root = Some(target.path.clone());
        self.descriptions.open = Some(target);
        self.descriptions.focus_pending = true;
        cx.notify();
    }

    pub(super) fn start_description_conversation(
        &mut self,
        path: String,
        scope: Scope,
        _source: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        let source = match (Source {
            checkout: path,
            scope,
        })
        .normalized()
        {
            Ok(source) => source,
            Err(error) => {
                self.description_error(error, cx);
                return;
            }
        };
        let path = source.checkout.clone();
        let scope = source.scope.clone();
        if let Some((id, target)) = self.active_description_for_source(&source, cx) {
            self.open_description_session(id, target, cx);
            return;
        }
        let result = (|| -> Result<_, String> {
            let provider = self.checkout_commit_provider(cx)?;
            let template = if self.dashboard_open {
                self.dashboard
                    .as_ref()
                    .map(|d| d.session.read(cx).state.saved.clone())
            } else if let Some(Selection::Session(id)) = self.selection {
                self.description_session(id, cx)
                    .map(|s| s.read(cx).state.saved.clone())
            } else {
                None
            };
            let source = Source {
                checkout: path.clone(),
                scope: scope.clone(),
            };
            let store = self.description_store()?;
            let new_key = grove_core::identity::random_id()?;
            let project = grove_core::session_meta::project_owner(
                &cx.global::<SettingsState>().store.projects,
                &path,
            )
            .ok()
            .map(|p| p.name.clone())
            .unwrap_or_default();
            let mut saved = template.map_or_else(
                || SavedSession::empty(new_key.clone(), provider, project.clone(), path.clone()),
                |seed| seed.independent_conversation(new_key.clone()),
            );
            saved.cwd.clone_from(&path);
            saved.project = project;
            saved.roots.clear();
            let title = match &scope {
                Scope::Local => "Describe local changes".into(),
                Scope::PullRequest { url } => format!(
                    "Describe PR #{}",
                    url.trim_end_matches('/').rsplit('/').next().unwrap_or("")
                ),
            };
            grove_core::session_title::set_manual_title(&mut saved, &title);
            store.bind(&saved.key, &source)?;
            let id = self.runtime.update(cx, |runtime, cx| {
                runtime.start_description_session(saved, cx)
            })?;
            Ok((id, new_key))
        })();
        let (id, session_key) = match result {
            Ok(value) => value,
            Err(error) => {
                self.description_error(error, cx);
                return;
            }
        };
        self.dashboard_open = false;
        self.snapshot = self.runtime.update(cx, |runtime, cx| runtime.snapshot(cx));
        self.select(Selection::Session(id), cx);
        self.checkout_panel.chosen_root = Some(path.clone());
        let target = key(&self.selection, &path, &scope);
        let entry = self.descriptions.entries.entry(target.clone()).or_default();
        entry.scope = Some(scope);
        entry.session = Some(id);
        entry.session_key = Some(session_key.clone());
        self.descriptions.linked.insert(id, target.clone());
        self.descriptions.hydrated.insert((id, session_key));
        self.descriptions.open = Some(target.clone());
        self.descriptions.focus_pending = true;
        // The source composer belongs to a different session. Back goes to the
        // new conversation; its view supplies focus once mounted.
        self.descriptions.return_focus = None;
        self.observe_description_session(id, target.clone(), cx);
        self.run_session_description(target, cx);
    }

    pub(super) fn run_session_description(&mut self, target: Key, cx: &mut Context<Self>) {
        if let Some(scope) = self
            .descriptions
            .entries
            .get(&target)
            .and_then(|entry| entry.scope.clone())
        {
            let source = Source {
                checkout: target.path.clone(),
                scope,
            };
            if let Ok(source) = source.normalized() {
                if let Some((id, active)) = self.active_description_for_source(&source, cx) {
                    self.open_description_session(id, active, cx);
                    return;
                }
            }
        }
        let store = match self.description_store() {
            Ok(store) => store,
            Err(error) => {
                self.description_error(error, cx);
                return;
            }
        };
        let Some(entry) = self.descriptions.entries.get_mut(&target) else {
            return;
        };
        if entry.cancel.is_some() || entry.publishing {
            self.descriptions.open = Some(target);
            return;
        }
        let (Some(id), Some(session_key), Some(scope)) = (
            entry.session,
            entry.session_key.clone(),
            entry.scope.clone(),
        ) else {
            return;
        };
        self.descriptions.revision = self.descriptions.revision.wrapping_add(1);
        let run = self.descriptions.revision;
        entry.run = run;
        entry.finalizing = false;
        entry.error = None;
        entry.pending = None;
        entry.preview = None;
        let cancel = Arc::new(CommitControl::default());
        entry.cancel = Some(cancel.clone());
        self.descriptions.open = Some(target.clone());
        self.descriptions.focus_pending = true;
        let path = target.path.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if cancel.canceled() {
                        return Err("Description generation canceled".into());
                    }
                    let source = Source {
                        checkout: path.clone(),
                        scope: scope.clone(),
                    };
                    let previous = store.load(&source)?;
                    let snapshot = domain::prepare(std::path::Path::new(&path), &scope)?;
                    if cancel.canceled() {
                        return Err("Description generation canceled".into());
                    }
                    // Reject an empty/omitted capture before creating an active durable job.
                    domain::conversation_prompt(&snapshot)?;
                    store.save_pending(&session_key, &snapshot)?;
                    let generation = store.pending_generation(&session_key)?.ok_or("Saved description request is unavailable")?;
                    let prompt = request_prompt(&snapshot, &generation)?;
                    Ok((previous, snapshot, prompt, generation))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(entry) = this
                    .descriptions
                    .entries
                    .get_mut(&target)
                    .filter(|e| e.run == run)
                else {
                    return;
                };
                match result {
                    Ok((previous, snapshot, prompt, generation)) => {
                        if let Some(previous) = previous {
                            if entry.result.is_none() {
                                install_result(entry, previous, cx);
                            }
                        }
                        if entry.cancel.as_ref().is_none_or(|c| c.canceled()) {
                            entry.cancel = None;
                            let session_key = entry.session_key.clone().unwrap_or_default();
                            this.retire_description_request(session_key, generation, cx);
                            cx.notify(); return;
                        }
                        let Some(session) = this.description_session(id, cx) else {
                            if let Some(entry) = this.descriptions.entries.get_mut(&target) { entry.cancel = None; entry.error = Some("The description session was closed. Start a new description session to try again.".into()); }
                            let session_key = this.descriptions.entries.get(&target).and_then(|entry| entry.session_key.clone()).unwrap_or_default();
                            this.retire_description_request(session_key, generation, cx);
                            cx.notify(); return;
                        };
                        let state = &session.read(cx).state;
                        let pending = Pending {
                            snapshot,
                            start_item: state.saved.items.len(),
                            prompt: prompt.clone(),
                            recovered: false,
                            sent: false,
                            generation,
                        };
                        if let Some(entry) = this.descriptions.entries.get_mut(&target) {
                            entry.pending = Some(pending);
                        }
                        session.update(cx, |_, cx| cx.notify());
                    }
                    Err(error) => {
                        entry.cancel = None;
                        entry.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn retire_description_request(
        &self,
        session_key: String,
        generation: String,
        cx: &mut Context<Self>,
    ) {
        let store = self.description_store();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { store?.retire_generation(&session_key, &generation) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.description_error(error, cx);
                }
            });
        })
        .detach();
    }

    fn observe_description_session(&mut self, id: SessionId, target: Key, cx: &mut Context<Self>) {
        if self.descriptions.sessions.contains_key(&id) {
            return;
        }
        let Some(session) = self.description_session(id, cx) else {
            return;
        };
        let store = self.description_store();
        let observer = cx.observe(&session, move |this, session, cx| {
            let Some(entry) = this.descriptions.entries.get_mut(&target) else {
                return;
            };
            let Some(pending) = &mut entry.pending else {
                return;
            };
            if !pending.sent {
                let generation = pending.generation.clone();
                if entry
                    .cancel
                    .as_ref()
                    .is_some_and(|cancel| cancel.canceled())
                {
                    entry.pending = None;
                    entry.cancel = None;
                    entry.error = Some(
                        "Description generation canceled. The saved result was preserved.".into(),
                    );
                    let session_key = entry.session_key.clone().unwrap_or_default();
                    this.retire_description_request(session_key, generation, cx);
                    cx.notify();
                    return;
                }
                let state = &session.read(cx).state;
                if let Some(error) = &state.error {
                    entry.error = Some(error.clone());
                    entry.cancel = None;
                    entry.pending = None;
                    let session_key = entry.session_key.clone().unwrap_or_default();
                    this.retire_description_request(session_key, generation, cx);
                    cx.notify();
                    return;
                }
                if !matches!(state.status, Status::Idle | Status::Done)
                    || state.turn_id.is_some()
                    || (state.saved.provider == grove_core::codex::Provider::Codex
                        && state.saved.thread_id.is_none())
                {
                    return;
                }
                pending.start_item = state.saved.items.len();
                pending.sent = true;
                let prompt = pending.prompt.clone();
                session.update(cx, |session, cx| session.send(Action::Prompt(prompt), cx));
                return;
            }
            let generation = pending.generation.clone();
            let state = &session.read(cx).state;
            let Some(prediction) = completed_description(state, pending) else {
                return;
            };
            entry.pending = None;
            let result = if entry.cancel.as_ref().is_some_and(|c| c.canceled()) {
                Err("Description generation canceled. The saved result was preserved.".into())
            } else {
                prediction
            };
            match result {
                Err(error) => {
                    entry.cancel = None;
                    entry.error = Some(error);
                    let session_key = entry.session_key.clone().unwrap_or_default();
                    this.retire_description_request(session_key, generation, cx);
                    cx.notify();
                }
                Ok(description) => {
                    entry.finalizing = true;
                    let run = entry.run;
                    let target = target.clone();
                    let session_key = entry.session_key.clone().unwrap_or_default();
                    let store = store.clone();
                    cx.spawn(async move |this, cx| {
                        let result = cx
                            .background_spawn(async move {
                                let store = store?;
                                store.save_generation(&session_key, &description, &generation)?;
                                Ok::<_, String>(description)
                            })
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(entry) = this
                                .descriptions
                                .entries
                                .get_mut(&target)
                                .filter(|e| e.run == run)
                            {
                                entry.cancel = None;
                                entry.finalizing = false;
                                match result {
                                    Ok(description) => {
                                        install_result(entry, description, cx);
                                        entry.receipt = Some("Description saved".into());
                                    }
                                    Err(error) => entry.error = Some(error),
                                }
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
        });
        self.descriptions.sessions.insert(id, observer);
        // A completed turn may predate hydration; do not wait for another turn.
        session.update(cx, |_, cx| cx.notify());
    }

    pub(in crate::views::sidebar) fn restore_session_description(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_open {
            return;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return;
        };
        let Some(session) = self.description_session(id, cx) else {
            return;
        };
        let session_key = session.read(cx).state.saved.key.clone();
        if !self.descriptions.hydrated.insert((id, session_key.clone())) {
            return;
        }
        let store = self.description_store();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let store = store?;
                    let Some(source) = store.source_for_session(&session_key)? else {
                        return Ok(None);
                    };
                    let result = store.load(&source);
                    let pending = store.pending_for_session(&session_key).and_then(|pending| {
                        pending
                            .map(|snapshot| {
                                let generation = store
                                    .pending_generation(&session_key)?
                                    .ok_or("Saved description request identity is missing")?;
                                Ok::<_, String>((snapshot, generation))
                            })
                            .transpose()
                    });
                    Ok::<_, String>(Some((source, result, pending, session_key)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some((source, result, pending, session_key))) => {
                        let selection = Some(Selection::Session(id));
                        let target = key(&selection, &source.checkout, &source.scope);
                        let entry = this.descriptions.entries.entry(target.clone()).or_default();
                        entry.scope = Some(source.scope);
                        entry.session = Some(id);
                        entry.session_key = Some(session_key);
                        match result {
                            Ok(Some(result)) => install_result(entry, result, cx),
                            Ok(None) => {}
                            Err(error) => entry.error = Some(error),
                        }
                        if let Ok(Some((snapshot, generation))) = pending {
                            entry.pending =
                                request_prompt(&snapshot, &generation)
                                    .ok()
                                    .map(|prompt| Pending {
                                        snapshot,
                                        start_item: 0,
                                        prompt,
                                        recovered: true,
                                        sent: true,
                                        generation,
                                    });
                            entry.cancel = Some(Arc::new(CommitControl::default()));
                        } else if let Err(error) = pending {
                            entry.error = Some(error);
                        } else if entry.result.is_none() && entry.error.is_none() {
                            entry.error = Some(
                                "No saved description yet. Generate it from this session.".into(),
                            );
                        }
                        this.descriptions.linked.insert(id, target.clone());
                        this.observe_description_session(id, target, cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.description_error(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reload_description_result(&mut self, target: &Key, cx: &mut Context<Self>) {
        let Some(entry) = self.descriptions.entries.get(target) else {
            return;
        };
        if entry.cancel.is_some() || entry.publishing {
            return;
        }
        let Some(scope) = entry.scope.clone() else {
            return;
        };
        let target = target.clone();
        let run = entry.run;
        let source = Source {
            checkout: target.path.clone(),
            scope,
        };
        let store = self.description_store();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store?.load(&source) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) =
                    this.descriptions.entries.get_mut(&target).filter(|entry| {
                        entry.run == run && entry.cancel.is_none() && !entry.publishing
                    })
                {
                    match result {
                        Ok(Some(description)) if entry.result.as_deref() != Some(&description) => {
                            install_result(entry, description, cx);
                        }
                        Ok(_) => {}
                        Err(error) => entry.error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::views::sidebar) fn description_scope_for_session(
        &self,
        id: SessionId,
    ) -> Option<Scope> {
        self.descriptions
            .linked
            .get(&id)
            .and_then(|key| self.descriptions.entries.get(key))
            .and_then(|entry| entry.scope.clone())
    }

    pub(in crate::views::sidebar) fn linked_description_key(&self) -> Option<Key> {
        if self.dashboard_open {
            return None;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return None;
        };
        self.descriptions.linked.get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Provider, Snapshot, WorkPeriod};
    use serde_json::json;

    fn completed(provider: Provider) -> (Snapshot, Pending) {
        let captured = domain::Snapshot {
            scope: Scope::Local,
            fingerprint: "captured".into(),
            label: "Local changes".into(),
            head: "head".into(),
            base: String::new(),
            checkout: "/fixture".into(),
            original_body: None,
            title: String::new(),
            files: vec![domain::SnapshotFile {
                path: "handler.rs".into(),
                added: 1,
                removed: 0,
                patch: "+deduplicate();".into(),
                omitted: None,
            }],
        };
        let prompt = domain::conversation_prompt(&captured).unwrap();
        let prediction = json!({"title":"Prevent duplicate processing", "types":["Bug fix"], "summary":["Deduplicate incoming requests"], "files":[{"path":"handler.rs","title":"Skip duplicate incoming requests before processing","summary":["Check request identity before processing."],"category":"Behavior"}], "diagram":null});
        let mut state = Snapshot::new(SavedSession::empty(
            "fixture".into(),
            provider,
            String::new(),
            "/fixture".into(),
        ));
        state.status = Status::Done;
        state.saved.thread_id = Some("thread".into());
        state.saved.items = vec![
            json!({"id":"request", "type":"userMessage", "content":[{"type":"text","text":prompt}]}),
            json!({"id":"result", "type":"agentMessage", "phase":"final_answer", "text":format!("Saved result\n```grove-description\n{prediction}\n```\n")}),
            json!({"id":"followup", "type":"userMessage", "content":[{"type":"text","text":"Explain the tests"}]}),
            json!({"id":"other", "type":"agentMessage", "phase":"final_answer", "text":"Unrelated follow-up answer"}),
        ].into();
        state.saved.work_periods.push(WorkPeriod {
            start_item: 0,
            end_item: Some(2),
            duration_ms: None,
            subagents: Vec::new(),
        });
        (
            state,
            Pending {
                snapshot: captured,
                prompt,
                start_item: 0,
                recovered: false,
                sent: true,
                generation: "fixture-attempt".into(),
            },
        )
    }

    #[gpui::test]
    fn reopened_session_identity_reloads_its_existing_canonical_description(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().to_path_buf());
        let (state, pending) = completed(Provider::Codex);
        let description = domain::from_conversation(
            pending.snapshot,
            state.saved.items[1]["text"].as_str().unwrap(),
        )
        .unwrap();
        let saved = state.saved;
        store.bind(&saved.key, &Source::from(&description)).unwrap();
        store.save(&description).unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let mut reopened = None;
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let ids = [0, 1].map(|_| {
                let (handle, _receive) = grove_core::codex::Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
                let session = cx.new(|cx| {
                    crate::entities::codex_session::CodexSession::from_connection(
                        grove_core::codex::Snapshot::new(saved.clone()),
                        grove_core::codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                runtime.update(cx, |runtime, cx| {
                    runtime.adopt_dashboard_session(session, cx)
                })
            });
            assert_ne!(ids[0], ids[1]);
            reopened = Some(ids[1]);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.dashboard_open = false;
            sidebar.initial_selection_pending = false;
            sidebar.selection = Some(Selection::Session(ids[1]));
            sidebar.descriptions.store_root = Some(directory.path().to_path_buf());
            sidebar
                .descriptions
                .hydrated
                .insert((ids[0], saved.key.clone()));
            sidebar
        });
        cx.update(|_, cx| {
            sidebar.update(
                cx,
                crate::views::sidebar::Sidebar::restore_session_description,
            );
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if sidebar.read_with(cx, |sidebar, _| {
                sidebar.descriptions.linked.contains_key(&reopened.unwrap())
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        sidebar.read_with(cx, |sidebar, _| {
            let linked = sidebar
                .descriptions
                .linked
                .get(&reopened.unwrap())
                .expect("reopened identity must hydrate");
            assert_eq!(
                sidebar.descriptions.entries[linked].result.as_deref(),
                Some(&description)
            );
        });
    }

    #[gpui::test]
    fn cancellation_retires_its_durable_job_before_recovery(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().to_path_buf());
        let (mut state, mut pending) = completed(Provider::Codex);
        state.saved.key = "cancel-session".into();
        state.status = Status::Working;
        state.turn_id = Some("owned-turn".into());
        let source = Source {
            checkout: pending.snapshot.checkout.clone(),
            scope: pending.snapshot.scope.clone(),
        };
        store.bind(&state.saved.key, &source).unwrap();
        store
            .save_pending(&state.saved.key, &pending.snapshot)
            .unwrap();
        pending.generation = store.pending_generation(&state.saved.key).unwrap().unwrap();
        let token = pending.generation.clone();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let mut target = None;
        let mut actions = None;
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let (handle, receiver) = grove_core::codex::Handle::channel();
            actions = Some(receiver);
            let (_updates, receive) = std::sync::mpsc::channel();
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    state,
                    grove_core::codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let id = runtime.update(cx, |runtime, cx| {
                runtime.adopt_dashboard_session(session, cx)
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.dashboard_open = false;
            sidebar.initial_selection_pending = false;
            sidebar.selection = Some(Selection::Session(id));
            sidebar.descriptions.store_root = Some(directory.path().to_path_buf());
            sidebar
                .descriptions
                .hydrated
                .insert((id, "cancel-session".into()));
            let key = key(&sidebar.selection, &source.checkout, &source.scope);
            target = Some(key.clone());
            sidebar.descriptions.entries.insert(
                key,
                Entry {
                    session: Some(id),
                    session_key: Some("cancel-session".into()),
                    pending: Some(pending),
                    cancel: Some(Arc::new(CommitControl::default())),
                    ..Default::default()
                },
            );
            sidebar
        });
        let target = target.unwrap();
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.cancel_description(&target, cx));
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if store
                .pending_for_session("cancel-session")
                .unwrap()
                .is_none()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(Store::new(directory.path().to_path_buf())
            .pending_for_session("cancel-session")
            .unwrap()
            .is_none());
        assert!(actions
            .unwrap()
            .try_iter()
            .any(|action| matches!(action, Action::Interrupt)));
        // A stale cancellation acknowledgment cannot remove a later generation.
        store
            .save_pending("cancel-session", &completed(Provider::Codex).1.snapshot)
            .unwrap();
        store.retire_generation("cancel-session", &token).unwrap();
        assert!(store
            .pending_for_session("cancel-session")
            .unwrap()
            .is_some());
    }

    #[test]
    fn completed_result_is_bound_to_its_request_even_after_followups_and_restart() {
        for provider in [Provider::Codex, Provider::Claude] {
            let (state, mut pending) = completed(provider);
            assert_eq!(
                completed_description(&state, &pending)
                    .unwrap()
                    .unwrap()
                    .title,
                "Prevent duplicate processing"
            );
            let mut later_turn = state.clone();
            later_turn.status = Status::Working;
            later_turn.turn_id = Some("later-turn".into());
            later_turn.error = Some("Later follow-up failed".into());
            assert_eq!(
                completed_description(&later_turn, &pending)
                    .unwrap()
                    .unwrap()
                    .title,
                "Prevent duplicate processing"
            );
            pending.recovered = true;
            assert_eq!(
                completed_description(&state, &pending)
                    .unwrap()
                    .unwrap()
                    .snapshot
                    .fingerprint,
                "captured"
            );
            let mut repeated = state.clone();
            let old_request = repeated.saved.items[0].as_ref().clone();
            let mut newest = repeated.saved.items[1].as_ref().clone();
            newest["text"] = json!(newest["text"]
                .as_str()
                .unwrap()
                .replace("Prevent duplicate processing", "Newest saved result"));
            repeated.saved.items.push(old_request);
            repeated.saved.items.push(newest);
            repeated.saved.work_periods.push(WorkPeriod {
                start_item: 4,
                end_item: Some(6),
                duration_ms: None,
                subagents: Vec::new(),
            });
            assert_eq!(
                completed_description(&repeated, &pending)
                    .unwrap()
                    .unwrap()
                    .title,
                "Newest saved result"
            );
            pending.prompt = "Different captured request".into();
            assert!(completed_description(&state, &pending).unwrap().is_err());
        }
    }

    #[test]
    fn active_and_incomplete_results_are_never_imported() {
        let (mut state, mut pending) = completed(Provider::Codex);
        state.status = Status::Working;
        state.saved.work_periods.clear();
        assert!(completed_description(&state, &pending).is_none());
        state.status = Status::Idle;
        state.saved.work_periods.clear();
        assert!(completed_description(&state, &pending).is_none());
        pending.recovered = true;
        state.saved.items[1]["text"] = json!("```grove-description\n{\"title\":");
        assert!(completed_description(&state, &pending).unwrap().is_err());
        state.error = Some("Provider authentication failed".into());
        assert_eq!(
            completed_description(&state, &pending)
                .unwrap()
                .unwrap_err(),
            "Provider authentication failed"
        );
    }
}
