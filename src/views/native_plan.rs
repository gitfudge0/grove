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
    Disableable, Selectable,
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
    source_focus: Option<FocusHandle>,
    error: Option<String>,
}

fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn entries(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

fn target_key(kind: &str, id: &str) -> String {
    json!([kind, id]).to_string()
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
            self.source_focus
                .take()
                .unwrap_or_else(|| self.expand_focus.clone())
                .focus(window, cx);
        } else {
            self.source_focus = window.focused(cx);
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
                                .child(crate::icons::icon("arrow-right", ICON_SM, c::FG_DIM()))
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
        if self.comments_open.contains(&claim_key) {
            body = body.child(self.comment(&claim_key, snapshot, window, cx));
        }
        if open {
            if !claim["exhibit"].is_null() {
                body =
                    body.child(self.exhibit(&claim["exhibit"], &claim_key, snapshot, window, cx));
                let target = target_key("exhibit", &claim_key);
                let toggle = target.clone();
                body = body.child(
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
                let own_active = snapshot["review"]["own_answers"][&decision_id]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty());
                let answer = snapshot["review"]["answers"][&decision_id]
                    .as_str()
                    .filter(|_| !own_active)
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
        let extra_counts = format!(
            "{}{}",
            if doc["changes"]["new"].as_u64().unwrap_or_default() > 0 {
                format!(" · {} new", doc["changes"]["new"])
            } else {
                String::new()
            },
            if doc["changes"]["deleted"].as_u64().unwrap_or_default() > 0 {
                format!(" · {} deleted", doc["changes"]["deleted"])
            } else {
                String::new()
            }
        );
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
                    .icon(
                        gpui_component::Icon::default()
                            .path(gpui::SharedString::from(format!(
                                "icons/{}.svg",
                                if expanded { "unified" } else { "zen" }
                            )))
                            .size(rpx(ICON_SM)),
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
                    "Proposed · Revision {} · {} files · {} changed{}",
                    self.revision_id,
                    doc["changes"]["files"].as_u64().unwrap_or_default(),
                    doc["changes"]["changed"].as_u64().unwrap_or_default(),
                    extra_counts
                )),
        );
        let why = target_key("why", "");
        let toggle_why = why.clone();
        body = body.child(
            transcript_action(why.clone(), "Why · original requests", false, window, cx)
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/question.svg")
                        .size(rpx(ICON_SM)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggle_why, cx))),
        );
        if self.open.contains(&why) {
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
        if self.comments_open.contains(&target_key("plan", "")) {
            body = body.child(self.comment(&target_key("plan", ""), &snapshot, window, cx));
        }
        if !self.review_open {
            if let Some(error) = &self.error {
                body = body.child(div().text_color(c::FORM_ERROR()).child(error.clone()));
            }
        }
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(rpx(SPACE_LG))
                .child(
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
                )
                .child(
                    transcript_action("plan-review", "Review response", true, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/check.svg")
                                .size(rpx(ICON_SM)),
                        )
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
                .when_some(self.error.clone(), |body, error| {
                    body.child(
                        div()
                            .debug_selector(|| "native-plan-send-error".into())
                            .text_color(c::FORM_ERROR())
                            .child(error),
                    )
                })
                .when(status == "failed", |body| {
                    body.child(
                        div()
                            .text_color(c::FORM_ERROR())
                            .child(string(delivery, "error")),
                    )
                })
                .child(self.text(
                    "plan-response-text".into(),
                    response_preview(&response),
                    window,
                ))
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
                            transcript_action(
                                "plan-send-response",
                                "Send response",
                                true,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/arrow-up.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "native-plan-send".into())
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                this.send_pending = true;
                                this.session.update(cx, |session, cx| {
                                    session.send(
                                        Action::SendPlanResponse {
                                            plan_id: this.plan_id.clone(),
                                            revision_id: this.revision_id.clone(),
                                        },
                                        cx,
                                    );
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

fn response_preview(response: &str) -> String {
    // Only presentation markers generated by response_text are removed. The
    // original payload remains the source for Copy response and Send response.
    response
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.strip_prefix("# ").unwrap_or(line)
            } else {
                line.strip_prefix("> ").unwrap_or(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    #[test]
    fn response_preview_hides_generated_markers_without_interpreting_feedback() {
        let payload = "# Plan review: Test\nPlan: test · Revision: r1\n\nAnswer [choice] Where?:\n> [link](https://example.com)\n> <script>inert</script>\n";
        assert_eq!(
            response_preview(payload),
            "Plan review: Test\nPlan: test · Revision: r1\n\nAnswer [choice] Where?:\n[link](https://example.com)\n<script>inert</script>"
        );
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
