//! Native review of validated, immutable session plan snapshots.
use super::status::{self, StatusKind};
use super::{
    components::{header_control_frame, project_field_well, question_choice, transcript_action},
    rpx,
    tokens::*,
};
use crate::{entities::codex_session::CodexSession, theme as c};
use gpui::{
    div, prelude::*, AnyElement, App, Context, Entity, EntityId, FocusHandle, Focusable,
    Subscription, WeakEntity, Window,
};
use gpui_component::{
    input::{Input, InputEvent, InputState},
    text::TextView,
    Disableable, Selectable,
};
use grove_core::codex::Action;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

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

pub(super) fn inline(plan: &Entity<NativePlan>, cx: &App) -> AnyElement {
    if cx
        .try_global::<PlanUi>()
        .and_then(|ui| ui.expanded.as_ref().or(ui.peek.as_ref()))
        .is_some_and(|p| p.entity_id() == plan.entity_id())
    {
        div()
            .text_color(c::FG_DIM())
            .child("Plan open beside the conversation")
            .into_any_element()
    } else {
        plan.clone().into_any_element()
    }
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

pub(super) struct NativePlan {
    identity: EntityId,
    session: Entity<CodexSession>,
    html: Entity<super::inline_html::HtmlSession>,
    plan_id: String,
    revision_id: String,
    open: HashSet<String>,
    comments_open: HashSet<String>,
    inputs: HashMap<String, Entity<InputState>>,
    subscriptions: Vec<Subscription>,
    review_open: bool,
    local_review: Option<grove_core::rich_plan::ReviewState>,
    restored: bool,
    send_pending: bool,
    images: HashMap<String, std::sync::Arc<gpui::Image>>,
    line_pages: HashMap<String, usize>,
    fit_images: HashSet<String>,
    selected_states: HashMap<String, String>,
    expand_focus: FocusHandle,
    return_focus: FocusHandle,
    source_focus: Option<FocusHandle>,
    error: Option<String>,
    scroll: gpui::ScrollHandle,
    decision_anchors: HashMap<String, gpui::ScrollAnchor>,
    decision_focus: HashMap<String, FocusHandle>,
    active_decision: Option<String>,
}

// Editorial document dimensions are local to this view, not application chrome.
const PLAN_TITLE_SIZE: f32 = 26.0;
const PLAN_READING_WIDTH: f32 = 840.0;

fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn entries(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

fn target_key(kind: &str, id: &str) -> String {
    json!([kind, id]).to_string()
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

fn flattened_decisions(claims: &Value) -> Vec<(Value, Vec<String>)> {
    fn visit(claims: &Value, parents: &[String], result: &mut Vec<(Value, Vec<String>)>) {
        for claim in entries(claims) {
            let mut path = parents.to_vec();
            path.push(string(&claim, "id"));
            result.extend(
                entries(&claim["decisions"])
                    .into_iter()
                    .map(|d| (d, path.clone())),
            );
            visit(&claim["children"], &path, result);
        }
    }
    let mut result = Vec::new();
    visit(claims, &[], &mut result);
    result
}

fn decision_counts(claims: &Value, review: &Value) -> (usize, usize) {
    let mut total = 0;
    let mut unanswered = 0;
    for claim in entries(claims) {
        for decision in entries(&claim["decisions"]) {
            total += 1;
            let id = string(&decision, "id");
            let selected = review["answers"][&id].as_str().is_some_and(|answer| {
                entries(&decision["options"])
                    .iter()
                    .any(|option| option["id"] == answer)
            });
            let own = review["own_answers"][&id]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty());
            unanswered += usize::from(!selected && !own);
        }
        let (nested, pending) = decision_counts(&claim["children"], review);
        total += nested;
        unanswered += pending;
    }
    (total, unanswered)
}

fn decision_status(total: usize, unanswered: usize) -> String {
    if total == 0 {
        "No decisions requested".into()
    } else {
        format!(
            "{unanswered} unanswered · {total} {}",
            if total == 1 { "decision" } else { "decisions" }
        )
    }
}

fn initial_claims(claims: &Value, open: &mut HashSet<String>) {
    for claim in entries(claims) {
        open.insert(target_key("claim", &string(&claim, "id")));
        initial_claims(&claim["children"], open);
    }
}

fn visible_decisions(claims: &Value, open: &HashSet<String>, viewed: &mut Vec<Value>) {
    for claim in entries(claims) {
        if !open.contains(&target_key("claim", &string(&claim, "id"))) {
            continue;
        }
        for decision in entries(&claim["decisions"]) {
            let id = json!(string(&decision, "id"));
            if !viewed.contains(&id) {
                viewed.push(id);
            }
        }
        visible_decisions(&claim["children"], open, viewed);
    }
}

fn code_markdown(text: &str, language: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    let language = language
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    format!("{fence}{language}\n{text}\n{fence}")
}

impl NativePlan {
    pub(super) fn new(
        session: Entity<CodexSession>,
        html: Entity<super::inline_html::HtmlSession>,
        plan_id: String,
        revision_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&session, |this: &mut Self, session, cx| {
            let state = &session.read(cx).state;
            if this.send_pending {
                if let Some(error) = &state.error {
                    this.error = Some(error.clone());
                }
            }
            if state
                .saved
                .rich_plans
                .revision(&this.plan_id, &this.revision_id)
                .is_some_and(|r| {
                    matches!(
                        r.review.delivery,
                        grove_core::rich_plan::ResponseDelivery::Queued { .. }
                            | grove_core::rich_plan::ResponseDelivery::Uncertain { .. }
                    ) || matches!(
                        r.review.approval,
                        grove_core::rich_plan::ResponseDelivery::Queued { .. }
                            | grove_core::rich_plan::ResponseDelivery::Sent { .. }
                            | grove_core::rich_plan::ResponseDelivery::Uncertain { .. }
                    )
                })
            {
                this.local_review = None;
                this.inputs.clear();
                this.error = None;
            }

            if state.error.is_some()
                || state
                    .saved
                    .rich_plans
                    .revision(&this.plan_id, &this.revision_id)
                    .is_some_and(|r| {
                        !matches!(
                            r.review.delivery,
                            grove_core::rich_plan::ResponseDelivery::Draft
                        ) || !matches!(
                            r.review.approval,
                            grove_core::rich_plan::ResponseDelivery::Draft
                        )
                    })
            {
                this.send_pending = false;
            }
            cx.notify();
        });
        Self {
            identity: cx.entity_id(),
            session,
            html,
            plan_id,
            revision_id,
            open: HashSet::new(),
            comments_open: HashSet::new(),
            inputs: HashMap::new(),
            subscriptions: vec![subscription],
            review_open: false,
            local_review: None,
            restored: false,
            send_pending: false,
            images: HashMap::new(),
            line_pages: HashMap::new(),
            fit_images: HashSet::new(),
            selected_states: HashMap::new(),
            expand_focus: cx.focus_handle().tab_stop(true),
            return_focus: cx.focus_handle().tab_stop(true),
            source_focus: None,
            error: None,
            scroll: gpui::ScrollHandle::new(),
            decision_anchors: HashMap::new(),
            decision_focus: HashMap::new(),
            active_decision: None,
        }
    }

    fn snapshot(&self, cx: &App) -> Value {
        let mut snapshot = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .find(|r| {
                r.document.plan_id == self.plan_id && r.document.revision_id == self.revision_id
            })
            .map(|r| json!({"document": &r.document, "review": &r.review}))
            .unwrap_or_default();
        if let Some(review) = &self.local_review {
            let delivery = snapshot["review"]["delivery"].clone();
            let approval = snapshot["review"]["approval"].clone();
            snapshot["review"] = serde_json::to_value(review).unwrap_or_default();
            snapshot["review"]["delivery"] = delivery;
            snapshot["review"]["approval"] = approval;
        }
        snapshot
    }

    fn save_review(&mut self, review: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<grove_core::rich_plan::ReviewState>(review) {
            Ok(review) => {
                self.local_review = Some(review.clone());
                self.session.update(cx, |session, cx| {
                    session.send(
                        Action::UpdatePlanReview {
                            plan_id: self.plan_id.clone(),
                            revision_id: self.revision_id.clone(),
                            review,
                        },
                        cx,
                    );
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn choose(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        if self.send_pending || review_locked(&self.snapshot(cx)) {
            return;
        }
        self.inputs.remove(&format!("own_answers:{key}"));
        self.set_feedback("answers", key, value.to_owned(), cx);
    }

    fn set_feedback(&mut self, bucket: &str, target: &str, value: String, cx: &mut Context<Self>) {
        if self.send_pending || review_locked(&self.snapshot(cx)) {
            return;
        }
        let mut review = self.snapshot(cx)["review"].clone();
        if bucket == "answers" {
            if let Some(own) = review["own_answers"].as_object_mut() {
                own.remove(target);
            }
        }
        review[bucket][target] = json!(value);
        self.save_review(review, cx);
    }

    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.open.remove(id) {
            self.open.insert(id.to_owned());
        }
        let mut review = self.snapshot(cx)["review"].clone();
        if self.open.contains(id) {
            fn decision_ids(claims: &Value, id: &str) -> Vec<String> {
                for claim in entries(claims) {
                    if claim["id"] == id {
                        return entries(&claim["decisions"])
                            .iter()
                            .map(|d| string(d, "id"))
                            .collect();
                    }
                    let found = decision_ids(&claim["children"], id);
                    if !found.is_empty() {
                        return found;
                    }
                }
                Vec::new()
            }
            let mut viewed = entries(&review["viewed_decisions"]);
            let claim_id = serde_json::from_str::<(String, String)>(id)
                .ok()
                .filter(|(kind, _)| kind == "claim")
                .map(|(_, id)| id)
                .unwrap_or_default();
            for decision in decision_ids(&self.snapshot(cx)["document"]["claims"], &claim_id) {
                if !viewed.contains(&json!(decision)) {
                    viewed.push(json!(decision));
                }
            }
            review["viewed_decisions"] = json!(viewed);
        }
        review["expanded_claims"] = json!(self.open.iter().collect::<Vec<_>>());
        self.save_review(review, cx);
    }

    pub(super) fn title_and_status(&self, cx: &App) -> (String, String) {
        let snapshot = self.snapshot(cx);
        let (total, pending) =
            decision_counts(&snapshot["document"]["claims"], &snapshot["review"]);
        let status = match snapshot["review"]["approval"]["status"].as_str() {
            Some("queued") => "Approval queued".into(),
            Some("sent") => "Approved · sent to agent".into(),
            Some("uncertain") => "Check approval delivery".into(),
            Some("failed") => "Approval failed".into(),
            _ if matches!(
                snapshot["review"]["delivery"]["status"].as_str(),
                Some("queued" | "sending")
            ) =>
            {
                "Feedback queued".into()
            }
            _ if snapshot["review"]["delivery"]["status"] == "uncertain" => {
                "Check feedback delivery".into()
            }
            _ if pending > 0 => format!(
                "{pending} {} need you",
                if pending == 1 {
                    "decision"
                } else {
                    "decisions"
                }
            ),
            _ if total > 0 => "Ready for approval".into(),
            _ => "No decisions requested".into(),
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
        if let Some(ui) = cx.try_global::<PlanUi>() {
            if ui
                .peek
                .as_ref()
                .is_some_and(|p| p.entity_id() == self.identity)
            {
                cx.global_mut::<PlanUi>().peek = None;
                cx.notify();
                cx.refresh_windows();
            }
        }
    }

    pub(super) fn is_expanded(&self, cx: &App) -> bool {
        self.expanded(cx)
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn answer_preview_fixture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.choose("location", "alpha", cx);
        self.next_decision(window, cx);
        self.choose("checks", "beta", cx);
    }

    fn dispatch(&mut self, approve: bool, cx: &mut Context<Self>) {
        if self.send_pending {
            return;
        }
        self.error = None;
        self.send_pending = true;
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

    fn next_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        let decisions = flattened_decisions(&snapshot["document"]["claims"]);
        let start = self
            .active_decision
            .as_ref()
            .and_then(|id| decisions.iter().position(|(d, _)| string(d, "id") == *id))
            .map_or(0, |i| i + 1);
        let next = (0..decisions.len())
            .map(|offset| (start + offset) % decisions.len())
            .find(|i| !decision_answered(&decisions[*i].0, &snapshot["review"]));
        if let Some(i) = next {
            let (decision, ancestors) = &decisions[i];
            let id = string(decision, "id");
            for parent in ancestors {
                self.open.insert(target_key("claim", parent));
            }
            self.active_decision = Some(id.clone());
            self.review_open = false;
            cx.notify();
            // Expanded ancestors must be laid out before scrolling to their decision.
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
        }
    }

    fn expanded(&self, cx: &App) -> bool {
        cx.try_global::<PlanUi>()
            .and_then(|ui| ui.expanded.as_ref())
            .is_some_and(|p| p.entity_id() == self.identity)
    }

    pub(super) fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.expanded(cx) {
            self.toggle_expansion(window, cx);
        }
    }

    fn toggle_expansion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let expanded = self.expanded(cx);
        if !expanded && !self.send_pending && !review_locked(&self.snapshot(cx)) {
            let snapshot = self.snapshot(cx);
            let mut review = snapshot["review"].clone();
            let mut viewed = entries(&review["viewed_decisions"]);
            visible_decisions(&snapshot["document"]["claims"], &self.open, &mut viewed);
            if viewed != entries(&review["viewed_decisions"]) {
                review["viewed_decisions"] = json!(viewed);
                self.save_review(review, cx);
            }
        }
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

    fn text(&self, id: String, text: String, window: &Window) -> AnyElement {
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
                        row.child(
                            TextView::html(
                                line_id,
                                format!(
                                    "<p>{}</p>",
                                    line.replace('&', "&amp;")
                                        .replace('<', "&lt;")
                                        .replace('>', "&gt;")
                                ),
                            )
                            .style(super::codex_view::markdown_style(window))
                            .selectable(true),
                        )
                    })
            }))
            .into_any_element()
    }

    fn input(
        &mut self,
        bucket: &'static str,
        target: String,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("{bucket}:{target}");
        if !self.inputs.contains_key(&key) {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(3, 10)
                    .placeholder("e.g. Keep the compact layout and explain the retry state")
                    .default_value(initial)
            });
            let subscription = cx.subscribe_in(&input, window, move |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.set_feedback(bucket, &target, input.read(cx).value().to_string(), cx);
                }
            });
            self.subscriptions.push(subscription);
            self.inputs.insert(key.clone(), input);
        }
        let input = &self.inputs[&key];
        project_field_well(input.focus_handle(cx).is_focused(window), false)
            .child(
                Input::new(input)
                    .disabled(self.send_pending || review_locked(&self.snapshot(cx)))
                    .aria_label(if bucket == "own_answers" {
                        "Your own answer"
                    } else {
                        "Review comment"
                    })
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .text_color(c::FG())
                    .p_0(),
            )
            .into_any_element()
    }

    fn comment(
        &mut self,
        target: &str,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = snapshot["review"]["comments"][target]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.input("comments", target.to_owned(), value, window, cx)
    }

    fn html_link(
        &self,
        link: &Value,
        _snapshot: &Value,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let asset_id = string(link, "path");
        let label = string(link, "label");
        let source = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .find(|r| {
                r.document.plan_id == self.plan_id && r.document.revision_id == self.revision_id
            })
            .and_then(|r| r.assets.iter().find(|asset| asset.path == asset_id))
            .and_then(|asset| std::str::from_utf8(&asset.bytes).ok())
            .map(str::to_owned);
        let key = format!("plan:{}", json!([self.plan_id, self.revision_id, asset_id]));
        transcript_action(
            key.clone(),
            if label.is_empty() {
                "Open linked HTML".to_owned()
            } else {
                label
            },
            false,
            window,
            cx,
        )
        .icon(
            gpui_component::Icon::default()
                .path("icons/external-link.svg")
                .size(rpx(ICON_SM)),
        )
        .disabled(source.is_none())
        .on_click(cx.listener(move |this, _, window, cx| {
            if let Some(source) = source.clone() {
                this.html.update(cx, |html, cx| {
                    html.open_snapshot(key.clone(), source, window, cx);
                });
            }
        }))
        .into_any_element()
    }

    fn exhibit(
        &mut self,
        exhibit: &Value,
        target: &str,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .p(rpx(SPACE_2XL))
            .bg(c::BG_RAIL())
            .rounded(rpx(RADIUS_GROUP));
        match exhibit["kind"].as_str().unwrap_or_default() {
            "code" | "schema" => {
                let text = string(exhibit, "text");
                body = body.child(
                    TextView::markdown(
                        format!("plan-code-{target}"),
                        code_markdown(&text, &string(exhibit, "language")),
                    )
                    .style(super::codex_view::markdown_style(window))
                    .selectable(true),
                );
                let lines_target = target_key("line_picker", target);
                let show_lines = lines_target.clone();
                body = body.child(
                    transcript_action(
                        format!("comment-{lines_target}"),
                        "Comment on a line",
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/plus.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.comments_open.remove(&show_lines) {
                            this.comments_open.insert(show_lines.clone());
                        }
                        cx.notify();
                    })),
                );
                if self.comments_open.contains(&lines_target) {
                    const LINES_PER_PAGE: usize = 40;
                    let lines = text.lines().collect::<Vec<_>>();
                    let page = self.line_pages.get(target).copied().unwrap_or_default();
                    for (index, line) in lines
                        .iter()
                        .enumerate()
                        .skip(page * LINES_PER_PAGE)
                        .take(LINES_PER_PAGE)
                    {
                        let line_id = target_key("line", &json!([target, index + 1]).to_string());
                        let toggle = line_id.clone();
                        body = body.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .child(div().text_color(c::FG_DIM()).child((index + 1).to_string()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child((*line).to_owned()),
                                )
                                .child(
                                    transcript_action(
                                        format!("comment-{line_id}"),
                                        "Comment on line",
                                        false,
                                        window,
                                        cx,
                                    )
                                    .icon_only()
                                    .tooltip("Comment on line")
                                    .child(crate::icons::icon("plus", ICON_MD, c::FG_DIM()))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if !this.comments_open.remove(&toggle) {
                                                this.comments_open.insert(toggle.clone());
                                            }
                                            cx.notify();
                                        },
                                    )),
                                ),
                        );
                        if self.comments_open.contains(&line_id) {
                            body = body.child(self.comment(&line_id, snapshot, window, cx));
                        }
                    }
                    let previous = target.to_owned();
                    let next = target.to_owned();
                    body = body.child(
                        div()
                            .flex()
                            .gap(rpx(SPACE_LG))
                            .child(
                                transcript_action(
                                    format!("lines-previous-{target}"),
                                    "Previous lines",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/arrow-left.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .disabled(page == 0)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.line_pages
                                            .insert(previous.clone(), page.saturating_sub(1));
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                transcript_action(
                                    format!("lines-next-{target}"),
                                    "Next lines",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(gpui_component::Icon::default().path("icons/arrow-right.svg"))
                                .disabled((page + 1) * LINES_PER_PAGE >= lines.len())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.line_pages.insert(next.clone(), page + 1);
                                        cx.notify();
                                    },
                                )),
                            ),
                    );
                }
                if !exhibit["source"].is_null() {
                    body = body.child(
                        div()
                            .text_color(c::FG_DIM())
                            .child(string(exhibit, "source")),
                    );
                }
            }
            "calls" => {
                for row in entries(&exhibit["entries"]) {
                    let id = target_key("call", &json!([target, string(&row, "id")]).to_string());
                    let toggle = id.clone();
                    body = body.child(
                        transcript_action(id.clone(), string(&row, "title"), false, window, cx)
                            .icon(
                                gpui_component::Icon::default()
                                    .path(gpui::SharedString::from(format!(
                                        "icons/{}.svg",
                                        if self.open.contains(&id) {
                                            "chev-down"
                                        } else {
                                            "chev-right"
                                        }
                                    )))
                                    .size(rpx(ICON_SM)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggle, cx))),
                    );
                    if self.open.contains(&id) {
                        body = body.child(self.text(
                            id,
                            format!("{}\n{}", string(&row, "detail"), row["source"]),
                            window,
                        ));
                    }
                }
            }
            "state_machine" => {
                let states = entries(&exhibit["states"]);
                let selected = self
                    .selected_states
                    .get(target)
                    .cloned()
                    .unwrap_or_else(|| string(exhibit, "initial"));
                let mut tabs = div().flex().flex_wrap().gap(rpx(SPACE_LG));
                for state in &states {
                    let id = string(state, "id");
                    let key = target.to_owned();
                    let pick = id.clone();
                    tabs = tabs.child(
                        transcript_action(
                            format!("state-{target}-{id}"),
                            string(state, "label"),
                            false,
                            window,
                            cx,
                        )
                        .selected(id == selected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_states.insert(key.clone(), pick.clone());
                            cx.notify();
                        })),
                    );
                }
                body = body.child(tabs);
                if let Some(state) = states.iter().find(|s| s["id"] == selected) {
                    body = body.child(self.text(
                        format!("state-description-{target}"),
                        string(state, "description"),
                        window,
                    ));
                    if !state["screen"].is_null() {
                        body = body.child(self.exhibit(
                            &state["screen"],
                            &target_key("screen", &json!([target, selected]).to_string()),
                            snapshot,
                            window,
                            cx,
                        ));
                    }
                }
                for edge in entries(&exhibit["transitions"]) {
                    body = body.child(format!(
                        "{} → {} · {}",
                        string(&edge, "from"),
                        string(&edge, "to"),
                        string(&edge, "label")
                    ));
                }
            }
            "html_link" => body = body.child(self.html_link(exhibit, snapshot, window, cx)),
            "image" => {
                let id = string(exhibit, "path");
                if !self.images.contains_key(&id) {
                    let image = self
                        .session
                        .read(cx)
                        .state
                        .saved
                        .rich_plans
                        .revisions
                        .iter()
                        .find(|r| {
                            r.document.plan_id == self.plan_id
                                && r.document.revision_id == self.revision_id
                        })
                        .and_then(|r| r.assets.iter().find(|a| a.path == id))
                        .and_then(|a| {
                            gpui::ImageFormat::from_mime_type(&a.mime).map(|format| {
                                std::sync::Arc::new(gpui::Image::from_bytes(
                                    format,
                                    (*a.bytes).clone(),
                                ))
                            })
                        });
                    if let Some(image) = image {
                        self.images.insert(id.clone(), image);
                    }
                }
                if let Some(image) = self.images.get(&id) {
                    body = body.child(
                        gpui::img(image.clone())
                            .max_w_full()
                            .max_h(rpx(MODAL_SCROLL_MAX_H))
                            .when(self.fit_images.contains(&id), gpui::Styled::w_full)
                            .object_fit(gpui::ObjectFit::Contain),
                    );
                }
                body = body.child(div().text_color(c::FG_DIM()).child(string(exhibit, "alt")));
                let fit = id.clone();
                body = body.child(
                    transcript_action(
                        format!("image-fit-{target}"),
                        if self.fit_images.contains(&id) {
                            "Original size"
                        } else {
                            "Fit to width"
                        },
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/zen.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.fit_images.remove(&fit) {
                            this.fit_images.insert(fit.clone());
                        }
                        cx.notify();
                    })),
                );
            }
            "layout" => {
                for node in entries(&exhibit["nodes"]) {
                    body = body.child(self.layout(&node, target, window));
                }
            }
            _ => body = body.child("Unsupported exhibit"),
        }
        body.into_any_element()
    }

    fn layout(&self, node: &Value, target: &str, window: &Window) -> AnyElement {
        let mut body = div().flex().gap(rpx(SPACE_LG)).min_w_0();
        if node["kind"] != "row" {
            body = body.flex_col();
        }
        if node["kind"] == "surface" {
            body = body
                .p(rpx(SPACE_2XL))
                .bg(c::FIELD_FILL())
                .rounded(rpx(RADIUS_GROUP));
        }
        if node["kind"] == "control" {
            body = body
                .p(rpx(SPACE_LG))
                .bg(c::FIELD_FILL())
                .rounded(rpx(RADIUS_CONTROL))
                .child(string(node, "text"));
        } else if node["kind"] == "text" {
            body = body.child(self.text(
                format!("layout-{target}-{}", string(node, "id")),
                string(node, "text"),
                window,
            ));
        }
        for child in entries(&node["children"]) {
            body = body.child(self.layout(&child, target, window));
        }
        body.into_any_element()
    }

    fn decision(
        &mut self,
        decision: &Value,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap(rpx(SPACE_LG)).min_w_0();
        let decision_id = string(decision, "id");
        let own_active = snapshot["review"]["own_answers"][&decision_id]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty());
        let answer = snapshot["review"]["answers"][&decision_id]
            .as_str()
            .filter(|_| !own_active)
            .map(str::to_owned);
        body = body.child(
            div()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .pt(rpx(SPACE_3XL))
                .text_size(rpx(TEXT_SMALL))
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .text_color(if answer.is_some() || own_active {
                    StatusKind::Success.color()
                } else {
                    StatusKind::NeedsInput.color()
                })
                .child(status::mark(
                    format!("decision-answer-status-{decision_id}"),
                    if answer.is_some() || own_active {
                        StatusKind::Success
                    } else {
                        StatusKind::NeedsInput
                    },
                    ICON_SM,
                    cx,
                ))
                .child(if answer.is_some() || own_active {
                    "Decision · answer recorded"
                } else {
                    "Decision · needs your judgment"
                }),
        );
        body = body.child(self.text(
            format!("decision-{decision_id}"),
            string(decision, "prompt"),
            window,
        ));
        for option in entries(&decision["options"]) {
            let option_id = string(&option, "id");
            let key = decision_id.clone();
            let value = option_id.clone();
            let default = decision["default"].as_str() == Some(&option_id);
            let label = format!(
                "{}{}",
                string(&option, "label"),
                if default { " (recommended)" } else { "" }
            );
            let keyboard_key = key.clone();
            let keyboard_value = value.clone();
            body = body.child(
                question_choice(
                    format!("option-{decision_id}-{option_id}"),
                    label,
                    string(&option, "description"),
                    false,
                    answer.as_deref() == Some(&option_id),
                )
                .debug_selector(move || format!("native-plan-option-{value}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose(&key, &option_id, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.choose(&keyboard_key, &keyboard_value, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        body = body.child(
            div().text_color(c::FG_DIM()).child(
                if answer.is_some()
                    || snapshot["review"]["own_answers"][&decision_id]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty())
                {
                    "Answer recorded"
                } else {
                    "Recommendation has not been accepted"
                },
            ),
        );
        body = body.child(div().text_color(c::FG_DIM()).child("Your own answer"));
        body = body.child(
            self.input(
                "own_answers",
                decision_id.clone(),
                snapshot["review"]["own_answers"][&decision_id]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                window,
                cx,
            ),
        );

        let id = string(decision, "id");
        let anchor = self
            .decision_anchors
            .entry(id.clone())
            .or_insert_with(|| gpui::ScrollAnchor::for_handle(self.scroll.clone()))
            .clone();
        let focus = self
            .decision_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        body.id(format!("plan-decision-{id}"))
            .debug_selector(move || format!("native-plan-decision-{id}"))
            .track_focus(&focus)
            .anchor_scroll(Some(anchor))
            .into_any_element()
    }

    fn claim(
        &mut self,
        claim: &Value,
        number: String,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = string(claim, "id");
        let claim_key = target_key("claim", &id);
        let toggle = claim_key.clone();
        let comment = claim_key.clone();
        let open = self.open.contains(&claim_key);
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .py(rpx(SPACE_20))
            .gap(rpx(SPACE_2XL));
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_TITLE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(self.text(
                            format!("claim-text-{id}"),
                            string(claim, "text"),
                            window,
                        )),
                )
                .child(
                    transcript_action(
                        format!("comment-{id}"),
                        "Comment on claim",
                        false,
                        window,
                        cx,
                    )
                    .icon_only()
                    .tooltip("Comment on claim")
                    .child(crate::icons::icon("plus", ICON_MD, c::FG_DIM()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.comments_open.remove(&comment) {
                            this.comments_open.insert(comment.clone());
                        }
                        cx.notify();
                    })),
                ),
        );
        let (total, unanswered) = decision_counts(&json!([claim]), &snapshot["review"]);
        if total > 0 {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .text_color(if unanswered > 0 {
                        StatusKind::NeedsInput.color()
                    } else {
                        StatusKind::Success.color()
                    })
                    .child(status::mark(
                        format!("claim-decision-status-{claim_key}"),
                        if unanswered > 0 {
                            StatusKind::NeedsInput
                        } else {
                            StatusKind::Success
                        },
                        ICON_SM,
                        cx,
                    ))
                    .child(decision_status(total, unanswered)),
            );
        }
        if self.comments_open.contains(&claim_key) {
            body = body.child(self.comment(&claim_key, snapshot, window, cx));
        }
        if open {
            if let Some(detail) = claim["detail"].as_str().filter(|s| !s.is_empty()) {
                body = body.child(self.text(format!("claim-detail-{id}"), detail.into(), window));
            }
            if !claim["exhibit"].is_null() {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Evidence"),
                );
                body =
                    body.child(self.exhibit(&claim["exhibit"], &claim_key, snapshot, window, cx));
                let target = target_key("exhibit", &claim_key);
                let toggle = target.clone();
                body = body.child(
                    div().flex().child(
                        transcript_action(
                            format!("comment-{target}"),
                            "Comment on exhibit",
                            false,
                            window,
                            cx,
                        )
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/plus.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.comments_open.remove(&toggle) {
                                this.comments_open.insert(toggle.clone());
                            }
                            cx.notify();
                        })),
                    ),
                );
                if self.comments_open.contains(&target) {
                    body = body.child(self.comment(&target, snapshot, window, cx));
                }
            }
            for link in entries(&claim["html_links"]) {
                body = body.child(self.html_link(&link, snapshot, window, cx));
            }
            for decision in entries(&claim["decisions"]) {
                body = body.child(self.decision(&decision, snapshot, window, cx));
            }
            for (index, child) in entries(&claim["children"]).iter().enumerate() {
                body = body.child(div().pl(rpx(SPACE_3XL)).child(self.claim(
                    child,
                    format!("{number}.{}", index + 1),
                    snapshot,
                    window,
                    cx,
                )));
            }
        }
        div()
            .flex()
            .items_start()
            .gap(rpx(SPACE_LG))
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .flex_shrink_0()
                    .w(rpx(ICON_BTN_W))
                    .py(rpx(SPACE_20))
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .text_color(c::FG_DIM())
                            .child(number),
                    )
                    .child(
                        header_control_frame(
                            format!("claim-{id}"),
                            format!(
                                "{} {}",
                                if open { "Collapse" } else { "Expand" },
                                string(claim, "text")
                            ),
                            c::BG_HOVER(),
                        )
                        .aria_expanded(open)
                        .tab_index(0)
                        .focus_visible(|s| s.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            if open { "chev-down" } else { "chev-right" },
                            ICON_MD,
                            c::FG_DIM(),
                        ))
                        .on_key_down(cx.listener({
                            let key = claim_key.clone();
                            move |this, event: &gpui::KeyDownEvent, _, cx| {
                                let selected = match event.keystroke.key.as_str() {
                                    "right" => !this.open.contains(&key),
                                    "left" => this.open.contains(&key),
                                    "space" | "enter" => true,
                                    _ => false,
                                };
                                if selected {
                                    this.toggle(&key, cx);
                                    cx.stop_propagation();
                                }
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggle, cx))),
                    ),
            )
            .child(body.flex_1().min_w_0())
            .into_any_element()
    }
}

impl Render for NativePlan {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot(cx);
        let doc = &snapshot["document"];
        let expanded = self.expanded(cx);
        let peek = cx
            .try_global::<PlanUi>()
            .and_then(|ui| ui.peek.as_ref())
            .is_some_and(|p| p.entity_id() == self.identity);
        if !self.restored {
            self.restored = true;
            self.open.extend(
                entries(&snapshot["review"]["expanded_claims"])
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            );
        }
        let marker = target_key("presentation", "editorial");
        if !self.open.contains(&marker) {
            if self.open.is_empty() {
                initial_claims(&doc["claims"], &mut self.open);
            }
            self.open.insert(marker);
        }
        let (total, unanswered) = decision_counts(&doc["claims"], &snapshot["review"]);
        let progress = if total == 0 {
            "No decisions requested".into()
        } else {
            format!("{} of {total} answered", total - unanswered)
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .min_w_0()
            .w_full()
            .when(expanded, |s| s.max_w(rpx(PLAN_READING_WIDTH)).mx_auto())
            .p(rpx(SPACE_3XL));
        let title = div()
            .min_w_0()
            .text_size(rpx(if expanded {
                PLAN_TITLE_SIZE
            } else {
                TEXT_TITLE
            }))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(string(doc, "title"));
        let navigation = transcript_action(
            "plan-expand",
            if expanded {
                "Conversation"
            } else if peek {
                "Expand"
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
        })
        .on_click(cx.listener(|this, _, window, cx| this.toggle_expansion(window, cx)));
        let header = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .min_w_0()
            .child(div().flex_1().min_w_0().child(title))
            .child(navigation);
        let mut fixed_header = Some(header);
        if !expanded && !peek {
            if let Some(header) = fixed_header.take() {
                body = body.child(header);
            }
        }
        if let Some(summary) = doc["summary"].as_str().filter(|s| !s.is_empty()) {
            body = body.child(self.text("plan-summary".into(), summary.into(), window));
        }
        body = body.child(
            div()
                .text_color(c::FG_DIM())
                .text_size(rpx(TEXT_SMALL))
                .child(format!(
                    "{} · {} · Revision {}",
                    if self.session.read(cx).state.saved.provider
                        == grove_core::codex::Provider::Claude
                    {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    self.session
                        .read(cx)
                        .state
                        .saved
                        .title
                        .as_deref()
                        .unwrap_or(&self.session.read(cx).state.saved.label),
                    self.revision_id
                )),
        );
        if !expanded && !peek {
            return div()
                .id("native-plan-card")
                .debug_selector(|| "native-plan-card".into())
                .child(
                    body.child(
                        div()
                            .text_color(c::FG_DIM())
                            .text_size(rpx(TEXT_SMALL))
                            .child(decision_status(total, unanswered)),
                    ),
                );
        }
        let response = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revision(&self.plan_id, &self.revision_id)
            .map(|r| {
                let mut r = r.clone();
                if let Ok(review) = serde_json::from_value(snapshot["review"].clone()) {
                    r.review = review;
                }
                r.response_text()
            })
            .unwrap_or_default();
        if self.review_open {
            // Review occupies the same body, rather than being appended after a long plan.
            body = div().flex().flex_col().gap(rpx(SPACE_2XL)).min_w_0().w_full()
                .when(expanded, |s| s.max_w(rpx(PLAN_READING_WIDTH)).mx_auto()).p(rpx(SPACE_3XL))
                .child(div().text_size(rpx(TEXT_TITLE)).font_weight(gpui::FontWeight::SEMIBOLD).child("Review your response"))
                .child(div().text_color(c::FG_DIM()).child("Exact feedback to your agent. Sending feedback does not approve implementation."))
                .child(self.text("plan-response-text".into(), response.clone(), window));
        } else if peek {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Main changes"),
            );
            for claim in entries(&doc["claims"]) {
                body = body.child(self.text(
                    format!("peek-claim-{}", string(&claim, "id")),
                    string(&claim, "text"),
                    window,
                ));
            }
            let decisions = flattened_decisions(&doc["claims"]);
            let active = self
                .active_decision
                .as_ref()
                .and_then(|id| decisions.iter().find(|(d, _)| string(d, "id") == *id))
                .or_else(|| {
                    decisions
                        .iter()
                        .find(|(d, _)| !decision_answered(d, &snapshot["review"]))
                })
                .or_else(|| decisions.first());
            if let Some((decision, _)) = active {
                self.active_decision = Some(string(decision, "id"));
                body = body.child(self.decision(decision, &snapshot, window, cx));
            }
            if self.comments_open.contains(&target_key("plan", "")) {
                body = body.child(self.comment(&target_key("plan", ""), &snapshot, window, cx));
            }
        } else {
            let why = target_key("why", "");
            let toggle = why.clone();
            body = body.child(
                div().flex().child(
                    transcript_action(why.clone(), "Why · original requests", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/question.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggle, cx))),
                ),
            );
            if self.open.contains(&why) {
                for (i, source) in entries(&doc["sources"]).iter().enumerate() {
                    body = body.child(self.text(
                        format!("quote-{i}"),
                        format!(
                            "{}: {}",
                            string(source, "attribution"),
                            string(source, "text")
                        ),
                        window,
                    ));
                }
            }
            for (i, claim) in entries(&doc["claims"]).iter().enumerate() {
                body =
                    body.child(self.claim(claim, format!("{:02}", i + 1), &snapshot, window, cx));
            }
            for scope in entries(&doc["scope"]) {
                body = body.child(
                    div()
                        .text_color(c::FG_DIM())
                        .child(scope.as_str().unwrap_or_default().to_owned()),
                );
            }
            if self.comments_open.contains(&target_key("plan", "")) {
                body = body.child(self.comment(&target_key("plan", ""), &snapshot, window, cx));
            }
        }
        let feedback_status = snapshot["review"]["delivery"]["status"]
            .as_str()
            .unwrap_or("draft");
        let approval_status = snapshot["review"]["approval"]["status"]
            .as_str()
            .unwrap_or("draft");
        let feedback_locked = self.send_pending
            || matches!(feedback_status, "queued" | "sending" | "sent" | "uncertain");
        let feedback_pending = matches!(feedback_status, "queued" | "sending" | "uncertain");
        let approval_locked = self.send_pending
            || feedback_pending
            || matches!(approval_status, "queued" | "sending" | "sent" | "uncertain");
        let stale = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .rev()
            .find(|r| r.document.plan_id == self.plan_id)
            .is_none_or(|r| r.document.revision_id != self.revision_id);
        let reason = if stale {
            "A newer revision is available. Open the current plan.".into()
        } else if approval_status == "sent" {
            "Approval sent to your agent".into()
        } else if approval_status == "queued" {
            "Approval queued for your agent".into()
        } else if approval_status == "uncertain" {
            "Approval delivery is uncertain. Check the conversation before retrying.".into()
        } else if feedback_pending {
            "Wait for feedback delivery before approving".into()
        } else if unanswered > 0 {
            format!(
                "Answer {unanswered} {} to approve",
                if unanswered == 1 {
                    "decision"
                } else {
                    "decisions"
                }
            )
        } else {
            "Ready for your approval".into()
        };
        let mut actions = div().flex().flex_wrap().items_center().gap(rpx(SPACE_LG));
        if unanswered > 0 {
            actions = actions.child(
                transcript_action("plan-next-decision", "Next decision", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/chev-down.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(|| "native-plan-next-decision".into())
                    .on_click(cx.listener(|this, _, window, cx| this.next_decision(window, cx))),
            );
        }
        if !self.review_open {
            actions = actions.child(
                transcript_action("plan-comment", "Comment", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/plus.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.comments_open.insert(target_key("plan", ""));
                        cx.notify();
                    })),
            );
        }
        actions = actions
            .child(
                div()
                    .flex_auto()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(c::FG_DIM())
                    .text_size(rpx(TEXT_SMALL))
                    .debug_selector(|| "native-plan-progress".into())
                    .child(progress),
            )
            .child(
                transcript_action(
                    "plan-review",
                    if self.review_open {
                        "Back to plan"
                    } else {
                        "Review response"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(if self.review_open {
                            "icons/unified.svg"
                        } else {
                            "icons/check.svg"
                        })
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(|| "native-plan-review".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.review_open = !this.review_open;
                    this.scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                    cx.notify();
                })),
            );
        if self.review_open {
            actions = actions
                .child(
                    transcript_action("plan-copy-response", "Copy response", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/copy.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                response.clone(),
                            ));
                        }),
                )
                .child(
                    transcript_action("plan-send-response", "Send response", true, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/arrow-up.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .debug_selector(|| "native-plan-send".into())
                        .disabled(feedback_locked)
                        .on_click(cx.listener(|this, _, _, cx| this.dispatch(false, cx))),
                );
        }
        actions = actions.child(
            transcript_action("plan-approve", "Approve & implement", true, window, cx)
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/check.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(|| "native-plan-approve".into())
                .disabled(unanswered > 0 || stale || approval_locked)
                .on_click(cx.listener(|this, _, _, cx| this.dispatch(true, cx))),
        );
        let mut footer = div()
            .id("native-plan-footer")
            .debug_selector(|| "native-plan-footer".into())
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .p(rpx(SPACE_2XL))
            .bg(c::SURFACE_RAISED())
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(actions)
            .child(
                div()
                    .text_color(c::FG_DIM())
                    .text_size(rpx(TEXT_SMALL))
                    .child(reason),
            );
        if feedback_status != "draft" {
            footer = footer.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(format!("Feedback {feedback_status}")),
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
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .bg(c::BG())
            .when_some(fixed_header, |surface, header| {
                surface.child(header.flex_shrink_0().p(rpx(SPACE_3XL)).pb_0())
            })
            .child(
                div()
                    .id("native-plan-body")
                    .debug_selector(|| "native-plan-body".into())
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .child(footer)
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && expanded {
                        this.toggle_expansion(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
    }
}

fn review_locked(snapshot: &Value) -> bool {
    matches!(
        snapshot["review"]["delivery"]["status"].as_str(),
        Some("queued" | "sending" | "uncertain")
    ) || matches!(
        snapshot["review"]["approval"]["status"].as_str(),
        Some("queued" | "sending" | "sent" | "uncertain")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
    use std::sync::mpsc;

    #[test]
    fn unanswered_counts_ignore_recommendations_and_include_nested_decisions() {
        let claims = json!([{"decisions":[{"id":"audience","default":"all","options":[{"id":"all"}]}],
            "children":[{"decisions":[{"id":"activation","options":[{"id":"user"}]}]}]}]);
        assert_eq!(decision_counts(&claims, &json!({})), (2, 2));
        assert_eq!(
            decision_counts(
                &claims,
                &json!({"answers":{"audience":"all", "activation":"invalid"}})
            ),
            (2, 1)
        );
        assert_eq!(
            decision_counts(
                &claims,
                &json!({"answers":{"audience":"all"},"own_answers":{"activation":"  "}})
            ),
            (2, 1)
        );
        assert_eq!(
            decision_counts(
                &claims,
                &json!({"answers":{"audience":"all"},"own_answers":{"activation":"Only administrators"}})
            ),
            (2, 0)
        );
    }

    #[test]
    fn only_visible_claim_decisions_become_viewed_without_accepting_defaults() {
        let claims = json!([{"id":"one","decisions":[{"id":"visible","default":"yes"}],"children":[{"id":"nested","decisions":[{"id":"hidden-child"}]}]}, {"id":"two","decisions":[{"id":"hidden-sibling"}]}]);
        let open = HashSet::from([target_key("claim", "one")]);
        let mut viewed = vec![];
        visible_decisions(&claims, &open, &mut viewed);
        assert_eq!(viewed, vec![json!("visible")]);
        visible_decisions(&claims, &open, &mut viewed);
        assert_eq!(viewed, vec![json!("visible")]);
    }

    /// Actual Metal/text shaping without touching the installed application.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn native_plan_brief_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
        });
        let previous_theme = grove_core::theme::current().name.clone();
        let previous_font_size = cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size);
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for (width, zoom) in [(1024.0, 1.0), (400.0, 1.0), (400.0, 1.25)] {
                cx.update(|cx| {
                    gpui_component::Theme::global_mut(cx).font_size =
                        gpui::px(crate::zoom::REM_BASE * zoom);
                });
                for mode in ["entry", "peek", "plan", "response"] {
                    let nested_claim = json!({"id":"verification","text":"Verify the approved change before delivery.", "decisions":[{"id":"verification-choice","prompt":"Which checks should run?","options":[{"id":"focused","label":"Focused checks","description":"Exercise changed interactions before delivery."}]}]});
                    let saved: SavedSession = serde_json::from_value(json!({
                        "key":"plan-brief-qa","provider":"codex","project":"QA","cwd":"/tmp",
                        "label":"QA","roots":[],"thread_id":"qa-thread","items":[],
                        "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],
                            "document":{"schema_version":1,"plan_id":"plan","revision_id":"r1",
                                "title":"Invite customers to try Insights",
                                "summary":"Give existing customers a useful first look, then let eligible users opt in with a clear, recoverable activation flow.",
                                "claims":[{"id":"audience","text":"Make audience and activation an explicit choice",
                                    "detail":"These choices change permissions, page copy and the enrollment contract. Resolve them before building the invitation page and server APIs.",
                                    "exhibit":{"kind":"schema","text":"Authenticated session → eligibility → invitation → enrollment"},
                                    "decisions":[{"id":"targeting","prompt":"Who sees the invitation?", "default":"eligible",
                                        "options":[{"id":"eligible","label":"Eligible signed-in users","description":"Show the invitation to people whose workspace qualifies."},
                                            {"id":"admins","label":"Workspace administrators only","description":"Let administrators introduce early access to the workspace."}]}], "children":[nested_claim]}],
                                "scope":["No email delivery or billing changes."]}}]}
                    })).unwrap();
                    let state = Snapshot::new(saved);
                    let (handle, _actions) = Handle::channel();
                    let (_updates, receive) = mpsc::channel();
                    let mut captured = None;
                    let window = cx
                        .open_window(
                            gpui::size(
                                gpui::px(width),
                                gpui::px(if mode == "peek" { 620. } else { 1200.0 }),
                            ),
                            |window, cx| {
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
                                let html = cx.new(|_| {
                                    super::super::inline_html::HtmlSession::new(
                                        "/tmp".into(),
                                        gpui::ScrollHandle::new(),
                                    )
                                });
                                let view = cx.new(|cx| {
                                    NativePlan::new(session, html, "plan".into(), "r1".into(), cx)
                                });
                                captured = Some(view.clone());
                                cx.new(|cx| {
                                    gpui_component::Root::new(view, window, cx)
                                        .bordered(false)
                                        .bg(c::BG())
                                })
                            },
                        )
                        .unwrap();
                    cx.update_window(window.into(), |_, window, cx| {
                        captured.as_ref().unwrap().update(cx, |view, cx| {
                            if mode == "peek" { view.show_peek(None, cx); }
                            else if mode != "entry" { view.reveal(window, cx); }
                            if mode == "response" {
                                view.choose("targeting", "eligible", cx);
                                view.set_feedback("comments", &target_key("claim", "audience"), "Keep tenant context server-derived.\nVerify repeat enrollment.".into(), cx);
                                view.review_open = true;
                                // Capture the review footer without duplicating the long document.
                                view.open.insert(target_key("presentation", "editorial"));
                                view.open.remove(&target_key("claim", "audience"));
                            }
                            cx.notify();
                        });
                    }).unwrap();
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        let _ = window.draw(cx);
                        assert_eq!(
                            window.rem_size(),
                            gpui::px(crate::zoom::REM_BASE * zoom),
                            "Root must use the requested QA zoom"
                        );
                    })
                    .unwrap();
                    let suffix = if zoom > 1.0 { "-zoom125" } else { "" };
                    let path = format!(
                        "/tmp/grove-native-plan-{appearance}-{mode}-{width:.0}{suffix}.png"
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    if zoom > 1.0 {
                        let baseline =
                            format!("/tmp/grove-native-plan-{appearance}-{mode}-{width:.0}.png");
                        assert_ne!(
                            fs_err::read(&path).unwrap(),
                            fs_err::read(&baseline).unwrap(),
                            "zoom capture must visibly differ from its baseline"
                        );
                    }
                    eprintln!("Native plan fixture: {path}");
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous_theme));
            c::sync_component_theme(cx);
            gpui_component::Theme::global_mut(cx).font_size = previous_font_size;
        });
    }

    #[test]
    fn repository_authoring_example_matches_the_native_contract() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut store = grove_core::rich_plan::PlanStore::default();
        store
            .import(
                "example",
                "docs/feat/native-plans/example.grove-plan.json",
                root.to_str().unwrap(),
                std::slice::from_ref(&root),
            )
            .unwrap();
        let revision = &store.revisions[0];
        assert_eq!(revision.document.plan_id, "native-review-example");
        assert_eq!(revision.assets.len(), 2);
        assert!(revision.assets.iter().any(|a| a.mime == "image/png"));
        assert!(revision.assets.iter().any(|a| a.mime == "text/html"));
    }

    #[test]
    fn indented_authored_fences_remain_one_inert_code_block() {
        let source = "   ```\n[Unexpected](https://example.com)\n   ```";
        let parsed = markdown::to_mdast(
            &code_markdown(source, "text"),
            &markdown::ParseOptions::default(),
        )
        .unwrap();
        let markdown::mdast::Node::Root(root) = parsed else {
            panic!("expected root")
        };
        assert_eq!(
            root.children.len(),
            1,
            "authored content cannot escape the code block"
        );
        let markdown::mdast::Node::Code(code) = &root.children[0] else {
            panic!("expected inert code")
        };
        assert_eq!(code.value, source);
    }

    fn exercise_review(provider: Provider, cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let saved: SavedSession = serde_json::from_value(json!({
            "key":"native-plan-test","provider":provider,"project":"Test","cwd":"/tmp",
            "label":"Test","roots":[],"thread_id":"test-thread","items":[],"draft":"KEEP THIS DRAFT",
            "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],
              "document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"A native plan",
                "claims":[{"id":"claim","text":"The native card preserves this session.","decisions":[
                  {"id":"decision","prompt":"Where should this open?","options":[{"id":"inline","label":"Inline card"}],"default":"inline"}]}]}}]}
        })).unwrap();
        let mut snapshot = Snapshot::new(saved);
        snapshot.status = Status::Idle;
        let mut plan = None;
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
            plan = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let plan = plan.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|window, cx| {
            plan.update(cx, |view, cx| {
                assert!(
                    entries(&view.snapshot(cx)["review"]["viewed_decisions"]).is_empty(),
                    "compact entry has not shown decisions"
                );
                view.reveal(window, cx);
                assert_eq!(
                    entries(&view.snapshot(cx)["review"]["viewed_decisions"]),
                    vec![json!("decision")]
                );
                assert!(view.snapshot(cx)["review"]["answers"]["decision"].is_null());
                view.toggle(&target_key("claim", "claim"), cx);
                view.set_feedback("comments", "claim", "First comment".into(), cx);
                view.set_feedback("own_answers", "decision", "Keep it native".into(), cx);
                view.set_feedback("comments", "claim:exhibit", "Second comment".into(), cx);
                // Deliberately no actor poll between edits: the last patch must retain all fields.
                let pending = view.snapshot(cx);
                assert_eq!(pending["review"]["comments"]["claim"], "First comment");
                assert_eq!(
                    pending["review"]["comments"]["claim:exhibit"],
                    "Second comment"
                );
                assert_eq!(
                    pending["review"]["own_answers"]["decision"],
                    "Keep it native"
                );
                let _ = view.input(
                    "own_answers",
                    "decision".into(),
                    "Keep it native".into(),
                    window,
                    cx,
                );
                assert!(view.inputs.contains_key("own_answers:decision"));
                view.choose("decision", "inline", cx);
                assert!(!view.inputs.contains_key("own_answers:decision"));
                let pending = view.snapshot(cx);
                assert_eq!(pending["review"]["answers"]["decision"], "inline");
                assert!(pending["review"]["own_answers"]["decision"].is_null());
                view.set_feedback("own_answers", "decision", "A custom answer".into(), cx);
                assert_eq!(view.snapshot(cx)["review"]["answers"]["decision"], "inline");
                view.set_feedback("own_answers", "decision", String::new(), cx);
                assert_eq!(view.snapshot(cx)["review"]["answers"]["decision"], "inline");
                let plan_comment = target_key("plan", "");
                let claim_comment = target_key("claim", "plan");
                view.set_feedback("comments", &plan_comment, "Whole plan".into(), cx);
                view.set_feedback(
                    "comments",
                    &claim_comment,
                    "The claim named plan".into(),
                    cx,
                );
                let pending = view.snapshot(cx);
                assert_eq!(pending["review"]["comments"][&plan_comment], "Whole plan");
                assert_eq!(
                    pending["review"]["comments"][&claim_comment],
                    "The claim named plan"
                );
                let reserved_claim = target_key("claim", "plan-why");
                let why = target_key("why", "");
                view.toggle(&reserved_claim, cx);
                view.toggle(&why, cx);
                view.toggle(&reserved_claim, cx);
                assert!(view.open.contains(&why));
                assert!(!view.open.contains(&reserved_claim));

                view.open.clear();
                view.review_open = true;
                view.reveal(window, cx);
                assert!(view.expanded(cx));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("native-plan-card").is_some());
        // Inspect the rendered layout, not merely response_text's newlines. A
        // single TextView with <br> tags previously flattened all these blocks.
        for width in [460., 1000.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1600.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let mut bottom = None;
            for selector in [
                "plan-response-text-line-0", // title
                "plan-response-text-line-1", // plan/revision
                "plan-response-text-line-2", // paragraph spacing
                "plan-response-text-line-3", // feedback notice
                "plan-response-text-line-4", // paragraph spacing
                "plan-response-text-line-5", // decision prompt
                "plan-response-text-line-6", // selected answer
            ] {
                let bounds = cx
                    .debug_bounds(selector)
                    .expect("every response line has an independent rendered block");
                assert!(bounds.size.height > gpui::px(0.));
                if let Some(previous_bottom) = bottom {
                    assert!(
                        bounds.top() >= previous_bottom,
                        "response lines cannot overlap"
                    );
                }
                assert!(bounds.size.width > gpui::px(0.));
                bottom = Some(bounds.bottom());
            }
        }
        let send = cx
            .debug_bounds("native-plan-send")
            .expect("explicit Send response is visible");
        cx.simulate_click(send.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            plan.update(cx, |view, cx| {
                let before = view.snapshot(cx);
                view.set_feedback("comments", "claim", "Unsafely late edit".into(), cx);
                assert_eq!(view.snapshot(cx)["review"], before["review"]);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_click(send.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        let sent = actions.try_iter().collect::<Vec<_>>();
        assert_eq!(
            sent.iter()
                .filter(|a| matches!(a, Action::SendPlanResponse { .. }))
                .count(),
            1
        );
        assert!(!sent.iter().any(|a| matches!(
            a,
            Action::Draft(_)
                | Action::Prompt(_)
                | Action::Submit { .. }
                | Action::SubmitWithMode { .. }
        )));
        plan.read_with(cx, |view, cx| {
            assert_eq!(view.session.read(cx).state.saved.draft, "KEEP THIS DRAFT");
        });
        cx.update(|_, cx| {
            plan.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.error = Some("Provider unavailable for this review".into());
                    cx.notify();
                });
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        plan.read_with(cx, |view, _| {
            assert_eq!(
                view.error.as_deref(),
                Some("Provider unavailable for this review")
            );
            assert!(!view.send_pending);
        });
        assert!(cx.debug_bounds("native-plan-send-error").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        plan.read_with(cx, |view, cx| assert!(!view.expanded(cx)));
    }

    #[gpui::test]
    fn codex_native_review_preserves_rapid_edits_draft_and_single_explicit_send(
        cx: &mut gpui::TestAppContext,
    ) {
        exercise_review(Provider::Codex, cx);
    }

    #[gpui::test]
    fn claude_native_review_preserves_rapid_edits_draft_and_single_explicit_send(
        cx: &mut gpui::TestAppContext,
    ) {
        exercise_review(Provider::Claude, cx);
    }
}
