//! UI observable projection of the background Codex actor.
use gpui::{Context, Task};
use grove_core::codex::{self, Action, Handle, SavedSession, Snapshot};
use std::time::Duration;
pub struct CodexSession {
    pub state: Snapshot,
    pub handle: Handle,
    _poll: Task<()>,
}
impl CodexSession {
    pub fn new(
        saved: SavedSession,
        args: Vec<String>,
        task_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = Snapshot::new(saved.clone());
        let connection = codex::connect(saved, args, task_id);
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
                        this.state = state;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            state,
            handle,
            _poll: poll,
        }
    }
    pub fn send(&mut self, action: Action, cx: &mut Context<Self>) {
        if let Err(e) = self.handle.send(action) {
            self.state.error = Some(e);
            cx.notify();
        }
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
            || matches!(
                self.state.status,
                codex::Status::Starting | codex::Status::Disconnected
            )
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
        self.state.error = None;
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
        self.state.error = None;
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
        self.state.error = None;
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
        self.state.error = None;
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
        self.state.saved.draft.clone_from(&text);
        self.state.saved.draft_images.clone_from(&images);
        let submission = if tracked {
            self.handle.submit(text, images)?
        } else {
            self.handle.send(Action::Prompt(text))?;
            0
        };
        self.state.error = None;
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
