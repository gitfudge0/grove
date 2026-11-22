//! Provider commands, submission handles, and deferred actor activation.

use super::{QuestionAnswers, ReferenceDraft, SavedSideChat, Snapshot, Speed, SubmissionMode};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    mpsc, Arc,
};

pub type StartupValidation = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub(crate) fn startup_submission(action: &Action) -> Option<Option<u64>> {
    match action {
        Action::Submit { id, .. }
        | Action::SubmitWithMode { id, .. }
        | Action::ProjectPrompt { id, .. }
        | Action::ActivityPrompt { id, .. } => Some(Some(*id)),
        Action::Prompt(_) | Action::PromptWithImages { .. } | Action::PromptInline { .. } => {
            Some(None)
        }
        _ => None,
    }
}
pub enum Action {
    /// Metadata-only refresh; never reconnects or alters the conversation.
    UpdateSkills(Vec<Value>),
    /// Replace advertised model metadata without reconnecting or configuring the provider.
    UpdateModels(Vec<Value>),
    /// Explicitly chosen title; automatic generation must preserve it.
    SetTitle(String),
    /// Regenerate from bounded conversation context and resume automatic naming.
    RegenerateTitle,
    /// A receipt for the exact completed result displayed by the native UI.
    MarkReviewed {
        work_period: usize,
    },
    /// UI responses are bound to the connection that rendered the interaction.
    Interaction {
        epoch: u64,
        action: Box<Action>,
    },
    UpdatePlanReview {
        plan_id: String,
        revision_id: String,
        review: Box<crate::rich_plan::ReviewState>,
    },
    SendPlanResponse {
        plan_id: String,
        revision_id: String,
    },
    ApprovePlan {
        plan_id: String,
        revision_id: String,
    },
    RetryPlanImport {
        message_id: String,
        source: String,
    },
    /// Start a fresh provider conversation while retaining the Grove session.
    ClearConversation,
    EditMessage {
        index: usize,
        message_id: String,
    },
    RewindMessage {
        index: usize,
        message_id: String,
        restore_files: bool,
    },
    RewindMessageReserved {
        index: usize,
        message_id: String,
        reservation: crate::checkpoint::RestoreReservation,
    },
    RestoreRewindDraft {
        id: String,
    },
    EditMessageInline {
        index: usize,
        message_id: String,
    },
    PromptInline {
        text: String,
        images: Vec<String>,
    },
    ResetConfiguration,
    /// Restore configured model and effort without resetting effective access.
    ResetModelEffort,
    SetSpeed(Speed),
    Configure {
        model: Option<String>,
        effort: Option<String>,
        access: Option<String>,
    },
    /// An explicit project action enters the native queue without consuming the composer.
    ProjectPrompt {
        id: u64,
        text: String,
    },
    /// Activity reply owns a separate draft; steer is bound to the observed turn.
    ActivityPrompt {
        id: u64,
        text: String,
        mode: SubmissionMode,
        expected_turn: Option<String>,
    },
    SubmitWithMode {
        id: u64,
        text: String,
        images: Vec<String>,
        mode: SubmissionMode,
    },
    RemoveQueued(u64),
    BeginQueuedEdit(u64),
    SaveQueuedEdit {
        id: u64,
        text: String,
        images: Vec<String>,
    },
    CancelQueuedEdit(u64),
    SteerQueued(u64),
    DismissUncertainQueued(u64),
    ResumeQueue,
    Submit {
        id: u64,
        text: String,
        images: Vec<String>,
    },
    Prompt(String),
    PromptWithImages {
        text: String,
        images: Vec<String>,
    },
    DraftImages(Vec<String>),
    DraftReference(Option<ReferenceDraft>),
    /// Compact the conversation context (`/compact`); rejected while a turn is active.
    Compact,
    /// Request supported provider context attribution without submitting a prompt.
    RefreshContext,
    Interrupt,
    Approval(Value, bool),
    RespondApproval {
        id: Value,
        choice: String,
        input: Option<Value>,
        feedback: Option<String>,
    },
    RespondElicitation {
        id: Value,
        action: String,
        content: Option<Value>,
    },
    AnswerQuestions(Value, Option<QuestionAnswers>),
    Reconnect,
    Draft(String),
    SideChat {
        checkout: String,
        chat: SavedSideChat,
    },
    SideChatDraft {
        checkout: String,
        draft: String,
    },
    Metadata {
        project: String,
        roots: Vec<crate::session_meta::ContextRoot>,
    },
    Stop {
        forget: bool,
        done: mpsc::Sender<()>,
        archive_outcome: Option<mpsc::Sender<Result<(), String>>>,
    },
}
#[derive(Clone)]
pub struct Handle(
    pub(crate) mpsc::Sender<Action>,
    pub(crate) Arc<AtomicU32>,
    pub(crate) Arc<AtomicBool>,
    pub(crate) Arc<AtomicU64>,
    Option<Arc<dyn Fn() + Send + Sync>>,
);
impl Handle {
    /// Create the actor command channel independently of process startup.
    /// Also permits embedding hosts to exercise the UI with a deterministic protocol peer.
    pub fn channel() -> (Self, mpsc::Receiver<Action>) {
        let (send, receive) = mpsc::channel();
        (
            Self(
                send,
                Arc::new(AtomicU32::new(0)),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU64::new(1)),
                None,
            ),
            receive,
        )
    }
    /// Bind a native actor's command queue to its thread wake token. Commands
    /// queued before startup remain in the queue and are drained before parking.
    pub(crate) fn actor_channel() -> (
        Self,
        mpsc::Receiver<Action>,
        Arc<std::sync::OnceLock<std::thread::Thread>>,
    ) {
        let thread = Arc::new(std::sync::OnceLock::<std::thread::Thread>::new());
        let wake = thread.clone();
        let (handle, receiver) = Self::channel_with_wake(Arc::new(move || {
            if let Some(thread) = wake.get() {
                thread.unpark();
            }
        }));
        (handle, receiver, thread)
    }
    /// Wake an embedding host after each command is queued. The callback must be
    /// nonblocking; hosts may coalesce notifications and drain the command queue.
    pub fn channel_with_wake(wake: Arc<dyn Fn() + Send + Sync>) -> (Self, mpsc::Receiver<Action>) {
        let (mut handle, receiver) = Self::channel();
        handle.4 = Some(wake);
        (handle, receiver)
    }
    pub fn submit(&self, text: String, images: Vec<String>) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::Submit { id, text, images })?;
        Ok(id)
    }
    pub fn project_prompt(&self, text: String) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::ProjectPrompt { id, text })?;
        Ok(id)
    }
    pub fn activity_prompt(
        &self,
        text: String,
        mode: SubmissionMode,
        expected_turn: Option<String>,
    ) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::ActivityPrompt {
            id,
            text,
            mode,
            expected_turn,
        })?;
        Ok(id)
    }
    pub fn submit_with_mode(
        &self,
        text: String,
        images: Vec<String>,
        mode: SubmissionMode,
    ) -> Result<u64, String> {
        if mode == SubmissionMode::Send {
            return self.submit(text, images);
        }
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::SubmitWithMode {
            id,
            text,
            images,
            mode,
        })?;
        Ok(id)
    }
    pub fn send(&self, action: Action) -> Result<(), String> {
        self.0
            .send(action)
            .map_err(|_| "Provider session is disconnected".to_owned())?;
        if let Some(wake) = &self.4 {
            wake();
        }
        Ok(())
    }
    pub fn stop_completion(&self, forget: bool) -> mpsc::Receiver<()> {
        self.stop_with_outcome(forget, None)
    }
    pub fn stop_archiving(&self) -> (mpsc::Receiver<()>, mpsc::Receiver<Result<(), String>>) {
        let (send, receive) = mpsc::channel();
        (self.stop_with_outcome(true, Some(send)), receive)
    }
    fn stop_with_outcome(
        &self,
        forget: bool,
        archive_outcome: Option<mpsc::Sender<Result<(), String>>>,
    ) -> mpsc::Receiver<()> {
        self.2.store(true, Ordering::SeqCst);
        let pid = self.1.swap(0, Ordering::SeqCst);
        #[cfg(not(unix))]
        let _ = pid;
        // Process termination is immediate even if the UI still holds this entity.
        #[cfg(unix)]
        if pid != 0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let (done, receive) = mpsc::channel();
        let _ = self.send(Action::Stop {
            forget,
            done,
            archive_outcome,
        });
        receive
    }
    pub fn stop(&self, forget: bool) {
        self.stop_completion(forget);
    }
}
/// A stable command identity whose actor is created only when a hidden session
/// is selected or receives an action. Queued commands survive activation.
pub struct DeferredConnection {
    pub handle: Handle,
    pub(crate) receiver: mpsc::Receiver<Action>,
    pub(crate) thread: Arc<std::sync::OnceLock<std::thread::Thread>>,
}
impl DeferredConnection {
    /// Take commands that could not be delivered during hydration. The host
    /// must preserve draft edits, reject unsent requests and answer Stop receipts.
    pub fn take_pending_actions(&self) -> Vec<Action> {
        self.receiver.try_iter().collect()
    }
    /// Fail a permanently abandoned dormant connection without starting a CLI.
    /// Explicit close callers still receive an archival error and completion;
    /// the original recovery files remain untouched for a later recovery attempt.
    pub fn reject_pending(self, error: String) {
        for action in self.receiver.try_iter() {
            if let Action::Stop {
                done,
                archive_outcome,
                ..
            } = action
            {
                if let Some(outcome) = archive_outcome {
                    let _ = outcome.send(Err(error.clone()));
                }
                let _ = done.send(());
            }
        }
    }
}
pub fn deferred_connection(wake: Arc<dyn Fn() + Send + Sync>) -> DeferredConnection {
    let thread = Arc::new(std::sync::OnceLock::<std::thread::Thread>::new());
    let actor = thread.clone();
    let (handle, receiver) = Handle::channel_with_wake(Arc::new(move || {
        if let Some(thread) = actor.get() {
            thread.unpark();
        } else {
            wake();
        }
    }));
    DeferredConnection {
        handle,
        receiver,
        thread,
    }
}
pub struct Connection {
    pub handle: Handle,
    pub updates: crate::latest::Receiver<Snapshot>,
}
