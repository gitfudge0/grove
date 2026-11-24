use super::*;

#[gpui::test]
fn plan_approval_receipt_discloses_payload_and_opens_exact_revision(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.expanded.clear();
                view.session.update(cx, |session, cx| {
                    session.state = approval_receipt_snapshot(provider).into();
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("plan-approval-receipt-0").is_some());
        assert!(cx.debug_bounds("plan-approval-details-0").is_none());
        assert!(cx.debug_bounds("codex-message-body-0").is_none());
        let payload = view.read_with(cx, |view, cx| {
            codex::item_text(&view.session.read(cx).state.saved.items[0])
        });
        let row = cx.debug_bounds("codex-message-row-0").unwrap();
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        draw(cx);
        let copy = cx.debug_bounds("codex-copy").unwrap();
        cx.simulate_click(copy.center(), gpui::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(payload)
        );
        let toggle = cx.debug_bounds("plan-approval-details-toggle-0").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("plan-approval-details-0").is_some());
        let toggle = cx.debug_bounds("plan-approval-details-toggle-0").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("plan-approval-details-0").is_none());
        let open = cx.debug_bounds("plan-approval-open-0").unwrap();
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.native_plans
            [&("receipt-plan".into(), "reviewed-r1".into())]
            .read(cx)
            .is_expanded(cx)));
        cx.update(|_, cx| crate::views::native_plan::set_context(None, false, cx));
    }
}

#[gpui::test]
fn extended_number_and_text_questions_render_defaults_validate_and_cancel(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let mut request = codex::QuestionRequest::from_codex(
        json!("extended"),
        &json!({"questions":[
            {"id":"count","question":"How many slides?"},
            {"id":"title","question":"What title?"}
        ]}),
    )
    .unwrap();
    // Core tests cover the Claude wire decoder; this exercises its typed view contract.
    request.title = Some("Before building".into());
    request.questions[0].kind = codex::QuestionKind::Number(codex::QuestionNumber {
        min: 1.,
        max: 20.,
        step: Some(1.),
        default_value: Some(8.),
        unit: Some("slides".into()),
    });
    request.questions[0].description = Some("For the briefing.".into());
    request.questions[1].placeholder = Some("e.g. Quarterly briefing".into());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.status = Status::Waiting;
                session.state.questions.push(request.clone());
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("question-input-0-0").is_some());
    assert!(cx.debug_bounds("question-input-0-1").is_some());
    let key = view.read_with(cx, |view, cx| {
        question_key(&view.session.read(cx).state, &request)
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.question_drafts[&key].questions[0]
                    .input
                    .read(cx)
                    .value()
                    .as_ref(),
                "8"
            );
            let title = view.question_drafts[&key].questions[1].input.clone();
            title.update(cx, |input, cx| {
                input.set_value("Quarterly briefing", window, cx);
            });
            let number = view.question_drafts[&key].questions[0].input.clone();
            number.update(cx, |input, cx| input.set_value("21", window, cx));
            view.answer_questions(&key, json!("extended"), false, cx);
        });
    });
    draw(cx);
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    assert!(actions_at_epoch(&actions, epoch)
        .into_iter()
        .all(|action| !matches!(action, Action::AnswerQuestions(_, _))));
    assert!(cx.debug_bounds("questions-validation-0").is_some());
    let next = cx.debug_bounds("questions-next-0").unwrap();
    cx.simulate_click(next.center(), gpui::Modifiers::default());
    cx.update(|window, cx| {
        view.update(cx, |view, _| {
            assert!(view.question_drafts[&key].row_focus[0].is_focused(window));
        });
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let input = view.question_drafts[&key].questions[0].input.clone();
            input.update(cx, |input, cx| input.set_value("12", window, cx));
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("questions-validation-0").is_none());
    let submit = cx.debug_bounds("questions-submit-0").unwrap();
    cx.simulate_click(submit.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions_at_epoch(&actions, epoch)
        .into_iter()
        .all(|action| !matches!(action, Action::AnswerQuestions(_, _))));
    let submit = cx.debug_bounds("questions-submit-0").unwrap();
    cx.simulate_click(submit.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions_at_epoch(&actions, epoch)
            .into_iter()
            .any(|action| matches!(action, Action::AnswerQuestions(_, Some(answers)) if answers["count"] == vec!["12"] && answers["title"] == vec!["Quarterly briefing"])));
    // A provider error permits cancelling the same live request without losing the answers.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.error = Some("Retry input".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    let cancel = cx.debug_bounds("questions-cancel-0").unwrap();
    cx.simulate_click(cancel.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions_at_epoch(&actions, epoch)
        .into_iter()
        .any(|action| matches!(action, Action::AnswerQuestions(_, None))));
}

#[gpui::test]
fn codex_and_claude_question_receipts_contain_wrapped_answers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let previous_rem = cx.update(|_, cx| gpui_component::Theme::global(cx).font_size);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (width, rem) in [(1024.0, 16.0), (520.0, 16.0), (520.0, 24.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1000.0)));
            cx.update(|window, cx| {
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                window.set_rem_size(gpui::px(rem));
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, _| {
                        session.state.saved.provider = provider;
                    });
                    view.question_receipts = vec![wrapped_question_receipt(cx)];
                    cx.notify();
                });
            });
            draw(cx);
            cx.update(|window, _| assert_eq!(window.rem_size(), gpui::px(rem)));
            let collapsed = cx.debug_bounds("question-receipt-0").unwrap();
            assert!(cx.debug_bounds("question-receipt-summary-0").is_some());
            assert!(cx.debug_bounds("question-receipt-0-question-0").is_none());
            let toggle = cx.debug_bounds("question-receipt-toggle-0").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            draw(cx);
            let receipt = cx.debug_bounds("question-receipt-0").unwrap();
            assert!(receipt.size.height > collapsed.size.height);
            let mut previous_bottom = receipt.top();
            for (question, answer) in [
                (
                    "question-receipt-0-question-0",
                    "question-receipt-0-answer-0",
                ),
                (
                    "question-receipt-0-question-1",
                    "question-receipt-0-answer-1",
                ),
                (
                    "question-receipt-0-question-2",
                    "question-receipt-0-answer-2",
                ),
                (
                    "question-receipt-0-question-3",
                    "question-receipt-0-answer-3",
                ),
                (
                    "question-receipt-0-question-4",
                    "question-receipt-0-answer-4",
                ),
            ] {
                let question = cx.debug_bounds(question).unwrap();
                let answer = cx.debug_bounds(answer).unwrap();
                assert!(
                    question.top() >= previous_bottom,
                    "question must follow preceding content"
                );
                assert!(
                    answer.top() >= question.bottom(),
                    "answer must follow its wrapped question"
                );
                previous_bottom = answer.bottom();
            }
            let bottom_padding = gpui::px(SPACE_2XL * rem / 16.0);
            assert!(
                    previous_bottom + bottom_padding <= receipt.bottom(),
                    "{provider:?} width={width} rem={rem}: final answer bottom {previous_bottom:?} must stay within receipt {receipt:?} with bottom padding"
                );
            cx.update(|window, cx| {
                let focus = view.read(cx).question_receipts[0].details_focus.clone();
                focus.focus(window, cx);
            });
            cx.simulate_keystrokes("space");
            draw(cx);
            assert!(cx.debug_bounds("question-receipt-0-question-0").is_none());
            assert!(cx.debug_bounds("question-receipt-summary-0").is_some());
        }
    }
    cx.update(|_, cx| gpui_component::Theme::global_mut(cx).font_size = previous_rem);
}

#[gpui::test]
fn long_question_actions_and_receipts_stay_above_the_composer(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, actions, _) = fixture(cx);
        let answer = "A long answer that needs to wrap across several lines.\n".repeat(28);
        let mut request = codex::QuestionRequest::from_codex(
                json!("long-question"),
                &json!({"questions": [
                    {"id":"scope", "question":"What should this change cover?", "isOther": true,
                     "options":[{"label":"Suggested scope", "description":"Include one optional check.\n".repeat(36)}]}
                ]}),
            ).unwrap();
        for i in 1..8 {
            let mut question = request.questions[0].clone();
            question.id = format!("scope-{i}");
            request.questions.push(question);
        }
        cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(800.)));
        draw(cx);
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Waiting;
                    session.state.questions.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        let key = view.read_with(cx, |view, cx| {
            question_key(&view.session.read(cx).state, &request)
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                for i in 0..request.questions.len() {
                    view.select_question_choice(&key, i, None, false, window, cx);
                    view.question_drafts[&key].questions[i]
                        .input
                        .update(cx, |input, cx| {
                            input.set_value(answer.trim(), window, cx);
                        });
                }
                cx.notify();
            });
        });
        draw(cx);
        let assert_above_composer = |selector, cx: &mut gpui::VisualTestContext| {
            let content = cx.debug_bounds(selector).unwrap();
            let transcript = cx.debug_bounds("codex-messages").unwrap();
            let composer = cx.debug_bounds("codex-composer-input").unwrap();
            assert!(
                    content.bottom() <= transcript.bottom() && transcript.bottom() <= composer.top(),
                    "{provider:?}: {selector} {content:?} must end inside {transcript:?} above {composer:?}"
                );
            content
        };
        let position = cx.debug_bounds("codex-messages").unwrap().center();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-10000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        cx.simulate_resize(gpui::size(gpui::px(520.), gpui::px(480.)));
        draw(cx);
        let review = assert_above_composer("questions-submit-0", cx);
        cx.simulate_click(review.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.question_drafts[&key].reviewing));
        let send = assert_above_composer("questions-submit-0", cx);
        cx.simulate_click(send.center(), gpui::Modifiers::default());
        draw(cx);
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        assert!(actions_at_epoch(&actions, epoch).iter().any(|action| {
                matches!(action, Action::AnswerQuestions(_, Some(answers)) if answers["scope"] == [answer.trim()])
            }));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    let thread_id = session.state.saved.thread_id.clone();
                    let turn_id = session.state.turn_id.clone();
                    session
                        .state
                        .question_answer_acks
                        .push(codex::QuestionAnswerAck {
                            sequence: 1,
                            id: request.id.clone(),
                            thread_id,
                            turn_id,
                        });
                    session.state.questions.clear();
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert_above_composer("question-receipt-toggle-0", cx);
        assert!(cx.debug_bounds("question-receipt-0-answer-7").is_none());
    }
}

#[gpui::test]
fn question_receipts_require_ack_and_redact_secrets(cx: &mut gpui::TestAppContext) {
    let (view, cx, _actions, _updates) = fixture(cx);
    let request = codex::QuestionRequest::from_codex(
        json!("secret-ack"),
        &json!({"questions":[
            {"id":"secret","question":"Enter access token","isSecret":true}
        ]}),
    )
    .unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Waiting;
                session.state.questions.push(request.clone());
                cx.notify();
            });
        });
    });
    draw(cx);
    let key = view.read_with(cx, |view, cx| {
        question_key(&view.session.read(cx).state, &request)
    });
    let anchor = view.read_with(cx, |view, _| view.question_drafts[&key].anchor);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let input = view.question_drafts[&key].questions[0].input.clone();
            input.update(cx, |input, cx| input.set_value("private-token", window, cx));
            view.toggle_question(&key, 0, window, cx);
            view.toggle_question(&key, 0, window, cx);
            assert_eq!(
                view.question_drafts[&key].questions[0]
                    .input
                    .read(cx)
                    .value()
                    .as_ref(),
                "private-token"
            );
            view.answer_questions(&key, request.id.clone(), false, cx);
            let pending = view.question_drafts[&key].pending_receipt.as_ref().unwrap();
            assert_eq!(pending.answers[0].values, ["Provided securely"]);
            assert!(view.question_receipts.is_empty());
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("question-receipt-0").is_none());
    // Transport failure keeps drafts, enables retry, and never creates a receipt.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.error = Some("Transport failed".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(!view.question_drafts[&key].submitting);
        assert!(view.question_drafts[&key].pending_receipt.is_none());
        assert!(view.question_receipts.is_empty());
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.answer_questions(&key, request.id.clone(), false, cx);
        });
    });
    // A successful acknowledgement and turn transition may arrive in the same snapshot.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let thread_id = session.state.saved.thread_id.clone();
                let turn_id = session.state.turn_id.clone();
                session
                    .state
                    .question_answer_acks
                    .push(codex::QuestionAnswerAck {
                        sequence: 1,
                        id: request.id.clone(),
                        thread_id,
                        turn_id,
                    });
                session.state.questions.clear();
                session.state.turn_id = Some("next-turn".into());
                session
                    .state
                    .saved
                    .items
                    .push(json!({"type":"agentMessage","id":"after-answer","text":"Continuing"}));
                cx.notify();
            });
        });
    });
    draw(cx);
    let receipt = cx.debug_bounds("question-receipt-0").unwrap();
    let continuation = cx.debug_bounds("codex-message-row-0").unwrap();
    assert!(
        receipt.bottom() <= continuation.top(),
        "receipt stays before later agent activity"
    );
    view.read_with(cx, |view, _| {
        assert!(view.question_drafts.is_empty());
        assert_eq!(view.question_receipts.len(), 1);
        assert_eq!(view.question_receipts[0].anchor, anchor);
        assert_eq!(
            view.question_receipts[0].answers[0].values,
            ["Provided securely"]
        );
    });
}

#[gpui::test]
fn question_review_keeps_all_types_and_request_local_edits_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state = question_review_snapshot(provider).into();
                    let mut second = session.state.questions[0].clone();
                    second.id = json!("independent");
                    session.state.questions.push(second);
                    cx.notify();
                });
                cx.notify();
            });
        });
        draw(cx);
        for selector in [
            "question-row-0-0",
            "question-row-0-1",
            "question-row-0-2",
            "question-row-0-3",
            "question-row-0-4",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{provider:?}: all question types render without a picker"
            );
        }
        let keys = view.read_with(cx, |view, cx| {
            view.session
                .read(cx)
                .state
                .questions
                .iter()
                .map(|r| question_key(&view.session.read(cx).state, r))
                .collect::<Vec<_>>()
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.select_question_choice(&keys[0], 0, None, false, window, cx);
                assert!(view.question_drafts[&keys[0]].questions[0].custom);
                view.select_question_choice(&keys[0], 0, None, false, window, cx);
                assert!(
                    !view.question_drafts[&keys[0]].questions[0].custom,
                    "suggested answers must disable custom mode"
                );
                for key in &keys {
                    view.question_drafts.get_mut(key).unwrap().reviewing = true;
                }
                let input = view.question_drafts[&keys[0]].questions[2].input.clone();
                input.update(cx, |_, cx| cx.emit(InputEvent::Focus));
            });
        });
        draw(cx);
        assert!(
            view.read_with(cx, |view, _| keys
                .iter()
                .all(|key| view.question_drafts[key].reviewing)),
            "focus alone must not invalidate review"
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.question_drafts[&keys[0]].questions[2].input.clone();
                input.update(cx, |input, cx| {
                    input.set_value("A measurable outcome", window, cx);
                    cx.emit(InputEvent::Change);
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(!view.question_drafts[&keys[0]].reviewing);
            assert!(
                view.question_drafts[&keys[1]].reviewing,
                "another request retains its review"
            );
        });
        assert!(actions.try_iter().all(|a| !matches!(a,Action::Interaction{action,..} if matches!(*action,Action::AnswerQuestions(_, _)))));
    }
}

#[gpui::test]
fn grouped_questions_preserve_drafts_and_submit_all_answers(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    let request = codex::QuestionRequest::from_codex(json!("ask"), &json!({"questions":[
            {"id":"choice","header":"Layout","question":"Which layout?","isOther":true,"options":[{"label":"Compact"},{"label":"Roomy"}]},
            {"id":"features","header":"Features","question":"Which features?","options":[{"label":"Search"},{"label":"History"}]},
            {"id":"token","header":"Token","question":"Enter token","isSecret":true}
        ]})).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Waiting;
                session.state.turn_id = Some("turn-question".into());
                let mut request = request.clone();
                request.questions[1].multiple = true;
                session.state.questions.push(request);
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(480.0)));
    draw(cx);
    let composer = cx.debug_bounds("codex-composer").unwrap();
    let messages = cx.debug_bounds("codex-messages").unwrap();
    let questions = cx.debug_bounds("agent-questions").unwrap();
    assert!(
        composer.bottom() <= gpui::px(480.0),
        "composer footer must remain inside a short viewport: {composer:?}"
    );
    assert!(
        messages.size.height > gpui::px(SPACE_20),
        "questions must leave visible transcript space"
    );
    assert!(
        cx.debug_bounds("agent-controls").is_none(),
        "questions belong in the transcript"
    );
    assert!(questions.size.width <= messages.size.width);
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    for selector in ["questions-cancel-0", "questions-submit-0"] {
        assert_eq!(
            cx.debug_bounds(selector).unwrap().size.height,
            gpui::px(CONTROL_H),
            "question footer uses compact controls"
        );
    }
    let key = view.read_with(cx, |view, cx| {
        question_key(&view.session.read(cx).state, &request)
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.question_drafts[&key].questions[0].choice_focus[0].focus(window, cx);
        });
    });
    cx.simulate_keystrokes("space right");
    draw(cx);
    assert!(view.read_with(cx, |view, _| {
        view.question_drafts[&key].questions[0]
            .selected
            .contains(&1)
    }));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.toggle_question(&key, 1, window, cx);
            view.question_drafts[&key].questions[1].choice_focus[0].focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("space");
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.question_drafts[&key].questions[1].choice_focus[1].focus(window, cx);
        });
    });
    cx.simulate_keystrokes("space");
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let input = view.question_drafts[&key].questions[2].input.clone();
            input.update(cx, |input, cx| input.set_value(" secret ", window, cx));
            // A streamed item triggers another render while the request stays live.
            view.session.update(cx, |session, cx| {
                session
                    .state
                    .saved
                    .items
                    .push(json!({"type":"reasoning","id":"stream","content":["working"]}));
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.answer_questions(&key, json!("ask"), false, cx);
            view.answer_questions(&key, json!("ask"), false, cx);
        });
    });
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let sent: Vec<_> = actions_at_epoch(&actions, epoch)
        .into_iter()
        .filter_map(|action| {
            if let Action::AnswerQuestions(id, Some(answers)) = action {
                Some((id, answers))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        sent.len(),
        1,
        "submit is guarded while awaiting provider acknowledgement"
    );
    assert_eq!(sent[0].0, json!("ask"));
    assert_eq!(sent[0].1["choice"], vec!["Roomy"]);
    assert_eq!(sent[0].1["features"], vec!["Search", "History"]);
    assert_eq!(sent[0].1["token"], vec![" secret "]);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.questions.clear();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.question_drafts.is_empty()));
    assert!(
        view.read_with(cx, |view, _| view.question_receipts.is_empty()),
        "disappearance without acknowledgement is not successful submission"
    );
    assert!(cx.debug_bounds("agent-questions").is_none());
}

#[gpui::test]
fn pending_approval_buttons_are_visible_and_clicks_emit_one_typed_response(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    for width in [1024.0, 600.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(480.0)));
        for (request_id, selector, expected_choice) in [
            ("approve", "approval-0-0", "allow_once"),
            ("decline", "approval-0-2", "deny"),
        ] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.status = Status::Waiting;
                        session.state.approvals = vec![json!({
                            "id": request_id,
                            "method": "item/commandExecution/requestApproval",
                            "params": {"command": "pwd", "cwd": "/tmp"}
                        })];
                        cx.notify();
                    });
                });
            });
            draw(cx);
            // Provider approval trays intentionally scroll within a bounded
            // area; reveal each action before checking visibility and routing.
            let controls = cx.debug_bounds("agent-controls").unwrap();
            let target = cx.debug_bounds(selector).unwrap();
            if target.top() < controls.top() || target.bottom() > controls.bottom() {
                let delta = if target.top() < controls.top() {
                    100000.0
                } else {
                    -100000.0
                };
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: controls.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                draw(cx);
            }
            let epoch =
                view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
            let controls = cx.debug_bounds("agent-controls").unwrap();
            let button = cx
                .debug_bounds(selector)
                .expect("pending approval exposes an actionable button");
            assert!(button.size.width > gpui::px(0.0));
            assert!(button.size.height > gpui::px(0.0));
            let control_height = cx.update(|window, _| rpx(CONTROL_H).to_pixels(window.rem_size()));
            assert!(
                button.size.height <= control_height,
                "approval uses compact transcript controls: {button:?}"
            );
            assert!(
                controls.contains(&button.origin),
                "approval button {selector}: {button:?}; controls: {controls:?}"
            );
            let composer = cx.debug_bounds("codex-composer").unwrap();
            assert!(controls.bottom() <= composer.top());
            assert!(controls.left() >= composer.left());
            assert!(controls.right() <= composer.right());
            assert!(cx.debug_bounds("codex-live-activity").is_none());
            assert!(button.right() <= controls.right());
            assert!(
                button.bottom() <= controls.bottom(),
                "{width}px {request_id}: button={button:?}, controls={controls:?}"
            );
            assert!(button.bottom() <= gpui::px(480.0));
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            draw(cx);
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            draw(cx);
            let responses: Vec<_> = actions_at_epoch(&actions, epoch)
                .into_iter()
                .filter_map(|action| match action {
                    Action::RespondApproval {
                        id,
                        choice,
                        input,
                        feedback,
                    } => Some((id, choice, input, feedback)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                responses,
                vec![(json!(request_id), expected_choice.into(), None, None)]
            );
        }
    }
}

#[gpui::test]
fn approval_edits_are_explicit_and_url_completion_is_separate(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    let approval = json!({"id":"claude-approval","method":"claude/tool","claudeInput":{"command":"pwd"},"claudeRequest":{"tool_name":"Bash","input":{"command":"pwd"}}});
    let request = ElicitationRequest::parse_codex(json!("browser"), &json!({"mode":"url","serverName":"Account","message":"Sign in","url":"https://example.com/sign-in","elicitationId":"sign-in"})).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.approvals.push(approval.clone());
                session.state.elicitations.push(request.clone());
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(
        actions.try_iter().next().is_none(),
        "rendering must not accept an approval or complete a browser request"
    );
    let key = view.read_with(cx, |view, cx| {
        CodexView::approval_key(&approval, view.session.read(cx).state.interaction_epoch)
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.toggle_approval_edit(&key, cx);
            let input = view.approval_drafts[&key].input.clone();
            input.update(cx, |input, cx| input.set_value("invalid", window, cx));
            view.respond_approval(&key, json!("claude-approval"), "allow_once".into(), cx);
            assert!(!view.approval_drafts[&key].submitting);
            input.update(cx, |input, cx| {
                input.set_value("{\"command\":\"ls\"}", window, cx);
            });
            view.respond_approval(&key, json!("claude-approval"), "allow_once".into(), cx);
            view.respond_elicitation(
                &CodexView::elicitation_key(
                    &request,
                    view.session.read(cx).state.interaction_epoch,
                ),
                json!("browser"),
                "decline",
                cx,
            );
        });
    });
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let sent = actions_at_epoch(&actions, epoch);
    assert!(sent.iter().any(|action| matches!(action, Action::RespondApproval { input: Some(input), .. } if input == &json!({"command":"ls"}))));
    assert!(sent.iter().any(|action| matches!(action, Action::RespondElicitation { action, content: None, .. } if action == "decline")));
}

#[gpui::test]
fn child_approval_draft_survives_parent_turn_and_old_control_expires_on_reconnect(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let approval = json!({"id":"reused","method":"item/commandExecution/requestApproval","claudeInput":{"command":"pwd"},"groveAgent":"Reviewer"});
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.turn_id = Some("parent-turn".into());
                session.state.status = Status::Waiting;
                session.state.approvals.push(approval.clone());
                session.state.interaction_owners.insert(
                    "\"reused\"".into(),
                    grove_core::interaction::InteractionOwner {
                        thread: "child".into(),
                        turn: Some("child-turn".into()),
                        agent: Some("Reviewer · child".into()),
                    },
                );
                cx.notify();
            });
        });
    });
    draw(cx);
    let old_epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let old_key = CodexView::approval_key(&approval, old_epoch);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let input = view.approval_drafts[&old_key].input.clone();
            view.toggle_approval_edit(&old_key, cx);
            input.update(cx, |input, cx| {
                input.set_value("{\"command\":\"cargo test\"}", window, cx);
            });
            view.session.update(cx, |session, cx| {
                session.state.turn_id = None;
                session.state.status = Status::Waiting;
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.approval_drafts[&old_key].input.read(cx).value(),
            "{\"command\":\"cargo test\"}"
        );
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.interaction_epoch += 1;
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.respond_approval(&old_key, json!("reused"), "allow_once".into(), cx);
        });
    });
    assert!(
        actions.try_iter().next().is_none(),
        "a stale approval control must not act on a reused request id"
    );
}
