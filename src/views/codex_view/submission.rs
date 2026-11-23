//! Composer submission, Dashboard launch and turn interruption.

use super::{
    active_turn_redirect_available, codex, Action, BubbleEntrance, CodexView, CodexViewEvent,
    StopConfirmation, SubmissionMode, STOP_CONFIRMATION_TIMEOUT,
};
use gpui::{Context, Entity, Window};

impl CodexView {
    pub(crate) fn attach_dashboard_tray(
        &mut self,
        tray: Entity<super::super::sidebar::dashboard::DestinationTray>,
        cx: &mut Context<Self>,
    ) {
        self.dashboard_tray_observer = Some(cx.observe(&tray, |_, _, cx| cx.notify()));
        self.dashboard_tray = Some(tray);
        self.input
            .update(cx, |input, cx| input.set_auto_grow(2, 10, cx));
    }

    pub(crate) fn launch_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dashboard_tray = None;
        self.dashboard_tray_observer = None;
        self.dashboard_handoffs.clear();
        self.input
            .update(cx, |input, cx| input.set_auto_grow(3, 10, cx));
        self.dashboard_pending = true;
        self.flush_dashboard_submission(window, cx);
        cx.notify();
    }

    pub(super) fn flush_dashboard_submission(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending
            && matches!(
                self.session.read(cx).state.status,
                codex::Status::Failed | codex::Status::Disconnected | codex::Status::Interrupted
            )
        {
            // A terminal launch failure returns the preserved draft to its composer.
            // Later readiness must not auto-send a draft the user has since edited.
            self.dashboard_pending = false;
            cx.notify();
            return;
        }
        if self.dashboard_pending
            && (self.session.read(cx).state.saved.provider == codex::Provider::Claude
                || self.session.read(cx).state.saved.thread_id.is_some())
            && (self.session.read(cx).dashboard_submission_ready()
                || self.session.read(cx).state.status == codex::Status::Idle)
        {
            self.dashboard_pending = false;
            self.submit(window, cx);
        }
    }

    pub(super) fn retry_dashboard_catalog(&mut self, cx: &mut Context<Self>) {
        if self.dashboard_tray.is_none() || !self.session.read(cx).is_deferred() {
            return;
        }
        let settings = &cx.global::<crate::settings::SettingsState>().store;
        let provider = self.session.read(cx).state.saved.provider;
        let args = provider.agent().launch_args_for_backend(
            true,
            settings.yolo_enabled,
            settings.chrome_enabled.unwrap_or(false),
        );
        self.session.update(cx, |session, cx| {
            session.retry_catalog(args, cx);
        });
    }

    pub(super) fn submit_modified_enter(
        &mut self,
        open_dashboard_transcript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open_dashboard_transcript && self.dashboard_tray.is_some() {
            self.submit_with_destination(true, window, cx);
        } else if self.session.read(cx).state.status.busy() {
            self.submit_mode(SubmissionMode::Steer, window, cx);
        } else {
            self.submit(window, cx);
        }
    }

    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_with_destination(false, window, cx);
    }

    fn submit_with_destination(
        &mut self,
        open_transcript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending {
            return;
        }
        if self.plan_answer.is_some() {
            self.save_plan_answer(window, cx);
            return;
        }
        if self.dashboard_tray.is_some() {
            // Native commands neither need metadata discovery nor activate a conversation.
            if (grove_core::describe_skill::is_invocation(self.input.read(cx).value().as_ref())
                || grove_core::review_skill::is_invocation(self.input.read(cx).value().as_ref()))
                && self.run_command(window, cx)
            {
                return;
            }
            if self.draft_action_pending(cx) {
                return;
            }
            let session = self.session.read(cx);
            if session.catalog_loading || session.catalog_error.is_some() {
                return;
            }
            if !self.input.read(cx).value().trim().is_empty() || !self.images.is_empty() {
                cx.emit(CodexViewEvent::DashboardSubmit { open_transcript });
            }
            return;
        }
        if self.edit_confirmation.is_some()
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }

        if self.queue_editor.is_some() {
            self.save_queue_edit(cx);
            return;
        }
        if let Some(id) = self.queue_selected {
            self.begin_queue_edit(id, cx);
            return;
        }
        if self.submitted.is_some() {
            return;
        }
        if self.run_command(window, cx) {
            return;
        }
        if self.draft_action_pending(cx) {
            return;
        }
        let mode = if self.session.read(cx).state.provider_work_active()
            && !self.session.read(cx).dashboard_submission_ready()
        {
            SubmissionMode::Queue
        } else {
            SubmissionMode::Send
        };
        self.submit_mode(mode, window, cx);
    }

    pub(super) fn clear_stop_confirmation(&mut self, cx: &mut Context<Self>) {
        self.stop_confirmation_timer = None;
        if self.stop_confirmation.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn validate_stop_confirmation(&mut self, cx: &mut Context<Self>) {
        let state = &self.session.read(cx).state;
        if self.stop_confirmation.as_ref().is_some_and(|confirmation| {
            confirmation.thread_id != state.saved.thread_id
                || Some(&confirmation.turn_id) != state.turn_id.as_ref()
                || !self.stop_escape_available(cx)
                || confirmation.started.elapsed() >= STOP_CONFIRMATION_TIMEOUT
        }) {
            self.clear_stop_confirmation(cx);
        }
    }

    fn stop_escape_available(&self, cx: &Context<Self>) -> bool {
        let state = &self.session.read(cx).state;
        if self.transcript_only()
            || self.dashboard_tray.is_some()
            || !state.status.busy()
            || state.turn_id.is_none()
            || self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref()
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || state.saved.pending_edit.is_some()
            || !state.approvals.is_empty()
            || !state.elicitations.is_empty()
            || !state.questions.is_empty()
        {
            return false;
        }
        true
    }

    pub(super) fn stop_with_escape(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.stop_escape_available(cx) || event.keystroke.modifiers.modified() {
            self.clear_stop_confirmation(cx);
            return false;
        }
        // Auto-repeat must neither arm a new confirmation nor confirm one.
        if event.is_held {
            return true;
        }
        self.validate_stop_confirmation(cx);
        if self.stop_confirmation.is_some() {
            self.clear_stop_confirmation(cx);
            self.interrupt(cx);
        } else {
            let state = &self.session.read(cx).state;
            let Some(turn_id) = state.turn_id.clone() else {
                return false;
            };
            self.stop_confirmation = Some(StopConfirmation {
                thread_id: state.saved.thread_id.clone(),
                turn_id,
                started: std::time::Instant::now(),
            });
            self.stop_confirmation_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(STOP_CONFIRMATION_TIMEOUT)
                    .await;
                let _ = this.update(cx, CodexView::clear_stop_confirmation);
            }));
            cx.notify();
        }
        true
    }

    pub(super) fn interrupt(&mut self, cx: &mut Context<Self>) {
        self.clear_stop_confirmation(cx);
        let state = &self.session.read(cx).state;
        if !state.status.busy()
            || state.turn_id.is_none()
            || self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref()
        {
            return;
        }
        self.interrupt_requested_for_turn = state.turn_id.clone();
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            session.send(Action::Interrupt, cx);
        });
        if self.session.read(cx).state.error.is_some() {
            self.interrupt_requested_for_turn = None;
        }
        cx.notify();
    }

    pub(super) fn submit_mode(
        &mut self,
        mode: SubmissionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending {
            return;
        }
        if self.plan_answer.is_some() {
            self.save_plan_answer(window, cx);
            return;
        }
        if self.transcript_only() {
            return;
        }
        if self.edit_confirmation.is_some()
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }
        if self.queue_editor.is_some() {
            self.save_queue_edit(cx);
            return;
        }
        if self.submitted.is_none() && self.run_command(window, cx) {
            return;
        }
        let state = &self.session.read(cx).state;
        if self.draft_action_pending(cx)
            || (mode == SubmissionMode::Steer && !active_turn_redirect_available(state))
        {
            return;
        }
        let draft = self.input.read(cx).value().to_string();
        let text = self
            .reference
            .as_ref()
            .map_or_else(|| draft.clone(), |quote| quote.serialize(&draft));
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        let result = self.session.update(cx, |session, cx| {
            codex::validate_prompt(&text, &self.images)?;
            session
                .handle
                .send(Action::DraftReference(self.reference.clone()))?;
            session.state.saved.draft.clone_from(&draft);
            session
                .state
                .saved
                .draft_reference
                .clone_from(&self.reference);
            session.submit_with_mode(text.clone(), self.images.clone(), mode, cx)
        });
        if let Ok(submission) = result {
            self.dismiss_next_reply(cx);
            if mode != SubmissionMode::Queue {
                self.record_submission_bubble(
                    submission,
                    text.clone(),
                    self.images.clone(),
                    mode == SubmissionMode::Steer,
                    false,
                    cx,
                );
            } else {
                let state = &self.session.read(cx).state;
                self.bubble_entrances.push(BubbleEntrance {
                    submission,
                    existing: state
                        .saved
                        .items
                        .iter()
                        .filter_map(|item| item["id"].as_str().map(str::to_owned))
                        .collect(),
                    thread: state.saved.thread_id.clone(),
                    text: text.clone(),
                    optimistic: None,
                });
            }
            self.submitted_reference = self.reference.as_ref().map(|quote| quote.id.clone());
            self.submitted_images = self.images.clone();
            self.submitted = Some(draft);
            self.submitted_ack = Some(submission);
            // An explicit send leaves browsing mode immediately; subsequent layout
            // changes continue to follow the new message through follow_after_submit.
            self.scroll.set_offset(gpui::point(
                self.scroll.offset().x,
                -self.scroll.max_offset().y,
            ));
            self.follow_after_submit = true;
            cx.notify();
        } else if let Err(error) = result {
            self.session.update(cx, |session, cx| {
                session.cancel_dashboard_title_waiting();
                session.state.error = Some(error);
                cx.notify();
            });
        }
    }
}
