use super::*;

#[test]
fn internal_activity_reports_do_not_join_tool_groups_or_response_copy() {
    let internal = json!({"id":"report","type":"mcpToolCall","server":"grove_activity","tool":"report_activity","arguments":{"private":"metadata"},"status":"completed"});
    assert!(!is_tool_item(&internal));
    assert_eq!(codex::item_text(&internal), "");
    let items = vec![
        json!({"type":"userMessage","content":[{"text":"Review"}]}),
        json!({"type":"commandExecution","command":"pwd"}),
        internal,
        json!({"type":"commandExecution","command":"ls"}),
        json!({"type":"agentMessage","text":"Reviewed navigation.","phase":"final_answer"}),
    ];
    assert_eq!(items.iter().filter(|item| is_tool_item(item)).count(), 2);
    let groups = response_groups(&items, Some("thread"));
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].copy_text(), "Reviewed navigation.");
}

#[test]
fn minimap_pairs_inputs_with_final_or_latest_response_without_crossing_next_input() {
    let items = vec![
        json!({"id":"u1", "type":"userMessage", "content":[{"text":"First input"}]}),
        json!({"type":"agentMessage", "phase":"commentary", "text":"Working"}),
        json!({"type":"commandExecution", "aggregatedOutput":"tool output"}),
        json!({"type":"agentMessage", "phase":"final_answer", "text":"Final result"}),
        json!({"type":"agentMessage", "phase":"commentary", "text":"Late commentary"}),
        json!({"id":"u2", "type":"userMessage", "content":[{"text":"Second input"}]}),
        json!({"type":"agentMessage", "text":"Streaming response"}),
        json!({"id":"u3", "type":"userMessage", "content":[{"type":"image"}]}),
        json!({"id":"u4", "type":"userMessage", "content":[{"text":"Fourth input"}]}),
        json!({"type":"agentMessage", "text":"Fourth response"}),
    ];
    let entries = message_minimap_entries(&items);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item_index)
            .collect::<Vec<_>>(),
        vec![0, 5, 7, 8]
    );
    assert_eq!(entries[0].prompt, "First input");
    assert_eq!(entries[0].response, "Final result");
    assert_eq!(entries[1].response, "Streaming response");
    assert_eq!(entries[2].prompt, "Image attachment");
    assert_eq!(entries[2].response, "No response yet");
    assert_eq!(entries[3].response, "Fourth response");
    assert!(minimap_excerpt(&"🦀".repeat(500)).ends_with('…'));
}

#[gpui::test]
fn minimap_click_uses_rendered_anchor_and_streaming_does_not_snap_back(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"first", "type":"userMessage", "content":[{"text":"First input"}]}),
                    json!({"id":"tool", "type":"commandExecution", "command":"git status", "status":"completed"}),
                    json!({"id":"first-reply", "type":"agentMessage", "text":"First response.\n\n".repeat(40)}),
                    json!({"id":"second", "type":"userMessage", "content":[{"text":"Second input"}]}),
                    json!({"id":"second-reply", "type":"agentMessage", "text":"Second response.\n\n".repeat(40)}),
                    json!({"id":"third", "type":"userMessage", "content":[{"text":"Third input"}]}),
                    json!({"id":"third-reply", "type":"agentMessage", "text":"Streaming.\n\n".repeat(40)}),
                ].into();
                cx.notify();
            });
        }));
    draw(cx);
    let rail = minimap_bounds(cx, "codex-message-minimap").unwrap();
    let first = minimap_bounds(cx, "codex-minimap-message-0").unwrap();
    let mark = minimap_bounds(cx, "codex-minimap-message-3").unwrap();
    let last = minimap_bounds(cx, "codex-minimap-message-5").unwrap();
    let row_height = rpx(SPACE_LG).to_pixels(gpui::px(16.0));
    assert!((mark.top() - first.bottom()).abs() < gpui::px(1.0));
    assert_eq!(
        mark.size.height, row_height,
        "preserve compact tick marker spacing"
    );
    for bounds in [first, mark, last] {
        assert!((bounds.center().x - rail.center().x).abs() < gpui::px(1.0));
    }
    let stack_center = (first.top() + last.bottom()) / 2.0;
    assert!((stack_center - rail.center().y).abs() < gpui::px(1.0));
    cx.simulate_click(mark.center(), gpui::Modifiers::default());
    draw(cx);
    let transcript = cx.debug_bounds("codex-messages").unwrap();
    let prompt = cx.debug_bounds("codex-message-anchor-3").unwrap();
    assert!(
        (prompt.top() - transcript.top()).abs() < gpui::px(1.0),
        "jump must align the selected user row after tool/header insertion"
    );
    let offset = view.read_with(cx, |view, _| {
        assert!(view.composer_collapsed);
        assert!(!view.smooth_follow);
        view.scroll.offset().y
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.items[6]["text"] = json!("Streaming more.\n\n".repeat(60));
                cx.notify();
            });
        });
    });
    draw(cx);
    assert_eq!(view.read_with(cx, |view, _| view.scroll.offset().y), offset);
    // Returning to the first turn must resolve against the same rendered list.
    cx.update(|window, cx| {
        window.blur();
        window.focus_next(cx);
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        (cx.debug_bounds("codex-message-anchor-0").unwrap().top()
            - cx.debug_bounds("codex-messages").unwrap().top())
        .abs()
            < gpui::px(1.0)
    );
}

#[gpui::test]
fn minimap_keeps_many_turns_reachable_in_narrow_panes(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = (0..50).flat_map(|index| [
                    json!({"id":format!("user-{index}"), "type":"userMessage", "content":[{"text":format!("Input {index}")}]}),
                    json!({"id":format!("reply-{index}"), "type":"agentMessage", "text":format!("Response {index}")}),
                ]).collect();
                cx.notify();
            });
        }));
    draw(cx);
    let rail = minimap_bounds(cx, "codex-message-minimap").unwrap();
    let transcript = cx.debug_bounds("codex-messages").unwrap();
    assert!(rail.right() <= transcript.left());
    for selector in [
        "codex-minimap-message-0",
        "codex-minimap-message-20",
        "codex-minimap-message-50",
        "codex-minimap-message-98",
    ] {
        let mark = minimap_bounds(cx, selector).unwrap();
        assert!(mark.size.height > gpui::Pixels::ZERO);
        assert!(mark.top() >= rail.top());
        assert!(mark.bottom() <= rail.bottom());
    }
    let mark = minimap_bounds(cx, "codex-minimap-message-50").unwrap();
    cx.simulate_click(mark.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(
        (cx.debug_bounds("codex-message-anchor-50").unwrap().top()
            - cx.debug_bounds("codex-messages").unwrap().top())
        .abs()
            < gpui::px(1.0)
    );
}

#[gpui::test]
fn minimap_overflow_keeps_first_and_last_turns_reachable(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = (0..200).map(|index|
                    json!({"id":format!("user-{index}"), "type":"userMessage", "content":[{"text":format!("Input {index}")}]}),
                ).collect();
                cx.notify();
            });
        }));
    draw(cx);
    let rail = minimap_bounds(cx, "codex-message-minimap").unwrap();
    let first = minimap_bounds(cx, "codex-minimap-message-0").unwrap();
    assert!(first.top() >= rail.top());
    assert!(first.bottom() <= rail.bottom());
    let scroll_rail = |delta, cx: &mut gpui::VisualTestContext| {
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: rail.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
    };
    scroll_rail(-100000.0, cx);
    let last = minimap_bounds(cx, "codex-minimap-message-199").unwrap();
    assert!(last.top() >= rail.top());
    assert!(last.bottom() <= rail.bottom());
    cx.simulate_click(last.center(), gpui::Modifiers::default());
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.scroll.offset().y < gpui::Pixels::ZERO);
    });
    scroll_rail(100000.0, cx);
    let first = minimap_bounds(cx, "codex-minimap-message-0").unwrap();
    assert!(first.top() >= rail.top());
    assert!(first.bottom() <= rail.bottom());
}

#[gpui::test]
fn tables_beside_code_controls_preserve_geometry_and_keyboard_copy(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture_with_navigation(cx, true);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let text = format!("```rust\nlet answer = 42;\n```\n\n{}", (0..12).fold(String::new(), |mut text, i| {
                use std::fmt::Write as _;
                let _ = write!(text, "## Decision {i}\n\n| Access | Expiry | Method | Acceptance | Audit | Scope |\n| --- | --- | --- | --- | --- | --- |\n| Member with a longer description | Seven days | Email and link | Immediate | Retained | Prototype only |\n\n");
                text
            }));
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.thread_id = Some(format!("table-code-{provider:?}"));
                    session.state.saved.items = vec![json!({"id":"answer", "type":"agentMessage", "text":text, "phase":"final_answer", "groveCompleted":true})].into();
                    session.state.status = Status::Idle;
                    cx.notify();
                });
                cx.notify();
            }));
        for width in [1000., 420.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            for _ in 0..4 {
                cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
                draw(cx);
            }
            let extent = view.read_with(cx, |view, _| view.scroll.max_offset());
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    // Table layout may stay eager; the invariant is that its
                    // original code-copy destination remains usable below.
                    view.message_focus
                        .iter()
                        .find(|(key, _)| key.contains(":code:"))
                        .unwrap()
                        .1
                        .focus(window, cx);
                });
            });
            draw(cx);
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(
                cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
                    .unwrap()
                    .trim(),
                "let answer = 42;"
            );
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    for owner in view.selection_views.values() {
                        owner.update(cx, gpui_component::text::TextViewState::select_all);
                    }
                    cx.notify();
                });
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.scroll.max_offset(), extent);
                let selected = view
                    .selection_views
                    .values()
                    .next()
                    .unwrap()
                    .read(cx)
                    .selected_text();
                assert!(selected.contains("Decision 11") && selected.contains("Prototype only"));
            });
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.selection_views.clear();
                    view.message_slots.clear();
                    cx.notify();
                });
            });
        }
    }
}

#[gpui::test]
fn measured_message_slots_preserve_geometry_owners_and_focus_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    fn slot_draw(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    struct ResetEager;
    impl Drop for ResetEager {
        fn drop(&mut self) {
            measured_slot::FORCE_EAGER.with(|flag| flag.set(false));
        }
    }
    let _reset = ResetEager;
    let (view, cx, _, _) = fixture_with_navigation(cx, true);
    let fork_events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = fork_events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let CodexViewEvent::ForkRequested(boundary, restore_user) = event {
                observed.borrow_mut().push((*boundary, *restore_user));
            }
        })
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.thread_id = Some(format!("slot-{provider:?}"));
                    session.state.saved.items = (0..80).map(|i| json!({
                        "id":format!("slot-{i}"), "type":if i%4 == 0 { "userMessage" } else { "agentMessage" },
                        "content":[{"type":"text","text":format!("Input {i}")}],
                        "text":format!("## Heading {i}\n\nComplete **Markdown** source with [remote link](https://example.com) and enough text to wrap at narrow widths.\n\nSecond paragraph."),
                        "phase":"final_answer", "groveCompleted":true,
                    })).collect();
                    session.state.status = Status::Idle;
                    cx.notify();
                });
                cx.notify();
            }));
        // Initialize session ownership before expanding the full-history fixture.
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let items = view.session.read(cx).state.saved.items.clone();
                view.turn_history.expand_all_for_test(&items);
                cx.notify();
            });
        });
        for (width, rem) in [(1000., 16.), (420., 16.), (420., 22.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|_, cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            measured_slot::FORCE_EAGER.with(|flag| flag.set(true));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            for _ in 0..4 {
                slot_draw(cx);
            }
            let expected_max = view.read_with(cx, |view, _| view.scroll.max_offset());
            let expected_first = cx.debug_bounds("codex-message-anchor-0").unwrap();
            let expected_last = cx.debug_bounds("codex-message-anchor-79").unwrap();
            let expected_owner = view.read_with(cx, |view, cx| {
                let key = format!("slot-{provider:?}:slot-0");
                view.selection_views[&key].read(cx).layout_bounds()
            });
            measured_slot::FORCE_EAGER.with(|flag| flag.set(false));
            for _ in 0..4 {
                cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
                slot_draw(cx);
            }
            measured_slot::MATERIALIZED.with(|count| count.set(0));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            slot_draw(cx);
            assert!(
                measured_slot::MATERIALIZED.with(Cell::get) < 20,
                "warm frame should only materialize viewport rows"
            );
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max
            );
            assert_eq!(
                cx.debug_bounds("codex-message-anchor-0").unwrap(),
                expected_first
            );
            assert_eq!(
                cx.debug_bounds("codex-message-anchor-79").unwrap(),
                expected_last
            );
            view.read_with(cx, |view, cx| {
                let key = format!("slot-{provider:?}:slot-0");
                assert_eq!(view.selection_views.len(), 80);
                assert_eq!(
                    view.selection_views[&key].read(cx).layout_bounds(),
                    expected_owner
                );
            });
            // The original work button remains the sole header Tab stop.
            // Reifying its offscreen proxy preserves Enter/Space disclosure.
            let header_focus_key = format!("work-header:slot-{provider:?}:1:work-0");
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.message_focus[&format!("slot-{provider:?}:slot-0")].focus(window, cx);
                });
            });
            slot_draw(cx);
            // Existing order is source -> Fork -> Edit -> Copy -> Worked button;
            // no extra header-container stop may be introduced.
            for _ in 0..4 {
                cx.simulate_keystrokes("tab");
                slot_draw(cx);
            }
            cx.update(|window, cx| {
                view.read_with(cx, |view, _| {
                    assert!(
                        view.message_focus[&header_focus_key].is_focused(window),
                        "Tab must enter the offscreen work button in its original position"
                    );
                });
            });
            assert!(cx.debug_bounds("codex-work-header-1").is_some());
            cx.simulate_keystrokes("enter");
            slot_draw(cx);
            view.read_with(cx, |view, _| assert!(view.expanded.contains("work-0")));
            assert!(cx.debug_bounds("codex-work-empty-1").is_some());
            cx.simulate_keystrokes("space");
            slot_draw(cx);
            view.read_with(cx, |view, _| assert!(!view.expanded.contains("work-0")));
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max,
                "expanding and collapsing a reified header restores exact flow geometry"
            );
            // Footer Copy and Fork retain their original Tab order.
            // Proxies reify the actual actions before keyboard activation.
            let (footer_key, response_text) = view.read_with(cx, |view, _| {
                let group = &view
                    .transcript_cache
                    .as_ref()
                    .unwrap()
                    .prepared
                    .response_groups[0];
                (group.key.clone(), group.copy_text())
            });
            assert!(
                !measured_slot::RENDERED_FOOTERS.with(|keys| keys.borrow().contains(&footer_key)),
                "target footer must still be offscreen and unmaterialized before Tab entry"
            );
            cx.simulate_keystrokes("tab");
            slot_draw(cx);
            cx.update(|window, cx| view.read_with(cx, |view, _| {
                    assert!(view.message_focus[&format!("{footer_key}:copy")].is_focused(window),
                        "Tab from Worked skips non-stop prose containers and reaches the original footer Copy button directly");
                }));
            cx.simulate_keystrokes("space");
            slot_draw(cx);
            assert_eq!(
                cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
                Some(response_text)
            );
            view.read_with(cx, |view, _| {
                assert_eq!(view.copied_message.as_ref().unwrap().0, footer_key);
            });
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max,
                "footer focus and copy feedback must preserve its fixed extent"
            );
            measured_slot::MATERIALIZED.with(|count| count.set(0));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            slot_draw(cx);
            assert!(
                measured_slot::MATERIALIZED.with(Cell::get) < 20,
                "one response's copy feedback must not reify the entire transcript"
            );
            // Footer Fork precedes the following user Fork/Edit/Copy;
            // no extra source-container stop is introduced.
            assert!(
                cx.debug_bounds("codex-edit-message-4").is_none(),
                "target user row must stay culled until Tab reaches its real Edit destination"
            );
            for key in [
                "fork:4:false".to_owned(),
                "fork:4:true".to_owned(),
                format!("slot-{provider:?}:slot-4:edit"),
                format!("slot-{provider:?}:slot-4:copy"),
            ] {
                cx.simulate_keystrokes("tab");
                slot_draw(cx);
                cx.update(|window, cx| {
                    view.read_with(cx, |view, _| {
                        assert!(
                            view.message_focus[&key].is_focused(window),
                            "offscreen user controls must preserve forward Tab order: {key}"
                        );
                    });
                });
                if key.starts_with("fork:") {
                    fork_events.borrow_mut().clear();
                    cx.simulate_keystrokes("enter");
                    slot_draw(cx);
                    assert_eq!(&*fork_events.borrow(), &[(4, key == "fork:4:true")]);
                }
            }
            cx.simulate_keystrokes("space");
            slot_draw(cx);
            assert_eq!(
                cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
                Some("Input 4".into())
            );
            for key in [
                format!("slot-{provider:?}:slot-4:edit"),
                "fork:4:true".to_owned(),
                "fork:4:false".to_owned(),
                format!("{footer_key}:copy"),
                header_focus_key.clone(),
            ] {
                cx.simulate_keystrokes("shift-tab");
                slot_draw(cx);
                cx.update(|window, cx| {
                    view.read_with(cx, |view, _| {
                        assert!(
                            view.message_focus[&key].is_focused(window),
                            "offscreen controls must preserve reverse Tab order: {key}"
                        );
                    });
                });
            }
            cx.update(|_, cx| view.update(cx, |view, _| view.copied_message = None));
            // A source focus target stays registered while its row is culled.
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let key = format!("slot-{provider:?}:slot-0");
                    view.message_focus[&key].focus(window, cx);
                });
            });
            slot_draw(cx);
            assert!(
                cx.debug_bounds("codex-edit-message-0").is_some(),
                "focusing an offscreen source must reify its keyboard actions"
            );
            cx.update(|window, _| window.blur());
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items.clear();
                    cx.notify();
                });
                cx.notify();
            });
        });
        slot_draw(cx);
        view.read_with(cx, |view, _| {
            assert!(
                view.selection_views.is_empty(),
                "removed sources must lose selection ownership"
            );
            assert!(
                view.message_slots.is_empty(),
                "removed rows must lose cached extents"
            );
            assert!(
                view.work_header_slots.is_empty(),
                "removed work headers must lose cached extents"
            );
        });
    }
    measured_slot::FORCE_EAGER.with(|flag| flag.set(false));
}

#[gpui::test]
fn completed_tool_details_preserve_keyboard_pointer_and_zoom_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            measured_slot::FORCE_EAGER.with(|flag| flag.set(false));
        }
    }
    fn frame(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    let _reset = Reset;
    let (view, cx, _, _) = fixture_with_navigation(cx, true);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.expanded.clear();
                    view.expanded.insert("work-0".into());
                    view.session.update(cx, |session, cx| {
                        let mut state = snapshot();
                        state.saved.provider = provider;
                        state.saved.thread_id = Some(format!("tool-test-{provider:?}"));
                        state.saved.items = vec![
                            json!({"id":"request", "type":"userMessage", "content":[{"type":"text","text":"Inspect the original tool output"}]}),
                            json!({"id":"cmd-1", "type":"commandExecution", "command":"printf 'complete original output with a long activity label for narrow truncation'", "status":"completed", "aggregatedOutput":"original first output"}),
                            json!({"id":"cmd-2", "type":"commandExecution", "command":"git status --short", "status":"completed", "aggregatedOutput":"original second output"}),
                            json!({"id":"reason", "type":"reasoning", "text":"Retained completed reasoning", "status":"completed"}),
                            json!({"id":"answer", "type":"agentMessage", "text":"Complete original answer", "phase":"final_answer", "groveCompleted":true}),
                        ].into();
                        for index in 0..40 {
                            state.saved.items.push(json!({"id":format!("later-user-{index}"),"type":"userMessage","content":[{"type":"text","text":format!("Later prompt {index}")}]}));
                            state.saved.items.push(json!({"id":format!("later-answer-{index}"),"type":"agentMessage","text":format!("Later complete answer {index}\n\nA second paragraph retained in full."),"phase":"final_answer","groveCompleted":true}));
                        }
                        session.state = state.into();
                        cx.notify();
                    });
                    cx.notify();
                });
            });
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.jump_to_transcript_item(0, window, cx);
            });
        });
        draw(cx);
        for (width, rem) in [(1000., 16.), (420., 16.), (420., 22.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|_, cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.smooth_follow = false;
                    view.follow_after_submit = false;
                    view.scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            measured_slot::FORCE_EAGER.with(|flag| flag.set(true));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            for _ in 0..4 {
                frame(cx);
            }
            let expected_max = view.read_with(cx, |view, _| view.scroll.max_offset());
            let command_group = cx.debug_bounds("codex-tool-group-1").unwrap();
            let reasoning_group = cx.debug_bounds("codex-tool-group-3").unwrap();
            measured_slot::FORCE_EAGER.with(|flag| flag.set(false));
            for _ in 0..4 {
                cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
                frame(cx);
            }
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max
            );
            assert_eq!(
                cx.debug_bounds("codex-tool-group-1").unwrap(),
                command_group
            );
            assert_eq!(
                cx.debug_bounds("codex-tool-group-3").unwrap(),
                reasoning_group
            );
            assert!(
                cx.debug_bounds("codex-toggle-tool-1").is_none(),
                "completed offscreen tool rows reuse geometry before keyboard entry"
            );
            measured_tool::MATERIALIZED.with(|count| count.set(0));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            frame(cx);
            assert_eq!(
                measured_tool::MATERIALIZED.with(Cell::get),
                0,
                "warm offscreen completed tools must not build or shape their summaries"
            );
            let work_key = format!("work-header:tool-test-{provider:?}:1:work-0");
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.message_focus[&work_key].focus(window, cx);
                });
            });
            frame(cx);
            // Worked -> existing group summary -> original first tool button.
            cx.simulate_keystrokes("tab");
            frame(cx);
            let summary_focus = cx.update(|window, cx| window.focused(cx)).unwrap();
            cx.simulate_keystrokes("tab");
            frame(cx);
            let first_focus = cx.update(|window, cx| window.focused(cx)).unwrap();
            assert!(cx.debug_bounds("codex-toggle-tool-1").is_some());
            cx.simulate_event(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("space").unwrap(),
                is_held: true,
                prefer_character_input: false,
            });
            frame(cx);
            view.read_with(cx, |view, _| {
                assert!(
                    !view.expanded.contains("tool-cmd-1"),
                    "held Space must not toggle details"
                );
            });
            cx.simulate_keystrokes("enter");
            frame(cx);
            assert!(cx.debug_bounds("codex-tool-detail-1").is_some());
            cx.simulate_keystrokes("space");
            frame(cx);
            assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max
            );
            let mut later_focus = Vec::new();
            for (index, selector) in [(2, "codex-tool-detail-2"), (3, "codex-tool-detail-3")] {
                cx.simulate_keystrokes("tab");
                frame(cx);
                later_focus.push(cx.update(|window, cx| window.focused(cx)).unwrap());
                cx.simulate_keystrokes("enter");
                frame(cx);
                assert!(
                    cx.debug_bounds(selector).is_some(),
                    "Tab must reach the original tool toggle {index}"
                );
                cx.simulate_keystrokes("space");
                frame(cx);
                assert!(cx.debug_bounds(selector).is_none());
            }
            for target in [&later_focus[0], &first_focus] {
                cx.simulate_keystrokes("shift-tab");
                frame(cx);
                cx.update(|window, _| {
                    assert!(target.is_focused(window), "reverse tool Tab order");
                });
            }
            cx.simulate_keystrokes("shift-tab");
            frame(cx);
            cx.update(|window, _| {
                assert!(
                    summary_focus.is_focused(window),
                    "group summary stays the preceding stop"
                );
            });
            cx.simulate_keystrokes("space");
            frame(cx);
            view.read_with(cx, |view, _| {
                assert!(
                    view.expanded.contains("tools-cmd-1"),
                    "group summary collapse keeps original inverse expansion state"
                );
            });
            cx.simulate_keystrokes("space");
            frame(cx);
            view.read_with(cx, |view, _| {
                assert!(!view.expanded.contains("tools-cmd-1"));
            });
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                expected_max
            );
            // Actual navigation reveals the same button for pointer toggles.
            cx.update(|window, cx| {
                view.update(cx, |view, cx| view.jump_to_message(2, window, cx));
            });
            for _ in 0..4 {
                frame(cx);
            }
            let before_click = view.read_with(cx, |view, _| view.scroll.max_offset());
            let button = cx.debug_bounds("codex-toggle-tool-1").unwrap();
            let viewport = view.read_with(cx, |view, _| view.scroll.bounds());
            assert!(viewport.contains(&button.center()),
                    "pointer target must be truly visible: provider={provider:?}, width={width}, rem={rem}, button={button:?}, viewport={viewport:?}");
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            frame(cx);
            view.read_with(cx, |view, _| assert!(view.expanded.contains("tool-cmd-1"),
                    "actual pointer callback must toggle the original key: provider={provider:?}, width={width}, rem={rem}, button={button:?}, viewport={viewport:?}"));
            assert!(cx.debug_bounds("codex-tool-detail-1").is_some());
            let button = cx.debug_bounds("codex-toggle-tool-1").unwrap();
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            frame(cx);
            assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                before_click
            );
            cx.update(|window, _| window.blur());
        }
    }
}

#[gpui::test]
fn static_replacements_retain_measurements_without_retaining_selection_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    fn frame(cx: &mut gpui::VisualTestContext) {
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    fn replace(view: &Entity<CodexView>, cx: &mut gpui::VisualTestContext, text: &str) {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[0]["text"] = json!(text);
                    cx.notify();
                });
                cx.notify();
            });
        });
        for _ in 0..4 {
            frame(cx);
        }
    }
    fn owner(view: &Entity<CodexView>, cx: &mut gpui::VisualTestContext) -> Entity<TextViewState> {
        view.read_with(cx, |view, cx| {
            let key = format!(
                "{}:answer",
                view.session
                    .read(cx)
                    .state
                    .saved
                    .thread_id
                    .as_deref()
                    .unwrap_or_default()
            );
            view.selection_views[&key].clone()
        })
    }
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let prefix = "Complete paragraph with **Markdown** and literal copy text.\n\n".repeat(40);
        let first = format!("{prefix}Alpha");
        let second = format!("{prefix}Bravo");
        cx.update(|window, cx| {
                window.clear_text_selection(cx);
                window.blur();
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        let mut state = snapshot();
                        state.saved.provider = provider;
                        state.saved.thread_id = Some(format!("replacement-{provider:?}"));
                        state.saved.items = vec![json!({"id":"answer","type":"agentMessage","text":first,"phase":"final_answer","groveCompleted":true})].into();
                        session.state = state.into();
                        cx.notify();
                    });
                    cx.notify();
                });
            });
        for _ in 0..5 {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            frame(cx);
        }
        let original_id = owner(&view, cx).entity_id();
        let original_extent = view.read_with(cx, |view, _| view.scroll.max_offset());
        replace(&view, cx, &second);
        assert_eq!(
            owner(&view, cx).entity_id(),
            original_id,
            "unselected static replacement keeps paragraph measurement IDs"
        );
        assert_eq!(
            owner(&view, cx).read_with(cx, |owner, _| owner.source().to_string()),
            second
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll.max_offset()),
            original_extent,
            "same-size replacement preserves exact geometry"
        );
        let copy = cx.debug_bounds("codex-copy-response").unwrap();
        cx.simulate_click(copy.center(), gpui::Modifiers::default());
        frame(cx);
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some(second.clone())
        );

        let selected_id = owner(&view, cx).entity_id();
        owner(&view, cx).update(cx, gpui_component::text::TextViewState::select_all);
        frame(cx);
        assert!(cx
            .update(gpui_component::WindowExt::text_selection_snapshot)
            .is_some());
        replace(&view, cx, &format!("{prefix}Gamma"));
        assert_ne!(
            owner(&view, cx).entity_id(),
            selected_id,
            "selected same-size replacement invalidates old owner"
        );
        assert!(
            cx.update(gpui_component::WindowExt::selected_text)
                .is_empty(),
            "old selection must not attach to new source"
        );

        // A different selected owner must also preserve the original
        // replacement identity behavior without clearing its selection.
        cx.update(|_,cx|view.update(cx,|view,cx| {
                view.session.update(cx,|session,cx| {
                    session.state.saved.items.push(json!({"id":"other","type":"userMessage","content":[{"type":"text","text":"Other selection"}]}));
                    cx.notify();
                });
                cx.notify();
            }));
        for _ in 0..4 {
            frame(cx);
        }
        let previous_id = owner(&view, cx).entity_id();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let key = format!(
                    "{}:other",
                    view.session
                        .read(cx)
                        .state
                        .saved
                        .thread_id
                        .as_deref()
                        .unwrap_or_default()
                );
                view.selection_views[&key]
                    .update(cx, gpui_component::text::TextViewState::select_all);
            });
        });
        frame(cx);
        replace(&view, cx, &format!("{prefix}Omega"));
        assert_ne!(
            owner(&view, cx).entity_id(),
            previous_id,
            "another window selection keeps conservative replacement invalidation"
        );
        assert_eq!(
            cx.update(gpui_component::WindowExt::selected_text),
            "Other selection\n"
        );
        cx.update(|window, cx| {
            window.clear_text_selection(cx);
            window.blur();
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items.pop();
                    cx.notify();
                });
                cx.notify();
            });
        });
        for _ in 0..4 {
            frame(cx);
        }

        // A zero-length actual text hit focuses the owner even without a
        // nonempty selection; preserve the original focus invalidation.
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        let viewport = view.read_with(cx, |view, _| view.scroll.bounds());
        let hit = gpui::point(body.left() + gpui::px(12.), viewport.top() + gpui::px(30.));
        cx.simulate_mouse_down(hit, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(hit, gpui::MouseButton::Left, gpui::Modifiers::default());
        frame(cx);
        let focused_owner = owner(&view, cx);
        cx.update(|window, cx| {
            assert!(focused_owner.read(cx).layout_projection_focused(window, cx));
        });
        drop(focused_owner);
        let focused_id = owner(&view, cx).entity_id();
        replace(&view, cx, &format!("{prefix}Delta"));
        assert_ne!(
            owner(&view, cx).entity_id(),
            focused_id,
            "focused replacement invalidates old owner"
        );
        cx.update(|window, cx| {
            window.clear_text_selection(cx);
            window.blur();
        });

        replace(&view, cx, "```rust\nlet value = 1;\n```");
        assert!(
            owner(&view, cx).read_with(cx, |owner, _| owner.supports_layout_projection()),
            "new code measures its own extent before projection"
        );
        let dynamic_id = owner(&view, cx).entity_id();
        replace(&view, cx, "```rust\nlet value = 2;\n```");
        assert_eq!(
            owner(&view, cx).entity_id(),
            dynamic_id,
            "unfocused code owners can reuse measurement identity"
        );
        assert!(owner(&view, cx).read_with(cx, |owner, _| owner.source().contains("value = 2")));
        cx.update(|window, cx| {
            window.clear_text_selection(cx);
            window.blur();
        });

        replace(&view, cx, "AAAAAAAAAAAAAAAAAAAA");
        let plain_id = owner(&view, cx).entity_id();
        replace(&view, cx, "Z");
        assert_eq!(owner(&view, cx).entity_id(), plain_id);
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        let old_glyph_area = body.origin + gpui::point(gpui::px(100.), gpui::px(8.));
        cx.simulate_mouse_down(
            old_glyph_area,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            old_glyph_area,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        frame(cx);
        assert!(
            cx.update(gpui_component::WindowExt::text_selection_snapshot)
                .is_none(),
            "old inline glyph bounds must not select replacement blank space"
        );
        assert_eq!(
            owner(&view, cx).read_with(cx, |owner, _| owner.source().to_string()),
            "Z"
        );
        cx.update(|window, cx| {
            window.clear_text_selection(cx);
            window.blur();
        });
    }
}

#[test]
fn rich_message_index_refreshes_without_transcript_edits() {
    let source = Rc::new(snapshot());
    let before = TranscriptRenderCache::new(source.clone(), Vec::new());
    assert!(!before.rich_messages.contains("plan-message"));
    let mut updated = source.as_ref().clone();
    updated.saved.rich_plans.errors = serde_json::from_value(json!([{
        "message_id": "plan-message", "source": "missing.grove-plan.json",
        "error": "missing"
    }, {
        "message_id": "other-message", "source": "other.grove-plan.json",
        "error": "missing"
    }]))
    .unwrap();
    let after = TranscriptRenderCache::refreshed(Rc::new(updated), Vec::new(), Some(&before));
    assert!(Rc::ptr_eq(&before.prepared, &after.prepared));
    assert_eq!(after.rich_messages.errors["plan-message"], vec![0]);
    assert_eq!(after.rich_messages.errors["other-message"], vec![1]);
    assert!(!before.rich_messages.contains("plan-message"));
    let removed = TranscriptRenderCache::refreshed(source, Vec::new(), Some(&after));
    assert!(!removed.rich_messages.contains("plan-message"));
}

#[gpui::test]
fn transcript_cache_reuses_frames_and_refreshes_notified_session_edits(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.items =
                        vec![json!({"id":"answer", "type":"agentMessage", "text":"one"})].into();
                    cx.notify();
                });
            });
        });
        draw(cx);
        let before = view.read_with(cx, |view, _| view.transcript_cache.clone().unwrap());
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(Rc::ptr_eq(&before, view.transcript_cache.as_ref().unwrap()));
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[0]["text"] = json!("two");
                    session.state.error = Some("new error".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            let after = view.transcript_cache.as_ref().unwrap();
            assert_ne!(before.content, after.content);
            assert_eq!(after.state.saved.items[0]["text"], "two");
            assert_eq!(after.state.error.as_deref(), Some("new error"));
            assert_eq!(after.response_groups[0].copy_text(), "two");
        });
    }
}

#[gpui::test]
fn pending_bubbles_keep_edit_and_fork_actions_on_source_items(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let CodexViewEvent::ForkRequested(index, restore) = event {
                observed.borrow_mut().push((*index, *restore));
            }
        })
    });
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Done;
                session.state.turn_id = None;
                session.state.edit_supported = true;
                session.state.saved.items = vec![
                    json!({"id":"earlier","type":"agentMessage","text":"Earlier"}),
                    json!({"id":"real-input","type":"userMessage","content":[{"text":"Real input"}]}),
                    json!({"id":"real-answer","type":"agentMessage","text":"Answer"}),
                ].into();
                session.state.saved.edit_checkpoints.push(codex::EditCheckpoint {
                    start_item: 1, turn_id: Some("turn".into()), files: None,
                });
            });
            let projected = TranscriptRenderCache::new(Rc::new(view.session.read(cx).state.clone()), vec![
                json!({"id":"local-pending","type":"userMessage","localAfterItem":"earlier","content":[{"text":"Pending"}]})
            ]);
            assert_eq!(projected.state.saved.items[2]["id"], "real-input");
            view.request_edit_item("local-pending", window, cx);
            assert!(view.edit_confirmation.is_none());
            view.request_edit_item("real-input", window, cx);
            assert_eq!(view.edit_confirmation.as_ref().unwrap().index, 1);
            view.fork_from_item("local-pending", true, cx);
            view.fork_from_item("real-input", true, cx);
            view.fork_from_item("real-answer", false, cx);
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.saved.items.push(json!({
                    "id":"streaming-answer","type":"agentMessage","text":"Streaming"
                }));
                session.state.saved.work_periods = vec![
                    codex::WorkPeriod {
                        start_item: 0, end_item: Some(3), duration_ms: None, subagents: Vec::new(),
                    },
                    codex::WorkPeriod {
                        start_item: 3, end_item: None, duration_ms: None, subagents: Vec::new(),
                    },
                ];
            });
            view.fork_from_item("real-input", true, cx);
            view.fork_from_item("real-answer", false, cx);
            view.fork_from_item("streaming-answer", false, cx);
        }));
    cx.run_until_parked();
    assert_eq!(*events.borrow(), [(1, true), (3, false), (3, false)]);
}

#[test]
fn incremental_preparation_preserves_absolute_response_keys_and_lazy_copy() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.saved.thread_id = Some("incremental".into());
        state.saved.items = (0..5).flat_map(|turn| [
                json!({"id":format!("user-{turn}"),"type":"userMessage","text":format!("Input {turn}")}),
                json!({"type":"agentMessage","text":format!("Answer {turn}")}),
                json!({"type":"reasoning"}),
            ]).collect();
        let initial = TranscriptRenderCache::new(Rc::new(state.clone()), vec![]);
        assert!(initial
            .response_groups
            .iter()
            .all(|group| group.copy_text.get().is_none()));
        // Dirty a middle turn, requiring several independently keyed tail groups.
        state.saved.items[4]["text"] = json!("Changed middle response");
        let refreshed =
            TranscriptRenderCache::refreshed(Rc::new(state.clone()), vec![], Some(&initial));
        let fresh = TranscriptRenderCache::new(Rc::new(state), vec![]);
        assert!(Rc::ptr_eq(
            &initial.response_groups[0],
            &refreshed.response_groups[0]
        ));
        assert_eq!(refreshed.response_groups.len(), fresh.response_groups.len());
        for (actual, expected) in refreshed.response_groups.iter().zip(&fresh.response_groups) {
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.source_keys, expected.source_keys);
            assert_eq!(actual.prose, expected.prose);
            assert_eq!(actual.anchor_index, expected.anchor_index);
            assert_eq!(actual.copy_text(), expected.copy_text());
        }
        assert_eq!(refreshed.minimap_entries, fresh.minimap_entries);
        assert_eq!(
            fresh.minimap_entries,
            message_minimap_entries(&fresh.state.saved.items)
        );
        let reference = response_groups(
            &fresh.state.saved.items,
            fresh.state.saved.thread_id.as_deref(),
        );
        for (actual, expected) in fresh.response_groups.iter().zip(&reference) {
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.copy_text(), expected.copy_text());
            assert_eq!(actual.prose, expected.prose);
        }
    }
}

#[gpui::test]
fn minimap_replays_unchanged_frames_and_refreshes_changed_content(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
                let mut state = snapshot(); state.saved.provider = provider;
                state.saved.items = (0..80).flat_map(|turn| [
                    json!({"id":format!("user-{turn}"),"type":"userMessage","text":format!("Input {turn}")}),
                    json!({"id":format!("answer-{turn}"),"type":"agentMessage","text":"Response","phase":"final_answer"}),
                ]).collect();
                session.state = state.into(); cx.notify();
            })));
        draw(cx);
        for _ in 0..4 {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            draw(cx);
        }
        let before = MINIMAP_BUILDS.with(Cell::get);
        for _ in 0..3 {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            draw(cx);
        }
        assert_eq!(
            MINIMAP_BUILDS.with(Cell::get),
            before,
            "unchanged {provider:?} frames must reuse the marker scene"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[159]["text"] = json!("Updated response");
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(MINIMAP_BUILDS.with(Cell::get) > before);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.minimap_view
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .prepared
                    .minimap_entries
                    .last()
                    .unwrap()
                    .response,
                "Updated response"
            );
        });
    }
}

#[gpui::test]
fn transcript_removal_releases_focus_callbacks_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let mut state = snapshot();
                state.saved.provider = provider;
                state.saved.thread_id = Some("focus-thread".into());
                state.saved.items =
                    vec![json!({"id":"old","type":"agentMessage","text":"Old response"})].into();
                let cache = TranscriptRenderCache::new(Rc::new(state.clone()), vec![]);
                view.prune_transcript_controls(&cache);
                view.ensure_control_focus("focus-thread:old:copy", window, cx);
                assert!(view
                    .focus_subscriptions
                    .contains_key("focus-thread:old:copy"));
                state.saved.items.clear();
                let next = TranscriptRenderCache::new(Rc::new(state), vec![]);
                view.prune_transcript_controls(&next);
                assert!(!view.message_focus.contains_key("focus-thread:old:copy"));
                assert!(!view
                    .focus_subscriptions
                    .contains_key("focus-thread:old:copy"));
            });
        });
    }
}

#[gpui::test]
fn double_escape_clears_on_typing_timeout_focus_and_turn_change(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("original".into());
                    cx.notify();
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        cx.simulate_input("draft");
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        cx.simulate_keystrokes("escape");
        cx.executor().advance_clock(STOP_CONFIRMATION_TIMEOUT);
        cx.run_until_parked();
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        cx.simulate_keystrokes("escape");
        cx.update(|window, _| window.blur());
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        cx.update(|window, cx| view.read(cx).input.focus_handle(cx).focus(window, cx));
        draw(cx);
        cx.simulate_keystrokes("escape");
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.turn_id = Some("replacement".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_some()));
        assert!(
            !actions.try_iter().any(|a| matches!(a, Action::Interrupt)),
            "{provider:?}"
        );
        view.update(cx, super::super::CodexView::clear_stop_confirmation);
    }
}

#[gpui::test]
fn tiny_upward_scroll_stays_collapsed_after_viewport_reaches_bottom(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![json!({"id":"long","type":"agentMessage","text":"Earlier response.\n\n".repeat(80)})].into();
                    cx.notify();
                });
            });
        });
    draw(cx);
    draw(cx);
    let scroll = |delta, cx: &mut gpui::VisualTestContext| {
        let bounds = cx.debug_bounds("codex-messages").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        draw(cx);
    };
    // Cross the browse threshold by less than the composer's height change.
    scroll(SPACE_20 + 1.0, cx);
    view.read_with(cx, |view, _| {
        assert!(view.composer_collapsed);
        assert_eq!(
            view.scroll.offset().y,
            -view.scroll.max_offset().y,
            "collapsing the composer clamps the transcript to its new bottom"
        );
    });
    for _ in 0..4 {
        scroll(1.0, cx);
        view.read_with(cx, |view, _| {
                assert!(view.composer_collapsed, "continued upward scrolling must not restore the composer at a layout-created bottom");
            });
    }
    scroll(-1.0, cx);
    view.read_with(cx, |view, _| {
        assert!(
            !view.composer_collapsed,
            "downward scrolling near the bottom restores the composer"
        );
        assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
    });
    scroll(-1.0, cx);
    view.read_with(cx, |view, _| assert!(!view.composer_collapsed));
}

#[gpui::test]
fn streaming_preserves_manual_scroll_and_follows_only_near_bottom(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let stream = |lines, cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![json!({"id":"stream","type":"agentMessage","text":"A streaming paragraph.\n\n".repeat(lines)})].into();
                    cx.notify();
                });
            }));
        draw(cx);
        for _ in 0..120 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            draw(cx);
        }
    };
    stream(40, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
    });
    // A user reading older content must not be pulled down by streaming or incidental renders.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
            cx.notify();
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, gpui::px(-100.));
    });
    stream(60, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, gpui::px(-100.));
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let text = session.state.saved.items[0]["text"]
                    .as_str()
                    .unwrap()
                    .replace('A', "B");
                session.state.saved.items[0]["text"] = json!(text);
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, gpui::px(-100.));
    });
    // Returning to the latest response restores automatic following.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.scroll.scroll_to_bottom();
            cx.notify();
        });
    });
    draw(cx);
    stream(80, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
    });
    // An explicit submission follows even when the user had scrolled upward.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
            view.follow_after_submit = true;
            cx.notify();
        });
    });
    draw(cx);
    let before = view.read_with(cx, |view, _| view.scroll.offset().y);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(16));
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(
            view.scroll.offset().y < before,
            "automatic following must move between frames"
        );
        assert!(
            view.scroll.offset().y > -view.scroll.max_offset().y,
            "automatic following must animate before reaching its endpoint"
        );
    });
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
fn claude_restore_defaults_row_and_later_access_selection_dispatch_with_pointer(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.saved.thread_id = None;
                session.state.permission_modes =
                    ["default", "auto", "acceptEdits", "plan", "dontAsk"]
                        .map(str::to_owned)
                        .to_vec();
                session.state.access = Some("dontAsk".into());
                session.state.resolved_access = Some("dontAsk".into());
                session.state.saved.composer.access = Some("dontAsk".into());
                cx.notify();
            });
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1800.), gpui::px(1430.)));
    draw(cx);
    let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    draw(cx);
    let popup = cx.debug_bounds("codex-options-popup").unwrap();
    let reset_row = cx.debug_bounds("access-choice-__configured").unwrap();
    let reset = reset_row.center();
    assert!(
        reset.y >= popup.top() && reset.y <= popup.bottom(),
        "Restore defaults must be visible without scrolling"
    );
    cx.simulate_mouse_move(reset, None, gpui::Modifiers::default());
    draw(cx);
    cx.simulate_click(reset, gpui::Modifiers::default());
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::ResetConfiguration)));
    assert!(view.read_with(cx, |view, _| view.popup.is_none()));
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    draw(cx);
    let popup = cx.debug_bounds("codex-options-popup").unwrap();
    let first = gpui::point(popup.left() + gpui::px(45.), popup.top() + gpui::px(17.));
    cx.simulate_mouse_move(first, None, gpui::Modifiers::default());
    draw(cx);
    cx.simulate_click(first, gpui::Modifiers::default());
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "default")));
    assert!(view.read_with(cx, |view, _| view.popup.is_none()));
}

#[gpui::test]
fn mixed_agent_controls_share_scroll_budget_and_emit_typed_responses(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let request = ElicitationRequest::parse_codex(
        json!("form"),
        &json!({
            "serverName":"Projects", "message":"Configure project", "requestedSchema": {
                "type":"object", "required":["name","count","enabled","mode","tags"],
                "properties": {
                    "name":{"type":"string","minLength":2},
                    "count":{"type":"integer","minimum":1},
                    "enabled":{"type":"boolean"},
                    "mode":{"type":"string","enum":["boolean","large"]},
                    "tags":{"type":"array","items":{"type":"string","enum":["one","two"]}},
                    "note":{"type":"string"}
                }
            }
        }),
    )
    .unwrap();
    let approval = json!({"id":"approval","method":"item/commandExecution/requestApproval","params":{"command":"pwd","cwd":"/tmp"}});
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Waiting;
                session.state.approvals.push(approval.clone());
                session.state.elicitations.push(request.clone());
                session.state.questions.push(
                    codex::QuestionRequest::from_codex(
                        json!("ask-mixed"),
                        &json!({"questions":[{"id":"question","question":"Anything else?"}]}),
                    )
                    .unwrap(),
                );
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(480.0)));
    draw(cx);
    assert!(cx.debug_bounds("codex-composer").unwrap().bottom() <= gpui::px(480.0));
    assert!(cx.debug_bounds("codex-messages").unwrap().size.height > gpui::px(SPACE_20));
    assert!(
        cx.debug_bounds("agent-controls").unwrap().size.height < gpui::px(QUESTION_PANEL_MAX_H)
    );
    let key = view.read_with(cx, |view, cx| {
        CodexView::elicitation_key(&request, view.session.read(cx).state.interaction_epoch)
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.elicitation_content(&key, &request, cx).is_err());
            let ElicitationMode::Form(form) = &request.mode else {
                unreachable!()
            };
            for (index, field) in form.fields.iter().enumerate() {
                match field.name.as_str() {
                    "name" | "count" => {
                        let input = view.elicitation_drafts[&key].fields[index].input.clone();
                        input.update(cx, |input, cx| {
                            input.set_value(
                                if field.name == "name" { "Grove" } else { "3" },
                                window,
                                cx,
                            );
                        });
                    }
                    "mode" => view.change_elicitation(
                        &key,
                        index,
                        Some("boolean".into()),
                        false,
                        false,
                        cx,
                    ),
                    "tags" => {
                        view.change_elicitation(&key, index, Some("one".into()), true, false, cx);
                        view.change_elicitation(&key, index, Some("two".into()), true, false, cx);
                    }
                    "enabled" => view.change_elicitation(&key, index, None, true, false, cx),
                    _ => {}
                }
            }
            assert!(view.elicitation_content(&key, &request, cx).is_ok());
            view.respond_elicitation(&key, json!("form"), "accept", cx);
            view.respond_elicitation(&key, json!("form"), "accept", cx);
            view.respond_approval(
                &CodexView::approval_key(&approval, view.session.read(cx).state.interaction_epoch),
                json!("approval"),
                "allow_once".into(),
                cx,
            );
        });
    });
    let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
    let sent = actions_at_epoch(&actions, epoch);
    let forms: Vec<_> = sent
        .iter()
        .filter_map(|action| match action {
            Action::RespondElicitation {
                id,
                action,
                content,
            } => Some((id, action, content)),
            _ => None,
        })
        .collect();
    assert_eq!(forms.len(), 1);
    assert_eq!(
        forms[0].2.as_ref().unwrap(),
        &json!({"name":"Grove","count":3,"enabled":true,"mode":"boolean","tags":["one","two"]})
    );
    assert!(sent.iter().any(
        |action| matches!(action, Action::RespondApproval { choice, .. } if choice == "allow_once")
    ));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.approvals.clear();
                session.state.elicitations.clear();
                session.state.questions.clear();
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(
        view.read_with(cx, |view, _| view.elicitation_drafts.is_empty()
            && view.approval_drafts.is_empty())
    );
}

#[gpui::test]
fn native_code_copy_preserves_indentation_and_escape_clears_selection(
    cx: &mut gpui::TestAppContext,
) {
    let (_view, cx, _actions, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"code", "text":"```rust\n    let selected = true;\n```"}));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let body = cx.debug_bounds("codex-message-body-0").unwrap();
    let start = body.origin + gpui::point(gpui::px(14.), gpui::px(24.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-a cmd-c");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    draw(cx);
    let selected = cx.update(gpui_component::WindowExt::selected_text);
    assert!(
        selected.starts_with("    let selected"),
        "code indentation was lost: {selected:?}"
    );
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
        Some(selected)
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx
        .update(gpui_component::WindowExt::selected_text)
        .is_empty());
}

#[gpui::test]
fn selection_reply_source_disappearance_clears_native_selected_text(cx: &mut gpui::TestAppContext) {
    let (_, cx, _, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"removed-source", "text":"selected text disappears"}));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let body = cx.debug_bounds("codex-message-body-0").unwrap();
    let start = body.origin + gpui::point(gpui::px(1.), gpui::px(8.));
    let end = body.origin + gpui::point(gpui::px(110.), gpui::px(8.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        end,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert!(!cx
        .update(gpui_component::WindowExt::selected_text)
        .is_empty());
    updates.send(snapshot()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-selection-reply").is_none());
    assert!(cx
        .update(gpui_component::WindowExt::selected_text)
        .is_empty());
}

#[test]
fn response_groups_split_only_at_users_and_copy_all_prose() {
    let items = vec![
        serde_json::json!({"type":"userMessage", "id":"u", "text":"exclude"}),
        serde_json::json!({"type":"agentMessage", "id":"first", "phase":"commentary", "text":"First", "groveReceivedAt":10}),
        serde_json::json!({"type":"commandExecution", "text":"exclude"}),
        serde_json::json!({"type":"reasoning", "text":"exclude"}),
        serde_json::json!({"type":"agentMessage", "id":"last", "phase":"final_answer", "text":"Last", "groveReceivedAt":20}),
        serde_json::json!({"type":"agentMessage", "id":"empty", "text":"  ", "groveReceivedAt":30}),
        serde_json::json!({"type":"userMessage", "id":"interrupt"}),
        serde_json::json!({"type":"agentMessage", "id":"next", "text":"Next"}),
        serde_json::json!({"type":"plan", "text":"exclude"}),
    ];
    let groups = response_groups(&items, Some("thread"));
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, "thread:response:first");
    assert_eq!(groups[0].copy_text(), "First\n\nLast");
    assert_eq!(groups[0].received_at, Some(20));
    assert_eq!(groups[0].end, 6);
    assert_eq!(groups[1].end, items.len());
    assert_eq!(groups[1].received_at, None);
    assert!(response_groups(&items[2..4], None).is_empty());
    assert!(response_groups(&items[5..6], None).is_empty());
    let mut streaming = items[..5].to_vec();
    let key = response_groups(&streaming, Some("thread"))[0].key.clone();
    streaming.push(serde_json::json!({"type":"agentMessage", "id":"resumed", "text":"Resumed"}));
    let continued = response_groups(&streaming, Some("thread"));
    assert_eq!(continued[0].key, key);
    assert_eq!(
        continued[0].received_at, None,
        "Never borrow an earlier timestamp"
    );
    streaming.truncate(2);
    assert_eq!(
        response_groups(&streaming, Some("thread"))[0].copy_text(),
        "First"
    );
}

#[gpui::test]
fn agent_code_blocks_and_finished_responses_copy_verbatim(cx: &mut gpui::TestAppContext) {
    let (view, cx, _actions, updates) = fixture(cx);
    let text = "Run this:\n\n```sh\n  cargo test --workspace\n```";
    let mut response = snapshot();
    response
            .saved
            .items
            .push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":text, "groveCompleted":true}));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let clipboard = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
    };
    assert!(
        cx.debug_bounds("codex-run-code").is_some(),
        "Finalized shell fences retain a disabled Run control when checkout runtime is unavailable"
    );
    let copy_bounds = cx.debug_bounds("codex-copy-code").unwrap();
    cx.simulate_click(copy_bounds.center(), gpui::Modifiers::default());
    draw(cx);
    assert_eq!(cx.debug_bounds("codex-code-copied"), Some(copy_bounds));
    assert_eq!(
        view.read_with(cx, |view, _| view.copied_code_blocks.len()),
        1
    );
    assert_eq!(
        clipboard(cx).as_deref().map(str::trim_end),
        Some("  cargo test --workspace")
    );
    let response = cx.debug_bounds("codex-copy-response").unwrap().center();
    cx.simulate_click(response, gpui::Modifiers::default());
    assert_eq!(clipboard(cx).as_deref(), Some(text));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS));
    cx.run_until_parked();
    draw(cx);
    assert!(cx.debug_bounds("codex-code-copied").is_none());
    assert_eq!(cx.debug_bounds("codex-copy-code"), Some(copy_bounds));
}

#[gpui::test]
fn snippet_copy_keyboard_shows_feedback(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, updates) = fixture_with_navigation(cx, true);
    let mut state = snapshot();
    state.saved.items.push(json!({"type":"agentMessage", "id":"keyboard", "text":"```rust\n  let value = 1;\n```", "groveCompleted":true}));
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    cx.update(|window, cx| {
        let focus = view
            .read(cx)
            .message_focus
            .iter()
            .find(|(key, _)| key.starts_with("thread-ui:keyboard:code:"))
            .unwrap()
            .1
            .clone();
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        cx.debug_bounds("codex-code-copied").is_some(),
        "Enter must activate focused snippet Copy"
    );
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("  let value = 1;".into())
    );
}

#[gpui::test]
fn snippet_copy_feedback_is_local_and_repeated_click_extends_timeout(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture(cx);
    let mut state = snapshot();
    state.saved.provider = codex::Provider::Claude;
    state.saved.items.push(json!({"type":"agentMessage", "id":"answer", "text":"```rust\nlet first = 1;\n```\n\n```rust\nlet second = 2;\n```", "groveCompleted":true}));
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let first = cx.debug_bounds("codex-copy-code").unwrap();
    cx.simulate_click(first.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    draw(cx);
    assert_eq!(cx.debug_bounds("codex-code-copied"), Some(first));
    let first_payload = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    let other = cx.debug_bounds("codex-copy-code").unwrap();
    assert_ne!(other, first, "other block must still offer Copy");
    cx.simulate_click(other.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.copied_code_blocks.len()),
        2
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
    cx.simulate_click(first.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
    cx.run_until_parked();
    draw(cx);
    assert_eq!(cx.debug_bounds("codex-code-copied"), Some(first));
    assert_eq!(cx.debug_bounds("codex-copy-code"), Some(other));
    assert_eq!(
        view.read_with(cx, |view, _| view.copied_code_blocks.len()),
        1
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
    cx.run_until_parked();
    draw(cx);
    assert!(cx.debug_bounds("codex-code-copied").is_none());
    assert!(view.read_with(cx, |view, _| view.copied_code_blocks.is_empty()));
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        first_payload
    );
}
