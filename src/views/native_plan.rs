//! Native review of validated, immutable session plan snapshots.
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
    Disableable,
};
use grove_core::codex::Action;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct PlanUi {
    expanded: Option<WeakEntity<NativePlan>>,
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
    }
    ui.owner = owner;
    ui.blocked = blocked;
}

pub(super) fn inline(plan: &Entity<NativePlan>, cx: &App) -> AnyElement {
    if cx
        .try_global::<PlanUi>()
        .and_then(|ui| ui.expanded.as_ref())
        .is_some_and(|p| p.entity_id() == plan.entity_id())
    {
        div()
            .text_color(c::FG_DIM())
            .child("Plan expanded · use Return to conversation above")
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
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::BG())
            .p(rpx(SPACE_3XL))
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
    error: Option<String>,
}

fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn entries(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
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
            if state
                .saved
                .rich_plans
                .revision(&this.plan_id, &this.revision_id)
                .is_some_and(|r| {
                    matches!(
                        r.review.delivery,
                        grove_core::rich_plan::ResponseDelivery::Queued { .. }
                            | grove_core::rich_plan::ResponseDelivery::Sent { .. }
                            | grove_core::rich_plan::ResponseDelivery::Uncertain { .. }
                    )
                })
            {
                this.local_review = None;
                this.inputs.clear();
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
            error: None,
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
            snapshot["review"] = serde_json::to_value(review).unwrap_or_default();
            snapshot["review"]["delivery"] = delivery;
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
                    )
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
        if bucket == "own_answers" {
            if let Some(answers) = review["answers"].as_object_mut() {
                answers.remove(target);
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
            for decision in decision_ids(&self.snapshot(cx)["document"]["claims"], id) {
                if !viewed.contains(&json!(decision)) {
                    viewed.push(json!(decision));
                }
            }
            review["viewed_decisions"] = json!(viewed);
        }
        review["expanded_claims"] = json!(self.open.iter().collect::<Vec<_>>());
        self.save_review(review, cx);
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
        if !cx.has_global::<PlanUi>() {
            cx.set_global(PlanUi::default());
        }
        let entity = cx.entity().downgrade();
        let ui = cx.global_mut::<PlanUi>();
        ui.expanded = (!expanded).then_some(entity);
        ui.owner = Some(self.html.entity_id());
        if expanded {
            self.expand_focus.focus(window, cx);
        } else {
            self.return_focus.focus(window, cx);
        }
        cx.notify();
        cx.refresh_windows();
    }

    fn text(&self, id: String, text: String, window: &Window) -> AnyElement {
        // Plain read-only text, not Markdown: authored links never bypass asset validation.
        TextView::html(
            id,
            format!(
                "<p>{}</p>",
                text.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('\n', "<br>")
            ),
        )
        .style(super::codex_view::markdown_style(window))
        .selectable(true)
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
        .disabled(source.is_none())
        .on_click(cx.listener(move |this, _, window, cx| {
            if let Some(source) = source.clone() {
                this.html.update(cx, |html, cx| {
                    html.open_snapshot(key.clone(), source, window, cx)
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
                let fence = "`".repeat(
                    text.lines()
                        .map(|l| l.chars().take_while(|c| *c == '`').count())
                        .max()
                        .unwrap_or_default()
                        .max(2)
                        + 1,
                );
                let language = string(exhibit, "language")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>();
                body = body.child(
                    TextView::markdown(
                        format!("plan-code-{target}"),
                        format!("{fence}{language}\n{text}\n{fence}"),
                    )
                    .style(super::codex_view::markdown_style(window))
                    .selectable(true),
                );
                let lines_target = format!("{target}:lines");
                let show_lines = lines_target.clone();
                body = body.child(
                    transcript_action(
                        format!("comment-{lines_target}"),
                        "Comment on a line",
                        false,
                        window,
                        cx,
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
                        let line_id = format!("{target}:line:{}", index + 1);
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
                    let id = format!("call:{}", string(&row, "id"));
                    let toggle = id.clone();
                    body = body.child(
                        transcript_action(id.clone(), string(&row, "title"), false, window, cx)
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
                            id == selected,
                            window,
                            cx,
                        )
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
                            &format!("{target}-screen"),
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
                            .when(self.fit_images.contains(&id), |image| image.w_full())
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

    fn claim(
        &mut self,
        claim: &Value,
        number: String,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = string(claim, "id");
        let toggle = id.clone();
        let comment = id.clone();
        let open = self.open.contains(&id);
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .py(rpx(SPACE_2XL))
            .gap(rpx(SPACE_2XL));
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
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
                        let key = id.clone();
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
                )
                .child(div().text_color(c::FG_DIM()).child(number.clone()))
                .child(div().flex_1().min_w_0().child(self.text(
                    format!("claim-text-{id}"),
                    string(claim, "text"),
                    window,
                )))
                .child(
                    transcript_action(
                        format!("comment-{id}"),
                        "Comment on claim",
                        false,
                        window,
                        cx,
                    )
                    .icon_only()
                    .child(crate::icons::icon("plus", ICON_MD, c::FG_DIM()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.comments_open.remove(&comment) {
                            this.comments_open.insert(comment.clone());
                        }
                        cx.notify();
                    })),
                ),
        );
        if self.comments_open.contains(&id) {
            body = body.child(self.comment(&id, snapshot, window, cx));
        }
        if open {
            if !claim["exhibit"].is_null() {
                body = body.child(self.exhibit(&claim["exhibit"], &id, snapshot, window, cx));
                let target = format!("{id}:exhibit");
                let toggle = target.clone();
                body = body.child(
                    transcript_action(
                        format!("comment-{target}"),
                        "Comment on exhibit",
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.comments_open.remove(&toggle) {
                            this.comments_open.insert(toggle.clone());
                        }
                        cx.notify();
                    })),
                );
                if self.comments_open.contains(&target) {
                    body = body.child(self.comment(&target, snapshot, window, cx));
                }
            }
            for link in entries(&claim["html_links"]) {
                body = body.child(self.html_link(&link, snapshot, window, cx));
            }
            for decision in entries(&claim["decisions"]) {
                let decision_id = string(&decision, "id");
                let answer = snapshot["review"]["answers"][&decision_id]
                    .as_str()
                    .map(str::to_owned);
                body = body.child(self.text(
                    format!("decision-{decision_id}"),
                    string(&decision, "prompt"),
                    window,
                ));
                for option in entries(&decision["options"]) {
                    let option_id = string(&option, "id");
                    let key = decision_id.clone();
                    let value = option_id.clone();
                    let default = decision["default"].as_str() == Some(&option_id);
                    let label = format!(
                        "{}{}{}",
                        string(&option, "label"),
                        if default { " (recommended)" } else { "" },
                        if answer.as_deref() == Some(&option_id) {
                            " ✓"
                        } else {
                            ""
                        }
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
                                .is_some_and(|s| !s.is_empty())
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
        body.into_any_element()
    }
}

impl Render for NativePlan {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot(cx);
        let doc = &snapshot["document"];
        let expanded = self.expanded(cx);
        if !self.restored {
            self.restored = true;
            self.open.extend(
                entries(&snapshot["review"]["expanded_claims"])
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            );
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .min_w_0()
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .bg(c::BG())
            .p(rpx(SPACE_3XL));
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_DISPLAY))
                        .child(string(doc, "title")),
                )
                .child(
                    transcript_action(
                        "plan-expand",
                        if expanded {
                            "Return to conversation"
                        } else {
                            "Expand"
                        },
                        false,
                        window,
                        cx,
                    )
                    .debug_selector(|| "native-plan-expand".into())
                    .with_focus_handle(if expanded {
                        self.return_focus.clone()
                    } else {
                        self.expand_focus.clone()
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_expansion(window, cx))),
                ),
        );
        body = body.child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(format!(
                    "Proposed · Revision {} · {} files · {} changed",
                    self.revision_id,
                    doc["changes"]["files"].as_u64().unwrap_or_default(),
                    doc["changes"]["changed"].as_u64().unwrap_or_default()
                )),
        );
        let why = "plan-why";
        body = body.child(
            transcript_action(why, "Why · original requests", false, window, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(why, cx))),
        );
        if self.open.contains(why) {
            for (index, source) in entries(&doc["sources"]).iter().enumerate() {
                body = body.child(self.text(
                    format!("quote-{index}"),
                    format!(
                        "{}: {}",
                        string(source, "attribution"),
                        string(source, "text")
                    ),
                    window,
                ));
            }
        }
        for (index, claim) in entries(&doc["claims"]).iter().enumerate() {
            body = body.child(self.claim(claim, (index + 1).to_string(), &snapshot, window, cx));
        }
        for scope in entries(&doc["scope"]) {
            body = body.child(
                div()
                    .text_color(c::FG_DIM())
                    .child(format!("– {}", scope.as_str().unwrap_or_default())),
            );
        }
        if self.comments_open.contains("plan") {
            body = body.child(self.comment("plan", &snapshot, window, cx));
        }
        if let Some(error) = &self.error {
            body = body.child(div().text_color(c::RED()).child(error.clone()));
        }
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(rpx(SPACE_LG))
                .child(
                    transcript_action("plan-comment", "Comment", false, window, cx).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.comments_open.insert("plan".into());
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    transcript_action("plan-review", "Review response", true, window, cx)
                        .debug_selector(|| "native-plan-review".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.review_open = !this.review_open;
                            cx.notify();
                        })),
                ),
        );
        if self.review_open {
            let response = self
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
                .map(|r| {
                    let mut r = r.clone();
                    if let Ok(review) = serde_json::from_value(snapshot["review"].clone()) {
                        r.review = review;
                    }
                    r.response_text()
                })
                .unwrap_or_default();
            let delivery = &snapshot["review"]["delivery"];
            let status = delivery
                .as_str()
                .unwrap_or_else(|| delivery["status"].as_str().unwrap_or("draft"));
            let locked =
                self.send_pending || matches!(status, "queued" | "sending" | "sent" | "uncertain");
            body = body
                .child(div().text_size(rpx(TEXT_TITLE)).child("Review response"))
                .child(div().text_color(c::FG_DIM()).child(format!(
                    "{} · Revision {} · {}",
                    self.plan_id, self.revision_id, status
                )))
                .child(self.text("plan-response-text".into(), response.clone(), window))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_end()
                        .gap(rpx(SPACE_LG))
                        .child(
                            transcript_action(
                                "plan-copy-response",
                                "Copy response",
                                false,
                                window,
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    response.clone(),
                                ))
                            }),
                        )
                        .child(
                            transcript_action(
                                "plan-send-response",
                                "Send response",
                                true,
                                window,
                                cx,
                            )
                            .debug_selector(|| "native-plan-send".into())
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send_pending = true;
                                this.session.update(cx, |session, cx| {
                                    session.send(
                                        Action::SendPlanResponse {
                                            plan_id: this.plan_id.clone(),
                                            revision_id: this.revision_id.clone(),
                                        },
                                        cx,
                                    )
                                });
                                cx.notify();
                            })),
                        ),
                );
        }
        div()
            .id("native-plan-card")
            .debug_selector(|| "native-plan-card".into())
            .when(expanded, |s| s.size_full().overflow_y_scroll())
            .child(body)
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
        Some("queued" | "sending" | "sent" | "uncertain")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
    use std::sync::mpsc;

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
        cx.update(|window, cx| {
            plan.update(cx, |view, cx| {
                view.toggle("claim", cx);
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
                view.open.clear();
                view.review_open = true;
                view.reveal(window, cx);
                assert!(view.expanded(cx));
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("native-plan-card").is_some());
        let send = cx
            .debug_bounds("native-plan-send")
            .expect("explicit Send response is visible");
        cx.simulate_click(send.center(), gpui::Modifiers::default());
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
            assert_eq!(view.session.read(cx).state.saved.draft, "KEEP THIS DRAFT")
        });
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
