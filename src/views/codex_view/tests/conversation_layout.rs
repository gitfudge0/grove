use super::*;

#[gpui::test]
fn shell_execution_requires_a_closed_fence(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    for (source, expected) in [
        ("```sh\necho hi\n```", true),
        ("~~~~bash\nprintf x\n~~~~", true),
        ("```sh\necho hi", false),
        ("```sh\necho hi\n``", false),
        ("echo hi", false),
        ("```sh\necho hi\n    ```", false),
        ("```sh\necho hi\n\u{a0}```", false),
    ] {
        let owner = cx.update(|cx| cx.new(|cx| TextViewState::markdown(source, cx)));
        cx.run_until_parked();
        owner.read_with(cx, |owner, _| {
            let blocks = owner.code_blocks();
            let closed = !blocks.is_empty() && blocks[0].is_closed_fence();
            assert_eq!(closed, expected, "{source}");
        });
    }
}

#[gpui::test]
fn yolo_access_is_readonly_and_cannot_open_access_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    });
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.config_args = provider
                        .launch_options(
                            &provider.agent().launch_args_for_backend(true, true, false),
                        )
                        .unwrap()
                        .0;
                    session.state.access = Some(
                        if provider == codex::Provider::Codex {
                            "full-access"
                        } else {
                            "bypassPermissions"
                        }
                        .into(),
                    );
                    cx.notify();
                });
                view.open_menu(ComposerMenu::Access, window, cx);
                assert!(view.popup.is_none());
                assert!(view.menu.is_none());
            });
        });
        for enabled in [false, true] {
            cx.update(|_, cx| {
                cx.global_mut::<crate::settings::SettingsState>()
                    .store
                    .yolo_enabled = enabled;
            });
            draw(cx);
            assert!(cx.debug_bounds("codex-yolo-access").is_some());
            assert!(cx.debug_bounds("codex-access-summary").is_none());
        }
    }
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. })));
}

#[gpui::test]
fn optimistic_acknowledgment_preserves_browsing_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(850.)));
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.saved.thread_id = Some(format!("ack-{provider:?}"));
                        session.state.saved.items = (0..30).map(|index| json!({
                            "id":format!("history-{index}"), "type":"agentMessage",
                            "phase":"final_answer", "text":format!("Response {index}.\n\nMore completed history to browse.\n\n")
                        })).collect();
                        session.state.status = Status::Idle;
                        cx.notify();
                    });
                    view.record_submission_bubble(777, "Pending local message".into(), Vec::new(), false, false, cx);
                    cx.notify();
                });
            });
        draw(cx);
        let pane = cx.debug_bounds("codex-messages").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: pane.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(500.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        let browsing = view.read_with(cx, |view, _| {
            assert!(view.scroll.offset().y > -view.scroll.max_offset().y + gpui::px(100.));
            view.scroll.offset().y
        });
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items.push(json!({"id":"acknowledged-message", "type":"userMessage", "content":[{"type":"text", "text":"Pending local message"}]}));
                    session.state.accepted_submission = Some(777);
                    cx.notify();
                });
            }));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(
                view.bubble_entrances.is_empty(),
                "{provider:?}: provider echo acknowledges the local bubble"
            );
            assert_eq!(
                view.scroll.offset().y,
                browsing,
                "{provider:?}: unchanged acknowledgement must not pull a browsing reader down"
            );
            assert_eq!(
                view.transcript_cache
                    .as_ref()
                    .unwrap()
                    .state
                    .saved
                    .items
                    .len(),
                31
            );
        });
    }
}

#[gpui::test]
fn history_window_renders_batches_and_reveals_navigation_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(850.)));
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.saved.thread_id = Some(format!("history-window-{provider:?}"));
                        session.state.status = Status::Idle;
                        session.state.saved.items = (0..84).flat_map(|turn| [
                            json!({"id":format!("user-{turn}"),"type":"userMessage","text":format!("Request {turn}")}),
                            json!({"id":format!("answer-{turn}"),"type":"agentMessage","phase":"final_answer","text":format!("Answer {turn}")}),
                        ]).collect();
                        cx.notify();
                    });
                    cx.notify();
                });
            });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.turn_history.start, 158, "{provider:?}");
            assert_eq!(
                view.message_slots.len(),
                10,
                "only 10 user/agent turns get layout slots"
            );
            assert_eq!(view.session.read(cx).state.saved.items.len(), 168);
        });
        assert!(cx.debug_bounds("codex-message-anchor-0").is_none());
        assert!(cx.debug_bounds("codex-message-anchor-158").is_some());
        for (width, rem) in [(1000., 16.), (420., 16.), (420., 22.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(850.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            draw(cx);
            let row = cx.debug_bounds("codex-history-window").unwrap();
            let pane = cx.debug_bounds("codex-messages").unwrap();
            assert!(row.left() >= pane.left() && row.right() <= pane.right());
            let earlier = cx.debug_bounds("codex-history-earlier").unwrap();
            assert!(earlier.left() >= row.left() && earlier.right() <= row.right());
        }
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(850.)));
        cx.update(|window, cx| {
            window.set_rem_size(gpui::px(16.));
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(16.);
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.smooth_follow = false;
                view.set_composer_collapsed(true, cx);
                view.scroll.scroll_to_top_of_item(0);
            });
        });
        draw(cx);
        let earlier = cx.debug_bounds("codex-history-earlier").unwrap();
        cx.simulate_click(earlier.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, _| assert_eq!(view.turn_history.start, 148));
        // Keyboard activation uses the same native disclosure and retains focus.
        cx.simulate_keystrokes("enter");
        draw(cx);
        view.read_with(cx, |view, _| assert_eq!(view.turn_history.start, 138));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.history_latest_focus.focus(window, cx));
        });
        cx.simulate_keystrokes("space");
        draw(cx);
        view.read_with(cx, |view, _| assert_eq!(view.turn_history.start, 158));
        // The minimap retains hidden source messages and reveals before mapping
        // their item identity to the new physical scroll-child index.
        let marker = minimap_bounds(cx, "codex-minimap-message-14").unwrap();
        cx.simulate_click(marker.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.turn_history.start, 8);
            assert!(view.turn_history.expanded);
        });
        let target = cx.debug_bounds("codex-message-anchor-14").unwrap();
        let pane = cx.debug_bounds("codex-messages").unwrap();
        assert!(
            (target.top() - pane.top()).abs() < gpui::px(2.),
            "{provider:?}: target aligned"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session
                        .state
                        .saved
                        .items
                        .push(json!({"id":"new-input","type":"userMessage","text":"Next"}));
                    cx.notify();
                });
                cx.notify();
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.turn_history.start, 8,
                "incoming turn preserves browsing"
            );
        });
    }
}

#[gpui::test]
fn transcript_slot_measurements_release_before_early_return_and_scope_changes(
    cx: &mut gpui::TestAppContext,
) {
    fn frame(cx: &mut gpui::VisualTestContext) {
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    let (view, cx, _, _) = fixture(cx);
    for scenario in ["thread-change", "none-bootstrap", "preview"] {
        let thread = match scenario {
            "none-bootstrap" => None,
            "preview" => Some("child".to_owned()),
            _ => Some("old-thread".to_owned()),
        };
        cx.update(|window, cx| view.update(cx, |view, cx| {
                cx.set_reduce_motion(true);
                view.preview_child = None;
                view.expanded.clear();
                view.input.update(cx, |input,cx| input.set_value("Preserved lifecycle draft",window,cx));
                view.session.update(cx, |session,cx| {
                    let mut state=snapshot();
                    state.saved.thread_id=thread;
                    state.saved.draft="Preserved lifecycle draft".into();
                    state.saved.items=(0..24).map(|i| if i%2==0 { json!({"id":format!("life-{i}"),"type":"userMessage","content":[{"type":"text","text":"Complete retained source"}]}) } else { json!({"id":format!("life-{i}"),"type":"agentMessage","text":"## Original source\n\nComplete original paragraph.","phase":"final_answer","groveCompleted":true}) }).collect();
                    session.state=state.into();
                    cx.notify();
                });
                cx.notify();
            }));
        for _ in 0..5 {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            frame(cx);
        }
        let (message, header) = view.read_with(cx, |view, _| {
            let message = view
                .message_slots
                .values()
                .find(|cache| cache.borrow().retains_source_for_test())
                .expect("warm cache must retain an actual item, not an empty slot");
            let header = view
                .work_header_slots
                .values()
                .find(|cache| cache.borrow().measured_for_test())
                .expect("warm header cache must retain a measured extent");
            (Rc::downgrade(message), Rc::downgrade(header))
        });
        cx.update(|_,cx|view.update(cx,|view,cx| {
                if scenario=="preview" {
                    view.preview_child=Some("child".into());
                    view.session.update(cx,|session,cx| {
                        let mut activity = codex::SubagentActivity::default();
                        activity.items = vec![json!({"id":"child-answer","type":"agentMessage","text":"Child transcript"})].into();
                        session.state.subagent_activity.insert("child".into(), activity);
                        cx.notify();
                    });
                } else {
                    view.session.update(cx,|session,cx| {
                        if scenario=="thread-change" { session.state.saved.thread_id=Some("new-thread".into()); }
                        session.state.saved.items.clear();
                        session.state.status=Status::Starting;
                        cx.notify();
                    });
                }
                cx.notify();
            }));
        for _ in 0..4 {
            frame(cx);
        }
        assert!(
            message.upgrade().is_none(),
            "{scenario}: old item measurement must release its strong item reference"
        );
        assert!(
            header.upgrade().is_none(),
            "{scenario}: old header measurement must release its entry"
        );
        view.read_with(cx, |view, cx| {
            if scenario != "preview" {
                assert!(view.message_slots.is_empty(), "{scenario}");
                assert!(view.work_header_slots.is_empty(), "{scenario}");
            } else {
                assert_eq!(
                    view.transcript_cache.as_ref().unwrap().state.saved.items[0]["id"],
                    "child-answer"
                );
            }
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Preserved lifecycle draft"
            );
            assert_eq!(
                view.session.read(cx).state.saved.draft,
                "Preserved lifecycle draft"
            );
        });
    }
}

#[gpui::test]
fn transcript_cache_sees_local_error_clear_before_observer_delivery(cx: &mut gpui::TestAppContext) {
    let (view, cx, _actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.error = Some("old error".into());
            });
            let _ = view.render(window, cx);
            let before = view.transcript_cache.as_ref().unwrap().source.clone();
            assert_eq!(before.error.as_deref(), Some("old error"));
            view.session.update(cx, |session, cx| {
                // Successful sends do not notify the entity; this mirrors local
                // error clearing before a provider has published a new snapshot.
                session.state.error = None;
                let forked = session.state.shared();
                assert!(!Rc::ptr_eq(&before, &forked));
                let forked_address = Rc::as_ptr(&forked);
                drop(forked);
                session.state.saved.draft = "local draft".into();
                assert_eq!(forked_address, Rc::as_ptr(&session.state.shared()));
                session.send(Action::Draft("local draft".into()), cx);
            });
            let _ = view.render(window, cx);
            let after = view.transcript_cache.as_ref().unwrap();
            assert!(after.state.error.is_none());
            assert_eq!(after.state.saved.draft, "local draft");
            assert_eq!(before.error.as_deref(), Some("old error"));
        });
    });
}

#[test]
fn transcript_cache_tracks_optimistic_acknowledgement_and_thread_changes() {
    let source = Rc::new(snapshot());
    let optimistic = vec![json!({"id":"pending", "type":"userMessage", "text":"next"})];
    let cache = TranscriptRenderCache::new(source.clone(), optimistic.clone());
    assert!(cache.matches(&source, &optimistic));
    assert!(!cache.matches(&source, &[])); // rejected or removed bubble
    assert!(source.saved.items.is_empty());
    assert_eq!(cache.state.saved.items.len(), 1);
    let mut acknowledged = source.as_ref().clone();
    acknowledged.saved.items = optimistic.clone().into();
    let acknowledged = Rc::new(acknowledged);
    assert!(!cache.matches(&acknowledged, &optimistic));
    let acknowledged_cache = TranscriptRenderCache::new(acknowledged.clone(), vec![]);
    assert_eq!(acknowledged_cache.state.saved.items.len(), 1);
    assert_eq!(
        cache.state.saved.items,
        acknowledged_cache.state.saved.items
    );
    // Acknowledgement has a distinct storage revision even when it replaces
    // identical optimistic content; all message bytes remain unchanged.
    assert_ne!(cache.content, acknowledged_cache.content);
    let mut changed_thread = acknowledged.as_ref().clone();
    changed_thread.saved.thread_id = Some("other-thread".into());
    let changed_thread = TranscriptRenderCache::new(Rc::new(changed_thread), vec![]);
    assert_ne!(acknowledged_cache.content, changed_thread.content);
}

#[test]
fn transcript_metadata_refresh_reuses_preparation_without_stale_state() {
    let mut initial = snapshot();
    initial.saved.items = vec![json!({
        "id":"answer", "type":"agentMessage", "text":"one",
        "result":{"opaque":"retained tool bytes"},
    })]
    .into();
    let initial = Rc::new(initial);
    let before = TranscriptRenderCache::new(initial.clone(), vec![]);
    let mut updated = initial.as_ref().clone();
    updated.error = Some("new provider error".into());
    updated.model = Some("new model".into());
    let after = TranscriptRenderCache::refreshed(Rc::new(updated), vec![], Some(&before));
    assert!(Rc::ptr_eq(&before.prepared, &after.prepared));
    assert_eq!(after.state.error.as_deref(), Some("new provider error"));
    assert_eq!(after.state.model.as_deref(), Some("new model"));
    assert_eq!(after.response_groups[0].copy_text(), "one");
    let mut edited = after.state.as_ref().clone();
    edited.saved.items[0]["text"] = json!("two");
    let edited = TranscriptRenderCache::refreshed(Rc::new(edited), vec![], Some(&after));
    assert!(!Rc::ptr_eq(&after.prepared, &edited.prepared));
    assert_ne!(after.content, edited.content);
    assert_eq!(edited.response_groups[0].copy_text(), "two");
    assert_eq!(
        edited.state.saved.items[0]["result"]["opaque"],
        "retained tool bytes"
    );
    assert_eq!(before.state.saved.items[0]["text"], "one");
}

#[test]
#[ignore = "manual incremental transcript preparation comparison"]
fn transcript_incremental_preparation_benchmark() {
    use std::{hint::black_box, time::Instant};
    let mut state = snapshot();
    state.saved.items = (0..2000).map(|index| json!({
            "id":format!("message-{index}"), "type":if index % 2 == 0 { "userMessage" } else { "agentMessage" },
            "text":"Representative transcript content with code and prose. ".repeat(20)
        })).collect();
    let original = Rc::new(state);
    let previous = TranscriptRenderCache::new(original.clone(), vec![]);
    let mut updated = original.as_ref().clone();
    updated.saved.items[1999]["text"] = json!("Appended streaming content");
    let updated = Rc::new(updated);
    for sample in 0..12 {
        let full = || {
            let start = Instant::now();
            for _ in 0..50 {
                black_box(TranscriptRenderCache::new(updated.clone(), vec![]));
            }
            start.elapsed().as_secs_f64() * 1000. / 50.
        };
        let incremental = || {
            let start = Instant::now();
            for _ in 0..50 {
                let cache =
                    TranscriptRenderCache::refreshed(updated.clone(), vec![], Some(&previous));
                assert!(Rc::ptr_eq(
                    &previous.response_groups[0],
                    &cache.response_groups[0]
                ));
                assert_eq!(
                    cache.response_groups.last().unwrap().copy_sources[0].as_ref(),
                    "Appended streaming content"
                );
                black_box(cache);
            }
            start.elapsed().as_secs_f64() * 1000. / 50.
        };
        let (full, incremental) = if sample % 2 == 0 {
            (full(), incremental())
        } else {
            let incremental = incremental();
            (full(), incremental)
        };
        if sample >= 3 {
            println!(
                "TRANSCRIPT_INCREMENTAL {}",
                json!({"sample":sample-3,"items":2000,"full_ms":full,"incremental_ms":incremental,"measurement":"tail-change preparation CPU only"})
            );
        }
    }
}

/// Measures preparation only, excluding GPUI element construction/layout/paint.
/// Run explicitly with --ignored --nocapture; timings are not FPS evidence.
#[test]
#[ignore = "manual transcript preparation benchmark"]
fn transcript_preparation_benchmark() {
    use std::{hint::black_box, time::Instant};
    let mut state = snapshot();
    state.saved.items = (0..2000)
        .map(|index| {
            json!({
                "id":format!("message-{index}"),
                "type":if index % 2 == 0 { "userMessage" } else { "agentMessage" },
                "text":"Representative transcript content with code and prose. ".repeat(20)
            })
        })
        .collect();
    let source = Rc::new(state);
    let cache = Rc::new(TranscriptRenderCache::new(source.clone(), vec![]));
    // Warm up both paths, then alternate their order across nine samples.
    for _ in 0..3 {
        black_box(TranscriptRenderCache::new(
            Rc::new(source.as_ref().clone()),
            vec![],
        ));
        black_box(cache.matches(&source, &[]));
    }
    for sample in 0..9 {
        let baseline = || {
            let start = Instant::now();
            for _ in 0..100 {
                let prepared = TranscriptRenderCache::new(Rc::new(source.as_ref().clone()), vec![]);
                assert_eq!(prepared.content, cache.content);
                assert_eq!(prepared.task_key, cache.task_key);
                assert_eq!(prepared.minimap_entries, cache.minimap_entries);
                assert_eq!(prepared.state.saved.items.len(), 2000);
                black_box(prepared);
            }
            start.elapsed().as_secs_f64() * 1000.0 / 100.0
        };
        let cached = || {
            let start = Instant::now();
            for _ in 0..100 {
                let source = black_box(source.clone());
                assert!(black_box(cache.matches(&source, &[])));
                black_box(cache.clone());
            }
            start.elapsed().as_secs_f64() * 1000.0 / 100.0
        };
        let (old, new) = if sample % 2 == 0 {
            (baseline(), cached())
        } else {
            let new = cached();
            (baseline(), new)
        };
        println!("sample={sample} preparation_ms={old:.6} cached_ms={new:.6}");
    }
}

#[gpui::test]
fn host_arguments_are_rejected_in_every_submission_mode(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for command in [
        "model", "effort", "access", "stop", "queue", "compact", "clear",
    ] {
        for mode in [
            SubmissionMode::Send,
            SubmissionMode::Queue,
            SubmissionMode::Steer,
        ] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let draft = format!("/{command} unexpected argument");
                    view.input
                        .update(cx, |input, cx| input.set_value(draft.clone(), window, cx));
                    view.images = vec!["attached.png".into()];
                    view.submit_mode(mode, window, cx);
                    assert_eq!(view.input.read(cx).value().as_ref(), draft);
                    assert_eq!(view.images, vec!["attached.png"]);
                    assert!(view
                        .session
                        .read(cx)
                        .state
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("does not accept arguments"));
                    assert!(view.menu.is_none());
                });
            });
        }
    }
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Submit { .. }
            | Action::SubmitWithMode { .. }
            | Action::ClearConversation
            | Action::Compact
            | Action::Interrupt
            | Action::ResumeQueue
    )));
}

#[gpui::test]
fn claude_provider_commands_pass_through_with_arguments(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.saved.provider = codex::Provider::Claude;
            });
            view.input.update(cx, |input, cx| {
                input.set_value("/my-skill useful instructions", window, cx);
            });
            view.submit(window, cx);
        });
    });
    assert!(actions.try_iter().any(|action| matches!(action, Action::Submit { text, .. } if text == "/my-skill useful instructions")));
    let mut claude = snapshot();
    claude.saved.provider = codex::Provider::Claude;
    claude.skills = vec![
        json!({"name":"disabled","enabled":false}),
        json!({"name":"enabled"}),
        json!({"name":"clear"}),
    ]
    .into();
    let choices = slash_choices(&claude, "");
    assert!(!choices.iter().any(|(value, _)| value == "/disabled"));
    assert!(choices.iter().any(|(value, _)| value == "/enabled"));
    assert_eq!(
        choices
            .iter()
            .filter(|(value, _)| value == "/clear")
            .count(),
        1
    );
}

#[gpui::test]
fn compact_command_is_hosted_once_and_rejected_inline_while_busy(cx: &mut gpui::TestAppContext) {
    let mut claude = snapshot();
    claude.saved.provider = codex::Provider::Claude;
    claude.skills = vec![
        json!({"name":"compact","description":"Claude command"}),
        json!({"name":"context","description":"Claude command"}),
    ]
    .into();
    let commands = slash_choices(&claude, "comp");
    assert_eq!(
        commands
            .iter()
            .filter(|(value, _)| value == "/compact")
            .count(),
        1
    );
    assert!(commands[0].1.contains("Compact conversation context"));
    let (view, cx, actions, _) = fixture(cx);
    let run = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("/compact", window, cx));
                view.submit(window, cx);
            });
        });
    };
    run(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
        Some(codex::NOTHING_TO_COMPACT.into())
    );
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, _| {
            session.state.saved.items =
                vec![json!({"id":"a","type":"agentMessage","text":"Done"})].into();
            session.state.status = Status::Working;
            session.state.turn_id = Some("turn".into());
        });
    });
    run(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
        Some(codex::COMPACT_BUSY.into())
    );
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Compact)));
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, _| {
            session.state.status = Status::Done;
            session.state.turn_id = None;
        });
    });
    run(cx);
    let sent = actions.try_iter().collect::<Vec<_>>();
    assert!(sent.iter().any(|action| matches!(action, Action::Compact)));
    assert!(!sent.iter().any(|action| matches!(
        action,
        Action::Submit { .. } | Action::SubmitWithMode { .. } | Action::Prompt(_)
    )));
}

#[test]
fn redirect_controls_require_an_active_unblocked_turn_for_both_providers() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.status = Status::Working;
        assert!(!active_turn_redirect_available(&state));
        state.turn_id = Some("active".into());
        if provider == codex::Provider::Claude {
            assert!(!active_turn_redirect_available(&state));
            state.can_redirect = true;
        }
        assert!(active_turn_redirect_available(&state));
        state.compacting = true;
        assert!(!active_turn_redirect_available(&state));
        state.compacting = false;
        state.status = Status::Starting;
        assert!(!active_turn_redirect_available(&state));
    }
}

#[test]
fn spinner_ticks_per_frame_inverts_the_animation_clock() {
    for frame in 0..crate::entities::animation_clock::SPINNER_FRAMES {
        assert_eq!(
            crate::entities::animation_clock::spinner_frame(frame * SPINNER_TICKS_PER_FRAME),
            frame
        );
    }
}

#[gpui::test]
fn provider_warning_shares_transcript_rail_at_narrow_widths(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for width in [1000.0, 560.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.0)));
            cx.update(|_, cx| view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Done;
                    session.state.saved.items = vec![
                        json!({"id":"reply","type":"agentMessage","text":"The configured instructions file could not be loaded. ".repeat(8)}),
                        json!({"id":"warning","type":"providerNotice","title":"Configuration warning","text":"The configured instructions file could not be loaded. ".repeat(8),"severity":"warning"}),
                        json!({"id":"compact","type":codex::COMPACTION_ITEM,"status":"completed","trigger":"auto"}),
                    ].into();
                    cx.notify();
                })));
            draw(cx);
            let response = cx.debug_bounds("codex-message-anchor-0").unwrap();
            let warning = cx.debug_bounds("provider-notice-1").unwrap();
            assert!(cx.debug_bounds("provider-notice-text-1-line-0").is_none());
            let toggle = cx.debug_bounds("provider-notice-toggle-1").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            draw(cx);
            let body = cx.debug_bounds("provider-notice-text-1-line-0").unwrap();
            assert_eq!(
                warning.left(),
                response.left(),
                "{provider:?}, width {width}"
            );
            assert_eq!(
                warning.right(),
                response.right(),
                "{provider:?}, width {width}"
            );
            assert!(body.left() > warning.left());
            assert!(body.right() <= warning.right());
            // TextView's wrapped descendant can exceed its borderless owner’s nominal
            // synthetic height. Native captures confirm the actual content stays clear
            // of the following event; compare painted content to that event directly.
            let next = cx.debug_bounds("codex-compaction-2").unwrap();
            assert!(
                body.bottom() <= next.top(),
                "{provider:?} width {width}: body={body:?} next={next:?}"
            );
            assert!(cx.debug_bounds("codex-error").is_none());
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            draw(cx);
        }
    }
}

#[gpui::test]
fn next_reply_single_candidate_chevrons_are_unavailable(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            seed_next_reply(
                view,
                codex::Provider::Codex,
                "",
                "Check layouts.",
                window,
                cx,
            );
        });
    });
    draw(cx);
    for id in ["next-reply-previous", "next-reply-next"] {
        let center = cx.debug_bounds(id).unwrap().center();
        cx.simulate_click(center, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((1, 1))
        );
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
    }
}

#[gpui::test]
fn next_reply_modifiers_menus_and_ime_retain_priority(cx: &mut gpui::TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (view, cx, _, _) = fixture_with_navigation(cx, true);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(view, provider, "", "Check layouts.", window, cx);
                view.next_reply.resolve(vec![
                    "Check layouts.".into(),
                    "Check keyboard navigation.".into(),
                ]);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-left");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((1, 2))
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.next_reply_focus[0].focus(window, cx));
        });
        draw(cx);
        assert!(
            cx.debug_bounds("next-reply-hint").is_some(),
            "Pager focus retains its controls"
        );
        cx.simulate_keystrokes("space");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((2, 2))
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.next_reply_focus[1].focus(window, cx));
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.next_reply.position()),
            Some((1, 2))
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "",
            "Pager activation must not insert text into the composer"
        );
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| {
            view.read_with(cx, |view, cx| {
                assert!(
                    view.input.read(cx).value().is_empty(),
                    "Shift+Tab must not accept a suggestion or insert a tab"
                );
                assert!(
                    !view.input.focus_handle(cx).is_focused(window),
                    "Shift+Tab retains normal focus traversal"
                );
            });
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.input.focus_handle(cx).focus(window, cx));
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.menu = Some(ComposerMenu::Settings);
                assert!(!view.move_next_reply(false, window, cx));
                assert!(!view.accept_next_reply(window, cx));
                view.menu = None;
                view.input.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "かな", None, window, cx);
                });
                assert!(!view.move_next_reply(false, window, cx));
                assert!(!view.accept_next_reply(window, cx));
                assert_eq!(view.input.read(cx).value().as_ref(), "かな");
            });
        });
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens isolated native X11 next reply fixture"]
fn next_reply_native_capture() {
    use gpui::AppContext as _;
    struct ReplyQaContainer {
        view: Entity<CodexView>,
        width: gpui::Pixels,
    }
    impl Render for ReplyQaContainer {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .w(self.width)
                .max_w_full()
                .h_full()
                .child(self.view.clone())
        }
    }
    assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
    let output = std::path::PathBuf::from(std::env::var("GROVE_NEXT_REPLY_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    // A fresh process per state avoids stale X11 compositor frames after
    // theme/zoom changes in an inactive QA window.
    let selected_case = std::env::var("GROVE_NEXT_REPLY_CAPTURE_CASE")
        .unwrap_or_else(|_| "codex-empty-dark".into());
    assert!(matches!(
        selected_case.as_str(),
        "codex-empty-dark"
            | "claude-selected-light"
            | "codex-narrow-empty"
            | "claude-restored-light"
            | "claude-zoom"
    ));
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
            let mut retained = None;
            let mut retained_container = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(20.), gpui::px(20.)),
                            gpui::size(gpui::px(900.), gpui::px(700.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove next reply QA");
                        let (handle, _actions) = Handle::channel();
                        let (_updates, receive) = mpsc::channel();
                        let session = cx.new(|cx| {
                            CodexSession::from_connection(
                                snapshot(),
                                Connection {
                                    handle,
                                    updates: receive.into(),
                                },
                                cx,
                            )
                        });
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        retained = Some(chat.clone());
                        let container = cx.new(|_| ReplyQaContainer {
                            view: chat,
                            width: gpui::px(600.),
                        });
                        retained_container = Some(container.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(container, window, cx).bordered(false)
                        })
                    },
                )
                .unwrap();
            let view = retained.unwrap();
            let container = retained_container.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (name, provider, draft, proposal, width, light, rem) in [
                    (
                        "codex-empty-dark",
                        codex::Provider::Codex,
                        "",
                        "Check keyboard navigation.",
                        600.,
                        false,
                        16.,
                    ),
                    (
                        "claude-selected-light",
                        codex::Provider::Claude,
                        "",
                        "Check keyboard navigation.",
                        600.,
                        true,
                        16.,
                    ),
                    (
                        "codex-narrow-empty",
                        codex::Provider::Codex,
                        "",
                        "Check keyboard navigation.",
                        300.,
                        false,
                        16.,
                    ),
                    (
                        "claude-restored-light",
                        codex::Provider::Claude,
                        "",
                        "Check keyboard navigation.",
                        360.,
                        true,
                        16.,
                    ),
                    (
                        "claude-zoom",
                        codex::Provider::Claude,
                        "",
                        "Check keyboard navigation.",
                        360.,
                        true,
                        20.,
                    ),
                ] {
                    if name != selected_case {
                        continue;
                    }
                    cx.update_window(window.into(), |_, window, cx| {
                        container.update(cx, |container, cx| {
                            container.width = gpui::px(width);
                            cx.notify();
                        });
                        window.set_rem_size(gpui::px(rem));
                        grove_core::theme::set_by_name(if light {
                            c::DEFAULT_LIGHT_THEME
                        } else {
                            c::DEFAULT_DARK_THEME
                        });
                        c::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        view.update(cx, |view, cx| {
                            seed_next_reply(view, provider, draft, proposal, window, cx);
                            view.next_reply.resolve(vec![
                                proposal.into(),
                                "Check narrow layouts.".into(),
                                "Show the shared fields.".into(),
                            ]);
                            if name == "claude-selected-light" {
                                view.next_reply.cycle(false);
                            }
                            if name == "claude-restored-light" {
                                view.input.update(cx, |input, cx| {
                                    input.set_value("Something else", window, cx);
                                    input.set_value("", window, cx);
                                });
                            }
                            view.session.update(cx, |session, _| {
                                session.state.model = Some(
                                    match provider {
                                        codex::Provider::Codex => "gpt-6.1-sol",
                                        codex::Provider::Claude => "sonnet",
                                    }
                                    .into(),
                                );
                                session.state.effort = Some("low".into());
                                session.state.access = Some("read-only".into());
                            });
                        });
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(800))
                        .await;
                    cx.update_window(window.into(), |_, window, _| window.refresh())
                        .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(200))
                        .await;
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove next reply QA"])
                        .arg(output.join(format!("{name}.png")))
                        .status()
                        .unwrap()
                        .success());
                }
                cx.update_window(window.into(), |_, window, _| {
                    window.blur();
                    window.refresh();
                })
                .unwrap();
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
                drop(view);
                drop(container);
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens isolated native X11 transcript fixture"]
fn provider_failures_native_capture() {
    use gpui::AppContext as _;
    let output = std::path::PathBuf::from(std::env::var("GROVE_FAILURE_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    gpui::point(gpui::px(20.), gpui::px(20.)),
                    gpui::size(gpui::px(900.), gpui::px(750.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove transcript failure QA");
                let mut state = snapshot();
                state.status = Status::Done;
                state.saved.items = ["Context7", "Google Docs", "Google Sheets", "Atlassian Rovo", "Sentry", "Google Calendar", "Google Drive"].iter().enumerate().map(|(i, name)| json!({
                    "id":format!("failure-{i}"), "type":"providerNotice", "severity":"warning",
                    "title":format!("MCP server {name} is unavailable"),
                    "text":"Authenticate this MCP server with the Claude Code CLI, then reconnect this conversation."
                })).collect::<Vec<_>>().into();
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let session = cx.new(|cx| CodexSession::from_connection(state, Connection { handle, updates:receive.into() }, cx));
                let chat = cx.new(|cx| CodexView::new(session, window, cx));
                retained = Some(chat.clone());
                cx.new(|cx| gpui_component::Root::new(chat, window, cx).bordered(false))
            }).unwrap();
            let view = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (name, expanded, light, rem) in [("collapsed-dark", false, false, 16.), ("expanded-dark", true, false, 16.), ("expanded-light-zoom", true, true, 20.)] {
                    cx.update_window(window.into(), |_, window, cx| {
                        grove_core::theme::set_by_name(if light { "grove-light" } else { "grove-dark" });
                        c::sync_component_theme(cx);
                        window.set_rem_size(gpui::px(rem));
                        view.update(cx, |view, cx| {
                            let state = &view.session.read(cx).state;
                            let key = format!("provider-notices:{}:failure-0", state.saved.thread_id.as_deref().unwrap_or_default());
                            if expanded { view.expanded.insert(key); } else { view.expanded.remove(&key); }
                            cx.notify();
                        });

                        window.refresh();
                    }).unwrap();
                    cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                    cx.update_window(window.into(), |_, window, _| window.refresh()).unwrap();
                    cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove transcript failure QA"])
                        .arg(output.join(format!("{name}.png")))
                        .status().unwrap().success());
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
                cx.update(|cx| cx.quit());
            }).detach();
        });
}

#[gpui::test]
fn provider_failures_group_and_expand_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.expanded.clear();
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Done;
                    session.state.saved.items = vec![
                        json!({"id":"a","type":"providerNotice","title":"MCP server Docs is unavailable","text":"Authenticate this MCP server","severity":"warning"}),
                        json!({"id":"b","type":"providerNotice","title":"MCP server Sheets is unavailable","text":"Authenticate this MCP server","severity":"warning"}),
                        json!({"id":"answer","type":"agentMessage","text":"Continue working"}),
                        json!({"id":"c","type":"providerNotice","title":"Separate warning","text":"Distinct diagnostic","severity":"warning"}),
                    ].into();
                    cx.notify();
                });
            }));
        draw(cx);
        assert!(cx.debug_bounds("provider-notice-0").is_some());
        assert!(cx.debug_bounds("provider-notice-1").is_none());
        assert!(cx.debug_bounds("provider-notice-3").is_some());
        assert!(cx.debug_bounds("provider-notice-text-1-line-0").is_none());
        let toggle = cx.debug_bounds("provider-notice-toggle-0").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        // Shared recovery guidance appears once; source diagnostics stay intact.
        assert!(cx.debug_bounds("provider-notice-text-0-line-0").is_none());
        assert!(cx.debug_bounds("provider-notice-text-1-line-0").is_some());
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(cx.debug_bounds("provider-notice-text-1-line-0").is_none());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("provider-notice-text-1-line-0").is_some());
        view.read_with(cx, |view, cx| {
            assert_eq!(view.session.read(cx).state.saved.items.len(), 4);
        });
    }
}

#[test]
fn specialized_claude_disclosures_preserve_results_arguments_and_errors() {
    for kind in ["fileChange", "webSearch", "plan"] {
        let item = serde_json::json!({"type":kind, "arguments":{"query":"requested input"}, "result":{"content":[{"text":"returned result"}]}, "error":{"message":"tool failed"}});
        let detail = tool_detail(&item);
        assert!(detail.contains("requested input"));
        assert!(detail.contains("returned result"));
        assert!(detail.contains("tool failed"));
        assert!(!detail.contains("\"content\""));
    }
}

#[gpui::test]
fn double_escape_respects_menus_idle_and_held_keys_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Idle;
                    session.state.turn_id = None;
                    cx.notify();
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape escape");
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some(format!("menu-{provider:?}"));
                    cx.notify();
                });
                view.open_menu(ComposerMenu::Add, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(view.menu.is_none());
            assert!(view.stop_confirmation.is_none());
        });
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("escape").unwrap(),
            is_held: true,
            prefer_character_input: false,
        });
        assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
    }
}

#[gpui::test]
fn double_escape_preserves_ime_and_fits_narrow_themes(cx: &mut gpui::TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, _| window.activate_window());
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (theme, width, rem) in [("grove-dark", 1000., 16.), ("grove-light", 400., 20.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|window, cx| {
                    grove_core::theme::set_by_name(theme);
                    c::sync_component_theme(cx);
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    window.set_rem_size(gpui::px(rem));
                    cx.set_reduce_motion(true);
                    view.update(cx, |view, cx| {
                        view.clear_stop_confirmation(cx);
                        view.session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.status = Status::Working;
                            session.state.turn_id = Some("ime-turn".into());
                            session.state.saved.items = vec![json!({"id":"grove-plan-escape", "type":"plan", "text":"", "plan":[{"step":"Working", "status":"inProgress"}]})].into();
                            cx.notify();
                        });
                        view.input.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                            input.replace_and_mark_text_in_range(None, "かな", None, window, cx);
                        });
                        view.input.focus_handle(cx).focus(window, cx);
                    });
                });
            draw(cx);
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(view.read_with(cx, |view, _| view.stop_confirmation.is_none()));
            cx.simulate_keystrokes("escape");
            draw(cx);
            let hint = cx.debug_bounds("codex-stop-confirmation").unwrap();
            let composer = cx.debug_bounds("codex-composer").unwrap();
            assert!(hint.left() >= composer.left() && hint.right() <= composer.right());
            assert!(hint.bottom() <= composer.top());
            let tasks = cx.debug_bounds("codex-task-strip-panel").unwrap();
            assert!(
                hint.bottom() <= tasks.top(),
                "stop feedback stays above tasks"
            );
            assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
        }
    }
}

#[gpui::test]
fn custom_radio_choice_supports_arrow_navigation_back_to_named_options(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    let request = codex::QuestionRequest::from_codex(json!("arrow"), &json!({"questions":[{"id":"layout","question":"Which layout?","isOther":true,"options":[{"label":"Compact","preview":"A compact preview"},{"label":"Roomy"}]}]})).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
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
            view.question_drafts[&key].questions[0].choice_focus[2].focus(window, cx);
        });
    });
    cx.simulate_keystrokes("left");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.question_drafts[&key].questions[0].selected,
            HashSet::from([1])
        );
        assert!(!view.question_drafts[&key].questions[0].custom);
    });
}

#[gpui::test]
fn root_preserves_raw_terminal_control_c_and_tab(cx: &mut gpui::TestAppContext) {
    struct RawTerminal {
        focus: FocusHandle,
        keys: Vec<String>,
    }
    impl Render for RawTerminal {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("terminal-key-probe")
                .size_full()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    this.keys.push(event.keystroke.key.clone());
                    cx.stop_propagation();
                }))
        }
    }
    cx.update(gpui_component::init);
    let mut terminal = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| RawTerminal {
            focus: cx.focus_handle(),
            keys: Vec::new(),
        });
        let focus = view.read(cx).focus.clone();
        focus.focus(window, cx);
        terminal = Some(view.clone());
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    draw(cx);
    cx.simulate_keystrokes("ctrl-c tab");
    let keys = terminal.unwrap().read_with(cx, |view, _| view.keys.clone());
    assert_eq!(
        keys,
        ["c", "tab"],
        "Root must not capture raw PTY shortcuts"
    );
}

#[gpui::test]
fn image_only_submission_waits_for_explicit_ack_and_retains_rejected_images(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, updates) = fixture(cx);
    let png = test_image(image::ImageFormat::Png);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.paste_images(
                vec![gpui::Image::from_bytes(gpui::ImageFormat::Png, png)],
                window,
                cx,
            );
            let _ = window;
        });
    });
    draw(cx);
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    draw(cx);
    assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
    assert!(view.read_with(cx, |view, _| view.submitted.is_some()));
    updates.send(snapshot()).unwrap(); // queued idle snapshot before the provider sees the request
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(
        view.read_with(cx, |view, _| view.submitted.is_some()),
        "stale idle must not count as an ack"
    );
    assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
    let image = view.read_with(cx, |view, _| view.images[0].clone());
    let mut rejected = snapshot();
    rejected.error = Some("Provider rejected image".into());
    rejected.rejected_submission = view.read_with(cx, |view, _| view.submitted_ack);
    rejected.saved.draft_images = vec![image.clone()];
    rejected.status = Status::Failed;
    updates.send(rejected.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.images.clone()),
        vec![image]
    );
    assert!(view.read_with(cx, |view, _| view.submitted.is_none()));
    // The immutable clipboard image retains its GPUI image identity across renders.
    let before = view.read_with(cx, |view, _| view.previews.values().next().unwrap().clone());
    draw(cx);
    let after = view.read_with(cx, |view, _| view.previews.values().next().unwrap().clone());
    assert!(Arc::ptr_eq(&before, &after));
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    assert_eq!(
        actions
            .try_iter()
            .filter(|action| matches!(action, Action::Submit { .. }))
            .count(),
        2,
        "rejected image-only draft remains retryable"
    );
    draw(cx);
    let retry_id = view
        .read_with(cx, |view, _| view.submitted_ack)
        .expect("retry stays pending through render");
    updates.send(rejected).unwrap(); // a queued rejection for the previous request
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.submitted_ack),
        Some(retry_id)
    );
    assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
    let mut accepted = snapshot();
    accepted.accepted_submission = Some(retry_id);
    accepted.status = Status::Working;
    updates.send(accepted).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.images.is_empty()));
    assert!(view.read_with(cx, |view, _| view.submitted_ack.is_none()));
}
