use super::*;

#[gpui::test]
fn queue_editor_waits_for_hold_and_restores_unrelated_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("Unrelated draft", window, cx);
            });
            view.images = vec!["draft-image".into()];
            view.session.update(cx, |session, _| {
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: 81,
                    text: "Queued original".into(),
                    images: vec!["queue-image".into()],
                });
            });
            view.begin_queue_edit(81, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "Unrelated draft");
            assert!(!view.queue_editor.as_ref().unwrap().loaded);
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::BeginQueuedEdit(81))));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("Latest draft before grant", window, cx);
            });
            view.images = vec!["latest-draft-image".into()];
        });
    });
    view.update(cx, |view, cx| {
        view.session
            .update(cx, |session, _| session.state.queue_edit = Some(81));
    });
    draw(cx);
    let cancel = cx.debug_bounds("codex-cancel-queue-edit").unwrap();
    let save = cx.debug_bounds("codex-save-queue-edit").unwrap();
    assert_eq!(cancel.size, save.size);
    assert_eq!(cancel.size.width, gpui::px(CHROME_CONTROL_H));
    assert_eq!(cancel.size.height, gpui::px(CHROME_CONTROL_H));
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Queued original");
        assert_eq!(view.images, vec!["queue-image"]);
    });
    cx.update(|window, cx| view.update(cx, |view, cx| view.cancel_queue_edit(window, cx)));
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.input.read(cx).value().as_ref(),
            "Latest draft before grant"
        );
        assert_eq!(view.images, vec!["latest-draft-image"]);
        assert!(view.queue_editor.is_none());
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::CancelQueuedEdit(81))));
}

#[gpui::test]
fn queue_dispatch_race_keeps_original_composer(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("Keep me", window, cx));
            view.session.update(cx, |session, _| {
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: 82,
                    text: "Already sending".into(),
                    images: vec![],
                });
            });
            view.begin_queue_edit(82, cx);
            view.session
                .update(cx, |session, _| session.state.queue_sending.push(82));
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Keep me");
        assert!(view.queue_editor.is_none());
    });
}

#[gpui::test]
fn queued_resume_is_keyboard_accessible_when_idle_and_absent_while_busy(
    cx: &mut gpui::TestAppContext,
) {
    fn tab_reaches_resume(view: &Entity<CodexView>, cx: &mut gpui::VisualTestContext) -> bool {
        cx.update(|window, _| window.blur());
        for _ in 0..32 {
            // Shell routes unconsumed Tab to this production GPUI traversal
            // boundary; the standalone conversation fixture has no Shell.
            cx.update(gpui::Window::focus_next);
            draw(cx);
            if cx.update(|window, cx| {
                view.read_with(cx, |view, _| view.queue_resume_focus.is_focused(window))
            }) {
                return true;
            }
        }
        false
    }
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Idle;
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: 1,
                    text: "Queued request".into(),
                    images: vec![],
                });
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-resume-queue").is_some());
    assert!(
        tab_reaches_resume(&view, cx),
        "Tab traversal reaches the idle Send next control"
    );
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::ResumeQueue)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-resume-queue").is_none());
    cx.simulate_keystrokes("space");
    draw(cx);
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::ResumeQueue)));
    for (status, uncertain) in [(Status::Disconnected, false), (Status::Idle, true)] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = status;
                    session.state.queue_uncertain = if uncertain { vec![1] } else { vec![] };
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(
            !tab_reaches_resume(&view, cx),
            "unavailable queue actions are outside Tab order"
        );
        if let Some(control) = cx.debug_bounds("codex-resume-queue") {
            cx.simulate_click(control.center(), gpui::Modifiers::default());
            draw(cx);
        }
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::ResumeQueue)));
    }
}

#[gpui::test]
fn queue_keyboard_traverses_waiting_rows_and_returns_to_input(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.queued_prompts = (1..=3)
                    .map(|id| codex::QueuedPrompt {
                        id,
                        text: id.to_string(),
                        images: vec![],
                    })
                    .collect();
                session.state.queue_sending.push(1);
            });
            view.input.focus_handle(cx).focus(window, cx);
            assert!(view.move_queue_selection(true, window, cx));
            assert_eq!(view.queue_selected, Some(3));
            assert!(view.move_queue_selection(true, window, cx));
            assert_eq!(view.queue_selected, Some(2));
            assert!(view.move_queue_selection(false, window, cx));
            assert_eq!(view.queue_selected, Some(3));
            assert!(view.move_queue_selection(false, window, cx));
            assert_eq!(view.queue_selected, None);
        });
    });
}

#[gpui::test]
fn queue_exit_rows_expire_and_reduced_motion_removes_immediately(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    view.update(cx, |view, cx| {
        let mut state = snapshot();
        state.queued_prompts.push(codex::QueuedPrompt {
            id: 1,
            text: "One".into(),
            images: vec![],
        });
        view.sync_queue_motion(&state, cx);
        state.queued_prompts.clear();
        view.sync_queue_motion(&state, cx);
        if cx.reduce_motion() {
            assert!(view.queue_motion_rows.is_empty());
        } else {
            assert_eq!(view.queue_motion_rows.len(), 1);
            assert!(view.queue_motion_rows[0].exiting);
            view.queue_motion_rows[0].started = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_millis(MOTION_BASE_MS + 1))
                .unwrap();
            view.sync_queue_motion(&state, cx);
            assert!(view.queue_motion_rows.is_empty());
        }
    });
}

#[gpui::test]
fn steering_bubble_survives_early_echo_until_ack_and_recovers_rejection(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("turn".into());
            });
            view.input
                .update(cx, |input, cx| input.set_value("steer me", window, cx));
            view.submit_mode(SubmissionMode::Steer, window, cx);
            assert_eq!(view.bubble_entrances.len(), 1);
            assert_eq!(
                view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                true
            );
            assert!(view.session.read(cx).state.saved.items.is_empty());
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-0").is_some());
    let id = actions
        .try_iter()
        .find_map(|action| match action {
            Action::SubmitWithMode {
                id,
                mode: SubmissionMode::Steer,
                ..
            } => Some(id),
            _ => None,
        })
        .unwrap();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let mut state = view.session.read(cx).state.clone();
            state
                .saved
                .items
                .push(json!({"id":"echo", "type":"userMessage", "content":[{"text":"steer me"}]}));
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            let bubble = view.bubble_entrances[0].optimistic.as_ref().unwrap();
            assert_eq!(bubble["id"], "echo");
            assert_eq!(bubble["localSteering"], true);
            state.accepted_submission = Some(id);
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert!(view.bubble_entrances.is_empty());
            view.record_submission_bubble(id + 1, "retry".into(), vec![], true, false, cx);
            state.rejected_submission = Some(id + 1);
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert!(view.bubble_entrances.is_empty());
        });
    });
}

#[gpui::test]
fn queued_steering_projects_once_and_keeps_echo_gap_visible(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("turn".into());
                session.state.accepted_submission = Some(42);
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: 42,
                    text: "queued steer".into(),
                    images: vec![],
                });
            });
            assert!(view.queue_action(Action::SteerQueued(42), cx));
            let mut state = view.session.read(cx).state.clone();
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert_eq!(
                view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                true
            );
            state.queued_prompts.clear();
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert_eq!(
                view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                false
            );
            state.saved.items.push(
                json!({"id":"echo", "type":"userMessage", "content":[{"text":"queued steer"}]}),
            );
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert!(view.bubble_entrances.is_empty());
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::SteerQueued(42))));
}

#[gpui::test]
fn clear_in_queue_and_steer_modes_preserves_edits_after_dispatch(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("/clear", window, cx));
                view.images = vec!["old.png".into()];
                view.submit_mode(mode, window, cx);
                // New composer edits must survive an asynchronous provider reset.
                view.input
                    .update(cx, |input, cx| input.set_value("new draft", window, cx));
                view.images = vec!["new.png".into()];
                view.session.update(cx, |session, cx| {
                    session.state.cleared_conversations += 1;
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "new draft");
            assert_eq!(view.images, vec!["new.png"]);
            assert!(view.pending_clear.is_none());
        });
    }
    let sent = actions.try_iter().collect::<Vec<_>>();
    assert_eq!(
        sent.iter()
            .filter(|action| matches!(action, Action::ClearConversation))
            .count(),
        2
    );
    assert!(!sent.iter().any(|action| matches!(
        action,
        Action::Submit { .. } | Action::SubmitWithMode { .. }
    )));
}

#[gpui::test]
fn queue_entry_routes_host_commands_and_reports_unavailable_actions(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            for (draft, error) in [
                ("/stop", "There is no active turn"),
                ("/queue", "There are no queued prompts"),
            ] {
                view.input
                    .update(cx, |input, cx| input.set_value(draft, window, cx));
                view.submit_mode(SubmissionMode::Queue, window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), draft);
                assert!(view
                    .session
                    .read(cx)
                    .state
                    .error
                    .as_deref()
                    .unwrap()
                    .contains(error));
            }
            view.input
                .update(cx, |input, cx| input.set_value("/access", window, cx));
            view.submit_mode(SubmissionMode::Queue, window, cx);
            assert!(view.menu == Some(ComposerMenu::Access));
            assert!(view.input.read(cx).value().is_empty());
        });
    });
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Submit { .. } | Action::SubmitWithMode { .. }
    )));
}

#[gpui::test]
fn enter_during_compaction_queues_and_waits_for_acceptance(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Done;
                    session.state.compacting = true;
                    cx.notify();
                });
                view.input
                    .update(cx, |input, cx| input.set_value("after compact", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        let id = view.read_with(cx, |view, _| view.submitted_ack.unwrap());
        assert!(actions.try_iter().any(|action| matches!(action, Action::SubmitWithMode {id:sent, mode:SubmissionMode::Queue, ..} if sent==id)));
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
            "after compact"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.accept_submission(id);
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id,
                        text: "after compact".into(),
                        images: vec![],
                    });
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
        assert!(cx.debug_bounds("codex-queued-prompts").is_some());
    }
}

#[gpui::test]
fn claude_active_turn_redirect_dispatches_and_preserves_draft_until_ack(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.status = Status::Working;
                session.state.turn_id = Some("active".into());
                session.state.can_redirect = true;
                cx.notify();
            });
            view.input.update(cx, |input, cx| {
                input.set_value("change direction", window, cx);
            });
            view.submit_mode(SubmissionMode::Steer, window, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "change direction");
        });
    });
    assert!(actions.try_iter().any(|action| matches!(action, Action::SubmitWithMode { mode: SubmissionMode::Steer, text, .. } if text == "change direction")));
}

#[gpui::test]
fn unavailable_redirect_keeps_draft_and_enter_queues(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.status = Status::Working;
                session.state.turn_id = None;
                cx.notify();
            });
            view.input
                .update(cx, |input, cx| input.set_value("follow up", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("secondary-enter");
    draw(cx);
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode { .. } | Action::Submit { .. }
    )));
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
        "follow up"
    );
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode {
            mode: SubmissionMode::Queue,
            ..
        }
    )));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode { .. } | Action::Submit { .. }
    )));
}

#[gpui::test]
fn pending_primary_retains_stop_command_and_stop_command_is_deduplicated(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("command-turn".into());
            });
            view.input
                .update(cx, |input, cx| input.set_value("/stop", window, cx));
            view.submitted = Some("awaiting acknowledgement".into());
            view.submit(window, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "/stop");
        });
    });
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Interrupt)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.submitted = None;
            view.submit(window, cx);
            view.input
                .update(cx, |input, cx| input.set_value("/stop", window, cx));
            view.submit(window, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "/stop");
        });
    });
    assert_eq!(
        actions
            .try_iter()
            .filter(|action| matches!(action, Action::Interrupt))
            .count(),
        1
    );
}

#[gpui::test]
fn composer_replaces_send_with_stop_until_turn_finishes(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    draw(cx);
    assert!(cx.debug_bounds("codex-send").is_some());
    assert!(cx.debug_bounds("codex-stop").is_none());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("Start work", window, cx));
            view.submit(window, cx);
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.turn_id = None;
                cx.notify();
            });
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { text, .. } if text == "Start work")));
    draw(cx);
    assert!(cx.debug_bounds("codex-send").is_none());
    assert!(cx.debug_bounds("codex-steer").is_none());
    let stop = cx.debug_bounds("codex-stop").unwrap();
    cx.simulate_click(stop.center(), gpui::Modifiers::default());
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Interrupt)));
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.state.turn_id = Some("active-turn".into());
            cx.notify();
        });
    });
    draw(cx);
    let stop = cx.debug_bounds("codex-stop").unwrap();
    cx.simulate_click(stop.center(), gpui::Modifiers::default());
    cx.simulate_click(stop.center(), gpui::Modifiers::default());
    assert_eq!(
        actions
            .try_iter()
            .filter(|action| matches!(action, Action::Interrupt))
            .count(),
        1
    );
    draw(cx);
    assert!(cx.debug_bounds("codex-send").is_none());
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.state.status = Status::Done;
            session.state.turn_id = None;
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-send").is_some());
    assert!(cx.debug_bounds("codex-stop").is_none());
    assert!(cx.debug_bounds("codex-steer").is_none());
}

#[gpui::test]
fn double_escape_stops_once_and_preserves_drafts_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.thread_id = Some(format!("escape-{provider:?}"));
                    session.state.status = Status::Working;
                    session.state.turn_id = Some(format!("turn-{provider:?}"));
                    cx.notify();
                });
                view.input
                    .update(cx, |input, cx| input.set_value("keep my draft", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        actions.try_iter().for_each(drop);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(
            cx.debug_bounds("codex-stop-confirmation").is_some(),
            "{provider:?}"
        );
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("escape").unwrap(),
            is_held: true,
            prefer_character_input: false,
        });
        assert!(!actions.try_iter().any(|a| matches!(a, Action::Interrupt)));
        // The second press works from a toolbar control within the same pane.
        cx.update(|window, cx| {
            let focus = view.read(cx).option_focus[3].clone();
            focus.focus(window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes("escape escape");
        draw(cx);
        assert_eq!(
            actions
                .try_iter()
                .filter(|a| matches!(a, Action::Interrupt))
                .count(),
            1,
            "{provider:?}"
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "keep my draft");
            assert!(view.stop_confirmation.is_none());
            assert!(view.interrupt_requested_for_turn.is_some());
        });
        // Acknowledgement removes stopping feedback, without fabricating success early.
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Interrupted;
                session.state.turn_id = None;
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-stop-confirmation").is_none());
    }
}

#[gpui::test]
fn stopping_is_independent_of_pending_draft_and_guards_repeat_actions(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("active-turn".into());
                cx.notify();
            });
            view.input
                .update(cx, |input, cx| input.set_value("follow up", window, cx));
            view.submit_mode(SubmissionMode::Queue, window, cx);
            view.interrupt(cx);
            view.interrupt(cx);
        });
    });
    let sent = actions.try_iter().collect::<Vec<_>>();
    assert_eq!(
        sent.iter()
            .filter(|action| matches!(action, Action::Interrupt))
            .count(),
        1
    );
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.submitted = None;
            view.submitted_ack = None;
            view.submit_mode(SubmissionMode::Queue, window, cx);
            view.submit_mode(SubmissionMode::Steer, window, cx);
        });
    });
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode { .. } | Action::Submit { .. }
    )));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Done;
                session.state.turn_id = None;
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.interrupt_requested_for_turn.is_none()));
}

#[gpui::test]
fn queueing_with_enter_while_browsing_returns_to_latest(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("active-turn".into());
                session.state.saved.items = vec![
                    json!({"id":"history", "type":"agentMessage", "text":"Earlier content.\n\n".repeat(80)}),
                    json!({"id":"grove-plan-t1", "type":"plan", "text":"", "explanation":"Keep working.", "plan":[
                        {"step":"Inspect","status":"completed"},
                        {"step":"Implement","status":"inProgress"}]}),
                    json!({"id":"latest", "type":"agentMessage", "text":"Latest content.\n\n".repeat(30)}),
                ].into();
                cx.notify();
            });
        }));
    draw(cx);
    for _ in 0..120 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        draw(cx);
    }
    assert!(cx.debug_bounds("codex-task-strip").is_some());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.scroll.max_offset().y > gpui::px(100.));
            view.scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
            view.input
                .update(cx, |input, cx| input.set_value("follow up", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    let queue_id = actions
        .try_iter()
        .find_map(|action| match action {
            Action::SubmitWithMode {
                id,
                text,
                mode: SubmissionMode::Queue,
                ..
            } if text == "follow up" => Some(id),
            _ => None,
        })
        .expect("Enter should queue the message");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.accepted_submission = Some(queue_id);
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: queue_id,
                    text: "follow up".into(),
                    images: Vec::new(),
                });
                cx.notify();
            });
        });
    });
    for _ in 0..120 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        draw(cx);
    }
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
    });
    let latest = cx.debug_bounds("codex-message-anchor-2").unwrap();
    let transcript = cx.debug_bounds("codex-messages").unwrap();
    assert!(latest.bottom() <= transcript.bottom());
}

#[gpui::test]
fn enqueue_does_not_clear_composer_and_provider_rejection_retains_draft(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, updates) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("keep my prompt", window, cx);
            });
            view.submit(window, cx);
        });
    });
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
        "keep my prompt"
    );
    assert!(actions
        .try_iter()
        .any(|action| matches!(action,Action::Submit{text,..}if text=="keep my prompt")));
    let mut failed = snapshot();
    failed.status = Status::Failed;
    failed.saved.draft = "keep my prompt".into();
    failed.error = Some("Provider rejected prompt".into());
    failed.rejected_submission = view.read_with(cx, |view, _| view.submitted_ack);
    updates.send(failed).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
        "keep my prompt"
    );
    assert!(cx.debug_bounds("codex-message-row-0").is_none());
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-0").is_some());
    assert!(cx.debug_bounds("codex-message-row-1").is_none());
}

#[gpui::test]
fn oversized_prompt_is_rejected_before_enqueue_and_keeps_input(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    let text = "x".repeat(512 * 1024 + 1);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
            view.submit(window, cx);
            assert!(view.session.read(cx).state.error.is_some());
            assert_eq!(view.input.read(cx).value().len(), text.len());
        });
    });
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Prompt(_) | Action::PromptWithImages { .. } | Action::Submit { .. }
    )));
}

#[gpui::test]
fn streaming_updates_do_not_duplicate_queued_text(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let state = cx.new(|cx| gpui_component::text::TextViewState::markdown("first", cx));
    state.update(cx, |state, cx| {
        state.set_text_incremental("first second", cx);
        state.set_text_incremental("first second", cx);
        state.set_text_incremental("first second third", cx);
        state.set_text_incremental("first second third", cx);
    });
    cx.run_until_parked();
    assert_eq!(
        state.read_with(cx, |state, _| state.source().to_string()),
        "first second third"
    );
}

#[gpui::test]
fn coalesced_acceptance_unlocks_queued_composer(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _actions, updates) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .update(cx, |session, _| session.state.saved.provider = provider);
                view.input
                    .update(cx, |input, cx| input.set_value("queued reply", window, cx));
                view.submit_mode(SubmissionMode::Queue, window, cx);
            });
        });
        let id = view.read_with(cx, |view, _| view.submitted_ack).unwrap();
        let mut accepted = snapshot();
        accepted.saved.provider = provider;
        accepted.accept_submission(id);
        updates.send(accepted.clone()).unwrap();
        // A different turn/start response wins the poll's latest snapshot.
        accepted.accept_submission(id.wrapping_add(1));
        updates.send(accepted).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.submitted.is_none()));
        assert!(view.read_with(cx, |view, _| view.submitted_ack.is_none()));
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
    }
}
