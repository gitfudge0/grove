use super::*;

#[test]
fn codex_and_claude_review_brief_stays_in_panel_not_transcript_copy_or_minimap() {
    use crate::views::review_summary::ReviewSummary;
    let answer = "Fixed duplication.\n\n[Evidence](https://example.com)";
    let brief = "The summary now appears only in its review panel. Live checks pending.";
    let raw = format!("{answer}\n\n<grove-review-summary>{brief}</grove-review-summary>");
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.saved.items = vec![
            json!({"type":"userMessage", "content":[{"text":"Remove duplicate summary"}]}),
            json!({"type":"agentMessage", "phase":"final_answer", "text":raw}),
        ]
        .into();
        state.saved.work_periods = vec![codex::WorkPeriod {
            start_item: 0,
            end_item: Some(2),
            duration_ms: None,
            subagents: vec![],
        }];
        state.saved.session_review.summary = brief.into();
        state.saved.session_review.pending = Some(codex::ReviewCompletion { id: 1, period: 0 });
        assert_eq!(
            transcript_item_text(&state.saved.items[1]),
            answer,
            "{provider:?}"
        );
        assert_eq!(
            response_groups(&state.saved.items, None)[0].copy_text(),
            answer,
            "{provider:?}"
        );
        assert_eq!(
            message_minimap_entries(&state.saved.items)[0].response,
            answer,
            "{provider:?}"
        );
        assert_eq!(
            ReviewSummary::from_snapshot(&state).unwrap().text,
            brief,
            "{provider:?}"
        );
        assert_eq!(
            codex::item_text(&state.saved.items[1]),
            raw,
            "raw response is retained"
        );
        let user = json!({"type":"userMessage", "content":[{"text":raw}]});
        assert_eq!(
            transcript_item_text(&user),
            raw,
            "user-authored text is untouched"
        );
    }
}

#[test]
fn review_summary_uses_current_completion_for_codex_and_claude() {
    use crate::views::review_summary::ReviewSummary;
    let artifacts = tempfile::tempdir().unwrap();
    fs_err::write(artifacts.path().join("mock.html"), "mock").unwrap();
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.saved.cwd = artifacts.path().to_string_lossy().into_owned();
        state.saved.items = vec![
                    json!({"type":"agentMessage", "text":"Old result"}),
                    json!({"type":"agentMessage", "phase":"final_answer", "text":"Added review links.\n\n[View mock](mock.html)"}),
                    json!({"type":"agentMessage", "phase":"commentary", "text":"Unrelated commentary"}),
                ].into();
        state.saved.work_periods = vec![codex::WorkPeriod {
            start_item: 1,
            end_item: Some(3),
            duration_ms: None,
            subagents: vec![],
        }];
        state.saved.session_review.pending = Some(codex::ReviewCompletion { id: 42, period: 0 });
        let summary = ReviewSummary::from_snapshot(&state).unwrap();
        assert_eq!(summary.text, "Added review links.");
        assert_eq!(
            summary.links[0].url,
            gpui::http_client::Url::from_file_path(artifacts.path().join("mock.html"))
                .unwrap()
                .to_string()
        );
        state.saved.session_review.summary =
            "Links ignored the theme. Updated shared hover colors. Native checks pending.".into();
        state.saved.session_review.reviewed_through = 1;
        assert_eq!(
            ReviewSummary::from_snapshot(&state).unwrap().text,
            state.saved.session_review.summary
        );
        assert_eq!(ReviewSummary::from_snapshot(&state).unwrap().links.len(), 1);
        state.status = Status::Working;
        assert!(ReviewSummary::from_snapshot(&state).is_none());
        state.status = Status::Done;
        state.saved.session_review.pending = None;
        assert!(ReviewSummary::from_snapshot(&state).is_none());
    }
}

#[test]
fn review_overview_keeps_only_a_bounded_opening_paragraph() {
    assert_eq!(
        review_overview("**Fixed the review panel.**\n\nDetails and evidence."),
        "Fixed the review panel."
    );
    assert_eq!(
        review_overview(" First line\ncontinues here. "),
        "First line continues here."
    );
    let long = review_overview(&"界 ".repeat(300));
    assert!(long.chars().count() <= 240);
    assert!(long.ends_with('…'));
    assert_eq!(review_overview("  "), "");
}

#[gpui::test]
fn child_preview_wheel_browsing_survives_updates_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.preview_child = Some("child".into());
                    view.preview_follow_latest = true;
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Working;
                        let mut activity = codex::SubagentActivity::default();
                        activity.observed_at = Some(std::time::Instant::now());
                        activity.items = (0..32).map(|index| json!({
                            "id": format!("child-{index}"), "type": "agentMessage",
                            "text": format!("Step {index}\n\nDetailed child response.\n\nMore details.")
                        })).collect();
                        session.state.subagent_activity.insert("child".into(), activity);
                        cx.notify();
                    });
                });
            });
        draw(cx);
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
            assert!(view.scroll.max_offset().y > gpui::px(500.));
        });
        let wheel = |delta, cx: &mut gpui::VisualTestContext| {
            let bounds = cx.debug_bounds("codex-messages").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: bounds.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(delta))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            draw(cx);
            draw(cx);
        };
        wheel(300., cx);
        let browsing = view.read_with(cx, |view, _| {
            assert!(
                view.scroll.offset().y > -view.scroll.max_offset().y + gpui::px(200.),
                "{provider:?}: wheel must reveal older content"
            );
            assert!(!view.composer_collapsed);
            view.scroll.offset().y
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.subagent_activity.get_mut("child").unwrap().items.push(json!({
                        "id": "incoming", "type": "agentMessage", "text": "Incoming child update.\n\nMore streamed content."
                    }));
                    cx.notify();
                });
            }));
        draw(cx);
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.scroll.offset().y,
                browsing,
                "{provider:?}: updates must preserve browsing"
            );
        });
        wheel(-100000., cx);
        view.read_with(cx, |view, _| {
            assert!(
                view.preview_follow_latest,
                "{provider:?}: returning to bottom resumes follow"
            );
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.subagent_activity.get_mut("child").unwrap().items.push(json!({
                        "id": "latest", "type": "agentMessage", "text": "Latest response.\n\nFollowing again."
                    }));
                    cx.notify();
                });
            }));
        draw(cx);
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.scroll.offset().y,
                -view.scroll.max_offset().y,
                "{provider:?}: new items follow after returning to bottom"
            );
        });
    }
}

#[gpui::test]
fn child_preview_uses_transcript_tools_and_follows_incoming_items(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.preview_child = Some("child".into());
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                let mut activity = codex::SubagentActivity::default();
                activity.observed_at = Some(std::time::Instant::now());
                activity.items = (0..24).map(|index| json!({"id":format!("child-{index}"),"type":"agentMessage","text":format!("## Step {index}\n\nDetailed investigation with **markdown** and `code`.\n\nAnother paragraph to exercise transcript scrolling.")})).collect();
                activity.items.push(json!({"id":"child-command","type":"commandExecution","command":"cargo test retry","status":"inProgress","aggregatedOutput":"running tests"}));
                session.state.subagent_activity.insert("child".into(), activity);
                cx.notify();
            });
        }));
    draw(cx);
    draw(cx);
    assert!(cx.debug_bounds("codex-composer").is_none());
    assert!(cx.debug_bounds("codex-edit-message-0").is_none());
    let toggle = cx
        .debug_bounds("codex-toggle-tool-24")
        .expect("child uses the shared tool disclosure");
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(cx);
    view.read_with(cx, |view, cx| {
        let state = view.session.read(cx).state.subagent_snapshot("child");
        assert!(
            tool_is_running(&state, 24),
            "shared shimmer predicate sees the active child tool"
        );
        assert!(view
            .expanded
            .contains(&tool_expansion_key(&state.saved.items[24], 24)));
        assert!(view.scroll.offset().y < gpui::Pixels::ZERO);
    });
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let activity = session.state.subagent_activity.get_mut("child").unwrap();
                activity.items[24]["status"] = json!("completed");
                activity.items.push(json!({"id":"child-latest","type":"agentMessage","text":"Latest child response after the completed command."}));
                cx.notify();
            });
        }));
    draw(cx);
    draw(cx);
    let latest = cx
        .debug_bounds("codex-message-anchor-25")
        .expect("incoming child item rendered");
    let transcript = cx.debug_bounds("codex-messages").unwrap();
    assert!(latest.bottom() <= transcript.bottom() + gpui::px(1.));
    assert!(latest.bottom() > transcript.top());
    view.read_with(cx, |view, cx| {
        let state = view.session.read(cx).state.subagent_snapshot("child");
        assert!(!tool_is_running(&state, 24));
    });
}

#[gpui::test]
fn restored_child_preview_exposes_saved_work_without_live_shimmer(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.preview_child = Some("saved-child".into());
                    view.expanded.clear();
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        let mut activity = codex::SubagentActivity::default();
                        activity.items = vec![
                            json!({"id":"saved-commentary", "type":"agentMessage", "phase":"commentary", "text":"Inspecting the saved retry path."}),
                            json!({"id":"saved-tool", "type":"commandExecution", "command":"cargo test retry", "status":"inProgress", "aggregatedOutput":"running tests before disconnect"}),
                            json!({"id":"saved-answer", "type":"agentMessage", "phase":"final_answer", "text":"The saved result remains available."}),
                        ].into();
                        session.state.subagent_activity.insert("saved-child".into(), activity);
                        cx.notify();
                    });
                });
            });
        draw(cx);
        draw(cx);
        let work = cx
            .debug_bounds("codex-work-header-0")
            .expect("restored child work remains reachable through the shared disclosure");
        cx.simulate_click(work.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(
            cx.debug_bounds("codex-message-anchor-0").is_some(),
            "{provider:?}: saved commentary expands"
        );
        let tool = cx
            .debug_bounds("codex-toggle-tool-1")
            .expect("saved child tool appears after expanding its work");
        cx.simulate_click(tool.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, cx| {
            let state = view.session.read(cx).state.subagent_snapshot("saved-child");
            assert_eq!(state.status, Status::Idle);
            assert!(
                !tool_is_running(&state, 1),
                "{provider:?}: saved in-progress status does not imply live work"
            );
            assert!(view
                .expanded
                .contains(&tool_expansion_key(&state.saved.items[1], 1)));
        });
    }
}

#[gpui::test]
fn codex_and_claude_review_control_dispatches_only_current_receipt(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.saved.items = vec![
                            json!({"id":"result","type":"agentMessage","text":"Completed result.\n\n<grove-review-summary>Dedicated panel brief.</grove-review-summary>"}),
                        ]
                        .into();
                        session.state.saved.work_periods = vec![codex::WorkPeriod {
                            start_item: 0,
                            end_item: Some(1),
                            duration_ms: Some(1),
                            subagents: Vec::new(),
                        }];
                        session.state.saved.session_review = codex::SessionReview {
                            summary: "Dedicated panel brief.".into(),
                            sequence: 7,
                            last_completed_period: Some(0),
                            pending: Some(codex::ReviewCompletion { id: 7, period: 0 }),
                            ..Default::default()
                        };
                        session.state.status = Status::Done;
                        cx.notify();
                    });
                    cx.notify();
                });
            });
        draw(cx);
        for (width, height, rem) in [(1000., 900., 16.), (400., 600., 16.), (400., 600., 20.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
            for _ in 0..4 {
                cx.update(gpui::Window::simulate_next_frame);
                draw(cx);
            }
            let review = cx.debug_bounds("session-review").unwrap();
            let composer = cx.debug_bounds("codex-composer").unwrap();
            assert!(
                review.bottom() >= composer.top(),
                "{provider:?}: review must attach to composer"
            );
            assert!(
                cx.debug_bounds("session-review-summary").is_some(),
                "{provider:?}: completed response is shown"
            );
            assert!(
                composer.bottom() <= gpui::px(height),
                "{provider:?}: composer clipped: {composer:?}"
            );
            assert!(
                composer.top() >= gpui::px(0.),
                "{provider:?}: composer above pane: {composer:?}"
            );
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.session_review.summary.clear();
                    session.state.saved.items =
                        vec![json!({"id":"result","type":"agentMessage","text":""})].into();
                    cx.notify();
                });
                cx.notify();
            });
        });
        draw(cx);
        assert!(
            cx.debug_bounds("session-review-summary").is_none(),
            "{provider:?}: empty result stays collapsed"
        );
        assert!(
            cx.debug_bounds("session-review-disclosure").is_none(),
            "{provider:?}: empty result has no disclosure"
        );
        let action = cx
            .debug_bounds("session-mark-reviewed")
            .expect("review action must be visible");
        cx.simulate_click(action.center(), gpui::Modifiers::default());
        assert!(
            actions
                .try_iter()
                .any(|action| matches!(action, Action::MarkReviewed { work_period: 7 })),
            "{provider:?}"
        );
        cx.update(|window, cx| {
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
        });
    }
}

#[gpui::test]
fn review_command_submit_intercepts_dashboard_before_catalog_or_provider_startup(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            observed.borrow_mut().push(match event {
                CodexViewEvent::ReviewChanges(..) => "review",
                CodexViewEvent::DashboardSubmit { .. } => "provider startup",
                _ => "other",
            });
        })
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for dashboard in [false, true] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, _| {
                        session.state.saved.provider = provider;
                        session.catalog_loading = true;
                    });
                    view.dashboard_tray = dashboard.then(|| {
                        cx.new(|_| {
                            crate::views::sidebar::dashboard::DestinationTray::picker_test_tray(
                                "/missing-native-command-fixture".into(),
                            )
                        })
                    });
                });
            });
            for command in [
                "/review local",
                "$review pr",
                "/review https://github.com/gitfudge0/grove/pull/17",
            ] {
                events.borrow_mut().clear();
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.input
                            .update(cx, |input, cx| input.set_value(command, window, cx));
                        view.submit(window, cx);
                        assert_eq!(view.input.read(cx).value().as_ref(), command);
                        assert!(view.submitted.is_none());
                        assert!(view.session.read(cx).state.error.is_none());
                    });
                });
                assert_eq!(
                    *events.borrow(),
                    ["review"],
                    "{provider:?}, Dashboard={dashboard}, {command}"
                );
            }
            for command in [
                "/review local changes",
                "/review https://example.com/pull/17",
            ] {
                events.borrow_mut().clear();
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.input
                            .update(cx, |input, cx| input.set_value(command, window, cx));
                        view.submit(window, cx);
                        assert_eq!(
                            view.session.read(cx).state.error.as_deref(),
                            Some(grove_core::review_skill::USAGE)
                        );
                        assert_eq!(view.input.read(cx).value().as_ref(), command);
                    });
                });
                assert!(events.borrow().is_empty());
            }
        }
    }
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Submit { .. }
            | Action::SubmitWithMode { .. }
            | Action::Prompt(_)
            | Action::PromptWithImages { .. }
    )));
}

#[gpui::test]
fn review_skill_routes_both_providers_without_submitting_or_losing_drafts(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let CodexViewEvent::ReviewChanges(scope, _) = event {
                observed.borrow_mut().push(scope.clone());
            }
        })
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.skills = vec![json!({"name":"review","enabled":true})].into();
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("/review local", window, cx);
                });
                assert!(view.run_command(window, cx));
                assert_eq!(view.input.read(cx).value().as_ref(), "/review local");
                view.pending_image_pastes = 1;
                view.input
                    .update(cx, |input, cx| input.set_value("$review pr", window, cx));
                assert!(view.run_command(window, cx));
                assert_eq!(view.pending_image_pastes, 1);
                assert_eq!(view.input.read(cx).value().as_ref(), "$review pr");
                assert!(view
                    .session
                    .read(cx)
                    .state
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("draft is preserved")));
                view.pending_image_pastes = 0;
                assert!(view.run_command(window, cx));
                view.input.update(cx, |input, cx| {
                    input.set_value("/review invalid", window, cx);
                });
                assert!(view.run_command(window, cx));
                assert_eq!(
                    view.session.read(cx).state.error.as_deref(),
                    Some(grove_core::review_skill::USAGE)
                );
                assert_eq!(
                    slash_choices(&view.session.read(cx).state, "review")
                        .iter()
                        .filter(|(name, _)| name == "/review")
                        .count(),
                    1
                );
                view.input.update(cx, |input, cx| {
                    input.set_value("$review", window, cx);
                    input.set_selected_range(7..7, cx);
                });
                let choices = view
                    .suggestion_choices(&view.session.read(cx).state, cx)
                    .unwrap()
                    .0;
                assert_eq!(
                    choices.iter().filter(|(name, _)| name == "$review").count(),
                    1
                );
            });
        });
    }
    assert_eq!(
        *events.borrow(),
        vec![
            grove_core::review_skill::ScopePreference::Local,
            grove_core::review_skill::ScopePreference::PullRequest,
            grove_core::review_skill::ScopePreference::Local,
            grove_core::review_skill::ScopePreference::PullRequest
        ]
    );
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. })));
}

#[gpui::test]
fn plan_requests_render_human_summaries_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _, _) = fixture(cx);
        let state = plan_request_snapshot(provider);
        let raw = codex::item_text(state.saved.items[0].as_ref());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state = state.into();
                    cx.notify();
                });
                cx.notify();
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            let rendered = view
                .selection_views
                .values()
                .map(|owner| owner.read(cx).source().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(rendered.contains("Change plan decisions"), "{rendered}");
            assert!(rendered.contains("Who can invite?"));
            assert!(rendered.contains("Workspace owners and team leads"));
            assert!(!rendered.contains("```json"));
            assert!(!rendered.contains("Original decision definitions"));
            assert_eq!(
                codex::item_text(view.session.read(cx).state.saved.items[0].as_ref()),
                raw
            );
            assert!(raw.contains("Original decision definitions"));
        });
    }
}

#[gpui::test]
fn plan_answer_uses_one_composer_and_preserves_drafts_for_codex_and_claude(
    cx: &mut gpui::TestAppContext,
) {
    use crate::views::native_plan::{NativePlanEvent, PlanComposerTarget};
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state = decision_plan_snapshot(provider).into();
                    cx.notify();
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("Keep my conversation draft", window, cx);
                });
                view.images = vec!["retained-image".into()];
                view.sync_native_plans(window, cx);
                view.begin_plan_composer(
                    NativePlanEvent::Compose {
                        plan_id: "invites".into(),
                        revision_id: "r1".into(),
                        target: PlanComposerTarget::CustomAnswer {
                            decision_id: "access".into(),
                            prompt: "Who can invite?".into(),
                        },
                    },
                    window,
                    cx,
                );
                assert_eq!(view.input.read(cx).value().as_ref(), "");
                assert!(view.images.is_empty());
                view.input.update(cx, |input, cx| {
                    input.set_value("Workspace owners and team leads", window, cx);
                });
                view.persist_plan_answer_draft(cx);
                view.cancel_plan_answer(window, cx);
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Keep my conversation draft"
                );
                assert_eq!(view.images, vec!["retained-image"]);
                view.begin_plan_composer(
                    NativePlanEvent::Compose {
                        plan_id: "invites".into(),
                        revision_id: "r1".into(),
                        target: PlanComposerTarget::CustomAnswer {
                            decision_id: "access".into(),
                            prompt: "Who can invite?".into(),
                        },
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Workspace owners and team leads"
                );
                view.session.update(cx, |session, _| {
                    session.state.error = Some("An earlier unrelated request failed".into());
                });
                view.submit(window, cx);
                assert!(view.plan_answer.as_ref().unwrap().saving.is_some());
            });
        });
        draw(cx);
        cx.simulate_keystrokes("x");
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "Workspace owners and team leads",
            "a pending save must reject edits until acknowledged"
        );
        let pending: Vec<_> = actions.try_iter().collect();
        assert!(!pending.iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::Prompt(_) | Action::ApprovePlan { .. }
        )));
        assert!(!pending.iter().any(
            |action| matches!(action,Action::Draft(text) if text=="Workspace owners and team leads")
        ));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    for action in pending {
                        if let Action::UpdatePlanReview {
                            plan_id,
                            revision_id,
                            review,
                        } = action
                        {
                            session
                                .state
                                .saved
                                .rich_plans
                                .update_review(&plan_id, &revision_id, *review)
                                .unwrap();
                        }
                    }
                    cx.notify();
                });
                view.sync_plan_answer(window, cx);
                assert!(view.plan_answer.is_none());
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Keep my conversation draft"
                );
                assert_eq!(view.images, vec!["retained-image"]);
                assert_eq!(
                    view.session.read(cx).state.saved.rich_plans.revisions[0]
                        .review
                        .own_answers["access"],
                    "Workspace owners and team leads"
                );
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("plan-answer-context").is_none());
        assert!(cx.debug_bounds("session-plan-ready").is_none());
    }
}

#[gpui::test]
fn plan_discussion_references_exact_revision_without_overwriting_composer(
    cx: &mut gpui::TestAppContext,
) {
    use crate::views::native_plan::{NativePlanEvent, PlanComposerTarget};
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state = decision_plan_snapshot(provider).into();
                    cx.notify();
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("What about contractors?", window, cx);
                });
                view.sync_native_plans(window, cx);
                view.begin_plan_composer(
                    NativePlanEvent::Compose {
                        plan_id: "invites".into(),
                        revision_id: "r1".into(),
                        target: PlanComposerTarget::Discuss {
                            section: "Access".into(),
                        },
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "What about contractors?"
                );
                let quote = view.reference.as_ref().unwrap();
                assert_eq!(quote.sources[0].message_id, "plan-message");
                assert!(quote.text.contains("Revision: r1"));
                assert!(quote.text.contains("Section: Access"));
                assert!(view.plan_answer.is_none());
            });
        });
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native X11 conversation window"]
fn plan_composer_linux_capture() {
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "use isolated configuration"
    );
    let output = std::env::var("GROVE_PLAN_CAPTURE_DIR").expect("capture directory");
    let capture_receipt = std::env::var_os("GROVE_QUESTION_RECEIPT_CAPTURE").is_some();
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_global(crate::settings::SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_reduce_motion(true);
            grove_core::theme::set_by_name("grove-dark");
            c::sync_component_theme(cx);
            let (handle,actions)=Handle::channel();
            let (updates,receive)=mpsc::channel();
            let connection=Connection{handle,updates:receive.into()};
            let mut chat=None;
            let window=cx.open_window(gpui::WindowOptions {
                app_id: Some("dev.grove.plan-overview-qa".into()),
                kind: gpui::WindowKind::PopUp,
                window_bounds:Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(gpui::point(gpui::px(40.),gpui::px(40.)),gpui::size(gpui::px(1000.),gpui::px(900.))))),
                ..Default::default()
            }, |window,cx| {
                window.set_window_title("Grove plans QA");
                let provider = if std::env::var("GROVE_PLAN_CAPTURE_PROVIDER").as_deref() == Ok("claude") { codex::Provider::Claude } else { codex::Provider::Codex };
                let state = if capture_receipt {
                    let mut state = snapshot();
                    state.saved.provider = provider;
                    state
                } else if std::env::var_os("GROVE_QUESTION_CAPTURE").is_some() { question_review_snapshot(provider) } else if std::env::var_os("GROVE_PLAN_BEHAVIOR_CAPTURE").is_some() { behavior_plan_snapshot(provider) } else if std::env::var_os("GROVE_PLAN_OVERVIEW_CAPTURE").is_some() { overview_plan_snapshot(provider) } else if std::env::var_os("GROVE_PLAN_REQUEST_CAPTURE").is_some() { plan_request_snapshot(provider) } else { decision_plan_snapshot(provider) };
                let session=cx.new(|cx|CodexSession::from_connection(state,connection,cx));
                let view=cx.new(|cx|CodexView::new(session,window,cx));
                chat=Some(view.clone());
                let shell=cx.new(|_|PlanCaptureShell(view));
                cx.new(|cx|gpui_component::Root::new(shell,window,cx))
            }).unwrap();
            let chat=chat.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let _keep_transport=(actions,updates);
                for (name,custom,theme,width,rem) in [("ready",false,"grove-dark",1000.,16.),("custom",true,"grove-light",1000.,16.),("custom-narrow",true,"grove-dark",640.,16.),("custom-zoom",true,"grove-light",1000.,20.)] {
                    if let Ok(selected) = std::env::var("GROVE_PLAN_CAPTURE_CASE") { if selected != name { continue; } }
                    cx.update(|cx| {grove_core::theme::set_by_name(theme);c::sync_component_theme(cx);});
                    cx.update_window(window.into(),|_,window,cx| {
                        window.resize(gpui::size(gpui::px(width),gpui::px(900.)));
                        window.set_rem_size(gpui::px(rem));
                        if capture_receipt {
                            chat.update(cx, |view,cx| {
                                let mut receipt = wrapped_question_receipt(cx);
                                receipt.expanded = custom;
                                view.question_receipts = vec![receipt];
                                view.scroll.scroll_to_top_of_item(0);
                                cx.notify();
                            });
                        } else if custom && std::env::var_os("GROVE_PLAN_BEHAVIOR_CAPTURE").is_none() && std::env::var_os("GROVE_PLAN_REQUEST_CAPTURE").is_none() && std::env::var_os("GROVE_QUESTION_CAPTURE").is_none() {chat.update(cx,|view,cx| {
                            view.begin_plan_composer(crate::views::native_plan::NativePlanEvent::Compose {
                                plan_id:"invites".into(),revision_id:"r1".into(),target:crate::views::native_plan::PlanComposerTarget::CustomAnswer{decision_id:"access".into(),prompt:"Who can invite?".into()}
                            },window,cx);
                            view.input.update(cx,|input,cx|input.set_value("Workspace owners and team leads can invite.",window,cx));
                        });}
                        if std::env::var_os("GROVE_PLAN_BEHAVIOR_CAPTURE").is_some() { chat.update(cx,|view,cx| {
                            if let Some(plan) = view.native_plans.get(&("invites".into(),"r1".into())) {
                                plan.update(cx, |plan,cx| plan.configure_behavior_capture(std::env::var_os("GROVE_PLAN_BEHAVIOR_REVIEW").is_some(),window,cx));
                            }
                        }); }
                        if std::env::var_os("GROVE_PLAN_OVERVIEW_CAPTURE").is_some() { chat.update(cx,|view,cx| {
                            let revision = if std::env::var("GROVE_PLAN_OVERVIEW_REVISION").as_deref() == Ok("finalized") { "r2" } else { "r1" };
                            if let Some(plan) = view.native_plans.get(&("invites".into(),revision.into())) {
                                plan.update(cx, |plan,cx| plan.configure_overview_capture(std::env::var_os("GROVE_PLAN_OVERVIEW_OPEN_DETAIL").is_some(),window,cx));
                            }
                        }); }
                        if std::env::var_os("GROVE_QUESTION_CAPTURE").is_some() { chat.update(cx, |view,cx| { view.scroll.scroll_to_top_of_item(0); cx.notify(); }); }
                        window.refresh();
                    }).unwrap();
                    cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                    if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                        for _ in 0..3 {
                            assert!(std::process::Command::new(&helper).arg("Grove plans QA").status().unwrap().success());
                            cx.update_window(window.into(),|_,window,_|window.refresh()).unwrap();
                            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                        }
                    }
                    // Force the isolated QA surface to paint even when the desktop is idle.
                    cx.update_window(window.into(), |_, window, cx| { let _ = window.draw(cx); }).unwrap();
                    let path=format!("{output}/{name}.png");
                    assert!(std::process::Command::new("import").args(["-window","Grove plans QA",&path]).status().unwrap().success());
                    eprintln!("Native plan capture: {path}");
                }
                // Render an unfocused frame so the platform releases its text-input handler.
                cx.update_window(window.into(), |_, window, _| { window.blur(); window.refresh(); }).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
                drop(chat);
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                cx.update(|cx|cx.quit());
            }).detach();
        });
}

#[test]
fn metadata_only_notifications_reuse_work_index_and_review_parse() {
    let mut state = snapshot();
    state.saved.items =
        vec![json!({"id":"answer","type":"agentMessage","text":"An answer"})].into();
    let first = TranscriptRenderCache::new(Rc::new(state.clone()), vec![]);
    first
        .review_overview
        .set(Some("Cached review".into()))
        .unwrap();
    state.model = Some("changed metadata".into());
    let metadata = TranscriptRenderCache::refreshed(Rc::new(state.clone()), vec![], Some(&first));
    assert!(Rc::ptr_eq(&first.work, &metadata.work));
    assert!(Rc::ptr_eq(
        &first.review_overview,
        &metadata.review_overview
    ));
    state.saved.items[0]["text"] = json!("Changed answer");
    let changed = TranscriptRenderCache::refreshed(Rc::new(state), vec![], Some(&metadata));
    assert!(!Rc::ptr_eq(&metadata.work, &changed.work));
    assert!(!Rc::ptr_eq(
        &metadata.review_overview,
        &changed.review_overview
    ));
}

#[gpui::test]
fn claude_checklists_are_hidden_and_legacy_plan_keeps_tool_details(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.saved.items = vec![
                    // A tool result with no structured list keeps today's rendering.
                    serde_json::json!({
                        "id":"claude-plan", "type":"plan", "tool":"TaskUpdate",
                        "text":"- [x] Verify parity", "status":"completed",
                        "arguments":{"taskId":"7","status":"completed"},
                        "result":{"content":[{"type":"text","text":"Updated task #7 status"}]}
                    }),
                    serde_json::json!({
                        "id":"claude-todos", "type":"plan", "tool":"TodoWrite",
                        "text":"- [x] Verify parity", "status":"completed",
                        "plan":[{"id":"todo-0","step":"Verify parity","status":"completed"}]
                    }),
                ]
                .into();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-0").is_some());
    assert!(cx.debug_bounds("codex-plan-detail-0").is_none());
    let toggle = cx.debug_bounds("codex-toggle-details-0").unwrap();
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-plan-detail-0").is_some());
    assert!(cx.debug_bounds("codex-message-body-0").is_some());
    assert!(view.read_with(cx, |view, _| view.expanded.contains("claude-plan")));
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-plan-detail-0").is_none());
    // Structured checklist updates only appear in the docked task panel.
    assert!(cx.debug_bounds("codex-task-diff-1").is_none());
    assert!(cx.debug_bounds("codex-message-anchor-1").is_none());
    assert!(cx.debug_bounds("codex-task-strip").is_some());
    assert!(cx.debug_bounds("codex-message-body-1").is_none());
    assert!(cx.debug_bounds("codex-toggle-details-1").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Codex;
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-0").is_some());
    assert!(cx.debug_bounds("codex-toggle-details-0").is_none());
}

#[gpui::test]
fn checklist_snapshots_are_hidden_and_proposed_plans_stay(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
                    json!({"id":"grove-plan-t1", "type":"plan", "text":"- [ ] A", "plan":[
                            {"step":"A","status":"inProgress"},{"step":"B","status":"pending"}]}),
                    json!({"id":"proposed", "type":"plan", "text":"Proposed plan body"}),
                    // Identical snapshot: nothing changed, so nothing renders.
                    json!({"id":"grove-plan-t2", "type":"plan", "text":"- [ ] A", "plan":[
                            {"step":"A","status":"inProgress"},{"step":"B","status":"pending"}]}),
                ]
                .into();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-task-diff-1").is_none());
    assert!(cx.debug_bounds("codex-message-anchor-1").is_none());
    assert!(cx.debug_bounds("codex-message-body-1").is_none());
    assert!(cx.debug_bounds("codex-message-body-2").is_some());
    assert!(cx.debug_bounds("codex-task-diff-3").is_none());
    assert!(cx.debug_bounds("codex-message-anchor-3").is_none());
    let list = view.read_with(cx, |view, _| {
        view.task_cache.as_ref().unwrap().list.clone().unwrap()
    });
    assert_eq!(list.total(), 2);
    assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
    assert!(cx.debug_bounds("codex-task-strip").is_some());
}

#[test]
fn collapsed_preview_marks_hidden_lines_without_changing_draft() {
    assert_eq!(
        collapsed_draft_preview("First line\nSecond line"),
        "First line…"
    );
    assert_eq!(
        collapsed_draft_preview("Long single line"),
        "Long single line"
    );
    assert_eq!(collapsed_draft_preview("\nSecond line"), "…");
    assert_eq!(collapsed_draft_preview("First line\n"), "First line…");
}

#[gpui::test]
fn reduced_motion_composer_preview_restores_preserved_editor(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        cx.set_reduce_motion(true);
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("First line\nSecond line", window, cx);
                input.set_selected_range(4..4, cx);
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| view.set_composer_collapsed(true, cx)));
    draw(cx);
    assert!(cx.debug_bounds("codex-composer-preview").is_some());
    view.read_with(cx, |view, cx| {
        assert!(view.composer_transition_from.is_none());
        assert_eq!(
            view.input.read(cx).value().as_ref(),
            "First line\nSecond line"
        );
        assert_eq!(view.input.read(cx).cursor(), 4);
    });
    // The original Input remains mounted and owns typed keys under the preview.
    cx.simulate_input("x");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(!view.composer_collapsed);
        assert_eq!(
            view.input.read(cx).value().as_ref(),
            "Firsxt line\nSecond line"
        );
        assert_eq!(view.input.read(cx).cursor(), 5);
    });
    assert!(cx.debug_bounds("codex-composer-preview").is_none());
}

#[gpui::test]
fn native_markdown_links_open_artifacts_and_web_urls(cx: &mut gpui::TestAppContext) {
    let (_view, cx, _actions, updates) = fixture(cx);
    for (target, expected) in [
        ("https://example.com/a?q=b#c", "https://example.com/a?q=b#c"),
        ("mailto:hello@example.com", "mailto:hello@example.com"),
    ] {
        let mut response = snapshot();
        response.saved.items.push(serde_json::json!({
            "type":"agentMessage", "id":"answer",
            "text":format!("[three variations](<{target}>)")
        }));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let body = cx
            .debug_bounds("codex-message-body-0")
            .expect("rendered Markdown body");
        let position = body.origin + gpui::point(gpui::px(8.), gpui::px(8.));
        cx.simulate_click(position, gpui::Modifiers::default());
        assert_eq!(cx.opened_url().as_deref(), Some(expected), "{target}");
    }
}

#[gpui::test]
fn explicit_html_fences_stay_code_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.provider = provider;
        response.saved.html_policy = grove_core::visuals::HtmlPolicy::Explicit;
        response.saved.items.push(serde_json::json!({"type":"agentMessage","id":"html-example","text":"```html\n<!doctype html><html><body>Example</body></html>\n```"}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("html-artifact").is_none());
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
            0
        );
    }
}

#[gpui::test]
fn codex_inline_html_has_no_document_toolbar(cx: &mut gpui::TestAppContext) {
    inline_html_toolbar_fixture(cx, codex::Provider::Codex);
}

#[gpui::test]
fn claude_inline_html_has_no_document_toolbar(cx: &mut gpui::TestAppContext) {
    inline_html_toolbar_fixture(cx, codex::Provider::Claude);
}

#[gpui::test]
fn native_html_completed_output_uses_real_cwd_with_display_checkout_label(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture(cx);
    let checkout = tempfile::tempdir().unwrap();
    let cwd = checkout.path().to_string_lossy().into_owned();
    fs_err::write(
        checkout.path().join("launch.html"),
        "<!doctype html><html><body>Completed launch output</body></html>",
    )
    .unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.set_checkout_context("Main checkout".into(), "main".into(), cx);
            view.session.update(cx, |session, cx| {
                session.state.saved.cwd.clone_from(&cwd);
                cx.notify();
            });
        });
    });
    draw(cx);
    let mut completed = snapshot();
    completed.saved.cwd = cwd;
    completed.status = Status::Done;
    completed.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"completed-html", "text":"The completed output is [Launch checklist](launch.html)."}));
    updates.send(completed).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("transcript-file-link").is_some());
    assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).opened_count()),
            1,
            "a completed relative HTML output must auto-open using saved.cwd without a rendered busy snapshot"
        );
    assert!(
        cx.debug_bounds("html-artifact").is_some(),
        "completed output must actually appear inline"
    );
    let artifact_ids = view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids());
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids()),
        artifact_ids
    );
    assert!(cx.opened_url().is_none());
}

#[gpui::test]
fn native_html_multiple_fences_preserve_artifact_identity_during_streaming(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":"```html\n<html><body>First</body></html>\n```\n\n~~~html\n<html><body>Second</body></html>\n~~~\n\n```html\n<html>Unfinished"}));
    updates.send(response.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
        2
    );
    let ids = view.read_with(cx, |view, cx| {
        assert!(view
            .html
            .read(cx)
            .artifact_owners_match(view.html.entity_id(), cx));
        view.html.read(cx).artifact_ids()
    });
    response.saved.items[0]["text"] = serde_json::Value::String(format!(
        "{} appended stream",
        response.saved.items[0]["text"].as_str().unwrap()
    ));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
        2,
        "streaming outside complete fences must not mount replacement artifacts"
    );
    assert_eq!(
        view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids()),
        ids
    );
}

#[gpui::test]
fn selection_reference_preview_is_bounded_at_narrow_width_and_zoom(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, updates) = fixture_with_navigation(cx, true);
    let mut response = snapshot();
    response.saved.items.push(
        json!({"id":"source", "type":"agentMessage", "text":"long literal selection ".repeat(200)}),
    );
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.set_reference(
                Some(codex::ReferenceDraft {
                    id: "long".into(),
                    thread_id: Some("thread-ui".into()),
                    sources: vec![codex::ReferenceSource {
                        message_id: "source".into(),
                        role: "Assistant".into(),
                    }],
                    text: "long literal selection ".repeat(200),
                }),
                cx,
            );
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    for (width, rem) in [(1000., 16.), (400., 16.), (400., 20.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
        cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
        for _ in 0..4 {
            cx.update(gpui::Window::simulate_next_frame);
            draw(cx);
        }
        let text = cx.debug_bounds("codex-reference-text").unwrap();
        let preview = cx.debug_bounds("codex-reference-preview").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(
            composer.bottom() <= gpui::px(900.),
            "composer must fit within the pane: {composer:?}"
        );
        assert!(text.size.height <= gpui::px(60. * rem / 16. + 1.));
        assert!(preview.left() >= gpui::px(0.) && preview.right() <= gpui::px(width));
        assert!((preview.left() - composer.left()).abs() < gpui::px(1.));
        assert!(
            preview.bottom() >= composer.top(),
            "preview attaches to composer"
        );
        let disclosure = cx.debug_bounds("codex-reference-disclosure").unwrap();
        cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, _| assert!(view.reference_expanded));
        let text = cx.debug_bounds("codex-reference-text").unwrap();
        assert!(text.size.height <= gpui::px(200. * rem / 16. + 1.));
        let mut focused = false;
        for _ in 0..20 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            focused =
                cx.update(|window, cx| view.read(cx).reference_disclosure_focus.is_focused(window));
            if focused {
                break;
            }
        }
        assert!(focused, "Quote disclosure must be reachable by Tab");
        cx.simulate_keystrokes("space");
        draw(cx);
        view.read_with(cx, |view, _| assert!(!view.reference_expanded));
    }
}
