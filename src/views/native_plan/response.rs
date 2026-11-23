//! Plan response composition, draft persistence and dispatch.

use super::{
    all_decisions, decision_answered, json, reveal_decision, review_locked, string, Action,
    NativePlan, NativePlanEvent, PlanComposerTarget, Value,
};
use gpui::{App, Context, Window};

impl NativePlan {
    fn save_review(&mut self, review: Value, cx: &mut Context<Self>) -> Result<(), String> {
        let review: grove_core::rich_plan::ReviewState =
            serde_json::from_value(review).map_err(|e| e.to_string())?;
        // Validate before the composer reports a save and restores its original draft.
        // The actor repeats this check at the persistence boundary.
        let revision = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revision(&self.plan_id, &self.revision_id)
            .cloned()
            .ok_or("This plan revision is no longer available")?;
        let mut checked = grove_core::rich_plan::PlanStore {
            revisions: vec![revision],
            ..Default::default()
        };
        checked.update_review(&self.plan_id, &self.revision_id, review)?;
        let review = checked.revisions.remove(0).review;
        self.reviewed_response = None;
        self.projection.borrow_mut().invalidate();
        self.local_review = Some(review.clone());
        self.session.update(cx, |session, cx| {
            session.send(
                Action::UpdatePlanReview {
                    plan_id: self.plan_id.clone(),
                    revision_id: self.revision_id.clone(),
                    review: Box::new(review),
                },
                cx,
            );
        });
        cx.notify();
        Ok(())
    }

    pub(super) fn choose(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        if self.send_pending
            || self.composer_pending
            || self.stale(cx)
            || review_locked(&self.snapshot(cx))
        {
            return;
        }
        let mut review = self.snapshot(cx)["review"].clone();
        if let Some(own) = review["own_answers"].as_object_mut() {
            own.remove(key);
        }
        review["answers"][key] = json!(value);
        if let Err(error) = self.save_review(review, cx) {
            self.error = Some(error);
        }
    }

    pub(in crate::views) fn save_custom_answer(
        &mut self,
        decision_id: &str,
        answer: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.stale(cx) {
            return Err(
                "This plan revision is outdated. Your draft is preserved; review the current plan."
                    .into(),
            );
        }
        let snapshot = self.snapshot(cx);
        if self.send_pending || review_locked(&snapshot) {
            return Err("This plan is awaiting delivery or has already been sent.".into());
        }
        if answer.trim().is_empty() {
            return Err("Write an answer before saving it.".into());
        }
        if !all_decisions(&snapshot["document"])
            .iter()
            .any(|d| d["id"] == decision_id)
        {
            return Err("This decision is no longer available.".into());
        }
        let mut review = snapshot["review"].clone();
        review["own_answers"][decision_id] = json!(answer.clone());
        review["answer_drafts"][decision_id] = json!(answer);
        if let Some(answers) = review["answers"].as_object_mut() {
            answers.remove(decision_id);
        }
        self.save_review(review, cx)
    }

    pub(in crate::views) fn save_custom_draft(
        &mut self,
        decision_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let snapshot = self.snapshot(cx);
        if !all_decisions(&snapshot["document"])
            .iter()
            .any(|d| d["id"] == decision_id)
        {
            return Err("This decision is no longer available.".into());
        }
        let mut review = snapshot["review"].clone();
        review["answer_drafts"][decision_id] = json!(text);
        self.save_review(review, cx)
    }

    pub(in crate::views) fn custom_answer_draft(&self, decision_id: &str, cx: &App) -> String {
        let snapshot = self.snapshot(cx);
        snapshot["review"]["answer_drafts"][decision_id]
            .as_str()
            .or_else(|| snapshot["review"]["own_answers"][decision_id].as_str())
            .unwrap_or_default()
            .to_owned()
    }

    pub(in crate::views) fn set_composer_pending(&mut self, pending: bool, cx: &mut Context<Self>) {
        if self.composer_pending != pending {
            self.composer_pending = pending;
            cx.notify();
        }
    }

    pub(super) fn compose(
        &mut self,
        target: PlanComposerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(target, PlanComposerTarget::CustomAnswer { .. })
            && (self.stale(cx) || self.send_pending || review_locked(&self.snapshot(cx)))
        {
            return;
        }
        if self.expanded(cx) {
            self.toggle_expansion(window, cx);
        }
        self.dismiss_peek(cx);
        cx.emit(NativePlanEvent::Compose {
            plan_id: self.plan_id.clone(),
            revision_id: self.revision_id.clone(),
            target,
        });
    }

    pub(super) fn dispatch(&mut self, approve: bool, cx: &mut Context<Self>) {
        if grove_core::connections::credentials_busy(
            self.session.read(cx).state.saved.provider.agent(),
        ) {
            self.error = Some(
                "Wait for provider authentication to finish before sending plan actions.".into(),
            );
            cx.notify();
            return;
        }
        if self.send_pending || self.composer_pending || self.stale(cx) {
            return;
        }
        let snapshot = self.snapshot(cx);
        if approve {
            if let Some(error) = self
                .session
                .read(cx)
                .state
                .saved
                .rich_plans
                .approval_error(&self.plan_id, &self.revision_id)
            {
                self.error = Some(error);
                cx.notify();
                return;
            }
        } else if review_locked(&snapshot)
            || all_decisions(&snapshot["document"])
                .iter()
                .any(|d| !decision_answered(d, &snapshot["review"]))
        {
            return;
        }
        self.error = None;
        self.send_pending = true;
        self.pending_approval = approve;
        self.session.update(cx, |session, cx| {
            session.send(
                if approve {
                    Action::ApprovePlan {
                        plan_id: self.plan_id.clone(),
                        revision_id: self.revision_id.clone(),
                    }
                } else {
                    Action::SendPlanResponse {
                        plan_id: self.plan_id.clone(),
                        revision_id: self.revision_id.clone(),
                    }
                },
                cx,
            );
        });
        cx.notify();
    }

    pub(super) fn request_decision_revision(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        if self.composer_pending || self.send_pending || self.stale(cx) || review_locked(&snapshot)
        {
            return;
        }
        let mut review = snapshot["review"].clone();
        review["request_decision_revision"] = json!(true);
        match self.save_review(review, cx) {
            Ok(()) => self.dispatch(false, cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn next_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        let decisions = all_decisions(&snapshot["document"]);
        let start = self
            .active_decision
            .as_ref()
            .and_then(|id| decisions.iter().position(|d| d["id"] == *id))
            .map_or(0, |i| i + 1);
        if let Some(i) = (0..decisions.len())
            .map(|offset| (start + offset) % decisions.len())
            .find(|i| !decision_answered(&decisions[*i], &snapshot["review"]))
        {
            let id = string(&decisions[i], "id");
            self.active_decision = Some(id.clone());
            self.decision_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .focus(window, cx);
            reveal_decision(&snapshot["document"]["claims"], &id, &mut self.open_claims);
            let section = string(&decisions[i], "section");
            for (index, (title, _)) in self.prepared.borrow().sections.iter().enumerate() {
                if *title == section {
                    self.open_sections.insert(index);
                }
            }
            self.reviewed_response = None;
            self.projection.borrow_mut().invalidate();
            self.pending_decision_reveal = true;
            cx.notify();
        }
    }
}
