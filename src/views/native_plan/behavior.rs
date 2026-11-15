//! Behavior-first reading, with decisions attached to the claim they change.
use super::*;
const LAYOUT_NODE_MIN_WIDTH: f32 = 150.0;

pub(super) fn response_answers(snapshot: &Value) -> Value {
    json!({"answers": snapshot["review"]["answers"], "own_answers": snapshot["review"]["own_answers"]})
}

/// Open only the ancestors needed to reach the requested decision.
pub(super) fn reveal_decision(claims: &Value, decision: &str, open: &mut HashSet<String>) -> bool {
    let mut found = false;
    for claim in entries(claims) {
        let child = reveal_decision(&claim["children"], decision, open);
        if child
            || entries(&claim["decisions"])
                .iter()
                .any(|d| d["id"] == decision)
        {
            open.insert(string(&claim, "id"));
            found = true;
        }
    }
    found
}

fn asset_paths(exhibit: &Value, paths: &mut Vec<String>) {
    if let Some(path) = exhibit["path"].as_str() {
        paths.push(path.to_owned());
    }
    for state in entries(&exhibit["states"]) {
        asset_paths(&state["screen"], paths);
    }
}

impl NativePlan {
    pub(super) fn focus_control(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.projection.borrow().handles_for_response(key);
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
    }

    pub(super) fn begin_response_review(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        let decisions = all_decisions(&snapshot["document"]);
        if self.stale(cx)
            || self.composer_pending
            || self.send_pending
            || review_locked(&snapshot)
            || decisions.is_empty()
            || decisions
                .iter()
                .any(|d| !decision_answered(d, &snapshot["review"]))
        {
            return;
        }
        self.reviewed_response = Some(response_answers(&snapshot));
        self.projection.borrow_mut().invalidate();
        let scroll = if self.expanded(cx) {
            self.scroll.clone()
        } else {
            self.transcript_scroll
                .clone()
                .unwrap_or_else(|| self.scroll.clone())
        };
        self.response_anchor = Some(gpui::ScrollAnchor::for_handle(scroll));
        self.pending_response_reveal = true;
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::views) fn configure_behavior_capture(
        &mut self,
        review: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_claims.insert("pairing".into());
        if !self.expanded(cx) {
            self.toggle_expansion(window, cx);
        }
        if review {
            self.begin_response_review(window, cx);
        }
        self.projection.borrow_mut().invalidate();
        cx.notify();
    }

    pub(super) fn source_context(
        &self,
        doc: &Value,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slot = usize::MAX - 12;
        let open = self.open_sections.contains(&slot);
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .child(self.disclosure(
                "plan-sources".into(),
                "Why this plan".into(),
                open,
                measured::Action::ToggleSection(slot),
                0,
                cx,
            ));
        if open {
            for (index, source) in entries(&doc["sources"]).iter().enumerate() {
                body = body.child(self.text(
                    format!("source-{index}"),
                    format!(
                        "{} — {}",
                        string(source, "text"),
                        string(source, "attribution")
                    ),
                    window,
                    cx,
                ));
            }
        }
        body.into_any_element()
    }

    pub(super) fn behavior_support(
        &self,
        doc: &Value,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().min_w_0().w_full();
        for (index, (key, label)) in [
            ("included", "Included"),
            ("deferred", "Later"),
            ("success_criteria", "Success criteria"),
            ("revision_changes", "Changed in this revision"),
        ]
        .iter()
        .enumerate()
        {
            let list = entries(&doc["presentation"][key]);
            if list.is_empty() {
                continue;
            }
            let slot = usize::MAX - index;
            let open = self.open_sections.contains(&slot);
            let mut section = div()
                .min_w_0()
                .w_full()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .child(self.disclosure(
                    format!("support:{key}"),
                    (*label).into(),
                    open,
                    measured::Action::ToggleSection(slot),
                    0,
                    cx,
                ));
            if open {
                for (item, entry) in list.iter().enumerate() {
                    section = section.child(self.text(
                        format!("support-{key}-{item}"),
                        entry.as_str().unwrap_or_default().to_owned(),
                        window,
                        cx,
                    ));
                }
            }
            body = body.child(section);
        }
        body.into_any_element()
    }

    pub(super) fn response_review_body(
        &mut self,
        doc: &Value,
        snapshot: &Value,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self.pending_response_reveal {
            self.pending_response_reveal = false;
            let anchor = self.response_anchor.clone();
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                if let Some(anchor) = anchor {
                    anchor.scroll_to(window, cx);
                }
                let _ = owner.update(cx, |this, cx| this.focus_control("update", window, cx));
            });
        }
        let mut body = div()
            .debug_selector(|| "native-plan-response-review".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL))
            .min_w_0()
            .w_full()
            .p(rpx(SPACE_3XL))
            .when(self.expanded(cx), |s| {
                s.max_w(rpx(PLAN_READING_WIDTH)).mx_auto()
            })
            .child(
                div()
                    .id("native-plan-review-heading")
                    .anchor_scroll(self.response_anchor.clone())
                    .text_size(rpx(TEXT_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Review your response"),
            )
            .child(self.text(
                "response-plan-title".into(),
                format!("{} · Revision {}", string(doc, "title"), self.revision_id),
                window,
                cx,
            ));
        for (index, decision) in all_decisions(doc).iter().enumerate() {
            let id = string(decision, "id");
            let answer = resolved_answer(decision, &snapshot["review"]);
            let description = entries(&decision["options"])
                .iter()
                .find(|o| o["id"] == snapshot["review"]["answers"][&id])
                .filter(|_| {
                    snapshot["review"]["own_answers"][&id]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                })
                .map(|o| string(o, "description"))
                .unwrap_or_default();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(rpx(SPACE_LG))
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .pt(rpx(SPACE_2XL))
                    .child(self.text(
                        format!("response-prompt-{id}"),
                        format!("{}. {}", index + 1, string(decision, "prompt")),
                        window,
                        cx,
                    ))
                    .child(self.text(format!("response-answer-{id}"), answer, window, cx))
                    .when(!description.is_empty(), |s| {
                        s.child(div().text_color(c::FG_DIM()).child(self.text(
                            format!("response-description-{id}"),
                            description,
                            window,
                            cx,
                        )))
                    }),
            );
        }
        body
    }

    pub(super) fn disclosure(
        &self,
        key: String,
        label: String,
        open: bool,
        action: measured::Action,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = self
            .projection
            .borrow_mut()
            .control(key.clone(), action.clone(), true, cx);
        let keyboard = action.clone();
        let debug = key.strip_prefix("section:").map_or_else(
            || format!("native-plan-{key}"),
            |index| format!("native-plan-detail-{index}"),
        );
        measured::Observed::new(
            div()
                .id(key)
                .debug_selector(move || debug.clone())
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .min_w_0()
                .w_full()
                .py(rpx(SPACE_2XL))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .aria_expanded(open)
                .cursor_pointer()
                .tab_index(0)
                .track_focus(&target.focus)
                .text_color(c::FG_DIM())
                .hover(|s| s.text_color(c::FG()))
                .focus_visible(|s| s.bg(c::BG_HOVER()))
                .child(crate::icons::icon(
                    if open { "chev-down" } else { "chev-right" },
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .child(div().flex_1().min_w_0().text_color(c::FG()).child(label))
                .when(count > 0, |s| {
                    s.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::STATUS_WARNING())
                            .child(format!("{count} to answer")),
                    )
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.activate(&action, window, cx)),
                )
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.activate(&keyboard, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                )
                .into_any_element(),
            target,
        )
        .into_any_element()
    }

    fn exhibit_content(
        &self,
        id: &str,
        exhibit: &grove_core::rich_plan::Exhibit,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let grove_core::rich_plan::Exhibit::Layout { nodes } = exhibit {
            let mut body = div()
                .flex()
                .flex_wrap()
                .gap(rpx(SPACE_LG))
                .min_w_0()
                .w_full();
            for node in nodes {
                let key = format!("{id}-{}", node.id);
                let nested = grove_core::rich_plan::Exhibit::Layout {
                    nodes: node.children.clone(),
                };
                body = body.child(
                    div()
                        .flex_1()
                        .flex_basis(rpx(LAYOUT_NODE_MIN_WIDTH))
                        .min_w_0()
                        .max_w_full()
                        .p(rpx(SPACE_LG))
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .rounded(rpx(RADIUS_CONTROL))
                        .child(self.text(format!("layout-{key}"), node.text.clone(), window, cx))
                        .when(!node.children.is_empty(), |s| {
                            s.child(self.exhibit_content(&key, &nested, window, cx))
                        }),
                );
            }
            return body.into_any_element();
        }
        self.measured_text(
            format!("claim-exhibit-{id}"),
            grove_core::rich_plan::exhibit_markdown(exhibit),
            true,
            window,
            cx,
        )
    }

    pub(super) fn behavior_claim(
        &mut self,
        claim: &Value,
        number: &str,
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = string(claim, "id");
        let open = self.open_claims.contains(&id);
        let count = all_decisions(&json!({"claims": [claim]}))
            .iter()
            .filter(|d| !decision_answered(d, &snapshot["review"]))
            .count();
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(self.disclosure(
                format!("claim:{id}"),
                format!("{number} · {}", string(claim, "text")),
                open,
                measured::Action::ToggleClaim(id.clone()),
                count,
                cx,
            ));
        if open {
            let mut content = div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("native-plan-claim-content-{id}")
                })
                .flex()
                .flex_col()
                .gap(rpx(SPACE_2XL))
                .min_w_0()
                .w_full()
                .pl(rpx(SPACE_3XL))
                .pb(rpx(SPACE_3XL));
            if let Some(detail) = claim["detail"].as_str() {
                content = content.child(self.measured_text(
                    format!("claim-detail-{id}"),
                    detail.to_owned(),
                    true,
                    window,
                    cx,
                ));
            }
            if let Ok(exhibit) =
                serde_json::from_value::<grove_core::rich_plan::Exhibit>(claim["exhibit"].clone())
            {
                content = content.child(
                    div()
                        .min_w_0()
                        .w_full()
                        .p(rpx(SPACE_2XL))
                        .bg(c::BG_RAIL())
                        .rounded(rpx(RADIUS_CONTROL))
                        .child(self.exhibit_content(&id, &exhibit, window, cx)),
                );
            }
            let mut paths: Vec<String> = entries(&claim["html_links"])
                .iter()
                .map(|link| string(link, "path"))
                .collect();
            asset_paths(&claim["exhibit"], &mut paths);
            if !paths.is_empty() {
                content = content.child(self.claim_assets(&paths, &id, window, cx));
            }
            for decision in entries(&claim["decisions"]) {
                content = content.child(self.decision(&decision, snapshot, window, cx));
            }
            for (index, child) in entries(&claim["children"]).iter().enumerate() {
                content = content.child(self.behavior_claim(
                    child,
                    &format!("{number}.{}", index + 1),
                    snapshot,
                    window,
                    cx,
                ));
            }
            content = content.child(div().flex().child(self.discuss_action(
                string(claim, "text"),
                format!("claim-discuss-{id}"),
                window,
                cx,
            )));
            body = body.child(content);
        }
        body.into_any_element()
    }

    pub(super) fn contextual_section(
        &mut self,
        section: (usize, String, String),
        decisions: &[Value],
        snapshot: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (index, title, markdown) = section;
        let open = self.open_sections.contains(&index);
        let count = decisions
            .iter()
            .filter(|d| !decision_answered(d, &snapshot["review"]))
            .count();
        let mut section = div()
            .min_w_0()
            .w_full()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(self.disclosure(
                format!("section:{index}"),
                title.clone(),
                open,
                measured::Action::ToggleSection(index),
                count,
                cx,
            ));
        if open {
            let mut content = div()
                .debug_selector(move || format!("native-plan-detail-content-{index}"))
                .min_w_0()
                .w_full()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_2XL))
                .child(self.measured_text(
                    format!("plan-detail-text-{index}"),
                    markdown,
                    true,
                    window,
                    cx,
                ));
            for decision in decisions {
                content = content.child(self.decision(decision, snapshot, window, cx));
            }
            section = section
                .child(content)
                .child(div().flex().child(self.discuss_action(
                    title,
                    format!("plan-detail-discuss-{index}"),
                    window,
                    cx,
                )));
        }
        section.into_any_element()
    }
}
