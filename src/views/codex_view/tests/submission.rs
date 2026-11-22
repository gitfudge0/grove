use super::*;

#[test]
fn composer_code_matches_transcript_surfaces() {
    let inline = super::super::composer_markdown_decorations("before `é🦀` after");
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0].style.background_color, Some(c::MENU_HOVER()));
    assert_eq!(inline[0].style.font_weight, Some(gpui::FontWeight::MEDIUM));
    assert_eq!(inline[0].border, None);
    assert_eq!(
        inline[0].font_family.as_deref(),
        Some(crate::fonts::MONO_FAMILY)
    );
    for source in ["```rust\nlet x = 1;\n```", "```\nstill typing"] {
        let decorations = super::super::composer_markdown_decorations(source);
        let code = decorations
            .iter()
            .find(|d| d.font_family.is_some())
            .unwrap();
        assert_eq!(code.style.background_color, Some(c::CODE_FILL()));
        assert_eq!(code.style.color, Some(c::FG()));
        assert_eq!(code.border, None);
        assert_eq!(code.font_family.as_deref(), Some(crate::fonts::MONO_FAMILY));
        assert!(source.get(code.range.clone()).is_some());
    }
    assert!(super::super::composer_markdown_decorations("ordinary text").is_empty());
}

#[gpui::test]
fn unchanged_composer_decorations_do_not_notify_input(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let notifications = Rc::new(Cell::new(0));
    let count = notifications.clone();
    let span = TextDecoration::new(
        2..6,
        gpui::HighlightStyle {
            font_weight: Some(gpui::FontWeight::BOLD),
            ..Default::default()
        },
    );
    let (input, decorations, _observer) = cx.update(|window, cx| {
        view.update(cx, |_, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("**bold**"));
            let decorations = input.update(cx, |input, cx| {
                input.create_decorations_collection(vec![span.clone()], cx)
            });
            let observer = cx.observe(&input, move |_, _, _| count.set(count.get() + 1));
            (input, decorations, observer)
        })
    });
    cx.run_until_parked();
    let baseline = notifications.get();
    cx.update(|_, cx| decorations.set(vec![span.clone()], cx));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        baseline,
        "identical spans must not schedule another render"
    );
    cx.update(|_, cx| decorations.set(vec![TextDecoration::new(2..5, span.style)], cx));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        baseline + 1,
        "changed spans must notify"
    );
    cx.update(|window, cx| {
        input.update(cx, |input, cx| input.set_value("**bold**", window, cx));
    });
    cx.run_until_parked();
    let after_edit = notifications.get();
    cx.update(|_, cx| decorations.set(vec![span.clone()], cx));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        after_edit + 1,
        "span restoration after replacement must notify"
    );
}

#[gpui::test]
fn history_transcript_keeps_structured_tools_and_sends_no_actions_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| view.update(cx, |view, cx| {
                view.history_readonly = true;
                view.expanded.clear();
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Done;
                    session.state.saved.draft = "Retained unsent draft".into();
                    session.state.saved.items = vec![
                        json!({"id":"history-user", "type":"userMessage", "content":[{"text":"Inspect the change"}]}),
                        json!({"id":"history-command", "type":"commandExecution", "command":"cargo test", "status":"completed", "aggregatedOutput":"Two tests passed"}),
                        json!({"id":"history-answer", "type":"agentMessage", "text":"The **structured** transcript is retained.", "status":"completed"}),
                    ].into();
                    cx.notify();
                });
                view.input.update(cx, |input, cx| input.set_value("Hidden edit", window, cx));
                view.request_edit(0, window, cx);
                view.submit_mode(SubmissionMode::Send, window, cx);
            }));
        draw(cx);
        assert!(cx.debug_bounds("codex-readonly-transcript").is_some());
        assert!(cx.debug_bounds("codex-composer").is_none());
        assert!(cx.debug_bounds("codex-edit-message-0").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
        let work = cx
            .debug_bounds("codex-work-header-1")
            .expect("retained completed work remains behind the original disclosure");
        cx.simulate_click(work.center(), gpui::Modifiers::default());
        draw(cx);
        let tool = cx
            .debug_bounds("codex-toggle-tool-1")
            .expect("expanding retained work reveals its original tool disclosure");
        cx.simulate_click(tool.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-tool-detail-1").is_some());
        assert!(
            actions.try_iter().next().is_none(),
            "{provider:?}: History must not send provider actions"
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.session.read(cx).state.saved.draft,
                "Retained unsent draft"
            );
            assert!(view.expanded.contains("tool-history-command"));
            assert!(view.native_plans.is_empty());
        });
    }
}

#[gpui::test]
fn sending_bubble_rises_and_settles_without_replay(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, updates) = fixture(cx);
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("hello", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    draw(cx);
    // The provider has not received or acknowledged the action yet.
    let entering = cx.debug_bounds("codex-message-row-0").unwrap();
    let animation_start = view.read_with(cx, |view, _| {
        view.animating_bubbles.values().next().unwrap().0
    });
    assert!(view.read_with(cx, |view, cx| {
        view.session.read(cx).state.saved.items.is_empty()
    }));
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { text, .. } if text == "hello")));
    let mut state = snapshot();
    state.accepted_submission = view.read_with(cx, |view, _| view.submitted_ack);
    state
        .saved
        .items
        .push(json!({"id":"fresh","type":"userMessage","content":[{"text":"hello"}]}));
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-0").is_some());
    assert!(cx.debug_bounds("codex-message-row-1").is_none());
    assert_eq!(
        view.read_with(cx, |view, _| view.animating_bubbles["fresh"].0),
        animation_start
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS));
    draw(cx);
    // A normal redraw consumes the finished clock without creating a new one.
    view.update(cx, |_, cx| cx.notify());
    draw(cx);
    let settled = cx.debug_bounds("codex-message-row-0").unwrap();
    assert!(entering.top() > settled.top());
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(settled, cx.debug_bounds("codex-message-row-0").unwrap());
    assert!(view.read_with(cx, |view, _| view.animating_bubbles.is_empty()));
}

#[gpui::test]
fn image_only_send_is_visible_and_reconciles_after_identical_history(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, updates) = fixture(cx);
    let image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jM1sAAAAASUVORK5CYII=";
    let mut state = snapshot();
    state.saved.items.push(
        json!({"id":"old-image", "type":"userMessage", "content":[{"type":"image", "url":image}]}),
    );
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.images = vec![image.into()];
            view.submit(window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-1").is_some());
    assert!(cx.debug_bounds("codex-message-images-1").is_some());
    let submission = actions
        .try_iter()
        .find_map(|action| match action {
            Action::Submit { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();
    state.accepted_submission = Some(submission);
    state.status = Status::Working;
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-images-1").is_some());
    state.saved.items.push(json!({"id":"other-image", "type":"userMessage", "content":[{"type":"image", "url":"different-image"}]}));
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-images-2").is_some());
    assert_eq!(view.read_with(cx, |view, _| view.bubble_entrances.len()), 1);
    state.saved.items.push(
        json!({"id":"new-image", "type":"userMessage", "content":[{"type":"image", "url":image}]}),
    );
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-images-2").is_some());
    assert!(cx.debug_bounds("codex-message-row-3").is_none());
    assert!(view.read_with(cx, |view, _| view.bubble_entrances.is_empty()));
}

#[test]
fn mention_triggers_require_a_token_boundary_and_commands_start_the_draft() {
    assert_eq!(trailing_trigger("look at @src/"), Some(('@', "src/")));
    assert_eq!(trailing_trigger("use $review"), Some(('$', "review")));
    assert_eq!(trailing_trigger("/mod"), Some(('/', "mod")));
    assert_eq!(trailing_trigger("email@example.com"), None);
    assert_eq!(trailing_trigger("inspect /tmp"), None);
    assert_eq!(trailing_trigger("@src/ "), None);
}

#[gpui::test]
fn provider_notices_match_task_panel_and_recover_without_reconnect(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (width, rem) in [(1000.0, 16.0), (560.0, 16.0), (560.0, 20.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.0)));
            cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Failed;
                        session.state.error = Some("Upstream request failed.\n\nUnexpected <response>.".into());
                        session.state.notice = None;
                        session.state.saved.items = vec![
                            json!({"id":"u1","type":"userMessage","content":[{"text":"Go"}]}),
                            json!({"id":"plan","type":"plan","plan":[{"step":"Inspect","status":"completed"},{"step":"Build","status":"inProgress"}]}),
                        ].into();
                        cx.notify();
                    }));
                });
            draw(cx);
            let notice = cx.debug_bounds("codex-error").unwrap();
            let tasks = cx.debug_bounds("codex-task-strip-panel").unwrap();
            assert_eq!(
                notice.left(),
                tasks.left(),
                "{provider:?} width={width} rem={rem}"
            );
            assert_eq!(
                notice.right(),
                tasks.right(),
                "{provider:?} width={width} rem={rem}"
            );
            assert!(notice.bottom() <= tasks.top());
            assert!(cx.debug_bounds("codex-reconnect").is_none());
            let first = cx.debug_bounds("session-notice-body-line-0").unwrap();
            let second = cx.debug_bounds("session-notice-body-line-1").unwrap();
            assert!(
                second.top() >= first.bottom(),
                "provider line breaks remain separate native blocks"
            );
            let third = cx.debug_bounds("session-notice-body-line-2").unwrap();
            assert!(
                third.top() >= second.bottom(),
                "blank provider lines retain their height"
            );
            assert!(third.right() <= notice.right());
            let copy = cx.debug_bounds("session-notice-copy").unwrap();
            cx.simulate_click(copy.center(), gpui::Modifiers::default());
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("Upstream request failed.\n\nUnexpected <response>.")
                );
            });
        }
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let body = "Connection interrupted".to_owned();
                session.state.status = Status::Working;
                session.state.error = Some(body.clone());
                session.state.notice = Some(codex::SessionNotice {
                    kind: codex::SessionNoticeKind::Retrying,
                    provider: "fixture",
                    body,
                    reset_at: None,
                    usage_url: None,
                });
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-error").is_none());
        assert!(cx.debug_bounds("codex-live-activity").is_some());
        assert!(cx.debug_bounds("codex-reconnect").is_none());
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.error = None;
                session.state.status = Status::Done;
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-error").is_none());
        assert!(cx.debug_bounds("codex-live-activity").is_none());
    }
}

#[gpui::test]
fn next_reply_carousel_arrows_clicks_and_send_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(view, provider, "", "Check keyboard navigation.", window, cx);
                view.next_reply.resolve(vec![
                    "Check keyboard navigation.".into(),
                    "Check narrow layouts.".into(),
                    "Show the shared fields.".into(),
                ]);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("left");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((3, 3))
        );
        cx.simulate_keystrokes("right");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((1, 3))
        );
        let next = cx.debug_bounds("next-reply-next").unwrap();
        cx.simulate_click(next.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((2, 3))
        );
        cx.simulate_input("My own reply");
        cx.simulate_keystrokes("left");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.next_reply.position(), Some((2, 3)));
            assert_eq!(
                view.input.read(cx).cursor(),
                "My own reply".len() - 1,
                "Nonempty arrows move the caret"
            );
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a backspace"
        } else {
            "ctrl-a backspace"
        });
        draw(cx);
        assert!(cx.debug_bounds("next-reply-hint").is_some());
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "Check narrow layouts."
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. } | Action::Prompt(_))));
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            None
        );
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                view.submitted = None;
                view.submitted_ack = None;
            });
        });
    }
}

#[gpui::test]
fn next_reply_generates_while_draft_is_nonempty(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(
                    view,
                    provider,
                    "Already typing",
                    "Check layouts.",
                    window,
                    cx,
                );
                view.next_reply_seen = None;
                view.next_reply_request = None;
                view.next_reply_generate = |_, _| Ok(vec!["Check layouts.".into()]);
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(
                view.next_reply_request.is_some(),
                "Typing before completion must not lose the suggestions"
            );
            assert_eq!(view.input.read(cx).value().as_ref(), "Already typing");
        });
        assert!(cx.debug_bounds("next-reply-hint").is_none());
    }
}

#[gpui::test]
fn next_reply_enter_does_not_submit_empty_ghost_and_selection_wins(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            seed_next_reply(
                view,
                codex::Provider::Codex,
                "",
                "Add coverage.",
                window,
                cx,
            );
        });
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        cx.debug_bounds("next-reply-hint").is_some(),
        "Empty Enter must not retire suggestions"
    );
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. } | Action::Prompt(_))));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            seed_next_reply(
                view,
                codex::Provider::Claude,
                "Add",
                "Add coverage.",
                window,
                cx,
            );
            view.input
                .update(cx, |input, cx| input.set_selected_range(0..3, cx));
            assert!(!view.accept_next_reply(window, cx));
            assert_eq!(view.input.read(cx).value().as_ref(), "Add");
        });
    });
}

#[gpui::test]
fn sending_while_browsing_returns_to_latest(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![json!({"id":"history", "type":"agentMessage", "text":"Earlier content.\n\n".repeat(80)})].into();
                cx.notify();
            });
        }));
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.scroll.max_offset().y > gpui::px(100.));
            view.scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
            view.input
                .update(cx, |input, cx| input.set_value("hello", window, cx));
            view.submit(window, cx);
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { text, .. } if text == "hello")));
    for _ in 0..120 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        draw(cx);
    }
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
    });
}

#[gpui::test]
fn async_metadata_renders_native_choices_and_custom_answer_without_replacing_composer(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, updates) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("Unrelated draft", window, cx);
            });
            view.images = vec!["draft-image".into()];
        });
    });
    let mut state = snapshot();
    // Identical answers from a previous question are not this submission's echo.
    state
        .saved
        .items
        .push(json!({"type":"userMessage", "id":"old-answer",
            "content":[{"type":"text", "text":"Choose scope\nOnly focus and scrolling"}]}));
    assert!(state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-ui","turnId":"turn-ui","item":{
            "type":"agentMessage","id":"async-ui","text":"Choose scope: All or Composer.","delivery":"async",
            "questions":[{"title":"Choose scope","options":["All","Composer"]}]
        }}})));
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("agent-questions").is_some());
    let other = cx
        .debug_bounds("question-other-0-0")
        .expect("async choices include custom answer");
    cx.simulate_click(other.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("question-input-0-0").is_some());
    cx.simulate_input("Only focus and scrolling");
    draw(cx);
    let original_key = view.read_with(cx, |view, _| {
        view.question_drafts.keys().next().unwrap().clone()
    });
    cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = None;
                    // Matching user text while the form is open is not its answer echo.
                    session.state.saved.items.push(json!({"type":"userMessage", "id":"before-submit",
                        "content":[{"type":"text", "text":"Choose scope\nOnly focus and scrolling"}]}));
                    cx.notify();
                });
            });
        });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.question_drafts.contains_key(&original_key));
        assert_eq!(
            view.question_drafts[&original_key].questions[0]
                .input
                .read(cx)
                .value()
                .as_ref(),
            "Only focus and scrolling"
        );
    });
    let submit = cx.debug_bounds("questions-submit-0").unwrap();
    cx.simulate_click(submit.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(view.read_with(cx, |view, _| view
        .question_drafts
        .values()
        .all(|group| group.reviewing && !group.submitting)));
    let submit = cx.debug_bounds("questions-submit-0").unwrap();
    cx.simulate_click(submit.center(), gpui::Modifiers::default());
    draw(cx);
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let answer = actions_at_epoch(&actions, epoch)
        .into_iter()
        .find_map(|action| match action {
            Action::AnswerQuestions(id, Some(answers)) => Some((id, answers)),
            _ => None,
        })
        .expect("native form sends an answer");
    assert_eq!(answer.0, json!("async-ui"));
    assert_eq!(answer.1["0"], vec!["Only focus and scrolling"]);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let thread_id = session.state.saved.thread_id.clone();
                session
                    .state
                    .question_answer_acks
                    .push(codex::QuestionAnswerAck {
                        sequence: 1,
                        id: json!("async-ui"),
                        thread_id,
                        turn_id: None,
                    });
                session.state.questions.clear();
                session.state.turn_id = Some("async-answer-turn".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("question-receipt-0").is_some());
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Unrelated draft");
        assert_eq!(view.images, vec!["draft-image"]);
    });
    // A later user-message echo replaces the receipt, preserving one record.
    // An unrelated message must not discard the acknowledged answers.
    for (text, receipt_visible) in [
        ("Unrelated message", true),
        ("Choose scope\nOnly focus and scrolling", false),
    ] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    let index = session.state.saved.items.len();
                    session.state.saved.items.push(json!({
                        "type":"userMessage", "id":format!("echo-{index}"),
                        "content":[{"type":"text", "text":text}]
                    }));
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert_eq!(
            cx.debug_bounds("question-receipt-0").is_some(),
            receipt_visible
        );
    }
    view.read_with(cx, |view, cx| {
        assert!(view.question_receipts.is_empty());
        assert_eq!(view.input.read(cx).value().as_ref(), "Unrelated draft");
        assert_eq!(view.images, vec!["draft-image"]);
    });
}

#[gpui::test]
fn matching_prefix_requires_disclosure_before_sending(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(700.0)));
    let approval = json!({"id":"prefix", "method":"item/commandExecution/requestApproval", "params":{
        "command":"cargo test --manifest-path vendor/gpui-component/ui/Cargo.toml --lib text::window_selection::tests --no-run --locked",
        "cwd":"/tmp/project", "proposedExecpolicyAmendment":["cargo","test"]
    }});
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Waiting;
                session.state.approvals = vec![approval.clone()];
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("approval-0-2").is_none());
    let scroll_controls_to_bottom = |cx: &mut gpui::VisualTestContext| {
        let controls = cx.debug_bounds("agent-controls").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: controls.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-100000.0))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
    };
    scroll_controls_to_bottom(cx);
    let disclosure = cx.debug_bounds("approval-policy-0").unwrap();
    cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
    draw(cx);
    scroll_controls_to_bottom(cx);
    let button = cx.debug_bounds("approval-0-2").unwrap();
    let controls = cx.debug_bounds("agent-controls").unwrap();
    assert!(button.top() >= controls.top() && button.bottom() <= controls.bottom());
    cx.simulate_click(button.center(), gpui::Modifiers::default());
    draw(cx);
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let responses: Vec<_> = actions_at_epoch(&actions, epoch)
        .into_iter()
        .filter_map(|action| match action {
            Action::RespondApproval { choice, .. } => Some(choice),
            _ => None,
        })
        .collect();
    assert_eq!(responses, vec!["execpolicy"]);
}

#[gpui::test]
fn standalone_mcp_draft_survives_turn_updates_and_expires_with_request(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    let request = ElicitationRequest::parse_codex(json!("standalone"), &json!({"message":"Name", "requestedSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}})).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.elicitations.push(request.clone());
                cx.notify();
            });
        });
    });
    draw(cx);
    let key = view.read_with(cx, |view, cx| {
        CodexView::elicitation_key(&request, view.session.read(cx).state.interaction_epoch)
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let input = view.elicitation_drafts[&key].fields[0].input.clone();
            input.update(cx, |input, cx| {
                input.set_value("Keep this draft", window, cx);
            });
            view.session.update(cx, |session, cx| {
                session.state.turn_id = Some("unrelated-turn".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.turn_id = None;
                cx.notify();
            });
        });
    });
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view
            .elicitation_content(&key, &request, cx)
            .unwrap()),
        json!({"name":"Keep this draft"})
    );
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.elicitations.clear();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.elicitation_drafts.is_empty()));
}

#[gpui::test]
fn composer_focus_and_secondary_enter_submit_multiline_prompt(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("line one\nline two", window, cx);
            });
            view.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    assert!(cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window)));
    cx.simulate_keystrokes("secondary-enter");
    draw(cx);
    assert!(actions.try_iter().any(
        |action| matches!(action,Action::Submit{text,..}if text.trim()=="line one\nline two")
    ));
}

#[gpui::test]
fn file_drop_composer_preserves_text_attaches_images_and_never_sends(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for fraction in [0.05, 0.95] {
            let (view, cx, actions, _updates) = fixture(cx);
            let dir = tempfile::tempdir().unwrap();
            let image_path = dir.path().join("screenshot.png");
            fs_err::write(&image_path, test_image(image::ImageFormat::Png)).unwrap();
            let text_path = dir.path().join("design notes.md");
            fs_err::write(&text_path, "Reference material").unwrap();
            let other_paths: Vec<_> = ["archive.zip", "document.pdf", "data.bin", "no-extension"]
                .into_iter()
                .map(|name| dir.path().join(name))
                .collect();
            for path in &other_paths {
                fs_err::write(path, b"reference").unwrap();
            }
            let folder = dir.path().join("folder");
            fs_err::create_dir(&folder).unwrap();
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session
                        .update(cx, |session, _| session.state.saved.provider = provider);
                    view.input.update(cx, |input, cx| {
                        input.set_value("Keep this draft", window, cx);
                    });
                });
            });
            draw(cx);
            let bounds = cx.debug_bounds("codex-composer-input").unwrap();
            let position = gpui::point(
                bounds.center().x,
                bounds.top() + bounds.size.height * fraction,
            );
            cx.simulate_event(gpui::FileDropEvent::Entered {
                position,
                paths: gpui::ExternalPaths(vec![text_path.clone(), image_path].into()),
            });
            draw(cx);
            cx.simulate_event(gpui::FileDropEvent::Pending { position });
            draw(cx);
            assert_eq!(
                cx.debug_bounds("input-file-drop-overlay")
                    .unwrap()
                    .size
                    .height,
                bounds.size.height
            );
            // Drag exit must not change the draft or create an attachment.
            cx.simulate_event(gpui::FileDropEvent::Exited);
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), "Keep this draft");
                assert!(view.images.is_empty());
            });
            cx.simulate_event(gpui::FileDropEvent::Entered {
                position,
                paths: gpui::ExternalPaths(
                    std::iter::once(text_path.clone())
                        .chain(std::iter::once(dir.path().join("screenshot.png")))
                        .chain(other_paths.iter().cloned())
                        .chain(std::iter::once(folder.clone()))
                        .collect(),
                ),
            });
            draw(cx);
            cx.simulate_event(gpui::FileDropEvent::Submit { position });
            draw(cx);
            view.read_with(cx, |view, cx| {
                let text = view.input.read(cx).value();
                assert!(text.contains("Keep this draft"), "{provider:?}: {text}");
                assert!(
                    text.contains(&format!("@{}", text_path.display())),
                    "{provider:?}: {text}"
                );
                for path in other_paths.iter().chain(std::iter::once(&folder)) {
                    assert!(
                        text.contains(&format!("@{}", path.display())),
                        "{provider:?}: missing file reference"
                    );
                }
                assert_eq!(view.images.len(), 1, "{provider:?}");
                assert!(view.images[0].starts_with("data:image/png;base64,"));
            });
            let emitted: Vec<_> = actions.try_iter().collect();
            assert!(emitted
                .iter()
                .any(|action| matches!(action, Action::DraftImages(images) if images.len() == 1)));
            assert!(emitted.iter().any(|action| matches!(action, Action::Draft(text) if text.contains(&format!("@{}", text_path.display())))));
            assert!(!emitted
                .iter()
                .any(|action| matches!(action, Action::Submit { .. })));
        }
    }
}

#[gpui::test]
fn composer_shift_enter_inserts_newline_and_enter_sends(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("first line", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("shift-enter");
    draw(cx);
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().contains('\n')));
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Prompt(_) | Action::PromptWithImages { .. } | Action::Submit { .. }
    )));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit{text,..} if text.contains("first line"))));
}

#[gpui::test]
fn repeated_message_copy_extends_feedback_and_resets_after_delay(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.copy_message("first".into(), "first text".into(), cx);
        });
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.copy_message("second".into(), "second text".into(), cx);
        });
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.copied_message.as_ref().map(|(key, _)| key.as_str()),
            Some("second")
        );
    });
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
        Some("second text".into())
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.copied_message.is_none()));
}
