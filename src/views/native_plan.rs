//! Native review of validated, immutable session plan snapshots.

mod render;

mod behavior;
#[cfg(test)]
mod behavior_tests;
mod measured;
mod overview;
mod response;

use behavior::{response_answers, reveal_decision};

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
    pending_decision_reveal: bool,
    show_previous: bool,
    open_sections: HashSet<usize>,
    open_claims: HashSet<String>,
    reviewed_response: Option<Value>,
    response_anchor: Option<gpui::ScrollAnchor>,
    pending_response_reveal: bool,
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
            pending_decision_reveal: false,
            show_previous: false,
            open_sections: HashSet::new(),
            open_claims: HashSet::new(),
            reviewed_response: None,
            response_anchor: None,
            pending_response_reveal: false,
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
    pub(super) fn set_transcript_scroll(&mut self, scroll: gpui::ScrollHandle) {
        self.transcript_scroll = Some(scroll);
        self.decision_anchors.clear();
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
            measured::Action::ToggleClaim(id) => {
                if !self.open_claims.remove(id) {
                    self.open_claims.insert(id.clone());
                }
                self.projection.borrow_mut().invalidate();
                cx.notify();
            }
            measured::Action::Update => self.begin_response_review(window, cx),
            measured::Action::EditResponse => {
                self.reviewed_response = None;
                self.projection.borrow_mut().invalidate();
                self.focus_control("update", window, cx);
                cx.notify();
            }
            measured::Action::SendResponse => {
                let snapshot = self.snapshot(cx);
                if self.reviewed_response.as_ref() == Some(&response_answers(&snapshot)) {
                    self.dispatch(false, cx);
                }
            }
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
    fn claim_assets(
        &mut self,
        paths: &[String],
        claim_id: &str,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
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
        for asset in assets
            .into_iter()
            .filter(|asset| paths.contains(&asset.path))
        {
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
                        format!("plan-asset-{claim_id}-{path}"),
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
        if self.pending_decision_reveal && self.active_decision.as_deref() == Some(id.as_str()) {
            self.pending_decision_reveal = false;
            let anchor = anchor.clone();
            let focus = focus.clone();
            window.on_next_frame(move |window, cx| {
                anchor.scroll_to(window, cx);
                focus.focus(window, cx);
            });
        }
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

#[cfg(test)]
mod tests;
