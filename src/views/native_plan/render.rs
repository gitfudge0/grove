//! Native plan body, controls, and focus routing.

use super::*;

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
        if self
            .reviewed_response
            .as_ref()
            .is_some_and(|answers| *answers != response_answers(&snapshot))
        {
            self.reviewed_response = None;
        }
        let response_review = self.reviewed_response.as_ref() == Some(&response_answers(&snapshot))
            && !stale
            && !review_locked(&snapshot);
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
        if response_review {
            body = self.response_review_body(doc, &snapshot, window, cx);
        } else if stale && !self.show_previous && !expanded {
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
            let structured = !entries(&doc["claims"]).is_empty();
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

            let sections = self.prepared.borrow().sections.clone();
            let mut rendered = HashSet::new();
            if structured {
                if !layered {
                    body = body.child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(string(doc, "title")),
                    );
                    if let Some(summary) = doc["summary"].as_str() {
                        body = body.child(self.text(
                            "plan-summary".into(),
                            summary.to_owned(),
                            window,
                            cx,
                        ));
                    }
                }
                if !entries(&doc["sources"]).is_empty() {
                    body = body.child(self.source_context(doc, window, cx));
                }
                for (index, claim) in entries(&doc["claims"]).iter().enumerate() {
                    body = body.child(self.behavior_claim(
                        claim,
                        &(index + 1).to_string(),
                        &snapshot,
                        window,
                        cx,
                    ));
                }
                if layered {
                    body = body.child(self.behavior_support(doc, window, cx));
                }
                for decision in entries(&doc["decisions"]) {
                    body = body.child(self.decision(&decision, &snapshot, window, cx));
                }
                for (index, scope) in entries(&doc["scope"]).iter().enumerate() {
                    body = body.child(self.text(
                        format!("scope-{index}"),
                        scope.as_str().unwrap_or_default().to_owned(),
                        window,
                        cx,
                    ));
                }
            }
            for (index, (title, markdown)) in sections.iter().cloned().enumerate() {
                if structured {
                    break;
                }
                if layered {
                    let contextual: Vec<_> = decisions
                        .iter()
                        .filter(|d| d["section"].as_str() == Some(title.as_str()))
                        .cloned()
                        .collect();
                    for d in &contextual {
                        rendered.insert(string(d, "id"));
                    }
                    body = body.child(self.contextual_section(
                        (index, title, markdown),
                        &contextual,
                        &snapshot,
                        window,
                        cx,
                    ));
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
            for decision in &decisions {
                if !structured && !rendered.contains(&string(decision, "id")) {
                    body = body.child(self.decision(decision, &snapshot, window, cx));
                }
            }
            if !layered && !structured && !decisions.is_empty() {
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
        let status = if response_review {
            "Sending requests an updated plan. It does not authorize implementation.".into()
        } else if self.composer_pending {
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
                    .flex_wrap()
                    .gap(rpx(SPACE_LG))
                    .when(response_review, |row| {
                        row.child(
                            self.measured_button(
                                transcript_action(
                                    "plan-edit-response",
                                    "Edit answers",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/arrow-left.svg")
                                        .size(rpx(ICON_SM)),
                                ),
                                "edit-response".into(),
                                measured::Action::EditResponse,
                                !self.send_pending,
                                cx,
                            ),
                        )
                    })
                    .debug_selector(|| "native-plan-action-row".into())
                    .flex()
                    .child(
                        self.measured_button(
                            transcript_action(
                                "plan-update",
                                if response_review {
                                    "Send response"
                                } else {
                                    "Review response"
                                },
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
                            if response_review {
                                measured::Action::SendResponse
                            } else {
                                measured::Action::Update
                            },
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
