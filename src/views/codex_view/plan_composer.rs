//! Native plan decisions and composer draft handoff.

use super::{
    c, codex, rpx, transcript_action, CodexView, Disableable, HashSet, PlanAnswerComposer,
};
use crate::views::tokens::{ICON_SM, SPACE_LG, TEXT_SMALL};
use gpui::prelude::{InteractiveElement, IntoElement, ParentElement, Styled};
use gpui::AppContext as _;
use gpui::{div, App, Context, Entity, Focusable, Window};

impl CodexView {
    pub(super) fn sync_native_plans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_readonly {
            self.native_plans.clear();
            return;
        }
        let plan_keys: HashSet<_> = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .map(|p| (p.document.plan_id.clone(), p.document.revision_id.clone()))
            .collect();
        self.native_plans.retain(|key, _| plan_keys.contains(key));
        for (plan_id, revision_id) in plan_keys {
            let key = (plan_id.clone(), revision_id.clone());
            if !self.native_plans.contains_key(&key) {
                let session = self.session.clone();
                let html = self.html.clone();
                let view = cx.new(|cx| {
                    super::super::native_plan::NativePlan::new(
                        session,
                        html,
                        plan_id,
                        revision_id,
                        cx,
                    )
                });
                view.update(cx, |plan, _| {
                    plan.set_transcript_scroll(self.scroll.clone());
                });
                self.subscriptions.push(cx.observe(&view, |this, _, cx| {
                    this.smooth_follow = false;
                    cx.notify();
                }));
                self.subscriptions.push(cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &super::super::native_plan::NativePlanEvent, window, cx| {
                        this.begin_plan_composer(event.clone(), window, cx);
                    },
                ));
                self.native_plans.insert(key, view);
            }
        }
    }

    pub(super) fn begin_plan_composer(
        &mut self,
        event: super::super::native_plan::NativePlanEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::super::native_plan::{NativePlanEvent, PlanComposerTarget};
        if self.transcript_only()
            || self.submitted.is_some()
            || self.queue_editor.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }
        if self
            .plan_answer
            .as_ref()
            .is_some_and(|answer| answer.saving.is_some())
        {
            return;
        }
        self.cancel_plan_answer(window, cx);
        let NativePlanEvent::Compose {
            plan_id,
            revision_id,
            target,
        } = event;
        let Some(plan) = self
            .native_plans
            .get(&(plan_id.clone(), revision_id.clone()))
            .cloned()
        else {
            return;
        };
        match target {
            PlanComposerTarget::CustomAnswer {
                decision_id,
                prompt,
            } => {
                let value = plan.read(cx).custom_answer_draft(&decision_id, cx);
                self.plan_answer = Some(PlanAnswerComposer {
                    plan_id,
                    revision_id,
                    decision_id,
                    prompt,
                    thread_id: self.session.read(cx).state.saved.thread_id.clone(),
                    draft: self.input.read(cx).value().to_string(),
                    images: std::mem::take(&mut self.images),
                    tokens: std::mem::take(&mut self.picked_tokens),
                    reference: self.reference.take(),
                    saving: None,
                    error_before_save: None,
                });
                self.input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
            }
            PlanComposerTarget::Discuss { section } => {
                let state = &self.session.read(cx).state;
                let Some(revision) = state.saved.rich_plans.revision(&plan_id, &revision_id) else {
                    return;
                };
                let reference = codex::ReferenceDraft {
                    id: format!("plan:{}", serde_json::json!([plan_id, revision_id])),
                    thread_id: state.saved.thread_id.clone(),
                    sources: vec![codex::ReferenceSource {
                        message_id: revision.message_id.clone(),
                        role: "Assistant".into(),
                    }],
                    text: format!(
                        "Plan: {} · Revision: {}\nSection: {}\n\n{}",
                        revision.document.title,
                        revision_id,
                        section,
                        revision.document.markdown_body()
                    ),
                };
                self.set_reference(Some(reference), cx);
            }
        }
        self.close_menu(cx);
        self.set_composer_collapsed(false, cx);
        self.input.focus_handle(cx).focus(window, cx);
        self.sync_plan_composer_pending(cx);
        cx.notify();
    }

    pub(super) fn persist_plan_answer_draft(&mut self, cx: &mut Context<Self>) {
        let Some(answer) = self.plan_answer.as_ref() else {
            return;
        };
        if answer.saving.is_some() {
            return;
        }
        let key = (answer.plan_id.clone(), answer.revision_id.clone());
        let decision = answer.decision_id.clone();
        let value = self.input.read(cx).value().to_string();
        if let Some(plan) = self.native_plans.get(&key).cloned() {
            plan.update(cx, |plan, cx| {
                let _ = plan.save_custom_draft(&decision, value, cx);
            });
        }
    }

    pub(super) fn cancel_plan_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.persist_plan_answer_draft(cx);
        let Some(answer) = self.plan_answer.take() else {
            return;
        };
        self.images = answer.images;
        self.picked_tokens = answer.tokens;
        self.reference = answer.reference;
        self.input
            .update(cx, |input, cx| input.set_value(answer.draft, window, cx));
        self.input.focus_handle(cx).focus(window, cx);
        self.sync_plan_composer_pending(cx);
        cx.notify();
    }

    pub(super) fn save_plan_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answer) = &self.plan_answer else {
            return;
        };
        let value = self.input.read(cx).value().to_string();
        if value.trim().is_empty() || answer.saving.is_some() {
            return;
        }
        let key = (answer.plan_id.clone(), answer.revision_id.clone());
        let decision = answer.decision_id.clone();
        let Some(plan) = self.native_plans.get(&key).cloned() else {
            return;
        };
        match plan.update(cx, |plan, cx| {
            plan.save_custom_answer(&decision, value.clone(), cx)
        }) {
            Ok(()) => {
                if let Some(answer) = &mut self.plan_answer {
                    answer.saving = Some(value);
                    answer
                        .error_before_save
                        .clone_from(&self.session.read(cx).state.error);
                }
                self.sync_plan_answer(window, cx);
            }
            Err(error) => self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            }),
        }
        cx.notify();
    }

    pub(super) fn sync_plan_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answer) = &self.plan_answer else {
            return;
        };
        let state = &self.session.read(cx).state;
        let invalidated = answer.thread_id != state.saved.thread_id
            || state
                .saved
                .rich_plans
                .revision(&answer.plan_id, &answer.revision_id)
                .is_none();
        let acknowledged = answer.saving.as_ref().is_some_and(|value| {
            state
                .saved
                .rich_plans
                .revision(&answer.plan_id, &answer.revision_id)
                .is_some_and(|r| r.review.own_answers.get(&answer.decision_id) == Some(value))
        });
        if invalidated || acknowledged {
            self.cancel_plan_answer(window, cx);
        } else if state.error.is_some() && state.error != answer.error_before_save {
            if let Some(answer) = &mut self.plan_answer {
                answer.saving = None;
            }
        }
    }

    pub(super) fn sync_plan_composer_pending(&mut self, cx: &mut Context<Self>) {
        let referenced: Option<Vec<String>> = self
            .reference
            .as_ref()
            .and_then(|r| r.id.strip_prefix("plan:"))
            .and_then(|id| serde_json::from_str(id).ok());
        let has_text = !self.input.read(cx).value().trim().is_empty();
        for ((id, _), plan) in &self.native_plans {
            let pending = self.plan_answer.as_ref().is_some_and(|a| &a.plan_id == id)
                || (has_text && referenced.as_ref().is_some_and(|r| r.first() == Some(id)));
            plan.update(cx, |plan, cx| plan.set_composer_pending(pending, cx));
        }
    }

    pub(super) fn render_plan_answer_context(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let answer = self.plan_answer.as_ref()?;
        let saving = answer.saving.is_some();
        Some(
            div()
                .id("plan-answer-context")
                .debug_selector(|| "plan-answer-context".into())
                .w_full()
                .min_w_0()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!(
                            "Your answer · {} · {}",
                            answer.revision_id, answer.prompt
                        )),
                )
                .child(
                    transcript_action("cancel-plan-answer", "Cancel answer", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/close.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .disabled(saving)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_plan_answer(window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Latest immutable revision of each plan, in conversation order.
    pub(in crate::views) fn native_plans_for_sidebar(
        &self,
        cx: &App,
    ) -> Vec<Entity<super::super::native_plan::NativePlan>> {
        let mut seen = HashSet::new();
        let mut plans: Vec<_> = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .rev()
            .filter(|r| seen.insert(r.document.plan_id.clone()))
            .filter_map(|r| {
                self.native_plans
                    .get(&(r.document.plan_id.clone(), r.document.revision_id.clone()))
                    .cloned()
            })
            .collect();
        plans.reverse();
        plans
    }
}
