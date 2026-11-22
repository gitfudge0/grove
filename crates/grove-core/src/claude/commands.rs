//! Host actions and protocol capability checks for the Claude session.

use super::{can_select_permission_mode, permission_mode, Actor, COMPACT_COMMAND};
use crate::provider::{self, Action, QueuedPrompt, Status, SubmissionMode};
use serde_json::{json, Value};

impl Actor {
    pub(super) fn action(&mut self, action: Action) -> Result<(), String> {
        if let (Some(error), Some(submission)) =
            (&self.startup_error, provider::startup_submission(&action))
        {
            if let Some(id) = submission {
                self.state.rejected_submission = Some(id);
            }
            return Err(format!(
                "{error}. Request was not sent; reconnect after resolving checkout validation."
            ));
        }

        if self.side_chat
            && !matches!(
                &action,
                Action::Submit { .. }
                    | Action::UpdateSkills(_)
                    | Action::UpdateModels(_)
                    | Action::Prompt(_)
                    | Action::Interrupt
                    | Action::Reconnect
                    | Action::Stop { .. }
            )
        {
            return Err("This action is unavailable in Side chat.".into());
        }
        if matches!(
            &action,
            Action::RewindMessage { .. }
                | Action::RewindMessageReserved { .. }
                | Action::RestoreRewindDraft { .. }
        ) {
            self.state.rewind_attempts = self.state.rewind_attempts.saturating_add(1);
        }
        if self.state.saved.pending_edit.is_some()
            && !matches!(
                action,
                Action::Reconnect
                    | Action::Stop { .. }
                    | Action::UpdateSkills(_)
                    | Action::UpdateModels(_)
                    | Action::Draft(_)
                    | Action::SetTitle(_)
                    | Action::RegenerateTitle
                    | Action::SideChat { .. }
                    | Action::SideChatDraft { .. }
                    | Action::DraftImages(_)
                    | Action::DraftReference(_)
            )
        {
            return Err("An interrupted edit needs recovery. Reconnect before continuing.".into());
        }
        match action {
            Action::MarkReviewed { work_period } => {
                self.state.mark_reviewed(work_period)?;
            }
            Action::UpdateSkills(skills) => self.state.skills = (skills).into(),
            Action::UpdateModels(models) => self.state.models = (models).into(),
            Action::Interaction { epoch, action } => {
                if epoch != self.state.interaction_epoch {
                    return Err("This interaction belongs to a previous connection".into());
                }
                return self.action(*action);
            }
            Action::UpdatePlanReview {
                plan_id,
                revision_id,
                review,
            } => {
                self.state
                    .saved
                    .rich_plans
                    .update_review(&plan_id, &revision_id, *review)?;
                self.persistence.flush(&self.state.saved)?;
            }
            Action::RetryPlanImport { message_id, source } => {
                crate::rich_plan::retry_import(&mut self.state.saved, &message_id, &source)?;
                self.persistence.flush(&self.state.saved)?;
            }
            Action::ApprovePlan {
                plan_id,
                revision_id,
            } => self.deliver_plan(&plan_id, &revision_id, true, provider::persist)?,
            Action::SendPlanResponse {
                plan_id,
                revision_id,
            } => self.send_plan_response(&plan_id, &revision_id, provider::persist)?,
            Action::RewindMessageReserved {
                index,
                message_id,
                reservation,
            } => {
                let target = self.state.rewind_target(index)?;
                let files = target
                    .files
                    .as_ref()
                    .ok_or("The workspace checkpoint is missing.")?;
                let mut reserved = reservation.roots();
                let mut expected = files.roots();
                reserved.sort();
                expected.sort();
                if reserved != expected {
                    return Err("The reserved worktrees no longer match this rewind target.".into());
                }
                self.restore_reservation = Some(reservation);
                if let Err(error) =
                    self.edit_message_with_options(index, message_id, None, true, true)
                {
                    if self.state.saved.pending_edit.is_none() {
                        self.restore_reservation = None;
                    }
                    return Err(error);
                }
            }
            Action::RewindMessage {
                index,
                message_id,
                restore_files,
            } => {
                if let Err(error) =
                    self.edit_message_with_options(index, message_id, None, restore_files, true)
                {
                    if self.state.saved.pending_edit.is_none() {
                        self.restore_reservation = None;
                    }
                    return Err(error);
                }
            }
            Action::RestoreRewindDraft { id } => {
                let mut saved = self.state.saved.clone();
                saved.restore_rewind_draft(&id)?;
                self.persistence.flush(&saved)?;
                self.state.saved = saved;
                self.state.edited_messages += 1;
            }
            Action::EditMessage { index, message_id } => {
                self.edit_message(index, message_id, None)?;
            }
            Action::EditMessageInline { index, message_id } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.edit_message(index, message_id, Some(draft))?;
            }
            Action::PromptInline { text, images } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.submit(0, text, images)?;
                self.inline_uuid.clone_from(&self.input_uuid);
                self.deferred_inline = self.deferred.is_some();
                self.state.saved.draft = draft.0;
                self.state.saved.draft_images = draft.1;
            }
            Action::ClearConversation => {
                self.persistence.flush(&self.state.saved)?;
                self.clear_conversation(provider::persist)?;
                if let Err(error) = self.start() {
                    self.fail(error);
                }
            }
            Action::Prompt(text) => self.submit(0, text, Vec::new())?,
            Action::RefreshContext => self.refresh_context(),
            Action::Compact => {
                if self.process.is_none() {
                    return Err(provider::COMPACT_DISCONNECTED.into());
                }
                if self.state.status.busy() || self.in_flight.is_some() {
                    return Err(provider::COMPACT_BUSY.into());
                }
                if !self.initialized {
                    return Err(provider::COMPACT_DISCONNECTED.into());
                }
                if self.state.saved.thread_id.is_none()
                    || !provider::has_conversation(&self.state.saved.items)
                {
                    return Err(provider::NOTHING_TO_COMPACT.into());
                }
                // Claude Code's own slash command, through the ordinary prompt path.
                self.submit(0, COMPACT_COMMAND.into(), Vec::new())?;
                self.compact_pending = true;
            }
            Action::PromptWithImages { text, images } => self.submit(0, text, images)?,
            Action::Submit { id, text, images } => self.submit(id, text, images)?,
            Action::ActivityPrompt {
                id,
                text,
                mode,
                expected_turn,
            } => {
                let result = (|| {
                    provider::validate_prompt(&text, &[])?;
                    if mode == SubmissionMode::Steer {
                        if expected_turn.is_none()
                            || self.state.turn_id != expected_turn
                            || !self.state.status.busy()
                        {
                            return Err("The active turn changed. Your reply was not sent; review it before sending again.".into());
                        }
                        self.state.queued_prompts.push(QueuedPrompt {
                            id,
                            text,
                            images: Vec::new(),
                        });
                        self.action(Action::SteerQueued(id))
                    } else {
                        if mode == SubmissionMode::Send
                            && (self.state.status.busy() || self.state.turn_id.is_some())
                        {
                            return Err("The session started working. Your reply was not sent; queue it when ready.".into());
                        }
                        self.action(Action::ProjectPrompt { id, text })
                    }
                })();
                if let Err(error) = result {
                    if !self.state.queue_uncertain.contains(&id) {
                        self.state.queued_prompts.retain(|prompt| prompt.id != id);
                    }
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::ProjectPrompt { id, text } => {
                provider::validate_prompt(&text, &[])?;
                if self.state.status == Status::Disconnected {
                    self.state.rejected_submission = Some(id);
                    return Err("Reconnect before sending a prompt".into());
                }
                self.state.queued_prompts.push(QueuedPrompt {
                    id,
                    text,
                    images: Vec::new(),
                });
                self.state.accept_submission(id);
            }
            Action::SubmitWithMode {
                id,
                text,
                images,
                mode,
            } => match mode {
                SubmissionMode::Send => self.submit(id, text, images)?,
                SubmissionMode::Steer => {
                    if let Err(error) = self.redirect(id, text, images) {
                        self.state.rejected_submission = Some(id);
                        return Err(error);
                    }
                }
                SubmissionMode::Queue => {
                    if let Err(error) = provider::validate_prompt(&text, &images) {
                        self.state.rejected_submission = Some(id);
                        return Err(error);
                    }
                    self.state.saved.clear_submitted_draft(&text, &images);
                    self.state
                        .queued_prompts
                        .push(QueuedPrompt { id, text, images });
                    self.state.accept_submission(id);
                }
            },
            Action::BeginQueuedEdit(id) => self.state.begin_queue_edit(id)?,
            Action::SaveQueuedEdit { id, text, images } => {
                self.state.save_queue_edit(id, text, images)?;
            }
            Action::CancelQueuedEdit(id) => {
                if self.state.queue_edit == Some(id) {
                    self.state.queue_edit = None;
                }
            }
            Action::SteerQueued(id) => {
                if !self.state.queue_editable(id) || self.state.queue_edit == Some(id) {
                    return Err("This queued input cannot be steered right now".into());
                }
                let prompt = self
                    .state
                    .queued_prompts
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
                    .ok_or("Queued input unavailable")?;
                self.redirect(id, prompt.text, prompt.images)?;
            }
            Action::RemoveQueued(id) => self.state.remove_queued(id)?,
            Action::DismissUncertainQueued(id) => {
                self.state.dismiss_uncertain_queued(id)?;
                if self.deferred.as_ref().is_some_and(|p| p.id == id) {
                    self.deferred = None;
                }
                self.queue_ready = false;
            }
            Action::ResumeQueue => {
                if !self.state.queue_uncertain.is_empty() {
                    return Err(
                        "Inspect and dismiss unconfirmed deliveries before resuming queued inputs"
                            .into(),
                    );
                }
                if self.state.queued_prompts.is_empty() {
                    return Err("There are no queued prompts to send.".into());
                }
                if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting
                {
                    return Err("Wait for the current turn before resuming queued inputs".into());
                }
                if self.process.is_none() {
                    return Err("Reconnect before resuming queued inputs".into());
                }
                self.queue_ready = true;
            }
            Action::DraftReference(reference) => {
                if let Some(reference) = &reference {
                    provider::validate_prompt(
                        &reference.serialize(&self.state.saved.draft),
                        &self.state.saved.draft_images,
                    )?;
                }
                self.state.saved.draft_reference = reference;
            }
            Action::RegenerateTitle => {
                self.title_generator.cancel();
                crate::session_title::request_regeneration(&mut self.state.saved);
            }
            Action::SetTitle(title) => {
                if !crate::session_title::set_manual_title(&mut self.state.saved, &title) {
                    return Err("Title must contain 1–80 characters".into());
                }
                self.title_generator.cancel();
            }
            Action::Draft(text) => {
                if text.len() > 512 * 1024 {
                    return Err("Draft text limit: 512 KiB".into());
                }
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((draft, _)) = &mut edit.composer_draft {
                        draft.clone_from(&text);
                    }
                }
                self.state.saved.draft = text;
            }
            Action::SideChat { checkout, chat } => {
                self.state.saved.side_chats.insert(checkout, chat);
            }
            Action::SideChatDraft { checkout, draft } => {
                self.state
                    .saved
                    .side_chats
                    .entry(checkout)
                    .or_default()
                    .draft = draft;
            }
            Action::DraftImages(images) => {
                provider::validate_images(&images)?;
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((_, draft_images)) = &mut edit.composer_draft {
                        draft_images.clone_from(&images);
                    }
                }
                self.state.saved.draft_images = images;
            }
            Action::Metadata { project, roots } => {
                self.state.saved.project = project;
                if self.state.saved.roots != roots {
                    self.reconfigure = true;
                    self.reset_configuration = true;
                }
                self.state.saved.roots = roots;
            }
            Action::Configure {
                model,
                effort,
                access,
            } => {
                let configure_model_effort = model.is_some() || effort.is_some();
                if let Some(effort) = &effort {
                    if !["low", "medium", "high", "xhigh", "max"].contains(&effort.as_str()) {
                        return Err(format!("Unsupported Claude Code effort: {effort}"));
                    }
                }
                if let Some(access) = &access {
                    let mode = permission_mode(access)?;
                    if !can_select_permission_mode(&self.state.saved, mode) {
                        return Err("Bypass permissions is unavailable for this Claude Code session because it was launched without bypass permission support".into());
                    }
                    if !self.state.permission_modes.is_empty()
                        && !self
                            .state
                            .permission_modes
                            .iter()
                            .any(|value| value == mode)
                    {
                        return Err(format!(
                            "This Claude Code version does not advertise permission mode {mode}"
                        ));
                    }
                }
                if let Some(model) = model {
                    if self.state.model.as_deref() != Some(model.as_str()) {
                        self.state.context_revision = self.state.context_revision.wrapping_add(1);
                        self.state.effort = None;
                        self.state.saved.composer.effort = None;
                    }
                    self.state.model = Some(model.clone());
                    self.state.saved.composer.model = Some(model);
                }
                if let Some(effort) = effort {
                    self.state.effort = Some(effort.clone());
                    self.state.saved.composer.effort = Some(effort);
                }
                if let Some(access) = access {
                    self.pending_access = Some(permission_mode(&access)?.into());
                }
                self.state.saved.composer.reset_to_configured = false;
                self.reconfigure |= configure_model_effort;
                self.configure_pending_access()?;
            }

            Action::SetSpeed(speed) => {
                if let Some(reason) = self.state.speed_unavailable(speed) {
                    return Err(reason.into());
                }
                self.state.saved.composer.speed = Some(speed);
                self.reconfigure = true;
            }
            Action::ResetModelEffort => {
                // Restarting is required to remove a live model override. Preserve
                // inherited permissions explicitly so that restart cannot reset them.
                if self.state.access.is_none() {
                    self.state.access = self.state.resolved_access.clone();
                }
                self.state.saved.composer.model = None;
                self.state.saved.composer.effort = None;
                self.state.saved.composer.access = self.state.access.clone();
                self.state.model = None;
                self.state.effort = None;
                self.state.resolved_model = None;
                self.state.resolved_effort = None;
                self.reset_configuration = true;
                self.reconfigure = true;
            }
            Action::ResetConfiguration => {
                self.pending_access = None;
                self.permission_changes.clear();
                self.reset_configuration = true;
                self.state.saved.composer = provider::ComposerSettings::default();
                self.state.model = None;
                self.state.effort = None;
                self.state.access = None;
                self.reconfigure = true;
            }
            Action::Interrupt => {
                if !self.state.status.busy() {
                    return Err("No active Claude Code turn to interrupt".into());
                }
                self.queue_ready = false;
                self.interrupting = true;
                self.control(json!({"subtype":"interrupt"}))?;
                self.state.questions.clear();
                self.state.elicitations.clear();
                self.state.approvals.clear();
            }
            Action::AnswerQuestions(id, answers) => {
                let at = self
                    .state
                    .questions
                    .iter()
                    .position(|q| q.id == id)
                    .ok_or("This question request has expired")?;
                let request = &self.state.questions[at];
                let answered = answers.is_some();
                let response = if let Some(answers) = answers {
                    request.validate(&answers)?;
                    request.claude_response(answers)
                } else {
                    json!({"behavior":"deny","message":"The user cancelled these questions."})
                };
                self.respond(id.clone(), response)?;
                if answered {
                    let ack = self.state.question_answer_ack(id);
                    self.state.question_answer_acks.push(ack);
                }
                let resolved = self.state.questions[at].id.clone();
                self.state.retire_interaction(&resolved);
                self.state.questions.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::Approval(id, accept) => {
                return self.action(Action::RespondApproval {
                    id,
                    choice: if accept { "allow_once" } else { "deny" }.into(),
                    input: None,
                    feedback: None,
                });
            }
            Action::RespondApproval {
                id,
                choice,
                input,
                feedback,
            } => {
                let at = self
                    .state
                    .approvals
                    .iter()
                    .position(|a| a["id"] == id)
                    .ok_or("This approval has expired")?;
                let response = crate::controls::approval_response(
                    &self.state.approvals[at],
                    &choice,
                    input,
                    feedback,
                )?;
                self.respond(id, response)?;
                let resolved = self.state.approvals[at]["id"].clone();
                self.state.retire_interaction(&resolved);
                self.state.approvals.remove(at);
                if choice == "cancel" {
                    self.interrupting = true;
                    self.queue_ready = false;
                    self.state.approvals.clear();
                    self.state.questions.clear();
                    self.state.elicitations.clear();
                }
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::RespondElicitation {
                id,
                action,
                content,
            } => {
                let at = self
                    .state
                    .elicitations
                    .iter()
                    .position(|r| r.id == id)
                    .ok_or("This request has expired")?;
                if !matches!(action.as_str(), "accept" | "decline" | "cancel") {
                    return Err("Invalid elicitation action".into());
                }
                if action == "accept" {
                    self.state.elicitations[at]
                        .validate_content(content.as_ref().unwrap_or(&Value::Null))?;
                }
                let content = if action == "accept" {
                    match &self.state.elicitations[at].mode {
                        crate::elicitation::ElicitationMode::Form(form) => Some(
                            form.validate(
                                content
                                    .as_ref()
                                    .and_then(Value::as_object)
                                    .ok_or("Form response must be an object")?,
                            )
                            .map_err(|_| "Invalid form content")?,
                        ),
                        crate::elicitation::ElicitationMode::Url { .. } => None,
                    }
                } else {
                    None
                };
                let response = if action == "accept" && content.is_some() {
                    json!({"action":action,"content":content})
                } else {
                    json!({"action":action})
                };
                self.respond(id, response)?;
                let resolved = self.state.elicitations[at].id.clone();
                self.state.retire_interaction(&resolved);
                self.state.elicitations.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    if self.state.turn_id.is_some() {
                        Status::Working
                    } else {
                        Status::Idle
                    }
                } else {
                    Status::Waiting
                };
            }
            Action::Reconnect => {
                self.queue_ready = false;
                self.state.reconnecting = true;
                if let Err(error) = self.start() {
                    self.state.reconnecting = false;
                    return Err(error);
                }
            }
            Action::Stop { .. } => unreachable!("Stop handled by run loop"),
        }
        Ok(())
    }
}
