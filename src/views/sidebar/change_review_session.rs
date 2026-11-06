//! Bind the Review presentation to Grove's shared retained-conversation controller.
use super::*;
use crate::views::change_session::ChangeSession;
use grove_core::{
    change_result_store::{Source, Store},
    codex::SavedSession,
};

fn retained_review_session(
    template: Option<SavedSession>,
    key: String,
    provider: grove_core::codex::Provider,
    source: &Source,
    project: String,
) -> SavedSession {
    let mut saved = template.map_or_else(
        || {
            SavedSession::empty(
                key.clone(),
                provider,
                project.clone(),
                source.checkout.clone(),
            )
        },
        |seed| seed.independent_conversation(key.clone()),
    );
    saved.cwd = source.checkout.clone();
    saved.project = project;
    saved.roots.clear();
    let title = match &source.scope {
        Scope::Local => "Review local changes".into(),
        Scope::PullRequest { url } => {
            format!("Review PR #{}", url.rsplit('/').next().unwrap_or(""))
        }
    };
    grove_core::session_title::set_manual_title(&mut saved, &title);
    saved
}

fn install(entry: &mut Entry, review: Arc<Review>, cx: &mut Context<Sidebar>) {
    entry.text = Some(cx.new(|cx| TextViewState::markdown(&review.summary_markdown(), cx)));
    entry.file_text = review
        .files
        .iter()
        .map(|file| {
            let text = format!("**{}**\n\n{}", file.title, file.summary.join("\n\n"));
            (
                file.path.clone(),
                cx.new(|cx| TextViewState::markdown(&text, cx)),
            )
        })
        .collect();
    entry.result = Some(review);
    entry.freshness = None;
    entry.checked_at = None;
    entry.checking = false;
    entry.expanded.clear();
    entry.diff = None;
    entry.scroll = ScrollHandle::default();
}
impl Sidebar {
    pub(super) fn review_store(&self) -> Result<Store<Review>, String> {
        #[cfg(test)]
        if let Some(root) = &self.reviews.store_root {
            return Ok(Store::new(root.clone()));
        }
        Store::open()
    }
    fn review_session(
        &self,
        id: SessionId,
        cx: &App,
    ) -> Option<Entity<crate::entities::codex_session::CodexSession>> {
        self.runtime.read(cx).registry.read(cx).codex(id).cloned()
    }
    fn review_controller(
        &mut self,
        id: SessionId,
        target: Key,
        store: Store<Review>,
        session_key: String,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ChangeSession<Review>>> {
        let session = self.review_session(id, cx)?;
        let controller = cx.new(|cx| ChangeSession::new(session, store, session_key, cx));
        let entry = self.reviews.entries.entry(target.clone()).or_default();
        entry.session = Some(id);
        entry.controller = Some(controller.clone());
        let observer = cx.observe(&controller, move |this, controller, cx| {
            let state = controller.read(cx);
            let result = state.result.clone();
            let error = state.error.clone();
            let busy = state.phase.busy();
            let saving = state.phase.saving();
            if let Some(entry) = this.reviews.entries.get_mut(&target) {
                let saved = entry.saving && !saving && error.is_none() && result.is_some();
                entry.saving = saving;
                if busy {
                    if entry.cancel.is_none() {
                        entry.cancel = Some(Arc::new(CommitControl::default()));
                    }
                } else {
                    entry.cancel = None;
                }
                entry.error = error;
                if let Some(result) = result {
                    if entry.result.as_deref() != Some(result.as_ref()) {
                        install(entry, result, cx);
                    }
                }
                if saved {
                    entry.receipt = Some("Review saved".into());
                }
                cx.notify();
            }
        });
        self.reviews.sessions.insert(id, observer);
        Some(controller)
    }
    fn open_review_session(&mut self, id: SessionId, target: Key, cx: &mut Context<Self>) {
        self.dashboard_open = false;
        self.select(Selection::Session(id), cx);
        self.checkout_panel.chosen_root = Some(target.path.clone());
        self.reviews.open = Some(target);
        self.reviews.focus_pending = true;
        self.reviews.return_focus = None;
        cx.notify();
    }
    pub(super) fn start_review_conversation(
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
            Ok(s) => s,
            Err(e) => {
                self.review_error(e, cx);
                return;
            }
        };
        if let Some((id, target)) = self.reviews.entries.iter().find_map(|(key, entry)| {
            let controller = entry.controller.as_ref()?;
            if !controller.read(cx).phase.busy() {
                return None;
            }
            let candidate = Source {
                checkout: key.path.clone(),
                scope: entry.scope.clone()?,
            }
            .normalized()
            .ok()?;
            let same = match (&source.scope, &candidate.scope) {
                (Scope::Local, Scope::Local) => source.checkout == candidate.checkout,
                (Scope::PullRequest { .. }, Scope::PullRequest { .. }) => {
                    source.scope == candidate.scope
                }
                _ => false,
            };
            same.then_some((entry.session?, key.clone()))
        }) {
            self.open_review_session(id, target, cx);
            return;
        }
        let started = (|| -> Result<_, String> {
            let store = self.review_store()?;
            let provider = self.checkout_commit_provider(cx)?;
            let template = if self.dashboard_open {
                self.dashboard
                    .as_ref()
                    .map(|d| d.session.read(cx).state.saved.clone())
            } else if let Some(Selection::Session(id) | Selection::Home(id)) = self.selection {
                self.review_session(id, cx)
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
            let saved =
                retained_review_session(template, new_key.clone(), provider, &source, project);
            store.bind(&saved.key, &source)?;
            let id = self
                .runtime
                .update(cx, |runtime, cx| runtime.start_change_session(saved, cx))?;
            Ok((id, new_key, store))
        })();
        let (id, session_key, store) = match started {
            Ok(s) => s,
            Err(e) => {
                self.review_error(e, cx);
                return;
            }
        };
        self.dashboard_open = false;
        self.snapshot = self.runtime.update(cx, |r, cx| r.snapshot(cx));
        self.select(Selection::Session(id), cx);
        let target = key(&self.selection, &source.checkout, &source.scope);
        let entry = self.reviews.entries.entry(target.clone()).or_default();
        entry.scope = Some(source.scope.clone());
        entry.session = Some(id);
        self.reviews.linked.insert(id, target.clone());
        self.reviews.hydrated.insert((id, session_key.clone()));
        let controller = self.review_controller(id, target.clone(), store, session_key, cx);
        self.open_review_session(id, target, cx);
        if let Some(controller) = controller {
            controller.update(cx, |c, cx| {
                c.start(source.checkout, source.scope, String::new(), cx)
            });
        }
    }
    pub(super) fn run_review(&mut self, target: &Key, cx: &mut Context<Self>) {
        let Some(entry) = self.reviews.entries.get_mut(target) else {
            return;
        };
        if entry.saving {
            return;
        }
        let (Some(controller), Some(scope)) = (entry.controller.clone(), entry.scope.clone())
        else {
            return;
        };
        entry.run = entry.run.wrapping_add(1);
        entry.checking = false;
        entry.checked_at = None;
        entry.freshness = None;
        entry.receipt = None;
        controller.update(cx, |c, cx| {
            c.start(target.path.clone(), scope, String::new(), cx)
        });
    }
    pub(in crate::views::sidebar) fn restore_session_review(&mut self, cx: &mut Context<Self>) {
        if self.dashboard_open {
            return;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return;
        };
        let Some(session) = self.review_session(id, cx) else {
            return;
        };
        let session_key = session.read(cx).state.saved.key.clone();
        if !self.reviews.hydrated.insert((id, session_key.clone())) {
            return;
        }
        let store = self.review_store();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let store = store?;
                    let source = store.source_for_session(&session_key)?;
                    Ok::<_, String>(source.map(|source| (store, source, session_key)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some((store, source, session_key))) => {
                        let target = key(
                            &Some(Selection::Session(id)),
                            &source.checkout,
                            &source.scope,
                        );
                        let entry = this.reviews.entries.entry(target.clone()).or_default();
                        entry.scope = Some(source.scope);
                        this.reviews.linked.insert(id, target.clone());
                        if let Some(controller) =
                            this.review_controller(id, target, store, session_key, cx)
                        {
                            controller.update(cx, |c, cx| c.hydrate(cx));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => this.review_error(e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn reload_review_result(&mut self, target: &Key, cx: &mut Context<Self>) {
        if let Some(controller) = self
            .reviews
            .entries
            .get(target)
            .and_then(|e| e.controller.clone())
        {
            controller.update(cx, |c, cx| c.reload(cx));
        }
    }
    pub(in crate::views::sidebar) fn linked_review_key(&self) -> Option<Key> {
        if self.dashboard_open {
            return None;
        }
        let Some(Selection::Session(id)) = self.selection else {
            return None;
        };
        self.reviews.linked.get(&id).cloned()
    }
    pub(in crate::views::sidebar) fn review_scope_for_session(
        &self,
        id: SessionId,
    ) -> Option<Scope> {
        self.reviews
            .linked
            .get(&id)
            .and_then(|key| self.reviews.entries.get(key))
            .and_then(|e| e.scope.clone())
    }
}

#[cfg(test)]
#[path = "change_review_retained_tests.rs"]
mod tests;
