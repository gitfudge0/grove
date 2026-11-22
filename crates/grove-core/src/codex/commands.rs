//! Host actions and admission checks for the Codex session.

use super::{Actor, Pending, StopTracking};
use crate::provider::{
    archive_closed_session, persist, startup_submission, validate_images, validate_prompt, Action,
    ComposerSettings, QuestionSource, QueuedPrompt, Status, SubmissionMode,
};
use serde_json::{json, Value};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl Actor {
    pub(super) fn action(&mut self, action: Action) -> Result<bool, String> {
        if let (Some(error), Some(submission)) = (&self.startup_error, startup_submission(&action))
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
        if self.stop_tracking.stopping
            && matches!(
                &action,
                Action::Submit { .. }
                    | Action::SubmitWithMode { .. }
                    | Action::ProjectPrompt { .. }
                    | Action::ActivityPrompt { .. }
                    | Action::Prompt(_)
                    | Action::PromptWithImages { .. }
                    | Action::PromptInline { .. }
                    | Action::ResumeQueue
                    | Action::SteerQueued(_)
                    | Action::Compact
                    | Action::Reconnect
                    | Action::ApprovePlan { .. }
                    | Action::SendPlanResponse { .. }
                    | Action::RewindMessage { .. }
                    | Action::RewindMessageReserved { .. }
                    | Action::EditMessage { .. }
                    | Action::EditMessageInline { .. }
            )
        {
            if let Action::Submit { id, .. }
            | Action::SubmitWithMode { id, .. }
            | Action::ProjectPrompt { id, .. }
            | Action::ActivityPrompt { id, .. } = &action
            {
                self.state.rejected_submission = Some(*id);
            }
            return Err("Stop is still checking running Codex work. Wait for it to finish before continuing.".into());
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
            if let Action::Submit { id, .. }
            | Action::SubmitWithMode { id, .. }
            | Action::ProjectPrompt { id, .. }
            | Action::ActivityPrompt { id, .. } = &action
            {
                self.state.rejected_submission = Some(*id);
            }
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
            } => self.deliver_plan(&plan_id, &revision_id, true, persist)?,
            Action::SendPlanResponse {
                plan_id,
                revision_id,
            } => self.send_plan_response(&plan_id, &revision_id, persist)?,
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
            Action::EditMessage { index, message_id } => self.edit_message(index, message_id)?,
            Action::EditMessageInline { index, message_id } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.edit_message_with_draft(index, message_id, Some(draft))?;
            }
            Action::PromptInline { text, images } => {
                validate_prompt(&text, &images)?;
                if self.state.status.busy()
                    || self.state.turn_id.is_some()
                    || !self.pending.is_empty()
                {
                    return Err("Wait for the provider before sending the edited message.".into());
                }
                if self.process.is_none() || self.state.saved.thread_id.is_none() {
                    return Err("Reconnect before sending the edited message.".into());
                }
                self.capture_edit_checkpoint();
                self.request(
                    "turn/start",
                    self.turn_params(self.input(&text, images)),
                    Pending::InlineTurn,
                )?;
                self.state.status = Status::Working;
                self.state.clear_error();
            }
            Action::ClearConversation => {
                self.persistence.flush(&self.state.saved)?;
                self.clear_conversation(persist)?;
                if let Err(error) = self.start() {
                    self.state.fail(error);
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                }
            }
            Action::SetSpeed(speed) => {
                if let Some(reason) = self.state.speed_unavailable(speed) {
                    return Err(reason.into());
                }
                self.state.saved.composer.speed = Some(speed);
            }
            Action::ResetModelEffort => {
                if let Some(details) = self.state.saved.context_details.as_mut() {
                    details.accounting = None;
                }
                self.state.model = None;
                self.state.effort = None;
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: true,
                    reset_model_effort_only: true,
                    access: self.state.access.clone(),
                    speed: self.state.saved.composer.speed,
                    ..Default::default()
                };
                self.state.resolved_model =
                    self.configured_turn["model"].as_str().map(str::to_owned);
                self.state.resolved_effort =
                    self.configured_turn["effort"].as_str().map(str::to_owned);
                self.reset_configuration = true;
            }
            Action::ResetConfiguration => {
                if let Some(details) = self.state.saved.context_details.as_mut() {
                    details.accounting = None;
                }
                self.state.model = None;
                self.state.effort = None;
                self.state.access = None;
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: true,
                    ..Default::default()
                };
                self.state.resolved_model =
                    self.configured_turn["model"].as_str().map(str::to_owned);
                self.state.resolved_effort =
                    self.configured_turn["effort"].as_str().map(str::to_owned);
                self.state.resolved_access =
                    match self.configured_turn["sandboxPolicy"]["type"].as_str() {
                        Some("dangerFullAccess") => Some("full-access".into()),
                        Some("readOnly") => Some("read-only".into()),
                        Some("workspaceWrite") => Some("workspace-write".into()),
                        _ => None,
                    };
                self.state.sandbox = self.configured_turn.get("sandboxPolicy").cloned();
                self.state.approval_policy = self.configured_turn.get("approvalPolicy").cloned();
                self.reset_configuration = true;
            }
            Action::Configure {
                model,
                effort,
                access,
            } => {
                if let Some(value) = &access {
                    if !["full-access", "read-only", "workspace-write"].contains(&value.as_str()) {
                        return Err("Unknown access mode".into());
                    }
                }
                if let Some(value) = &effort {
                    if ![
                        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
                    ]
                    .contains(&value.as_str())
                    {
                        return Err("Unknown reasoning effort".into());
                    }
                }
                if let Some(model) = model {
                    if self
                        .state
                        .saved
                        .context_details
                        .as_ref()
                        .and_then(|details| details.model.as_deref())
                        != Some(model.as_str())
                    {
                        if let Some(details) = self.state.saved.context_details.as_mut() {
                            details.accounting = None;
                        }
                    }
                    // App-server effort overrides survive subsequent turns. Send the new
                    // model's default explicitly so a previous model's effort cannot leak.
                    self.state.effort = self
                        .state
                        .models
                        .iter()
                        .find(|entry| {
                            entry["model"].as_str() == Some(model.as_str())
                                || entry["id"].as_str() == Some(model.as_str())
                        })
                        .and_then(|entry| entry["defaultReasoningEffort"].as_str())
                        .map(str::to_owned);
                    self.state.model = Some(model);
                }
                if let Some(effort) = effort {
                    self.state.effort = Some(effort);
                }
                if let Some(access) = access {
                    self.state.access = Some(access);
                }
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: self.reset_configuration,
                    reset_model_effort_only: self.state.saved.composer.reset_model_effort_only,
                    model: self.state.model.clone(),
                    effort: self.state.effort.clone(),
                    access: self.state.access.clone(),
                    speed: self.state.saved.composer.speed,
                };
            }
            Action::DismissUncertainQueued(id) => {
                self.state.dismiss_uncertain_queued(id)?;
                self.compaction_queue = None;
                self.queue_paused = true;
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
                self.queue_paused = false;
                self.queue_ready = true;
                self.dispatch_queue()?;
            }
            Action::BeginQueuedEdit(id) => self.state.begin_queue_edit(id)?,
            Action::SaveQueuedEdit { id, text, images } => {
                self.state.save_queue_edit(id, text, images)?;
                self.dispatch_queue()?;
            }
            Action::CancelQueuedEdit(id) => {
                if self.state.queue_edit == Some(id) {
                    self.state.queue_edit = None;
                }
                self.dispatch_queue()?;
            }
            Action::RemoveQueued(id) => {
                self.state.remove_queued(id)?;
                self.dispatch_queue()?;
            }
            Action::SteerQueued(id) => {
                if !self.state.queue_editable(id) || self.state.queue_edit == Some(id) {
                    return Err("This queued input cannot be steered right now".into());
                }
                let turn = self
                    .state
                    .turn_id
                    .clone()
                    .filter(|_| self.state.status.busy())
                    .ok_or("No active Codex turn to steer")?;
                let draft = self
                    .state
                    .queued_prompts
                    .iter()
                    .find(|p| p.id == id)
                    .ok_or("This queued input cannot be steered right now")?
                    .clone();
                let input = self.input(&draft.text, draft.images.clone());
                if self.process.is_none() {
                    return Err("Reconnect before steering a queued input".into());
                }
                self.state.queue_sending.push(id);
                self.state.queue_steering.push(id);
                self.request("turn/steer", json!({"threadId":self.state.saved.thread_id,"expectedTurnId":turn,"input":input}), Pending::Steer { submission: id, turn, draft, from_queue: true }).inspect_err(|_| { self.compaction_queue = None; self.queue_paused = true; self.queue_ready = false; self.state.mark_queue_uncertain(); })?;
                self.state.clear_error();
            }
            Action::ActivityPrompt {
                id,
                text,
                mode,
                expected_turn,
            } => {
                let result = (|| {
                    validate_prompt(&text, &[])?;
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
                        self.action(Action::SteerQueued(id)).map(|_| ())
                    } else {
                        if mode == SubmissionMode::Send
                            && (self.state.status.busy() || self.state.turn_id.is_some())
                        {
                            return Err("The session started working. Your reply was not sent; queue it when ready.".into());
                        }
                        self.action(Action::ProjectPrompt { id, text }).map(|_| ())
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
                let draft = self.state.saved.draft.clone();
                let images = self.state.saved.draft_images.clone();
                let reference = self.state.saved.draft_reference.clone();
                let result = self.submit_mode(id, text, Vec::new(), SubmissionMode::Queue);
                self.state.saved.draft = draft;
                self.state.saved.draft_images = images;
                self.state.saved.draft_reference = reference;
                if let Err(error) = result {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::SubmitWithMode {
                id,
                text,
                images,
                mode,
            } => {
                if let Err(error) = self.submit_mode(id, text, images, mode) {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::Stop {
                forget,
                done,
                archive_outcome,
            } => {
                self.compaction_queue = None;
                self.pid.store(0, Ordering::SeqCst);
                self.process = None;
                let outcome = if forget && !self.side_chat {
                    self.save();
                    archive_closed_session(&self.state.saved)
                } else {
                    self.save();
                    Ok(())
                };
                if let Err(error) = &outcome {
                    self.save();
                    eprintln!("Could not archive closed native session: {error}");
                }
                if let Some(sender) = archive_outcome {
                    let _ = sender.send(outcome);
                }
                let _ = done.send(());
                return Ok(false);
            }
            Action::Reconnect => {
                self.state.context_measurement_live = false;
                if self.state.status.busy() {
                    return Err("Interrupt the current turn before reconnecting".into());
                }
                self.pid.store(0, Ordering::SeqCst);
                self.process = None;
                self.state.reconnecting = true;
                if let Err(error) = self.start() {
                    self.state.fail(error);
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                }
            }
            Action::Prompt(text) => self.prompt(text)?,
            Action::Compact => self.compact()?,
            Action::RefreshContext => {
                self.state.context_query_error =
                    Some("Codex does not report a content breakdown.".into());
            }
            Action::PromptWithImages { text, images } => {
                self.prompt_with_images(text, images, None)?;
            }
            Action::Submit { id, text, images } => {
                if let Err(error) = self.prompt_with_images(text, images, Some(id)) {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::DraftReference(reference) => {
                if let Some(reference) = &reference {
                    validate_prompt(
                        &reference.serialize(&self.state.saved.draft),
                        &self.state.saved.draft_images,
                    )?;
                }
                self.state.saved.draft_reference = reference;
            }
            Action::DraftImages(images) => {
                validate_images(&images)?;
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((_, draft_images)) = &mut edit.composer_draft {
                        draft_images.clone_from(&images);
                    }
                }
                self.state.saved.draft_images = images;
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
            Action::Metadata { project, roots } => {
                self.state.saved.project = project;
                self.state.saved.roots = roots;
            }
            Action::Interrupt => {
                self.compaction_queue = None;
                self.queue_paused = true;
                self.queue_ready = false;
                let turn = self
                    .state
                    .turn_id
                    .clone()
                    .ok_or("No active Codex turn to interrupt")?;
                self.interrupt(turn.clone(), 0)?;
                if self.stop_tracking.root != turn {
                    self.stop_tracking = StopTracking {
                        root: turn,
                        ..Default::default()
                    };
                }
                self.stop_tracking.stopping = true;
                self.stop_tracking.settled_at = None;
                self.stop_tracking.root_pending = true;
                self.stop_tracking.deadline = Some(Instant::now() + Duration::from_secs(30));
                self.stop_descendants()?;
                if self
                    .stop_tracking
                    .candidate_turns
                    .keys()
                    .any(|id| !self.interaction_children.known(id))
                {
                    self.stop_error("Stop could not establish ownership of every Codex child turn. Check running work manually.".into());
                }
                if !self.stop_tracking.missing_process.is_empty() {
                    self.stop_error("Stop could not identify a Codex background terminal. Check running work manually.".into());
                }
                if self
                    .interaction_children
                    .active_ids()
                    .iter()
                    .any(|id| !self.stop_tracking.turns.contains_key(id))
                {
                    self.stop_error("Stop could not identify every active Codex child turn. Check running work manually.".into());
                }
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
                if let Some(answers) = &answers {
                    request.validate(answers)?;
                }
                if request.source == QuestionSource::AsyncMessage {
                    if self.pending.values().any(|pending| matches!(&pending.kind, Pending::AsyncAnswer { request_instance, .. } if *request_instance == request.instance)) {
                        return Err("This answer is already being submitted".into());
                    }
                    if let Some(answers) = answers {
                        let text = request.answer_text(&answers);
                        self.request(
                            "turn/start",
                            self.turn_params(self.input(&text, Vec::new())),
                            Pending::AsyncAnswer {
                                question_id: id.clone(),
                                request_instance: request.instance,
                                ack: self.state.question_answer_ack(id),
                            },
                        )?;
                        self.state.clear_error();
                    } else {
                        self.state.resolve_async_question(&id);
                        self.state.status = if !self.state.questions.is_empty()
                            || !self.state.approvals.is_empty()
                            || !self.state.elicitations.is_empty()
                        {
                            Status::Waiting
                        } else if self.state.turn_id.is_some() {
                            Status::Working
                        } else {
                            Status::Done
                        };
                    }
                    return Ok(true);
                }
                let answered = answers.is_some();
                let response = request.codex_response(answers.unwrap_or_default());
                self.write(&json!({"id":id,"result":response}))?;
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
                let result = crate::controls::approval_response(
                    &self.state.approvals[at],
                    &choice,
                    input,
                    feedback,
                )?;
                let owner = self.state.interaction_owners.get(&id.to_string()).cloned();
                let child = owner.as_ref().is_some_and(|owner| owner.agent.is_some());
                let permission_cancel = choice == "cancel"
                    && self.state.approvals[at]["method"] == "item/permissions/requestApproval";
                self.write(&json!({"id":id,"result":result}))?;
                let resolved = self.state.approvals[at]["id"].clone();
                self.state.retire_interaction(&resolved);
                self.state.approvals.remove(at);
                if choice == "cancel" && child {
                    if let Some(owner) = &owner {
                        self.state.expire_interactions(&owner.thread);
                    }
                }
                if choice == "cancel" && !child {
                    self.compaction_queue = None;
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.approvals.clear();
                    self.state.questions.clear();
                    self.state.elicitations.clear();
                }
                if permission_cancel {
                    if child {
                        if let Some(owner) = owner {
                            if let Some(turn) = owner.turn {
                                self.request(
                                    "turn/interrupt",
                                    json!({"threadId":owner.thread,"turnId":turn}),
                                    Pending::ChildInterrupt,
                                )?;
                            }
                        }
                    } else if let Some(turn) = self.state.turn_id.clone() {
                        self.interrupt(turn, 0)?;
                    }
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
                let result = if action == "accept" && content.is_some() {
                    json!({"action":action,"content":content})
                } else {
                    json!({"action":action})
                };
                self.write(&json!({"id":id,"result":result}))?;
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
        }
        self.state.refresh_interaction_status();
        Ok(true)
    }
    // Only provider events observed on this connection establish ownership. A
    // child turn's rootTurnId prevents a stop from touching a later generation.
    // Stop requests are optional provider controls. Their timeout must not
    // disconnect an otherwise usable conversation or consume unrelated work.
}
