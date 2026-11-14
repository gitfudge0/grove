//! Native review of validated, immutable session plan snapshots.
mod measured;
mod overview;

#[cfg(all(test, target_os = "linux"))]
pub(super) fn take_control_materialization_count() -> usize {
    measured::CONTROL_RENDERS.with(|n| n.replace(0))
}
#[cfg(all(test, target_os = "linux"))]
pub(super) fn take_materialization_count() -> usize {
    measured::MATERIALIZED.with(|count| count.replace(0))
}

use super::status::{self, StatusKind};
use super::{
    components::{question_choice, transcript_action},
    rpx,
    tokens::*,
};
use crate::{entities::codex_session::CodexSession, theme as c};
use gpui::{
    div, prelude::*, AnyElement, App, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Subscription, WeakEntity, Window,
};
use gpui_component::{text::TextView, Disableable};
use grove_core::codex::Action;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Default)]
struct PlanUi {
    expanded: Option<WeakEntity<NativePlan>>,
    peek: Option<WeakEntity<NativePlan>>,
    owner: Option<EntityId>,
    blocked: bool,
}
impl gpui::Global for PlanUi {}

pub(super) fn set_context(owner: Option<EntityId>, blocked: bool, cx: &mut App) {
    if !cx.has_global::<PlanUi>() {
        cx.set_global(PlanUi::default());
    }
    let ui = cx.global_mut::<PlanUi>();
    if ui.owner != owner {
        ui.expanded = None;
        ui.peek = None;
    }
    ui.owner = owner;
    ui.blocked = blocked;
}

/// Inline webviews are native surfaces above GPUI paint; keep them behind plan review.
/// An explicitly expanded linked HTML document remains handled by HtmlUi.
pub(super) fn blocks_inline_html(cx: &App) -> bool {
    cx.try_global::<PlanUi>()
        .is_some_and(|ui| ui.peek.is_some() || ui.expanded.is_some())
}

pub(super) fn inline(plan: &Entity<NativePlan>, window: &Window, cx: &App) -> AnyElement {
    if plan.read(cx).snapshot(cx)["document"]["markdown"].is_string()
        && !plan.read(cx).is_expanded(cx)
    {
        return measured::Plan::new(plan.clone()).into_any_element();
    }
    if cx
        .try_global::<PlanUi>()
        .and_then(|ui| ui.expanded.as_ref().or(ui.peek.as_ref()))
        .is_some_and(|p| p.entity_id() == plan.entity_id())
    {
        let view = plan.read(cx);
        let markdown = view.markdown(cx);
        div()
            .min_w_0()
            .w_full()
            .child(
                TextView::markdown(
                    format!(
                        "inline-plan-reference-{}-{}",
                        view.plan_id, view.revision_id
                    ),
                    markdown,
                )
                .style(super::codex_view::markdown_style(window))
                .markdown_extensions(super::mermaid::extensions())
                .selectable(true),
            )
            .into_any_element()
    } else {
        measured::Plan::new(plan.clone()).into_any_element()
    }
}

/// A separate reference surface keeps the inline plan and its draft mounted.
pub(super) fn render_peek(plan: &Entity<NativePlan>, window: &Window, cx: &App) -> AnyElement {
    let view = plan.read(cx);
    let snapshot = view.snapshot(cx);
    let markdown = view.markdown(cx);
    let target = plan.clone();
    div()
        .id("native-plan-reference")
        .debug_selector(|| "native-plan-reference".into())
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(c::BG())
        .child(
            div()
                .debug_selector(|| "native-plan-header".into())
                .flex_shrink_0()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .p(rpx(SPACE_3XL))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_TITLE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(string(&snapshot["document"], "title")),
                )
                .child(
                    transcript_action("plan-reference-expand", "Expand", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/zen.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .debug_selector(|| "native-plan-expand".into())
                        .on_click(move |_, window, cx| {
                            target.update(cx, |view, cx| view.reveal(window, cx));
                        }),
                ),
        )
        .child(
            div()
                .id("native-plan-reference-body")
                .debug_selector(|| "native-plan-body".into())
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_y_scroll()
                .track_scroll(&view.scroll)
                .p(rpx(SPACE_3XL))
                .pt_0()
                .child(
                    TextView::markdown(
                        format!("plan-reference-{}-{}", view.plan_id, view.revision_id),
                        markdown.clone(),
                    )
                    .style(super::codex_view::markdown_style(window))
                    .markdown_extensions(super::mermaid::extensions())
                    .selectable(true),
                ),
        )
        .into_any_element()
}

pub(super) fn render_expanded(cx: &mut App) -> Option<AnyElement> {
    let ui = cx.try_global::<PlanUi>()?;
    if ui.blocked {
        return None;
    }
    let plan = ui.expanded.as_ref()?.upgrade()?;
    Some(
        div()
            .id("native-plan-expanded")
            .debug_selector(|| "native-plan-expanded".into())
            .absolute()
            .inset_0()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .occlude()
            .bg(c::BG())
            .child(plan)
            .into_any_element(),
    )
}

#[derive(Clone, Debug)]
pub(super) enum PlanComposerTarget {
    CustomAnswer { decision_id: String, prompt: String },
    Discuss { section: String },
}

#[derive(Clone, Debug)]
pub(super) enum NativePlanEvent {
    Compose {
        plan_id: String,
        revision_id: String,
        target: PlanComposerTarget,
    },
}

#[derive(Default)]
struct PlanPreparation {
    source: std::rc::Weak<grove_core::codex::Snapshot>,
    local_review: Option<grove_core::rich_plan::ReviewState>,
    document: Option<grove_core::rich_plan::PlanDocument>,
    review: Option<grove_core::rich_plan::ReviewState>,
    snapshot: Rc<Value>,
    markdown: String,
    sections: Rc<Vec<(String, String)>>,
}

pub(super) struct NativePlan {
    projection: Rc<RefCell<measured::Projection>>,
    prepared: RefCell<PlanPreparation>,
    observed_review: Option<grove_core::rich_plan::ReviewState>,
    observed_stale: bool,
    identity: EntityId,
    session: Entity<CodexSession>,
    html: Entity<super::inline_html::HtmlSession>,
    plan_id: String,
    revision_id: String,
    _subscriptions: Vec<Subscription>,
    local_review: Option<grove_core::rich_plan::ReviewState>,
    images: HashMap<String, std::sync::Arc<gpui::Image>>,
    send_pending: bool,
    pending_approval: bool,
    composer_pending: bool,
    expand_focus: FocusHandle,
    return_focus: FocusHandle,
    source_focus: Option<FocusHandle>,
    error: Option<String>,
    scroll: gpui::ScrollHandle,
    transcript_scroll: Option<gpui::ScrollHandle>,
    decision_anchors: HashMap<String, gpui::ScrollAnchor>,
    decision_focus: HashMap<String, FocusHandle>,
    active_decision: Option<String>,
    show_previous: bool,
    open_sections: HashSet<usize>,
}
impl EventEmitter<NativePlanEvent> for NativePlan {}

const PLAN_READING_WIDTH: f32 = 840.0;
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn entries(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn decision_answered(decision: &Value, review: &Value) -> bool {
    let id = string(decision, "id");
    review["own_answers"][&id]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty())
        || review["answers"][&id].as_str().is_some_and(|answer| {
            entries(&decision["options"])
                .iter()
                .any(|o| o["id"] == answer)
        })
}
fn all_decisions(document: &Value) -> Vec<Value> {
    fn visit(claims: &Value, result: &mut Vec<Value>) {
        for claim in entries(claims) {
            result.extend(entries(&claim["decisions"]));
            visit(&claim["children"], result);
        }
    }
    let mut decisions = entries(&document["decisions"]);
    visit(&document["claims"], &mut decisions);
    decisions
}
fn resolved_answer(decision: &Value, review: &Value) -> String {
    let id = string(decision, "id");
    if let Some(own) = review["own_answers"][&id]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        return own.to_owned();
    }
    entries(&decision["options"])
        .iter()
        .find(|option| option["id"] == review["answers"][&id])
        .map_or_else(
            || "Needs your answer".into(),
            |option| string(option, "label"),
        )
}
fn review_locked(snapshot: &Value) -> bool {
    matches!(
        snapshot["review"]["delivery"]["status"].as_str(),
        Some("queued" | "sending" | "sent" | "uncertain")
    ) || matches!(
        snapshot["review"]["approval"]["status"].as_str(),
        Some("queued" | "sending" | "sent" | "uncertain")
    )
}

/// Split only at top-level Markdown headings; fenced code remains one literal block.
fn markdown_sections(markdown: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut title = String::from("Overview");
    let mut content = String::new();
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let ch = trimmed.chars().next().unwrap_or(' ');
        let run = trimmed.chars().take_while(|c| *c == ch).count();
        if indent < 4 && matches!(ch, '`' | '~') && run >= 3 {
            if let Some((held, length)) = fence {
                if ch == held && run >= length && trimmed[run..].trim().is_empty() {
                    fence = None;
                }
            } else {
                fence = Some((ch, run));
            }
        } else if fence.is_none()
            && indent < 4
            && ch == '#'
            && (2..=3).contains(&run)
            && trimmed[run..].starts_with(char::is_whitespace)
        {
            if !content.trim().is_empty() {
                result.push((std::mem::take(&mut title), std::mem::take(&mut content)));
            }
            trimmed[run..]
                .trim()
                .trim_end_matches('#')
                .trim()
                .clone_into(&mut title);
        }
        content.push_str(line);
    }
    if !content.trim().is_empty() {
        result.push((title, content));
    }
    result
}

impl NativePlan {
    pub(super) fn new(
        session: Entity<CodexSession>,
        html: Entity<super::inline_html::HtmlSession>,
        plan_id: String,
        revision_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let observed_review = session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revision(&plan_id, &revision_id)
            .map(|r| r.review.clone());
        let observed_stale = session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .rev()
            .find(|r| r.document.plan_id == plan_id)
            .is_none_or(|r| r.document.revision_id != revision_id);
        let subscription = cx.observe(&session, |this: &mut Self, session, cx| {
            let previous_error = this.error.clone();
            let previous_pending = this.send_pending;
            let previous_local = this.local_review.clone();
            let state = &session.read(cx).state;
            if this.send_pending {
                this.error.clone_from(&state.error);
            }
            if let Some(revision) = state
                .saved
                .rich_plans
                .revision(&this.plan_id, &this.revision_id)
            {
                let delivery = if this.pending_approval {
                    &revision.review.approval
                } else {
                    &revision.review.delivery
                };
                if !matches!(delivery, grove_core::rich_plan::ResponseDelivery::Draft)
                    || state.error.is_some()
                {
                    this.send_pending = false;
                    this.local_review = None;
                }
            }
            let review = state
                .saved
                .rich_plans
                .revision(&this.plan_id, &this.revision_id)
                .map(|r| &r.review);
            let stale = state
                .saved
                .rich_plans
                .revisions
                .iter()
                .rev()
                .find(|r| r.document.plan_id == this.plan_id)
                .is_none_or(|r| r.document.revision_id != this.revision_id);
            if review != this.observed_review.as_ref()
                || stale != this.observed_stale
                || previous_error != this.error
                || previous_pending != this.send_pending
                || previous_local != this.local_review
            {
                this.observed_review = review.cloned();
                this.observed_stale = stale;
                cx.notify();
            }
        });
        Self {
            projection: {
                let p = Rc::new(RefCell::new(measured::Projection::default()));
                p.borrow_mut().registry = Rc::downgrade(&p);
                p.borrow_mut().owner = Some(cx.entity().downgrade());
                p
            },
            prepared: RefCell::new(PlanPreparation::default()),
            observed_review,
            observed_stale,
            identity: cx.entity_id(),
            session,
            html,
            plan_id,
            revision_id,
            _subscriptions: vec![subscription],
            local_review: None,
            images: HashMap::new(),
            send_pending: false,
            pending_approval: false,
            composer_pending: false,
            expand_focus: cx.focus_handle().tab_stop(true),
            return_focus: cx.focus_handle().tab_stop(true),
            source_focus: None,
            error: None,
            scroll: gpui::ScrollHandle::new(),
            transcript_scroll: None,
            decision_anchors: HashMap::new(),
            decision_focus: HashMap::new(),
            active_decision: None,
            show_previous: false,
            open_sections: HashSet::new(),
        }
    }
    fn snapshot(&self, cx: &App) -> Rc<Value> {
        let state = self.session.read(cx).state.shared();
        let mut prepared = self.prepared.borrow_mut();
        // SessionSnapshot forks on every local mutation, even before observers
        // run. A weak identity avoids retaining an obsolete entire transcript.
        if prepared.source.ptr_eq(&Rc::downgrade(&state))
            && prepared.local_review == self.local_review
        {
            return prepared.snapshot.clone();
        }
        let Some(revision) = state
            .saved
            .rich_plans
            .revision(&self.plan_id, &self.revision_id)
        else {
            return Rc::new(Value::Null);
        };
        let mut local;
        let review = if let Some(review) = &self.local_review {
            local = review.clone();
            local.delivery = revision.review.delivery.clone();
            local.approval = revision.review.approval.clone();
            &local
        } else {
            &revision.review
        };
        let document_changed = prepared.document.as_ref() != Some(&revision.document);
        if document_changed {
            prepared.markdown = revision.document.markdown_body();
            prepared.sections = Rc::new(markdown_sections(
                revision
                    .document
                    .markdown
                    .as_deref()
                    .filter(|_| revision.document.presentation.is_some())
                    .unwrap_or(&prepared.markdown),
            ));
            prepared.document = Some(revision.document.clone());
        }
        if document_changed || prepared.review.as_ref() != Some(review) {
            prepared.snapshot = Rc::new(json!({"document": &revision.document, "review": review}));
            prepared.review = Some(review.clone());
        }
        prepared.source = Rc::downgrade(&state);
        prepared.local_review.clone_from(&self.local_review);
        prepared.snapshot.clone()
    }
    fn markdown(&self, cx: &App) -> String {
        self.snapshot(cx);
        self.prepared.borrow().markdown.clone()
    }
    fn stale(&self, cx: &App) -> bool {
        self.session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .rev()
            .find(|r| r.document.plan_id == self.plan_id)
            .is_none_or(|r| r.document.revision_id != self.revision_id)
    }
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
    fn choose(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
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
    pub(super) fn save_custom_answer(
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
    pub(super) fn save_custom_draft(
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
    pub(super) fn custom_answer_draft(&self, decision_id: &str, cx: &App) -> String {
        let snapshot = self.snapshot(cx);
        snapshot["review"]["answer_drafts"][decision_id]
            .as_str()
            .or_else(|| snapshot["review"]["own_answers"][decision_id].as_str())
            .unwrap_or_default()
            .to_owned()
    }
    pub(super) fn set_transcript_scroll(&mut self, scroll: gpui::ScrollHandle) {
        self.transcript_scroll = Some(scroll);
        self.decision_anchors.clear();
    }
    pub(super) fn set_composer_pending(&mut self, pending: bool, cx: &mut Context<Self>) {
        if self.composer_pending != pending {
            self.composer_pending = pending;
            cx.notify();
        }
    }
    fn compose(&mut self, target: PlanComposerTarget, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn title_and_status(&self, cx: &App) -> (String, String) {
        let snapshot = self.snapshot(cx);
        let decisions = all_decisions(&snapshot["document"]);
        let pending = decisions
            .iter()
            .filter(|d| !decision_answered(d, &snapshot["review"]))
            .count();
        let status = match snapshot["review"]["approval"]["status"].as_str() {
            Some("sent") => "Approved · sent to agent".into(),
            Some("queued") => "Approval queued".into(),
            Some("uncertain") => "Check approval delivery".into(),
            _ if matches!(
                snapshot["review"]["delivery"]["status"].as_str(),
                Some("queued" | "sent")
            ) =>
            {
                "Waiting for updated plan".into()
            }
            _ if snapshot["review"]["delivery"]["status"] == "uncertain" => {
                "Check update delivery".into()
            }
            _ if pending > 0 => format!("{pending} decisions remaining"),
            _ if !decisions.is_empty() => "Update plan to finalize decisions".into(),
            _ if snapshot["document"]["finalized_from"].is_object() => "Decisions finalized".into(),
            _ => "Ready for review".into(),
        };
        (string(&snapshot["document"], "title"), status)
    }
    pub(super) fn show_peek(&mut self, source: Option<FocusHandle>, cx: &mut Context<Self>) {
        self.source_focus = source;
        if !cx.has_global::<PlanUi>() {
            cx.set_global(PlanUi::default());
        }
        if !self.expanded(cx) {
            crate::platform::inline_html::NativeDocument::hide_all();
            cx.global_mut::<PlanUi>().peek = Some(cx.entity().downgrade());
            cx.notify();
            cx.refresh_windows();
        }
    }
    pub(super) fn dismiss_peek(&mut self, cx: &mut Context<Self>) {
        if cx.try_global::<PlanUi>().is_some_and(|ui| {
            ui.peek
                .as_ref()
                .is_some_and(|p| p.entity_id() == self.identity)
        }) {
            cx.global_mut::<PlanUi>().peek = None;
            cx.notify();
            cx.refresh_windows();
        }
    }
    pub(super) fn is_expanded(&self, cx: &App) -> bool {
        self.expanded(cx)
    }
    fn expanded(&self, cx: &App) -> bool {
        cx.try_global::<PlanUi>()
            .and_then(|ui| ui.expanded.as_ref())
            .is_some_and(|p| p.entity_id() == self.identity)
    }
    pub(super) fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_previous = true;
        if !self.expanded(cx) {
            self.toggle_expansion(window, cx);
        }
    }
    fn toggle_expansion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let expanded = self.expanded(cx);
        self.decision_anchors.clear();
        if !cx.has_global::<PlanUi>() {
            cx.set_global(PlanUi::default());
        }
        crate::platform::inline_html::NativeDocument::hide_all();
        let entity = cx.entity().downgrade();
        let ui = cx.global_mut::<PlanUi>();
        ui.expanded = (!expanded).then_some(entity);
        ui.peek = None;
        ui.owner = Some(self.html.entity_id());
        if expanded {
            self.source_focus
                .take()
                .unwrap_or_else(|| self.expand_focus.clone())
                .focus(window, cx);
        } else {
            if self.source_focus.is_none() {
                self.source_focus = window.focused(cx);
            }
            self.return_focus.focus(window, cx);
        }
        cx.notify();
        cx.refresh_windows();
    }
    fn dispatch(&mut self, approve: bool, cx: &mut Context<Self>) {
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
    fn request_decision_revision(&mut self, cx: &mut Context<Self>) {
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
    fn next_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if let Some(anchor) = this.decision_anchors.get(&id) {
                        anchor.scroll_to(window, cx);
                    }
                    if let Some(focus) = this.decision_focus.get(&id) {
                        focus.focus(window, cx);
                    }
                });
            });
            cx.notify();
        }
    }
    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn answer_preview_fixture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.choose("location", "alpha", cx);
        self.next_decision(window, cx);
        self.choose("checks", "beta", cx);
    }
    #[cfg(test)]
    pub(super) fn configure_overview_capture(
        &mut self,
        open_detail: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open_detail {
            self.open_sections.insert(0);
            self.projection.borrow_mut().invalidate();
        }
        self.reveal(window, cx);
        cx.notify();
    }
    fn activate(&mut self, action: &measured::Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            measured::Action::None => {}
            measured::Action::Expand => self.toggle_expansion(window, cx),
            measured::Action::Earlier => {
                self.show_previous = true;
                cx.notify();
            }
            measured::Action::ToggleSection(index) => {
                if !self.open_sections.remove(index) {
                    self.open_sections.insert(*index);
                }
                // Disclosure immediately changes both layout and keyboard targets.
                self.projection.borrow_mut().invalidate();
                cx.notify();
            }
            measured::Action::Next => self.next_decision(window, cx),
            measured::Action::Update => self.dispatch(false, cx),
            measured::Action::Approve => self.dispatch(true, cx),
            measured::Action::Change => self.request_decision_revision(cx),
            measured::Action::Choose(decision, option) => self.choose(decision, option, cx),
            measured::Action::Compose(target) => self.compose(target.clone(), window, cx),
        }
    }
    fn measured_button(
        &self,
        button: gpui_component::button::Button,
        key: String,
        action: measured::Action,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = if matches!(action, measured::Action::Expand) {
            let focus = if self.expanded(cx) {
                self.return_focus.clone()
            } else {
                self.expand_focus.clone()
            };
            self.projection
                .borrow_mut()
                .control_with_focus(key, action.clone(), enabled, focus)
        } else {
            self.projection
                .borrow_mut()
                .control(key, action.clone(), enabled, cx)
        };
        let button = button
            .with_focus_handle(target.focus.clone())
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(&action, window, cx)));
        measured::Observed::new(button.into_any_element(), target).into_any_element()
    }
    fn measured_text(
        &self,
        id: String,
        text: String,
        markdown: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = {
            let mut projection = self.projection.borrow_mut();
            let owner = projection
                .text
                .entry(id)
                .or_insert_with(|| {
                    cx.new(|cx| {
                        if markdown {
                            gpui_component::text::TextViewState::markdown(&text, cx)
                        } else {
                            gpui_component::text::TextViewState::html(&text, cx)
                        }
                    })
                })
                .clone();
            projection.active_text.push(owner.clone());
            owner
        };
        owner.update(cx, |state, cx| state.set_text_incremental(&text, cx));
        let target = self.projection.borrow_mut().control(
            format!("text:{}", owner.entity_id()),
            measured::Action::None,
            false,
            cx,
        );
        if markdown {
            self.projection
                .borrow_mut()
                .markdown
                .insert(owner.entity_id());
        }
        self.projection
            .borrow_mut()
            .text_targets
            .insert(owner.entity_id(), target.clone());
        measured::Observed::new(
            TextView::new(&owner)
                .style(super::codex_view::markdown_style(window))
                .when(markdown, |view| {
                    view.markdown_extensions(super::mermaid::extensions())
                })
                .selectable(true)
                .into_any_element(),
            target,
        )
        .into_any_element()
    }
    fn text(
        &self,
        id: String,
        text: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Plain read-only text, not Markdown: authored links never bypass asset validation.
        // TextView's HTML renderer drops <br> boundaries. Give each source line
        // its own layout block so feedback and authored multiline text stay readable.
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .children(text.lines().enumerate().map(|(index, line)| {
                let line_id = format!("{id}-line-{index}");
                div()
                    .debug_selector({
                        let line_id = line_id.clone();
                        move || line_id.clone()
                    })
                    .min_w_0()
                    .w_full()
                    .flex_shrink_0()
                    .when(line.is_empty(), |line| line.h(rpx(SPACE_LG)))
                    .when(!line.is_empty(), |row| {
                        row.child(self.measured_text(
                            line_id,
                            format!(
                                    "<p>{}</p>",
                                    line.replace('&', "&amp;")
                                        .replace('<', "&lt;")
                                        .replace('>', "&gt;")
                                ),
                            false,
                            window,
                            cx,
                        ))
                    })
            }))
            .into_any_element()
    }

    // Old documents retain immutable asset snapshots even though their text is Markdown.
    fn legacy_assets(&mut self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let assets = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revision(&self.plan_id, &self.revision_id)
            .map(|r| r.assets.clone())
            .unwrap_or_default();
        let mut body = div().flex().flex_col().gap(rpx(SPACE_LG)).min_w_0();
        for asset in assets {
            if asset.mime.starts_with("image/") {
                if let Some(format) = gpui::ImageFormat::from_mime_type(&asset.mime) {
                    let image = self
                        .images
                        .entry(asset.path.clone())
                        .or_insert_with(|| {
                            std::sync::Arc::new(gpui::Image::from_bytes(
                                format,
                                (*asset.bytes).clone(),
                            ))
                        })
                        .clone();
                    body = body.child(
                        gpui::img(image)
                            .max_w_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    );
                }
            } else if asset.mime == "text/html" {
                let path = asset.path.clone();
                let source = String::from_utf8_lossy(&asset.bytes).into_owned();
                let key = format!("plan:{}:{}:{}", self.plan_id, self.revision_id, path);
                body = body.child(
                    transcript_action(
                        format!("legacy-plan-asset-{path}"),
                        format!("Open saved attachment: {path}"),
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.html.update(cx, |html, cx| {
                            html.open_snapshot(key.clone(), source.clone(), window, cx);
                        });
                    })),
                );
            }
        }
        body.into_any_element()
    }

    fn discuss_action(
        &self,
        section: String,
        id: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.measured_button(
            transcript_action(id.clone(), format!("Discuss: {section}"), false, window, cx)
                // Keep the complete accessible name while allowing authored section
                // titles to yield to the pane width. The prompt remains visible above.
                .icon_only()
                .debug_selector({
                    let id = id.clone();
                    move || format!("native-plan-{id}")
                })
                .min_w_0()
                .max_w_full()
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/edit.svg")
                        .size(rpx(ICON_SM)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(format!("Discuss: {section}")),
                ),
            id,
            measured::Action::Compose(PlanComposerTarget::Discuss { section }),
            true,
            cx,
        )
        .into_any_element()
    }
    fn decision(
        &mut self,
        decision: &Value,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = string(decision, "id");
        let locked =
            self.stale(cx) || self.send_pending || self.composer_pending || review_locked(snapshot);
        let own = snapshot["review"]["own_answers"][&id]
            .as_str()
            .filter(|s| !s.trim().is_empty());
        let selected = snapshot["review"]["answers"][&id]
            .as_str()
            .filter(|_| own.is_none());
        let prompt = string(decision, "prompt");
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .min_w_0()
            .w_full()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .pt(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .w_full()
                    .gap(rpx(SPACE_LG))
                    .items_start()
                    .child(div().w_full().min_w_0().child(self.text(
                        format!("decision-prompt-{id}"),
                        prompt.clone(),
                        window,
                        cx,
                    )))
                    .child(self.discuss_action(
                        prompt.clone(),
                        format!("decision-discuss-{id}"),
                        window,
                        cx,
                    )),
            );
        if !locked {
            for option in entries(&decision["options"]) {
                let option_id = string(&option, "id");
                let debug_id = option_id.clone();
                let recommended = decision["default"].as_str() == Some(option_id.as_str());
                let label = format!(
                    "{}{}",
                    string(&option, "label"),
                    if recommended { " · recommended" } else { "" }
                );
                let key = format!("option:{id}:{option_id}");
                let target = self.projection.borrow_mut().control(
                    key,
                    measured::Action::Choose(id.clone(), option_id.clone()),
                    true,
                    cx,
                );
                let action = target.action.clone();
                let keyboard_action = action.clone();
                let choice = question_choice(
                    format!("option-{id}-{option_id}"),
                    label,
                    string(&option, "description"),
                    false,
                    selected == Some(option_id.as_str()),
                )
                .track_focus(&target.focus)
                .debug_selector(move || format!("native-plan-option-{debug_id}"))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.activate(&action, window, cx)),
                )
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.activate(&keyboard_action, window, cx);
                            cx.stop_propagation();
                        }
                    },
                ));
                body = body.child(measured::Observed::new(choice.into_any_element(), target));
            }
        }
        if locked || own.is_some() {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(self.text(
                        format!("decision-answer-{id}"),
                        resolved_answer(decision, &snapshot["review"]),
                        window,
                        cx,
                    )),
            );
        }
        let target = id.clone();
        body = body.child(
            div().flex().child(
                self.measured_button(
                    transcript_action(
                        format!("decision-custom-{id}"),
                        if own.is_some() {
                            "Edit my answer"
                        } else {
                            "Write my own answer"
                        },
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/edit.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("native-plan-custom-{id}")
                    })
                    .disabled(locked),
                    format!("custom:{id}"),
                    measured::Action::Compose(PlanComposerTarget::CustomAnswer {
                        decision_id: target,
                        prompt,
                    }),
                    !locked,
                    cx,
                ),
            ),
        );
        let scroll = if self.expanded(cx) {
            self.scroll.clone()
        } else {
            self.transcript_scroll
                .clone()
                .unwrap_or_else(|| self.scroll.clone())
        };
        let anchor = self
            .decision_anchors
            .entry(id.clone())
            .or_insert_with(|| gpui::ScrollAnchor::for_handle(scroll))
            .clone();
        let focus = self
            .decision_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let target = self.projection.borrow_mut().control_with_focus(
            format!("decision:{id}"),
            measured::Action::None,
            true,
            focus.clone(),
        );
        measured::Observed::new(
            body.id(format!("plan-decision-{id}"))
                .debug_selector(move || format!("native-plan-decision-{id}"))
                .track_focus(&focus)
                .anchor_scroll(Some(anchor))
                .into_any_element(),
            target,
        )
        .into_any_element()
    }
}

impl Render for NativePlan {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.projection.borrow_mut().begin();
        self.projection.borrow_mut().scroll = Some(if self.expanded(cx) {
            self.scroll.clone()
        } else {
            self.transcript_scroll
                .clone()
                .unwrap_or_else(|| self.scroll.clone())
        });
        let snapshot = self.snapshot(cx);
        let doc = &snapshot["document"];
        let expanded = self.expanded(cx);
        let stale = self.stale(cx);
        let decisions = all_decisions(doc);
        let unanswered = decisions
            .iter()
            .filter(|d| !decision_answered(d, &snapshot["review"]))
            .count();
        let delivery = snapshot["review"]["delivery"]["status"]
            .as_str()
            .unwrap_or("draft");
        let approval = snapshot["review"]["approval"]["status"]
            .as_str()
            .unwrap_or("draft");
        let approval_error = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .approval_error(&self.plan_id, &self.revision_id);
        let header = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "Plan · Revision {}{}",
                        self.revision_id,
                        if stale {
                            " · superseded"
                        } else if doc["finalized_from"].is_object() {
                            " · decisions finalized"
                        } else {
                            ""
                        }
                    )),
            )
            .child(
                self.measured_button(
                    transcript_action(
                        "plan-expand",
                        if expanded {
                            "Conversation"
                        } else {
                            "Open plan"
                        },
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path(if expanded {
                                "icons/unified.svg"
                            } else {
                                "icons/zen.svg"
                            })
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(|| "native-plan-expand".into())
                    .with_focus_handle(if expanded {
                        self.return_focus.clone()
                    } else {
                        self.expand_focus.clone()
                    }),
                    "expand".into(),
                    measured::Action::Expand,
                    true,
                    cx,
                ),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL))
            .min_w_0()
            .w_full()
            .when(expanded, |s| s.max_w(rpx(PLAN_READING_WIDTH)).mx_auto())
            .p(rpx(SPACE_3XL));
        if stale && !self.show_previous && !expanded {
            body = body
                .child(div().text_size(rpx(TEXT_TITLE)).child(string(doc, "title")))
                .child(
                    div().flex().child(
                        self.measured_button(
                            transcript_action(
                                "plan-show-previous",
                                "Read earlier revision",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/chev-down.svg")
                                    .size(rpx(ICON_SM)),
                            ),
                            "earlier".into(),
                            measured::Action::Earlier,
                            true,
                            cx,
                        ),
                    ),
                );
        } else {
            let layered = doc["presentation"].is_object();
            if layered {
                body = body.child(self.overview(doc, window, cx));
            }
            if doc["finalized_from"].is_object() {
                body = body.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                    .child(format!("Updated from revision {} · your decisions have been incorporated into this plan.", string(&doc["finalized_from"], "revision_id"))));
                let answers = doc["finalized_from"]["answers"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let parent = self
                    .session
                    .read(cx)
                    .state
                    .saved
                    .rich_plans
                    .revision(
                        &self.plan_id,
                        &string(&doc["finalized_from"], "revision_id"),
                    )
                    .map(|r| serde_json::to_value(&r.document).unwrap_or_default())
                    .unwrap_or_default();
                for decision in all_decisions(&parent) {
                    let id = string(&decision, "id");
                    if let Some(answer) = answers.get(&id).and_then(Value::as_str) {
                        body = body.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(rpx(SPACE_SM))
                                .border_b_1()
                                .border_color(c::BORDER_SOFT())
                                .pb(rpx(SPACE_LG))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(string(&decision, "prompt")),
                                )
                                .child(self.text(
                                    format!("final-answer-{id}"),
                                    answer.to_owned(),
                                    window,
                                    cx,
                                )),
                        );
                    }
                }
            }
            if !decisions.is_empty() {
                body = body.child(
                    div()
                        .debug_selector(|| "native-plan-progress-row".into())
                        .flex()
                        .flex_wrap()
                        .gap(rpx(SPACE_LG))
                        .items_center()
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(if unanswered > 0 {
                                    format!("{unanswered} decisions remaining")
                                } else {
                                    "All decisions answered · update the plan to finalize".into()
                                }),
                        )
                        .when(unanswered > 0, |row| {
                            row.child(
                                self.measured_button(
                                    transcript_action(
                                        "plan-next-decision",
                                        "Next unanswered decision",
                                        false,
                                        window,
                                        cx,
                                    )
                                    .icon(
                                        gpui_component::Icon::default()
                                            .path("icons/chev-down.svg")
                                            .size(rpx(ICON_SM)),
                                    )
                                    .disabled(stale || self.send_pending),
                                    "next".into(),
                                    measured::Action::Next,
                                    !stale && !self.send_pending,
                                    cx,
                                ),
                            )
                        }),
                );
            }
            if layered {
                for decision in &decisions {
                    body = body.child(self.decision(decision, &snapshot, window, cx));
                }
            }
            let sections = self.prepared.borrow().sections.clone();
            let mut rendered = HashSet::new();
            for (index, (title, markdown)) in sections.iter().cloned().enumerate() {
                if layered {
                    body = body.child(self.detail_section(index, title, markdown, window, cx));
                    continue;
                }
                let mut section =
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_SM))
                        .child(self.measured_text(
                            format!(
                                "plan-markdown-{}-{}-{index}",
                                self.plan_id, self.revision_id
                            ),
                            markdown,
                            true,
                            window,
                            cx,
                        ));
                for decision in &decisions {
                    if decision["section"]
                        .as_str()
                        .is_some_and(|section| section == title)
                    {
                        rendered.insert(string(decision, "id"));
                        section = section.child(self.decision(decision, &snapshot, window, cx));
                    }
                }
                body = body.child(section.child(div().flex().flex_wrap().child(
                    self.discuss_action(title, format!("plan-discuss-{index}"), window, cx),
                )));
            }
            if doc["markdown"].is_null() {
                body = body.child(self.legacy_assets(window, cx));
            }
            for decision in &decisions {
                if !layered && !rendered.contains(&string(decision, "id")) {
                    body = body.child(self.decision(decision, &snapshot, window, cx));
                }
            }
            if !layered && !decisions.is_empty() {
                body = body.child(div().text_size(rpx(TEXT_TITLE)).child("Your decisions"));
                for decision in &decisions {
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .border_b_1()
                            .border_color(c::BORDER_SOFT())
                            .pb(rpx(SPACE_LG))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(string(decision, "prompt")),
                            )
                            .child(self.text(
                                format!("summary-{}", string(decision, "id")),
                                resolved_answer(decision, &snapshot["review"]),
                                window,
                                cx,
                            )),
                    );
                }
            }
        }
        let status = if self.composer_pending {
            "Save or cancel your composer response before continuing.".into()
        } else if self.send_pending {
            if self.pending_approval {
                "Sending approval…"
            } else {
                "Sending decisions for an updated plan…"
            }
            .into()
        } else if stale {
            "A newer revision is available. This revision cannot be implemented.".into()
        } else if approval == "sent" {
            "Approved · implementation continues in this conversation.".into()
        } else if approval == "queued" {
            "Approval queued for this session.".into()
        } else if approval == "uncertain" {
            "Approval delivery is uncertain. Check the conversation before retrying.".into()
        } else if delivery == "queued" {
            "Plan update queued. Implementation is not authorized.".into()
        } else if delivery == "sent" {
            "Plan update requested. Waiting for the agent’s new revision before implementation."
                .into()
        } else if delivery == "uncertain" {
            "Update delivery is uncertain. Check the conversation before retrying.".into()
        } else if unanswered > 0 {
            "Resolve every decision before updating the plan.".into()
        } else if !decisions.is_empty() {
            "Your choices must be incorporated into a new plan before implementation.".into()
        } else {
            approval_error.clone().unwrap_or_else(|| {
                "Discuss any section in the composer, or approve this revision.".into()
            })
        };
        let mut footer = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .p(rpx(SPACE_3XL))
            .when(expanded, |s| s.max_w(rpx(PLAN_READING_WIDTH)).mx_auto())
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(status),
            );
        if !stale && !decisions.is_empty() {
            footer = footer.child(
                div()
                    .debug_selector(|| "native-plan-action-row".into())
                    .flex()
                    .child(
                        self.measured_button(
                            transcript_action(
                                "plan-update",
                                "Update plan with decisions",
                                true,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/arrow-up.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "native-plan-update".into())
                            .disabled(
                                unanswered > 0
                                    || self.composer_pending
                                    || self.send_pending
                                    || review_locked(&snapshot),
                            ),
                            "update".into(),
                            measured::Action::Update,
                            unanswered == 0
                                && !self.composer_pending
                                && !self.send_pending
                                && !review_locked(&snapshot),
                            cx,
                        ),
                    ),
            );
        } else if !stale {
            footer = footer.child(
                div()
                    .debug_selector(|| "native-plan-action-row".into())
                    .flex()
                    .flex_wrap()
                    .gap(rpx(SPACE_LG))
                    .when(doc["finalized_from"].is_object(), |row| {
                        row.child(
                            self.measured_button(
                                transcript_action(
                                    "plan-change-decisions",
                                    "Change decisions",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/edit.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .debug_selector(|| "native-plan-change-decisions".into())
                                .disabled(
                                    self.composer_pending
                                        || self.send_pending
                                        || review_locked(&snapshot),
                                ),
                                "change".into(),
                                measured::Action::Change,
                                !self.composer_pending
                                    && !self.send_pending
                                    && !review_locked(&snapshot),
                                cx,
                            ),
                        )
                    })
                    .child(
                        self.measured_button(
                            transcript_action(
                                "plan-approve",
                                if approval == "sent" {
                                    "Approved & sent"
                                } else {
                                    "Approve & implement"
                                },
                                true,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/check.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "native-plan-approve".into())
                            .disabled(
                                self.composer_pending
                                    || self.send_pending
                                    || approval_error.is_some()
                                    || matches!(approval, "queued" | "sent" | "uncertain"),
                            ),
                            "approve".into(),
                            measured::Action::Approve,
                            !self.composer_pending
                                && !self.send_pending
                                && approval_error.is_none()
                                && !matches!(approval, "queued" | "sent" | "uncertain"),
                            cx,
                        ),
                    ),
            );
        }
        if let Some(error) = self
            .error
            .clone()
            .or_else(|| {
                snapshot["review"]["approval"]["error"]
                    .as_str()
                    .map(str::to_owned)
            })
            .or_else(|| {
                snapshot["review"]["delivery"]["error"]
                    .as_str()
                    .map(str::to_owned)
            })
        {
            footer = footer.child(
                div()
                    .debug_selector(|| "native-plan-send-error".into())
                    .child(status::label(
                        "plan-error-status",
                        StatusKind::Failed,
                        error,
                        ICON_SM,
                        cx,
                    )),
            );
        }
        div()
            .id("native-plan-card")
            .debug_selector(|| "native-plan-card".into())
            .w_full()
            .min_h_0()
            .when(expanded, gpui::Styled::h_full)
            .flex()
            .flex_col()
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .bg(c::BG())
            .child(
                header
                    .debug_selector(|| "native-plan-header".into())
                    .flex_shrink_0()
                    .p(rpx(SPACE_3XL))
                    .pb_0(),
            )
            .child(
                div()
                    .id("native-plan-body")
                    .debug_selector(|| "native-plan-body".into())
                    .min_w_0()
                    .when(expanded, |s| {
                        s.min_h_0()
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                    })
                    .child(body),
            )
            .child(
                div()
                    .id("native-plan-footer")
                    .debug_selector(|| "native-plan-footer".into())
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .child(footer),
            )
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if expanded && event.keystroke.key == "escape" {
                        this.toggle_expansion(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
    use std::{cell::RefCell, rc::Rc, sync::mpsc};

    fn exercise_native_overview(provider: Provider, cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (handle, _actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let mut saved = long_decision_fixture(provider);
        let document = &mut saved.rich_plans.revisions[0].document;
        document.markdown = Some("## Implementation\nHidden but copyable technical detail.\n## Verification\nRun the provider checks.".into());
        document.presentation = Some(serde_json::from_value(json!({"outcome":"Keep the draft intact.","flow":[{"title":"Choose","detail":"Select the provider"},{"title":"Return","detail":"Verify the account"}],"included":["Sign in"],"deferred":["API keys"],"risks":["Verify provider response"],"success_criteria":["Cancel preserves drafts"],"revision_changes":[]})).unwrap());
        let mut plan = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    Snapshot::new(saved),
                    Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let html = cx.new(|_| {
                super::super::inline_html::HtmlSession::new(
                    "/tmp".into(),
                    gpui::ScrollHandle::new(),
                )
            });
            let view = cx.new(|cx| NativePlan::new(session, html, "plan".into(), "r1".into(), cx));
            plan = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let plan = plan.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(600.), gpui::px(1600.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("native-plan-overview").is_some());
        assert!(
            cx.debug_bounds("native-plan-decision-access").is_some(),
            "Decisions cannot be hidden by folded details"
        );
        assert!(cx.debug_bounds("native-plan-detail-content-0").is_none());
        let header = cx.debug_bounds("native-plan-detail-0").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        plan.read_with(cx, |view, cx| {
            assert!(view.open_sections.contains(&0));
            assert!(view
                .markdown(cx)
                .contains("Hidden but copyable technical detail."));
            assert!(view.markdown(cx).contains("Keep the draft intact."));
        });
        assert!(cx.debug_bounds("native-plan-detail-content-0").is_some());
        cx.update(|window, cx| {
            let focus = plan
                .read(cx)
                .projection
                .borrow()
                .focus_handle("section:0")
                .unwrap();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        plan.read_with(cx, |view, _| assert!(!view.open_sections.contains(&0)));
        assert!(cx.debug_bounds("native-plan-detail-content-0").is_none());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("native-plan-detail-content-0").is_some());
        for (width, rem) in [(400., 16.), (1000., 16.), (320., 32.)] {
            cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1600.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let body = cx.debug_bounds("native-plan-overview").unwrap();
            let flow = cx.debug_bounds("native-plan-flow").unwrap();
            assert!(flow.size.width <= body.size.width);
            for selector in ["native-plan-flow-step-0", "native-plan-flow-step-1"] {
                let step = cx.debug_bounds(selector).unwrap();
                assert!(
                    step.origin.x >= flow.origin.x && step.right() <= flow.right(),
                    "Flow child overflows at width {width}, rem {rem}: {step:?}, parent {flow:?}"
                );
            }
        }
    }
    #[gpui::test]
    fn codex_native_overview_preserves_decisions_copy_and_disclosure(
        cx: &mut gpui::TestAppContext,
    ) {
        exercise_native_overview(Provider::Codex, cx);
    }
    #[gpui::test]
    fn claude_native_overview_preserves_decisions_copy_and_disclosure(
        cx: &mut gpui::TestAppContext,
    ) {
        exercise_native_overview(Provider::Claude, cx);
    }

    #[test]
    fn markdown_sections_preserve_fences_and_associate_headings() {
        let source = "# Outcome\n\nSummary\n\n## Delivery\nUse a flow.\n```text\n## Not a section\n```\n\n## Checks\nVerify.\n";
        let sections = markdown_sections(source);
        assert_eq!(
            sections
                .iter()
                .map(|(title, _)| title.as_str())
                .collect::<Vec<_>>(),
            ["Overview", "Delivery", "Checks"]
        );
        assert_eq!(
            sections
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<String>(),
            source
        );
        assert!(sections[1].1.contains("## Not a section"));
    }

    #[test]
    fn unanswered_decisions_include_markdown_and_legacy_without_accepting_recommendations() {
        let document = json!({"decisions":[{"id":"one","default":"yes","options":[{"id":"yes","label":"Yes"}]}],"claims":[{"children":[{"decisions":[{"id":"two","options":[]}]}]}]});
        let decisions = all_decisions(&document);
        assert_eq!(decisions.len(), 2);
        assert!(!decision_answered(&decisions[0], &json!({})));
        assert!(!decision_answered(
            &decisions[0],
            &json!({"answers":{"one":"unknown"}})
        ));
        let review = json!({"answers":{"one":"yes"},"own_answers":{"two":"My custom answer"}});
        assert!(decisions.iter().all(|d| decision_answered(d, &review)));
        assert_eq!(resolved_answer(&decisions[0], &review), "Yes");
        assert_eq!(resolved_answer(&decisions[1], &review), "My custom answer");
    }

    #[test]
    fn repository_authoring_example_retains_legacy_assets() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut store = grove_core::rich_plan::PlanStore::default();
        store
            .import(
                "example",
                "tests/fixtures/native-plans/example.grove-plan.json",
                root.to_str().unwrap(),
                std::slice::from_ref(&root),
            )
            .unwrap();
        let revision = &store.revisions[0];
        assert_eq!(revision.document.plan_id, "native-review-example");
        assert!(revision.assets.iter().any(|a| a.mime == "image/png"));
        assert!(revision.assets.iter().any(|a| a.mime == "text/html"));
    }

    fn long_decision_fixture(provider: Provider) -> SavedSession {
        serde_json::from_value(json!({
            "key":"native-plan-test","provider":provider,"project":"Test","cwd":"/tmp","label":"Test","roots":[],"thread_id":"test-thread","items":[],"draft":"KEEP THIS DRAFT",
            "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],"document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Invitations","markdown":"# Invitations\n\n## Access\nChoose who can invite.","decisions":[{"id":"access","section":"Access","prompt":"QA wrote “When a case date has no tasks... the Tasks and Surveys tab should show No tasks”. Which empty states become the single line?","default":"admin","options":[{"id":"admin","label":"Admins","description":"Limit invitations to admins."}]}]}}]}
        })).unwrap()
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "requires macOS Metal renderer; captures native plan layout"]
    fn long_decision_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".grove-artifacts/long-plan-decision");
        std::fs::create_dir_all(&output).unwrap();
        let previous_theme = grove_core::theme::current().name.clone();
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for (width, rem) in [(1000., 16.), (400., 16.), (400., 24.)] {
                for provider in [Provider::Codex, Provider::Claude] {
                    let (handle, _actions) = Handle::channel();
                    let (_updates, receive) = mpsc::channel();
                    let window = cx
                        .open_window(
                            gpui::size(gpui::px(width), gpui::px(1000.)),
                            |window, cx| {
                                window.set_rem_size(gpui::px(rem));
                                let session = cx.new(|cx| {
                                    CodexSession::from_connection(
                                        Snapshot::new(long_decision_fixture(provider)),
                                        Connection {
                                            handle,
                                            updates: receive.into(),
                                        },
                                        cx,
                                    )
                                });
                                let html = cx.new(|_| {
                                    super::super::inline_html::HtmlSession::new(
                                        "/tmp".into(),
                                        gpui::ScrollHandle::new(),
                                    )
                                });
                                let plan = cx.new(|cx| {
                                    NativePlan::new(session, html, "plan".into(), "r1".into(), cx)
                                });
                                cx.new(|cx| {
                                    gpui_component::Root::new(plan, window, cx)
                                        .bordered(false)
                                        .bg(c::BG())
                                })
                            },
                        )
                        .unwrap();
                    for _ in 0..3 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    let path =
                        output.join(format!("{appearance}-{provider:?}-{width:.0}-{rem:.0}.png"));
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native long decision fixture: {}", path.display());
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous_theme));
            c::sync_component_theme(cx);
        });
    }

    fn exercise_decision_composer(provider: Provider, cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let saved = long_decision_fixture(provider);
        let mut snapshot = Snapshot::new(saved);
        snapshot.status = Status::Idle;
        let mut plan = None;
        let events: Rc<RefCell<Vec<NativePlanEvent>>> = Rc::default();
        let events_out = events.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    snapshot,
                    Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let html = cx.new(|_| {
                super::super::inline_html::HtmlSession::new(
                    "/tmp".into(),
                    gpui::ScrollHandle::new(),
                )
            });
            let view = cx.new(|cx| NativePlan::new(session, html, "plan".into(), "r1".into(), cx));
            cx.subscribe(&view, move |_, _, event: &NativePlanEvent, _| {
                events_out.borrow_mut().push(event.clone());
            })
            .detach();
            plan = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let plan = plan.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        // Warm rendering reuses the exact snapshot and prepared document. A
        // local answer invalidates review presentation without reparsing sections.
        cx.update(|_, cx| {
            plan.update(cx, |view, cx| {
                let first = view.snapshot(cx);
                let sections = view.prepared.borrow().sections.clone();
                assert!(Rc::ptr_eq(&first, &view.snapshot(cx)));
                view.local_review = Some(grove_core::rich_plan::ReviewState {
                    answers: HashMap::from([("access".into(), "admin".into())]),
                    ..Default::default()
                });
                let answered = view.snapshot(cx);
                assert!(!Rc::ptr_eq(&first, &answered));
                assert_eq!(answered["review"]["answers"]["access"], "admin");
                assert!(Rc::ptr_eq(&sections, &view.prepared.borrow().sections));
                view.local_review = None;
                assert!(view.snapshot(cx)["review"]["answers"]
                    .as_object()
                    .unwrap()
                    .is_empty());
                let original = view.session.read(cx).state.saved.rich_plans.revisions[0]
                    .document
                    .markdown
                    .clone();
                view.session.update(cx, |session, _| {
                    session.state.saved.rich_plans.revisions[0]
                        .document
                        .markdown = Some("## Replacement\nFresh document".into());
                });
                // Same plan/revision IDs and no observer notification: mutable
                // session access must still invalidate the identity fast path.
                let replaced = view.snapshot(cx);
                assert_eq!(
                    replaced["document"]["markdown"],
                    "## Replacement\nFresh document"
                );
                assert!(!Rc::ptr_eq(&sections, &view.prepared.borrow().sections));
                assert!(Rc::ptr_eq(&replaced, &view.snapshot(cx)));
                view.session.update(cx, |session, _| {
                    session.state.saved.rich_plans.revisions[0]
                        .document
                        .markdown = original;
                });
                assert!(!Rc::ptr_eq(&replaced, &view.snapshot(cx)));
            });
        });
        for width in [1000., 400.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1000.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let prompt = cx.debug_bounds("decision-prompt-access-line-0").unwrap();
            let decision = cx.debug_bounds("native-plan-decision-access").unwrap();
            assert!(
                prompt.size.width > decision.size.width * 0.9,
                "prompt must retain the decision width: {prompt:?} {decision:?}"
            );
            let discuss = cx
                .debug_bounds("native-plan-decision-discuss-access")
                .unwrap();
            assert!(
                discuss.origin.y >= prompt.bottom(),
                "discussion action must follow prompt"
            );
            assert!(
                discuss.right() <= decision.right() + gpui::px(1.),
                "discussion action must fit its decision: {discuss:?} {decision:?}"
            );
            assert!(
                prompt.size.height < gpui::px(300.),
                "prompt must wrap as prose rather than a character column: {prompt:?}"
            );
        }
        assert!(cx.debug_bounds("native-plan-update").is_some());
        assert!(cx.debug_bounds("native-plan-approve").is_none());
        assert!(cx
            .debug_bounds("native-plan-input-well-own_answers:access")
            .is_none());
        let notifications = Rc::new(std::cell::Cell::new(0));
        let observed = notifications.clone();
        let subscription = cx.update(|_, cx| {
            cx.observe(&plan, move |_, cx| {
                let _ = cx;
                observed.set(observed.get() + 1);
            })
        });
        let session = cx.update(|_, cx| plan.read(cx).session.clone());
        cx.update(|_, cx| {
            session.update(cx, |session, cx| {
                session.state.model = Some("unrelated-model-update".into());
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            0,
            "unrelated updates must not invalidate plans"
        );
        cx.update(|_, cx| {
            session.update(cx, |session, cx| {
                session.state.saved.rich_plans.revisions[0]
                    .review
                    .answer_drafts
                    .insert("access".into(), "Draft from actor".into());
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(
            notifications.get() > 0,
            "review updates must invalidate the plan"
        );
        drop(subscription);
        let button = cx.debug_bounds("native-plan-custom-access").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            matches!(&events.borrow()[0], NativePlanEvent::Compose { target: PlanComposerTarget::CustomAnswer { decision_id, .. }, .. } if decision_id == "access")
        );
        cx.update(|_, cx| {
            plan.update(cx, |view, cx| {
                view.save_custom_draft("access", "Team leads".into(), cx)
                    .unwrap();
                assert_eq!(view.custom_answer_draft("access", cx), "Team leads");
                assert!(!decision_answered(
                    &all_decisions(&view.snapshot(cx)["document"])[0],
                    &view.snapshot(cx)["review"]
                ));
                view.save_custom_answer("access", "Team leads".into(), cx)
                    .unwrap();
                assert!(decision_answered(
                    &all_decisions(&view.snapshot(cx)["document"])[0],
                    &view.snapshot(cx)["review"]
                ));
                assert_eq!(view.session.read(cx).state.saved.draft, "KEEP THIS DRAFT");
                view.set_composer_pending(true, cx);
                view.dispatch(false, cx);
            });
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
        )));
    }

    #[gpui::test]
    fn codex_plan_custom_answer_uses_composer_event(cx: &mut gpui::TestAppContext) {
        exercise_decision_composer(Provider::Codex, cx);
    }
    #[gpui::test]
    fn claude_plan_custom_answer_uses_composer_event(cx: &mut gpui::TestAppContext) {
        exercise_decision_composer(Provider::Claude, cx);
    }
}
