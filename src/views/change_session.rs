//! Retained provider lifecycle for typed change analysis; this entity has no UI widgets.
use crate::entities::codex_session::CodexSession;
use gpui::{AppContext as _, Context, Entity, Subscription};
use grove_core::{
    change_description::{self, Scope},
    change_result_store::{PendingRequest, Source, Store},
    codex::{Action, Provider, Status},
    commit_agent::CommitControl,
    conversation_result::{self, Workflow},
};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Reading,
    Waiting,
    Generating,
    Saving,
}
impl Phase {
    pub fn busy(self) -> bool {
        self != Self::Idle
    }
    pub fn saving(self) -> bool {
        self == Self::Saving
    }
}
pub struct ChangeSession<T: Workflow> {
    pub result: Option<Arc<T>>,
    pub error: Option<String>,
    pub phase: Phase,
    session: Entity<CodexSession>,
    store: Store<T>,
    key: String,
    pending: Option<PendingRequest>,
    start_item: usize,
    recovered: bool,
    sent: bool,
    interrupt_requested: bool,
    run: u64,
    control: Option<Arc<CommitControl>>,
    _observer: Subscription,
}
impl<T: Workflow> ChangeSession<T> {
    pub fn new(
        session: Entity<CodexSession>,
        store: Store<T>,
        key: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(&session, |this, _, cx| this.process(cx));
        Self {
            result: None,
            error: None,
            phase: Phase::Idle,
            session,
            store,
            key,
            pending: None,
            start_item: 0,
            recovered: false,
            sent: false,
            interrupt_requested: false,
            run: 0,
            control: None,
            _observer: observer,
        }
    }
    pub fn start(&mut self, path: String, scope: Scope, input: String, cx: &mut Context<Self>) {
        if self.phase.busy() {
            return;
        }
        if self.session.read(cx).state.saved.key != self.key {
            self.error = Some("Analysis session identity changed".into());
            cx.notify();
            return;
        }
        // An explicit retry may follow a terminal provider error; do not let that old
        // error reject the new request before its prompt can be submitted.
        self.session.update(cx, |session, cx| {
            if matches!(session.state.status, Status::Idle | Status::Done)
                && session.state.turn_id.is_none()
            {
                session.state.clear_error();
                cx.notify();
            }
        });
        self.run = self.run.wrapping_add(1);
        let run = self.run;
        let control = Arc::new(CommitControl::default());
        self.control = Some(control.clone());
        self.error = None;
        self.phase = Phase::Reading;
        self.pending = None;
        self.recovered = false;
        self.sent = false;
        self.interrupt_requested = false;
        let store = self.store.clone();
        let key = self.key.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let source = Source {
                        checkout: path.clone(),
                        scope: scope.clone(),
                    }
                    .normalized()?;
                    if store
                        .source_for_session(&key)?
                        .is_some_and(|bound| bound != source)
                    {
                        return Err("Analysis session belongs to another source".into());
                    }
                    store.bind(&key, &source)?;
                    let previous = store.load(&source)?;
                    let snapshot = change_description::prepare_with_control(
                        std::path::Path::new(&path),
                        &scope,
                        Some(&control),
                    )?;
                    if control.canceled() {
                        return Err("Analysis canceled".into());
                    }
                    let generation = grove_core::identity::random_id()?;
                    let scope_label = match &snapshot.scope {
                        Scope::Local => "local".to_owned(),
                        Scope::PullRequest { url } => url.clone(),
                    };
                    let display = format!(
                        "/{} {}{}{}",
                        T::NAME,
                        scope_label,
                        if input.is_empty() { "" } else { " " },
                        input
                    );
                    let prompt = conversation_result::compose_request(
                        T::NAME,
                        &display,
                        &generation,
                        &T::prompt(&snapshot, &input)?,
                    );
                    let request = PendingRequest {
                        snapshot,
                        input,
                        prompt,
                        generation,
                    };
                    store.save_pending(&key, &request)?;
                    Ok::<_, String>((previous, request))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                match result {
                    Ok((previous, request)) => {
                        if this.result.is_none() {
                            this.result = previous.map(Arc::new);
                        }
                        this.pending = Some(request);
                        this.phase = Phase::Waiting;
                        this.process(cx);
                    }
                    Err(error) => {
                        this.control = None;
                        this.phase = Phase::Idle;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// Restore an exact request without ever resubmitting uncertain provider work.
    pub fn hydrate(&mut self, cx: &mut Context<Self>) {
        if self.phase.busy() {
            return;
        }
        self.run = self.run.wrapping_add(1);
        let run = self.run;
        self.phase = Phase::Reading;
        self.error = None;
        let store = self.store.clone();
        let key = self.key.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let Some(source) = store.source_for_session(&key)? else {
                        return Ok(None);
                    };
                    Ok::<_, String>(Some((
                        store.load(&source)?,
                        store.pending_for_session(&key)?,
                    )))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                this.phase = Phase::Idle;
                match result {
                    Ok(Some((result, pending))) => {
                        this.result = result.map(Arc::new);
                        this.pending = pending;
                        if this.pending.is_some() {
                            this.sent = true;
                            this.recovered = true;
                            this.start_item = 0;
                            this.control = Some(Arc::new(CommitControl::default()));
                            this.phase = Phase::Generating;
                            this.process(cx);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.phase.busy() {
            self.hydrate(cx);
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.phase.busy() || self.phase.saving() {
            return;
        }
        if let Some(control) = &self.control {
            control.cancel();
        }
        self.process(cx);
        cx.notify();
    }
    fn process(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Saving {
            return;
        }
        let Some(request) = self.pending.clone() else {
            return;
        };
        let state = self.session.read(cx).state.shared();
        if state.saved.key != self.key {
            self.fail("Analysis session identity changed".into(), cx);
            return;
        }
        let canceled = self
            .control
            .as_ref()
            .is_some_and(|control| control.canceled());
        if canceled {
            let completed = self.sent
                && conversation_result::completed_text(
                    &state,
                    &request,
                    self.start_item,
                    self.recovered,
                )
                .is_some();
            if !self.sent || completed {
                self.fail(
                    "Analysis canceled; the previous saved result was preserved".into(),
                    cx,
                );
            } else if !self.interrupt_requested
                && state.turn_id.is_some()
                && state.saved.items.iter().skip(self.start_item).any(|item| {
                    item["type"] == "userMessage"
                        && item["content"].as_array().is_some_and(|parts| {
                            parts
                                .iter()
                                .any(|part| part["text"].as_str() == Some(request.prompt.as_str()))
                        })
                })
            {
                self.interrupt_requested = true;
                self.session
                    .update(cx, |session, cx| session.send(Action::Interrupt, cx));
            }
            // Submitted work without a turn id is uncertain, not acknowledged cancellation.
            return;
        }
        if !self.sent {
            if let Some(error) = &state.error {
                self.fail(error.clone(), cx);
                return;
            }
            if state.status == Status::Disconnected {
                self.fail(
                    "Analysis provider disconnected before submission".into(),
                    cx,
                );
                return;
            }
            if !matches!(state.status, Status::Idle | Status::Done)
                || state.turn_id.is_some()
                || (state.saved.provider == Provider::Codex && state.saved.thread_id.is_none())
            {
                return;
            }
            self.start_item = state.saved.items.len();
            self.sent = true;
            self.phase = Phase::Generating;
            self.session.update(cx, |session, cx| {
                session.send(Action::Prompt(request.prompt.clone()), cx);
            });
            cx.notify();
            return;
        }
        let Some(text) =
            conversation_result::completed_text(&state, &request, self.start_item, self.recovered)
        else {
            return;
        };
        if canceled {
            self.fail(
                "Analysis canceled; the previous saved result was preserved".into(),
                cx,
            );
            return;
        }
        let result =
            text.and_then(|text| T::parse(request.snapshot.clone(), &request.input, &text));
        let result = match result {
            Ok(value) => value,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.phase = Phase::Saving;
        let run = self.run;
        let store = self.store.clone();
        let key = self.key.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move {
                    store.save_generation(&key, &result, &request.generation)?;
                    Ok::<_, String>(result)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                this.phase = Phase::Idle;
                this.control = None;
                this.pending = None;
                match saved {
                    Ok(result) => {
                        this.result = Some(Arc::new(result));
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        let Some(request) = self.pending.take() else {
            self.phase = Phase::Idle;
            self.error = Some(error);
            cx.notify();
            return;
        };
        self.phase = Phase::Saving;
        let run = self.run;
        let store = self.store.clone();
        let key = self.key.clone();
        self.error = Some(error);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let retired = cx
                .background_spawn(async move { store.retire_generation(&key, &request.generation) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                this.phase = Phase::Idle;
                this.control = None;
                if let Err(error) = retired {
                    this.error = Some(error);
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl<T: Workflow> Drop for ChangeSession<T> {
    fn drop(&mut self) {
        if let Some(control) = &self.control {
            control.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::{
        change_description::{Snapshot, SnapshotFile},
        change_result_store::StoredResult,
        codex::{Connection, Handle, SavedSession, Snapshot as SessionState, WorkPeriod},
    };
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    #[derive(Clone, Serialize, Deserialize, PartialEq)]
    pub(super) struct TestResult {
        snapshot: Snapshot,
        input: String,
        text: String,
    }
    impl StoredResult for TestResult {
        const DIRECTORY: &'static str = "test-controller";
        fn snapshot(&self) -> &Snapshot {
            &self.snapshot
        }
        fn input(&self) -> &str {
            &self.input
        }
        fn validate(&self) -> Result<(), String> {
            if self.text.is_empty() {
                Err("Empty result".into())
            } else {
                Ok(())
            }
        }
    }
    impl Workflow for TestResult {
        const NAME: &'static str = "test";
        const LABEL: &'static str = "Test analysis";
        const FENCE: &'static str = "grove-test";
        fn prompt(_: &Snapshot, input: &str) -> Result<String, String> {
            Ok(format!("Question: {input}"))
        }
        fn parse(snapshot: Snapshot, input: &str, text: &str) -> Result<Self, String> {
            Ok(Self {
                snapshot,
                input: input.into(),
                text: text.into(),
            })
        }
    }
    fn request(path: &std::path::Path) -> PendingRequest {
        PendingRequest {
            snapshot: Snapshot {
                checkout: path.to_string_lossy().into_owned(),
                scope: Scope::Local,
                fingerprint: "captured".into(),
                label: "Local".into(),
                head: "head".into(),
                base: "head".into(),
                title: String::new(),
                original_body: None,
                files: vec![SnapshotFile {
                    path: "a.rs".into(),
                    added: 1,
                    removed: 0,
                    patch: "@@ -0,0 +1 @@\n+new".into(),
                    omitted: None,
                }],
            },
            input: "Original question".into(),
            prompt: "Exact original prompt".into(),
            generation: "request-one".into(),
        }
    }
    pub(super) fn peer(
        cx: &mut gpui::TestAppContext,
        provider: Provider,
    ) -> (
        Entity<CodexSession>,
        std::sync::mpsc::Receiver<Action>,
        std::sync::mpsc::Sender<SessionState>,
    ) {
        let (handle, actions) = Handle::channel();
        let (updates, receive) = std::sync::mpsc::channel();
        let mut state = SessionState::new(SavedSession::empty(
            "session".into(),
            provider,
            String::new(),
            "/fixture".into(),
        ));
        state.status = Status::Idle;
        state.saved.thread_id = Some("thread".into());
        let session = cx.new(|cx| {
            CodexSession::from_connection(
                state,
                Connection {
                    handle,
                    updates: receive.into(),
                },
                cx,
            )
        });
        (session, actions, updates)
    }
    pub(super) fn settle(
        cx: &mut gpui::TestAppContext,
        controller: &Entity<ChangeSession<TestResult>>,
    ) {
        for _ in 0..100 {
            cx.run_until_parked();
            if controller.read_with(cx, |v, _| !v.phase.busy()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("Controller did not settle");
    }
    #[gpui::test]
    fn recovered_job_imports_exact_response_without_resubmitting(cx: &mut gpui::TestAppContext) {
        for provider in [Provider::Codex, Provider::Claude] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::<TestResult>::new(dir.path().join("results"));
            let job = request(dir.path());
            store
                .bind(
                    "session",
                    &Source {
                        checkout: job.snapshot.checkout.clone(),
                        scope: Scope::Local,
                    },
                )
                .unwrap();
            store.save_pending("session", &job).unwrap();
            let (session, actions, _updates) = peer(cx, provider);
            session.update(cx,|s,cx|{s.state.status=Status::Done;s.state.saved.items=vec![json!({"type":"userMessage","content":[{"text":job.prompt}]}),json!({"type":"agentMessage","phase":"final_answer","text":"Exact recovered answer"}),json!({"type":"userMessage","content":[{"text":"Later question"}]}),json!({"type":"agentMessage","phase":"final_answer","text":"Unrelated"})].into();s.state.saved.work_periods.push(WorkPeriod{start_item:0,end_item:Some(2),duration_ms:None,subagents:vec![]});cx.notify();});
            let controller =
                cx.new(|cx| ChangeSession::new(session, store.clone(), "session".into(), cx));
            controller.update(cx, super::ChangeSession::hydrate);
            settle(cx, &controller);
            assert_eq!(
                controller.read_with(cx, |c, _| c.result.as_ref().unwrap().text.clone()),
                "Exact recovered answer"
            );
            assert!(!actions.try_iter().any(|a| matches!(a, Action::Prompt(_))));
            assert!(store.pending_for_session("session").unwrap().is_none());
        }
    }
    #[gpui::test]
    fn cancel_before_turn_id_interrupts_own_turn_when_it_arrives(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::<TestResult>::new(dir.path().join("results"));
        let job = request(dir.path());
        store
            .bind(
                "session",
                &Source {
                    checkout: job.snapshot.checkout.clone(),
                    scope: Scope::Local,
                },
            )
            .unwrap();
        store.save_pending("session", &job).unwrap();
        let (session, actions, _updates) = peer(cx, Provider::Codex);
        let controller =
            cx.new(|cx| ChangeSession::new(session.clone(), store.clone(), "session".into(), cx));
        controller.update(cx, |c, cx| {
            c.pending = Some(job.clone());
            c.sent = true;
            c.phase = Phase::Generating;
            c.control = Some(Arc::new(CommitControl::default()));
            c.cancel(cx);
            assert_eq!(c.phase, Phase::Generating);
        });
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
        session.update(cx, |s, cx| {
            s.state.status = Status::Working;
            s.state.turn_id = Some("own-turn".into());
            s.state
                .saved
                .items
                .push(json!({"type":"userMessage","content":[{"text":job.prompt}]}));
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            actions
                .try_iter()
                .filter(|a| matches!(a, Action::Interrupt))
                .count(),
            1
        );
        session.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
        session.update(cx, |s, cx| {
            s.state.status = Status::Done;
            s.state.turn_id = None;
            s.state.saved.work_periods.push(WorkPeriod {
                start_item: 0,
                end_item: Some(1),
                duration_ms: None,
                subagents: vec![],
            });
            cx.notify();
        });
        settle(cx, &controller);
        assert!(store.pending_for_session("session").unwrap().is_none());
        assert!(controller.read_with(cx, |c, _| c.result.is_none()
            && c.error.as_deref().is_some_and(|e| e.contains("canceled"))));
    }
    #[gpui::test]
    fn cancel_completed_request_never_interrupts_later_followup(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::<TestResult>::new(dir.path().join("results"));
        let job = request(dir.path());
        store
            .bind(
                "session",
                &Source {
                    checkout: job.snapshot.checkout.clone(),
                    scope: Scope::Local,
                },
            )
            .unwrap();
        store.save_pending("session", &job).unwrap();
        let (session, actions, _updates) = peer(cx, Provider::Claude);
        session.update(cx, |s, _| {
            s.state.status = Status::Working;
            s.state.turn_id = Some("later-turn".into());
            s.state.saved.items = vec![
                json!({"type":"userMessage","content":[{"text":job.prompt}]}),
                json!({"type":"agentMessage","text":"Completed result"}),
                json!({"type":"userMessage","content":[{"text":"Followup"}]}),
            ]
            .into();
            s.state.saved.work_periods.push(WorkPeriod {
                start_item: 0,
                end_item: Some(2),
                duration_ms: None,
                subagents: vec![],
            });
        });
        let controller = cx.new(|cx| ChangeSession::new(session, store, "session".into(), cx));
        controller.update(cx, |c, cx| {
            c.pending = Some(job);
            c.sent = true;
            c.phase = Phase::Generating;
            c.control = Some(Arc::new(CommitControl::default()));
            c.cancel(cx);
        });
        settle(cx, &controller);
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
    }

    #[gpui::test]
    fn start_captures_before_submission_and_preserves_native_draft(cx: &mut gpui::TestAppContext) {
        use grove_core::codex::WorkPeriod;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.test"]);
        fs_err::write(dir.path().join("a.rs"), "old\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "Fixture"]);
        fs_err::write(dir.path().join("a.rs"), "new\n").unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let store = Store::<TestResult>::new(dir.path().join(format!("saved-{provider:?}")));
            let (session, actions, _updates) = peer(cx, provider);
            session.update(cx, |s, _| {
                s.state.saved.draft = "Unsent native draft".into();
                s.state.error = Some("Previous request failed".into());
            });
            let controller = cx
                .new(|cx| ChangeSession::new(session.clone(), store.clone(), "session".into(), cx));
            controller.update(cx, |c, cx| {
                c.start(
                    dir.path().to_string_lossy().into_owned(),
                    Scope::Local,
                    "Question one".into(),
                    cx,
                );
            });
            let mut prompt = None;
            for _ in 0..200 {
                cx.run_until_parked();
                for action in actions.try_iter() {
                    if let Action::Prompt(text) = action {
                        prompt = Some(text);
                    }
                }
                if prompt.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let prompt = prompt.expect("captured request must reach ready provider");
            let saved = store.pending_for_session("session").unwrap().unwrap();
            assert_eq!(saved.prompt, prompt);
            assert_eq!(saved.input, "Question one");
            assert!(saved
                .snapshot
                .files
                .iter()
                .any(|f| f.path == "a.rs" && f.patch.contains("+new")));
            assert!(prompt.starts_with("Grove captured test request.\n"));
            assert_eq!(
                session.read_with(cx, |s, _| s.state.saved.draft.clone()),
                "Unsent native draft"
            );
            session.update(cx, |s, cx| {
                s.state.status = Status::Done;
                s.state.saved.items = vec![
                json!({"type":"userMessage","content":[{"text":prompt}]}),
                json!({"type":"agentMessage","phase":"final_answer","text":"Saved typed result"}),
            ]
            .into();
                s.state.saved.work_periods.push(WorkPeriod {
                    start_item: 0,
                    end_item: Some(2),
                    duration_ms: None,
                    subagents: vec![],
                });
                cx.notify();
            });
            settle(cx, &controller);
            assert_eq!(
                controller.read_with(cx, |c, _| c.result.as_ref().unwrap().input.clone()),
                "Question one"
            );
            assert!(store.pending_for_session("session").unwrap().is_none());
        }
    }
}
