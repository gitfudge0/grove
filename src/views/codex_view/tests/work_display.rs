use super::*;

#[test]
fn tool_shimmer_tracks_active_calls_and_claude_thinking() {
    let mut state = snapshot();
    state.status = Status::Working;
    state.saved.provider = codex::Provider::Claude;
    state.saved.items = vec![json!({"type":"reasoning"})].into();
    assert!(tool_is_running(&state, 0));
    state
        .saved
        .items
        .push(json!({"type":"commandExecution","status":"inProgress"}));
    assert!(!tool_is_running(&state, 0));
    assert!(tool_is_running(&state, 1));
    state.saved.items[1]["status"] = json!("completed");
    assert!(!tool_is_running(&state, 1));
    state.saved.items[1]["status"] = json!("inProgress");
    for status in [
        Status::Waiting,
        Status::Idle,
        Status::Done,
        Status::Interrupted,
    ] {
        state.status = status;
        assert!(!tool_is_running(&state, 1));
    }
    state.status = Status::Working;
    state.saved.items.pop();
    state.saved.items[0]["status"] = json!("completed");
    assert!(!tool_is_running(&state, 0));
    state.saved.items[0]
        .as_object_mut()
        .unwrap()
        .remove("status");
    state.saved.provider = codex::Provider::Codex;
    assert!(!tool_is_running(&state, 0));
}

#[gpui::test]
fn codex_and_claude_status_arrival_collapses_tasks_once(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let mut state = view.session.read(cx).state.clone();
                state.saved.provider = provider;
                state.error = None;
                state.status = Status::Working;
                view.sync_status_cards(&state);
                view.tasks_expanded = true;
                state.error = Some("First failure".into());
                view.sync_status_cards(&state);
                assert!(
                    !view.tasks_expanded,
                    "{provider:?}: new card collapses Tasks"
                );
                view.tasks_expanded = true;
                view.sync_status_cards(&state);
                assert!(
                    view.tasks_expanded,
                    "{provider:?}: rerender preserves manual expansion"
                );
                state.error = Some("Second failure".into());
                view.sync_status_cards(&state);
                assert!(
                    !view.tasks_expanded,
                    "{provider:?}: replacement card collapses Tasks"
                );
                state.error = None;
                view.sync_status_cards(&state);
                assert!(
                    !view.tasks_expanded,
                    "{provider:?}: removal does not reopen Tasks"
                );
            });
        });
    }
}

#[test]
fn work_index_matches_legacy_helpers_for_both_providers() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for busy in [false, true] {
            for pattern in 0..24 {
                let mut state = snapshot();
                state.saved.provider = provider;
                state.status = if busy {
                    codex::Status::Working
                } else {
                    codex::Status::Idle
                };
                state.work_started = None;
                state.saved.items = (0..40)
                    .map(|i| {
                        json!({
                            "id": format!("item-{i}"),
                            "type": if (i + pattern) % 9 < 2 { "userMessage" }
                                else if i % 5 == 0 { "agentMessage" } else { "commandExecution" }
                        })
                    })
                    .collect();
                // Deliberately unordered and overlapping: preserve reverse-vector
                // priority instead of assuming normalized persisted work periods.
                if pattern % 2 == 0 {
                    state.saved.work_periods = vec![
                        codex::WorkPeriod {
                            start_item: 20,
                            end_item: None,
                            duration_ms: None,
                            subagents: vec![],
                        },
                        codex::WorkPeriod {
                            start_item: 0,
                            end_item: Some(15),
                            duration_ms: Some(47000),
                            subagents: vec![],
                        },
                        codex::WorkPeriod {
                            start_item: 8,
                            end_item: Some(27),
                            duration_ms: Some(126000),
                            subagents: vec![],
                        },
                    ];
                } else {
                    state.saved.work_periods.clear();
                }
                let cache = TranscriptRenderCache::new(Rc::new(state.clone()), vec![]);
                for history_start in [0, 7, 23, 40] {
                    for index in history_start..=state.saved.items.len() {
                        assert_eq!(
                            WorkIndex::key(&cache.prepared, index),
                            work_key_at(&state, index)
                        );
                        assert_eq!(cache.work.active[index], work_is_active_at(&state, index));
                        assert_eq!(
                            cache.work.parent_activity[index],
                            work_has_parent_activity(&state, index)
                        );
                        if index < state.saved.items.len() {
                            assert_eq!(cache.commentary[index], work_commentary(&state, index));
                        }
                        assert_eq!(
                            cache.work.tool_visible(&state, index),
                            tool_visible(&state, index)
                        );
                        assert_eq!(
                            cache.work.header(&state, &cache.prepared, index),
                            work_header_at(&state, index)
                        );
                    }
                }
                // Work completion changes metadata without changing item identity.
                let mut completed = state;
                completed.status = codex::Status::Done;
                let next =
                    TranscriptRenderCache::refreshed(Rc::new(completed), vec![], Some(&cache));
                assert!(Rc::ptr_eq(&cache.prepared, &next.prepared));
                assert!(next.work.active.iter().all(|active| !active));
            }
        }
    }
}

#[test]
#[ignore = "manual indexed work lookup CPU comparison"]
fn work_index_long_tool_benchmark() {
    let mut state = snapshot();
    state.saved.items = std::iter::once(json!({"type":"userMessage"}))
        .chain((0..4000).map(|i| json!({"id":format!("tool-{i}"),"type":"commandExecution"})))
        .collect();
    let preparation = std::time::Instant::now();
    let cache = TranscriptRenderCache::new(Rc::new(state.clone()), vec![]);
    let preparation_ms = preparation.elapsed().as_secs_f64() * 1000.;
    let mut old = Vec::new();
    let mut new = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        for index in 0..state.saved.items.len() {
            std::hint::black_box(work_key_at(&state, index));
            std::hint::black_box(work_header_at(&state, index));
        }
        old.push(start.elapsed().as_secs_f64() * 1000.);
        let start = std::time::Instant::now();
        for index in 0..state.saved.items.len() {
            std::hint::black_box(WorkIndex::key(&cache.prepared, index));
            std::hint::black_box(cache.work.header(&state, &cache.prepared, index));
        }
        new.push(start.elapsed().as_secs_f64() * 1000.);
    }
    println!(
        "WORK_INDEX_PERF {}",
        json!({"items":state.saved.items.len(),"preparation_ms":preparation_ms,"legacy_ms":old,"indexed_ms":new,"scope":"work key/header lookup CPU only; not native frame presentation"})
    );
}

#[test]
fn pending_bubbles_stay_before_later_prose_and_preserve_work_boundaries() {
    let mut state = snapshot();
    state.saved.items = vec![
        json!({"id":"history","type":"agentMessage","text":"Earlier response"}),
        json!({"id":"reply","type":"agentMessage","text":"New response"}),
    ]
    .into();
    state.saved.work_periods = vec![
        codex::WorkPeriod {
            start_item: 0,
            end_item: Some(1),
            duration_ms: Some(10),
            subagents: vec![],
        },
        codex::WorkPeriod {
            start_item: 1,
            end_item: None,
            duration_ms: None,
            subagents: vec![],
        },
    ];
    let pending = vec![
        json!({"id":"pending-1","type":"userMessage","localAfterItem":"history","content":[{"text":"first"}]}),
        json!({"id":"pending-2","type":"userMessage","localAfterItem":"history","content":[{"text":"second"}]}),
    ];
    let cache = TranscriptRenderCache::new(Rc::new(state.clone()), pending);
    let ids = cache
        .state
        .saved
        .items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["history", "pending-1", "pending-2", "reply"]);
    assert_eq!(cache.state.saved.work_periods[0].end_item, Some(1));
    assert_eq!(cache.state.saved.work_periods[1].start_item, 1);
    assert_eq!(active_work_start(&cache.state), 1);
    assert_eq!(
        state.saved.items.len(),
        2,
        "projection leaves persisted transcript intact"
    );
    let missing_anchor = vec![
        json!({"id":"pending","type":"userMessage","localAfterItem":"compacted-away","content":[{"text":"input"}]}),
    ];
    let cache = TranscriptRenderCache::new(Rc::new(state), missing_anchor);
    assert_eq!(cache.state.saved.items[0]["id"], "pending");
    assert_eq!(cache.state.saved.work_periods[0].end_item, Some(2));
    assert_eq!(cache.state.saved.work_periods[1].start_item, 2);
}

#[gpui::test]
fn compaction_divider_renders_between_tool_groups_and_waits_while_running(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture(cx);
    view.update(cx, |view, _| {
        view.expanded.insert("work-0".into());
    });
    let mut state = snapshot();
    state.saved.items = vec![
            json!({"id":"a","type":"commandExecution","command":"pwd","status":"completed"}),
            json!({"id":"c","type":codex::COMPACTION_ITEM,"status":"inProgress","trigger":"auto","preTokens":182_000,"postTokens":null}),
        ].into();
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-compaction-1").is_none());
    state.saved.items[1]["status"] = json!("completed");
    state.saved.items[1]["postTokens"] = json!(24_000);
    state
        .saved
        .items
        .push(json!({"id":"b","type":"commandExecution","command":"ls","status":"completed"}));
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let divider = cx.debug_bounds("codex-compaction-1").unwrap();
    let before = cx.debug_bounds("codex-tool-group-0").unwrap();
    let after = cx.debug_bounds("codex-tool-group-2").unwrap();
    assert!(before.bottom() <= divider.top() && divider.bottom() <= after.top());
    assert_eq!(
        codex::item_text(&state.saved.items[1]),
        "Context compacted · auto · 182k → 24k"
    );
    assert!(!is_tool_item(&state.saved.items[1]));
}

#[gpui::test]
fn live_activity_avoids_duplicate_work_and_aligns_meaningful_status(cx: &mut gpui::TestAppContext) {
    let (_view, cx, _, updates) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.status = Status::Working;
        state.saved.items.clear();
        state.saved.thread_id = None;
        let connecting = format!("Connecting to {}…", provider.label());
        assert_eq!(live_activity(&state).as_deref(), Some(connecting.as_str()));
        state.saved.items = vec![
            json!({"id":"prompt","type":"userMessage","content":[{"text":"Check this"}]}),
            json!({"id":"reply","type":"agentMessage","phase":"commentary","text":"Checking this."}),
        ].into();
        assert!(work_header_at(&state, 1).unwrap().2);
        assert_eq!(live_activity(&state), None);
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-live-activity").is_none());
        state.compacting = true;
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Compacting context…")
        );
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        for width in [1000.0, 400.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            draw(cx);
            let status = cx.debug_bounds("codex-live-activity-text").unwrap();
            let body = cx.debug_bounds("codex-message-body-1").unwrap();
            assert_eq!(status.left(), body.left(), "status alignment at {width}");
        }
        state.compacting = false;
        state.approvals.push(json!({"id":"permission"}));
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Waiting for your permission…")
        );
        state.approvals.clear();
        state.elicitations.push(ElicitationRequest::parse_codex(json!("answer"), &json!({"message":"Name", "requestedSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}})).unwrap());
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Waiting for your answers…")
        );
    }
}

#[gpui::test]
fn manual_compaction_preserves_completed_work_disclosures(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, updates) = fixture(cx);
    let mut state = snapshot();
    state.status = Status::Done;
    state.saved.items = vec![
        json!({"id":"prompt","type":"userMessage","content":[{"text":"Check status"}]}),
        json!({"id":"comment","type":"agentMessage","phase":"commentary","text":"Checking status"}),
        json!({"id":"reason","type":"reasoning","text":"Inspect the checkout"}),
        json!({"id":"command","type":"commandExecution","command":"pwd","status":"completed"}),
        json!({"id":"answer","type":"agentMessage","phase":"final_answer","text":"Done"}),
    ]
    .into();
    // Imported history has no recorded work periods. A new compaction turn
    // must not make that completed history part of the live turn.
    assert!(state.saved.work_periods.is_empty());
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-1").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-3").is_none());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("/compact", window, cx));
            view.submit(window, cx);
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Compact)));
    state.apply(&json!({"method":"turn/started","params":{"threadId":state.saved.thread_id,"turn":{"id":"compact-turn"}}}));
    for method in ["item/started", "item/completed"] {
        state.apply(&json!({"method":method,"params":{"threadId":state.saved.thread_id,"turnId":"compact-turn","item":{"id":"compact","type":codex::COMPACTION_ITEM}}}));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(
            cx.debug_bounds("codex-message-body-1").is_none(),
            "{method}"
        );
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_none(), "{method}");
        assert!(cx.debug_bounds("codex-toggle-tool-3").is_none(), "{method}");
        assert!(cx.debug_bounds("codex-message-body-4").is_some());
    }
    state.apply(&json!({"method":"turn/completed","params":{"threadId":state.saved.thread_id,"turn":{"id":"compact-turn","status":"completed"}}}));
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-3").is_none());
    // The disclosure still opens normally after compaction.
    let header = cx.debug_bounds("codex-work-header-1").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-1").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-2").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-3").is_some());
}

#[gpui::test]
fn compaction_shows_the_live_activity_line_even_after_tools(cx: &mut gpui::TestAppContext) {
    let (_view, cx, _, updates) = fixture(cx);
    let mut state = snapshot();
    state.status = Status::Working;
    state.turn_id = Some("turn".into());
    state.saved.items =
        vec![json!({"id":"a","type":"commandExecution","command":"pwd","status":"completed"})]
            .into();
    assert_eq!(live_activity(&state), None);
    state
        .saved
        .items
        .push(json!({"type":"agentMessage","phase":"commentary","text":"Checking results"}));
    assert_eq!(live_activity(&state), None);
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
    assert!(cx.debug_bounds("codex-live-activity").is_none());
    state.compacting = true;
    assert_eq!(
        live_activity(&state).as_deref(),
        Some("Compacting context…")
    );
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-live-activity").is_some());
    // A Codex compaction item that has not completed also counts.
    state.compacting = false;
    state
        .saved
        .items
        .push(json!({"id":"c","type":codex::COMPACTION_ITEM,"status":"inProgress"}));
    assert_eq!(
        live_activity(&state).as_deref(),
        Some("Compacting context…")
    );
}

#[gpui::test]
fn completed_work_hides_commentary_and_keeps_answer_outside(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Done;
                session.state.saved.items = vec![
                    json!({"id":"prompt", "type":"userMessage", "content":[{"text":"Check status"}]}),
                    json!({"id":"comment", "type":"agentMessage", "phase":"commentary", "text":"Checking the repository."}),
                    json!({"id":"command", "type":"commandExecution", "command":"git status", "status":"completed", "aggregatedOutput":"output\n".repeat(100)}),
                    json!({"id":"answer", "type":"agentMessage", "phase":"final_answer", "text":"The working tree is clean."}),
                ].into();
                cx.notify();
            });
        }));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-1").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
    assert!(cx.debug_bounds("codex-message-body-3").is_some());
    let header = cx.debug_bounds("codex-work-header-1").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-1").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-2").is_some());
    assert!(cx.debug_bounds("codex-message-body-3").is_some());
    let tool = cx.debug_bounds("codex-toggle-tool-2").unwrap();
    cx.simulate_click(tool.center(), gpui::Modifiers::default());
    draw(cx);
    let detail = cx.debug_bounds("codex-tool-detail-2").unwrap();
    assert!(
        detail.size.height
            <= cx.update(|window, _| rpx(TOOL_DETAIL_MAX_H).to_pixels(window.rem_size()))
    );
    assert!(cx.debug_bounds("codex-message-body-3").is_some());
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
    draw(cx);
    let detail = cx.debug_bounds("codex-tool-detail-2").unwrap();
    assert!(
        detail.size.height
            <= cx.update(|window, _| rpx(TOOL_DETAIL_MAX_H).to_pixels(window.rem_size()))
    );
    let header = cx.debug_bounds("codex-work-header-1").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-1").is_none());
    assert!(cx.debug_bounds("codex-tool-detail-2").is_none());
    assert!(cx.debug_bounds("codex-message-body-3").is_some());
}

#[test]
fn work_commentary_preserves_explicit_answers_and_legacy_last_response() {
    let mut state = snapshot();
    state.saved.items = vec![
        json!({"type":"userMessage"}),
        json!({"type":"agentMessage", "text":"I will inspect"}),
        json!({"type":"commandExecution"}),
        json!({"type":"agentMessage", "phase":"final_answer", "text":"Done"}),
        json!({"type":"agentMessage", "phase":"commentary", "text":"Late progress"}),
        json!({"type":"userMessage"}),
        json!({"type":"agentMessage", "text":"Legacy final"}),
    ]
    .into();
    assert!(work_commentary(&state, 1));
    assert!(!work_commentary(&state, 3));
    assert!(work_commentary(&state, 4));
    assert!(!work_commentary(&state, 6));
}

#[gpui::test]
fn completed_work_disclosure_contains_tools_and_active_call_stays_visible(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-0".into());
                view.expanded.insert("tools-old-a".into());
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![
                        json!({"id":"old-user","type":"userMessage"}),
                        json!({"id":"old-a","type":"commandExecution","command":"pwd","status":"completed"}),
                        json!({"id":"old-b","type":"commandExecution","command":"ls","status":"completed"}),
                        json!({"id":"new-user","type":"userMessage"}),
                        json!({"id":"new-a","type":"commandExecution","command":"git fetch","status":"completed"}),
                        json!({"id":"new-b","type":"commandExecution","command":"git status","status":"inProgress"}),
                    ].into();
                    cx.notify();
                });
            });
        });
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-4").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("work-3".into());
            view.session.update(cx, |session, cx| {
                session.state.saved.items[5]["status"] = json!("completed");
                cx.notify();
            });
        });
    });
    draw(cx);
    // Explicit expansion reveals earlier calls while the current turn is still running.
    assert!(cx.debug_bounds("codex-toggle-tool-4").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
    cx.update(|_, cx| {
        view.update(cx, |view, _| {
            view.expanded.remove("work-3");
        });
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Done;
                session.state.saved.items[5]["status"] = json!("completed");
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tools-1").is_some());
    assert!(cx.debug_bounds("codex-toggle-tools-4").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-4").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-5").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("work-3".into());
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tools-4").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-4").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
}

#[gpui::test]
fn consecutive_tools_share_one_expandable_summary(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"a","type":"commandExecution","command":"pwd"}),
                    json!({"id":"b","type":"mcpToolCall","server":"docs","tool":"search"}),
                    json!({"id":"c","type":"agentMessage","text":"Results"}),
                    json!({"id":"d","type":"commandExecution","command":"ls"}),
                    json!({"id":"e","type":"reasoning","text":"Reviewing"}),
                ]
                .into();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-tool-group-0").is_none());
    assert!(cx.debug_bounds("codex-message-body-2").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("work-0".into());
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-tool-group-0").is_some());
    assert!(cx.debug_bounds("codex-message-body-2").is_some());
    assert!(cx.debug_bounds("codex-tool-group-1").is_none());
    assert!(cx.debug_bounds("codex-tool-group-3").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-1").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-3").is_some());
    let commands = cx.debug_bounds("codex-tool-group-3").unwrap();
    let thought = cx.debug_bounds("codex-tool-group-4").unwrap();
    let compact_gap = cx.update(|window, _| rpx(SPACE_LG).to_pixels(window.rem_size()));
    assert_eq!(thought.top() - commands.bottom(), compact_gap);
    let first = cx.debug_bounds("codex-toggle-tool-0").unwrap();
    let second = cx.debug_bounds("codex-toggle-tool-1").unwrap();
    let row_gap = cx.update(|window, _| rpx(SPACE_XS).to_pixels(window.rem_size()));
    assert_eq!(second.top() - first.bottom(), row_gap);
    assert!(cx.debug_bounds("codex-tool-detail-0").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("tools-a".into());
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tool-0").is_none());
    assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.remove("tools-a");
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
    assert!(cx.debug_bounds("codex-toggle-tool-1").is_some());
    assert!(cx.debug_bounds("codex-tool-detail-0").is_none());
    assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("tool-a".into());
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-tool-detail-0").is_some());
    assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
}

#[test]
fn task_phase_maps_session_status() {
    for (status, phase) in [
        (Status::Starting, TaskPhase::Running),
        (Status::Working, TaskPhase::Running),
        (Status::Waiting, TaskPhase::Waiting),
        (Status::Interrupted, TaskPhase::Halted),
        (Status::Failed, TaskPhase::Halted),
        (Status::Disconnected, TaskPhase::Halted),
        (Status::Idle, TaskPhase::Stale),
        (Status::Done, TaskPhase::Stale),
    ] {
        assert_eq!(task_phase(status), phase, "{status:?}");
    }
    // Completed marks stay green even when the session is idle.
    assert_eq!(
        task_mark_phase(TaskStatus::Completed, TaskPhase::Stale),
        TaskPhase::Running
    );
    assert_eq!(
        task_mark_phase(TaskStatus::InProgress, TaskPhase::Halted),
        TaskPhase::Halted
    );
}

#[test]
fn task_summary_reads_per_state() {
    use TaskStatus::{Completed, InProgress, Pending};
    let list = task_list(&[Completed, InProgress, Pending]);
    assert_eq!(task_header_status(&list), InProgress);
    assert_eq!(
        task_summary(&list, TaskPhase::Running).text(),
        "Doing step 1"
    );
    assert_eq!(
        task_summary(&list, TaskPhase::Waiting),
        TaskSummary::Waiting("Step 1".into())
    );
    assert_eq!(
        task_summary(&list, TaskPhase::Waiting).text(),
        "Step 1 · Waiting for you"
    );
    assert_eq!(
        task_summary(&list, TaskPhase::Halted).text(),
        "Stopped at Step 1"
    );
    assert_eq!(task_summary(&list, TaskPhase::Stale).text(), "Doing step 1");
    let done = task_list(&[Completed; 7]);
    assert_eq!(task_header_status(&done), Completed);
    for phase in [TaskPhase::Running, TaskPhase::Halted, TaskPhase::Waiting] {
        assert_eq!(task_summary(&done, phase).text(), "All 7 tasks done");
    }
    let pending = task_list(&[Completed, Pending]);
    assert_eq!(task_header_status(&pending), Pending);
    assert_eq!(
        task_summary(&pending, TaskPhase::Running),
        TaskSummary::None
    );
}

#[test]
fn task_cache_ignores_streaming_text_but_tracks_checklists_and_new_items() {
    let mut items = vec![
        json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
        json!({"id":"p1", "type":"plan", "text":"", "plan":[{"step":"A","status":"inProgress"}]}),
        json!({"id":"a1", "type":"agentMessage", "text":"Working"}),
    ];
    let key = task_cache_key(&items);
    items[2]["text"] = json!("Working on it");
    assert_eq!(task_cache_key(&items), key);
    items[1]["plan"][0]["status"] = json!("completed");
    let changed = task_cache_key(&items);
    assert_ne!(changed, key);
    items.push(json!({"id":"u2", "type":"userMessage", "content":[{"text":"Next"}]}));
    assert_ne!(task_cache_key(&items), changed);
    let cache = task_cache(&items, task_cache_key(&items));
    assert_eq!(cache.list.unwrap().done(), 1);
}

#[gpui::test]
fn session_task_strip_survives_a_clarification_answer(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        cx.set_reduce_motion(true);
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Waiting;
                session.state.saved.items = vec![
                    json!({"id":"p1", "type":"plan", "plan":[
                            {"step":"Inspect", "status":"completed"},
                            {"step":"Implement", "status":"inProgress"},
                            {"step":"Verify", "status":"pending"}]}),
                    json!({"id":"q1", "type":"agentMessage", "text":"Which option?"}),
                ]
                .into();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-task-strip").is_some());
    let before = view.read_with(cx, |view, _| {
        view.task_cache.as_ref().unwrap().list.clone().unwrap()
    });
    assert_eq!((before.done(), before.total()), (1, 3));
    assert_eq!(before.now_label(), Some("Implement"));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.items.push(
                    json!({"id":"u2", "type":"userMessage", "content":[{"text":"Both equally"}]}),
                );
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-task-strip").is_some());
    assert!(cx.debug_bounds("codex-task-row-1").is_some());
    assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
    let after = view.read_with(cx, |view, _| {
        view.task_cache.as_ref().unwrap().list.clone().unwrap()
    });
    assert_eq!(after, before);
}

#[test]
fn tool_failure_summary_is_local_and_explicit() {
    let command =
        json!({"type":"commandExecution","command":"npm test","exitCode":1,"status":"completed"});
    assert!(tool_activity_summary(&command, false).starts_with("Command failed"));
    assert!(!is_tool_item(&json!({"type":"providerNotice"})));
    assert!(!tool_failed(
        &json!({"type":"commandExecution","exitCode":0,"status":"completed"})
    ));
}

#[test]
fn work_headers_show_seconds_until_two_minutes_and_skip_unknown_duration() {
    use serde_json::json;
    let mut state = snapshot();
    state.saved.items = vec![
        json!({"type":"userMessage","id":"prompt"}),
        json!({"type":"reasoning","id":"thought"}),
        json!({"type":"agentMessage","id":"answer","text":"Done"}),
    ]
    .into();
    assert!(work_header_at(&state, 0).is_none());
    assert_eq!(work_header_at(&state, 1).unwrap().1, "Worked");
    assert!(work_header_at(&state, 2).is_none());
    state.saved.work_periods.push(codex::WorkPeriod {
        start_item: 0,
        end_item: Some(3),
        duration_ms: Some(23_900),
        subagents: Vec::new(),
    });
    for (duration_ms, expected) in [
        (0, "Worked for 0s"),
        (23_900, "Worked for 23s"),
        (45_000, "Worked for 45s"),
        (59_999, "Worked for 59s"),
        (60_000, "Worked for 1m 0s"),
        (119_999, "Worked for 1m 59s"),
        (120_000, "Worked for 2m"),
        (125_000, "Worked for 2m"),
    ] {
        state.saved.work_periods[0].duration_ms = Some(duration_ms);
        assert_eq!(work_header_at(&state, 1).unwrap().1, expected);
    }
    assert_eq!(work_key_at(&state, 1), work_key_at(&state, 2));
    state.saved.items.truncate(1);
    state.saved.work_periods.clear();
    state.status = Status::Working;
    assert_eq!(work_header_at(&state, 1).unwrap().1, "Working");
    state.work_started = Some(std::time::Instant::now());
    assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for 0s");
    state.work_started = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(125))
            .unwrap(),
    );
    assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for 2m");
}

#[gpui::test]
fn expanded_work_reports_empty_parent_activity_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, updates) = fixture(cx);
    view.update(cx, |view, _| {
        view.expanded.insert("work-0".into());
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.status = Status::Working;
        state.saved.items = vec![
                json!({"id":"prompt","type":"userMessage","content":[{"text":"Work"}]}),
                json!({"id":"child","type":"subAgentActivity","kind":"started","agentThreadId":"child-1"}),
            ].into();
        assert!(!work_has_parent_activity(&state, 1));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-work-empty-1").is_some());
        assert!(cx.debug_bounds("codex-tool-group-1").is_none());
        state.saved.items.push(
            json!({"id":"first","type":"commandExecution","command":"pwd","status":"completed"}),
        );
        state.saved.items.push(
            json!({"id":"last","type":"commandExecution","command":"ls","status":"inProgress"}),
        );
        assert!(work_has_parent_activity(&state, 1));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-work-empty-1").is_none());
        assert!(cx.debug_bounds("codex-tool-row-label-2").is_some());
        assert!(cx.debug_bounds("codex-tool-row-label-3").is_some());
    }
}

#[test]
fn running_turn_shows_only_current_tool_then_reveals_history() {
    let mut state = snapshot();
    state.saved.items = vec![
        serde_json::json!({"type":"userMessage"}),
        serde_json::json!({"type":"commandExecution","status":"completed"}),
        serde_json::json!({"type":"commandExecution","status":"inProgress"}),
    ]
    .into();
    state.status = Status::Working;
    assert!(!tool_visible(&state, 1));
    assert!(tool_visible(&state, 2));
    state.saved.items[2]["status"] = serde_json::json!("completed");
    assert!(tool_visible(&state, 2));
    state.status = Status::Done;
    assert!(tool_visible(&state, 1));
    assert!(tool_visible(&state, 2));
}

#[test]
fn command_activity_rows_use_tense_without_output() {
    let item = serde_json::json!({"type":"commandExecution","command":"git status\n--short","aggregatedOutput":"private output"});
    assert_eq!(
        tool_activity_summary(&item, true),
        "Running git status --short…"
    );
    assert_eq!(
        tool_activity_summary(&item, false),
        "Ran git status --short"
    );
}

#[test]
fn active_tool_rows_describe_work_while_preserving_raw_disclosures() {
    use serde_json::json;
    for (tool, expected) in [
        ("sleep", "Sleeping…"),
        ("clock__sleep", "Sleeping…"),
        ("wait", "Waiting…"),
        ("functions__wait", "Waiting…"),
        ("wait_agent", "Waiting…"),
        ("thought", "Thinking…"),
        ("Read", "Reading…"),
        ("Write", "Writing…"),
        ("Edit", "Editing…"),
        ("MultiEdit", "Editing…"),
        ("Grep", "Searching…"),
        ("Glob", "Searching…"),
        ("WebSearch", "Searching…"),
        ("WebFetch", "Fetching…"),
        ("custom_operation", "Using custom_operation…"),
    ] {
        let item = json!({"type":"mcpToolCall","server":"fixture","tool":tool,"arguments":{"secret":"input"}});
        assert_eq!(tool_activity_summary(&item, true), expected);
        let raw = format!("Tool · fixture / {tool}");
        assert_eq!(tool_summary(&item), raw);
        assert_eq!(
            tool_activity_summary(&item, false),
            if expected == "Waiting…" {
                expected
            } else {
                raw.as_str()
            }
        );
        assert!(tool_detail(&item).contains("input"));
    }
    let reasoning = json!({"type":"reasoning"});
    assert_eq!(tool_activity_summary(&reasoning, true), "Thinking…");
    assert_eq!(tool_activity_summary(&reasoning, false), "Thinking…");
    assert_eq!(
        tool_activity_summary(&json!({"type":"fileChange"}), true),
        "Updating files…"
    );
    assert_eq!(
        tool_activity_summary(&json!({"type":"webSearch"}), true),
        "Searching the web…"
    );
}

#[test]
fn tool_summaries_describe_inputs_without_leaking_results() {
    use serde_json::json;
    for (item, expected) in [
        (
            json!({"type":"commandExecution","command":"cargo test\n--lib","aggregatedOutput":"secret result"}),
            "Command · cargo test --lib",
        ),
        (
            json!({"type":"fileChange","changes":[{"path":"src/main.rs","diff":"secret result"},{"path":"Cargo.toml"}]}),
            "File changes · src/main.rs, Cargo.toml",
        ),
        (
            json!({"type":"mcpToolCall","server":"docs","tool":"search","arguments":{"query":"gpui"},"result":{"content":[{"text":"secret result"}]}}),
            "Tool · docs / search",
        ),
        (
            json!({"type":"webSearch","action":{"query":"GPUI docs"}}),
            "Web search · GPUI docs",
        ),
    ] {
        assert_eq!(tool_summary(&item), expected);
    }
    let detail = tool_detail(
        &json!({"type":"mcpToolCall","server":"docs","tool":"search","arguments":{"query":"gpui"},"result":{"content":[{"text":"returned documentation"}]}}),
    );
    assert!(detail.contains("gpui"));
    assert!(detail.contains("returned documentation"));
}

#[gpui::test]
fn publications_remain_visible_with_collapsed_work_and_do_not_duplicate(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.provider = provider;
        response.saved.html_policy = grove_core::visuals::HtmlPolicy::Explicit;
        response.saved.items.push(serde_json::json!({"id":"published-tool","type":"mcpToolCall","server":"grove_visuals","tool":"render_html","status":"completed","arguments":{"title":"Chart"}}));
        response.saved.visuals.push(grove_core::visuals::Reference {
            scope: "0".repeat(64),
            publication_id: "1".repeat(64),
            visual_id: "1".repeat(64),
            revision_id: "1".repeat(64),
            source_digest: "2".repeat(64),
            title: "Chart".into(),
            item_id: "published-tool".into(),
            turn_id: "turn".into(),
            presentation: grove_core::visuals::PresentationOptions::default(),
        });
        response.saved.items.push(serde_json::json!({"id":"answer","type":"agentMessage","text":"Published chart. [Source](chart.html)"}));
        updates.send(response.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("html-publication-attachment").is_some());
        let publication = cx.debug_bounds("codex-visual-publication-0").unwrap();
        let answer = cx.debug_bounds("codex-message-anchor-1").unwrap();
        assert_eq!(publication.left(), answer.left());
        assert_eq!(publication.size.width, answer.size.width);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
            1
        );
        updates.send(response.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
            1
        );
        // A removed source event does not leave its native artifact in the transcript.
        response.saved.items.clear();
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("html-publication-attachment").is_none());
    }
}
