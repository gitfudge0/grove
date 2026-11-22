use super::*;

#[test]
fn grove_workflow_guidance_preserves_custom_prompts_on_start_and_resume() {
    for resume in [false, true] {
        for custom in [
            vec!["--append-system-prompt".into(), "Custom guidance".into()],
            vec!["--append-system-prompt=Custom guidance".into()],
            Vec::new(),
        ] {
            let mut actor = actor();
            actor.state.saved.thread_id = resume.then(|| "existing-session".into());
            let mut args = custom.clone();
            args.extend(["--system-prompt".into(), "User system prompt".into()]);
            let command = launch_command(&args, &actor.state.saved).unwrap();
            assert!(command.get_envs().any(|(key, value)| {
                key == "CLAUDE_CODE_ENABLE_TODO_TOOLS" && value == Some("1".as_ref())
            }));
            let argv = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let appended = argv
                .windows(2)
                .filter(|pair| pair[0] == "--append-system-prompt")
                .collect::<Vec<_>>();
            assert_eq!(appended.len(), 1);
            assert!(appended[0][1].contains("You are running inside Grove."));
            assert!(appended[0][1]
                .contains("even when a skill prescribes a different format or handoff"));
            assert!(appended[0][1].contains("Markdown link to the .grove-plan.json artifact"));
            assert!(appended[0][1].contains("Grove's Review response and Send response controls"));
            assert!(appended[0][1].contains("does not override explicit user preferences"));
            assert!(appended[0][1].contains("TodoWrite or TaskCreate/TaskUpdate"));
            assert!(appended[0][1].contains("select:TaskCreate,TaskUpdate"));
            assert!(appended[0][1].contains(crate::tasks::CLAUDE_GUIDANCE));
            assert!(!appended[0][1].contains(crate::tasks::CODEX_GUIDANCE));
            assert!(appended[0][1].contains("<!-- grove:integrated-report -->"));
            assert!(appended[0][1].contains(crate::reports::REPORT_GUIDANCE));
            assert!(appended[0][1].contains(crate::describe_skill::SKILL));
            assert!(appended[0][1].contains(crate::review_skill::SKILL));
            assert!(appended[0][1].ends_with(crate::rich_plan::GUIDANCE));
            if !custom.is_empty() {
                assert!(appended[0][1].starts_with("Custom guidance\n\n"));
            }
            assert!(argv
                .windows(2)
                .any(|pair| pair == ["--system-prompt", "User system prompt"]));
            assert_eq!(argv.iter().any(|arg| arg == "--resume"), resume);
        }
    }
}

#[test]
fn skill_metadata_refresh_preserves_conversation_and_draft() {
    let mut actor = actor();
    actor.state.saved.thread_id = Some("retained-thread".into());
    actor.state.saved.draft = "unsent draft".into();
    actor.state.status = Status::Working;
    let skills = vec![json!({"name":"newly-installed","path":"/skills/new/SKILL.md"})];
    actor.action(Action::UpdateSkills(skills.clone())).unwrap();
    assert_eq!(actor.state.skills, skills);
    let models = vec![json!({"model":"new-catalog-model", "supportsFastMode":true})];
    actor.action(Action::UpdateModels(models.clone())).unwrap();
    assert_eq!(actor.state.models, models);
    assert_eq!(
        actor.state.saved.thread_id.as_deref(),
        Some("retained-thread")
    );
    assert_eq!(actor.state.saved.draft, "unsent draft");
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.process.is_none());
}

#[test]
fn reference_quote_only_interrupt_before_echo_retains_raw_draft_for_retry() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    let reference = provider::ReferenceDraft {
        id: "quote-only".into(),
        thread_id: None,
        sources: vec![provider::ReferenceSource {
            message_id: "source".into(),
            role: "assistant".into(),
        }],
        text: "literal quote".into(),
    };
    actor.state.saved.draft_reference = Some(reference.clone());
    let canonical = reference.serialize("");
    actor.submit(91, canonical.clone(), vec![]).unwrap();
    let sent = read(&mut reader);
    assert_eq!(sent["message"]["content"][0]["text"], canonical);
    actor.interrupting = true;
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.rejected_submission, Some(91));
    assert!(actor.state.saved.draft.is_empty());
    assert_eq!(actor.state.saved.draft_reference, Some(reference.clone()));
    actor
        .submit(92, reference.serialize(&actor.state.saved.draft), vec![])
        .unwrap();
    let retry = read(&mut reader);
    assert_eq!(retry["message"]["content"][0]["text"], canonical);
}

#[test]
fn reference_claude_dispatch_literal_once_and_acknowledgement_owns_identity() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    let reference = provider::ReferenceDraft {
        id: "old".into(),
        thread_id: None,
        sources: vec![provider::ReferenceSource {
            message_id: "source".into(),
            role: "assistant".into(),
        }],
        text: "/compact $skill\n<User reply:>\n```rust\n```".into(),
    };
    actor.state.saved.draft_reference = Some(reference.clone());
    let canonical = reference.serialize("");
    actor
        .action(Action::Submit {
            id: 91,
            text: canonical.clone(),
            images: vec![],
        })
        .unwrap();
    let sent = read(&mut reader);
    assert_eq!(sent["message"]["content"][0]["text"], canonical);
    let mut replacement = reference.clone();
    replacement.id = "new".into();
    actor
        .action(Action::DraftReference(Some(replacement.clone())))
        .unwrap();
    actor.acknowledge_input(&sent["uuid"]);
    assert_eq!(actor.state.saved.draft_reference, Some(replacement.clone()));
    assert_eq!(
        actor.state.saved.items.last().unwrap()["content"][0]["text"],
        canonical
    );
    actor.state.status = Status::Idle;
    actor
        .action(Action::Submit {
            id: 92,
            text: replacement.serialize(""),
            images: vec![],
        })
        .unwrap();
    let sent = read(&mut reader);
    actor.acknowledge_input(&sent["uuid"]);
    assert!(actor.state.saved.draft_reference.is_none());
}

#[test]
fn inline_prompt_echo_preserves_identical_independent_composer() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.saved.draft = "same text".into();
    actor
        .action(Action::PromptInline {
            text: "same text".into(),
            images: vec![],
        })
        .unwrap();
    let sent = read(&mut reader);
    actor.acknowledge_input(&sent["uuid"]);
    assert_eq!(actor.state.saved.draft, "same text");
}

#[test]
fn submission_receipt_uses_acknowledged_uuid_for_repeated_inputs() {
    let mut actor = actor();
    for (submission, uuid) in [(41, "first-input"), (42, "second-input")] {
        actor.input_uuid = Some(uuid.into());
        actor.in_flight = Some(QueuedPrompt {
            id: submission,
            text: "same input".into(),
            images: vec![],
        });
        actor.acknowledge_input(&json!("foreign-input"));
        assert_eq!(actor.state.submission_item_id(submission), None);
        actor.acknowledge_input(&json!(uuid));
        assert_eq!(actor.state.submission_item_id(submission), Some(uuid));
    }
    assert_eq!(actor.state.submission_item_id(41), Some("first-input"));
    assert_eq!(actor.state.submission_item_id(42), Some("second-input"));
    assert_eq!(actor.state.saved.items.len(), 2);
}

#[test]
fn claude_steer_joins_the_running_turn_at_a_tool_boundary() {
    // Sequence A: the echo arrives inside the same turn; one result ends it.
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    let steer = steered_turn(&mut reader, &mut actor);
    actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"done"}]}})).unwrap();
    actor
        .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
        .unwrap();
    assert!(steer_acknowledged(&actor));
    assert!(actor.state.status.busy());
    assert_steer_turn_finishes(&mut actor);
}

#[test]
fn claude_steer_after_the_last_tool_runs_as_a_follow_up_turn() {
    // Sequence B: the turn completes before the echo, then the steer runs.
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.tracks_session_state = true;
    actor.session_idle = false;
    let steer = steered_turn(&mut reader, &mut actor);
    actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"done"}]}})).unwrap();
    actor.message(json!({"type":"assistant","message":{"id":"m1b","content":[{"type":"text","text":"Built."}]}})).unwrap();
    actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0,"result_index":0})).unwrap();
    assert!(actor.state.status.busy());
    assert!(actor.state.turn_id.is_some());
    assert!(actor.pending_completion.is_none());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert!(actor.state.status.busy());
    actor
        .message(json!({"type":"system","subtype":"init","model":"claude"}))
        .unwrap();
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
        .unwrap();
    actor
        .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
        .unwrap();
    assert!(steer_acknowledged(&actor));
    actor.message(json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Switched."}]}})).unwrap();
    actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0,"result_index":1,"user_message_uuids":[steer["uuid"]]})).unwrap();
    assert!(actor.state.status.busy());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Done);
    assert!(actor.state.error.is_none());
    assert!(actor.state.rejected_submission.is_none());
}

#[test]
fn claude_aborted_results_before_a_steer_echo_keep_the_turn_alive() {
    // Sequences C and D: an aborted result precedes the steer's echo.
    for (reason, subtype, is_error) in [
        ("aborted_tools", "success", false),
        ("aborted_streaming", "error_during_execution", true),
    ] {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        let steer = steered_turn(&mut reader, &mut actor);
        actor.message(json!({"type":"result","subtype":subtype,"terminal_reason":reason,"is_error":is_error,"queued_turn_count":0})).unwrap();
        assert!(actor.state.status.busy(), "{reason}");
        assert!(actor.state.turn_id.is_some());
        assert!(actor.state.error.is_none());
        assert!(actor.state.rejected_submission.is_none());
        let tool = actor
            .state
            .saved
            .items
            .iter()
            .find(|item| item["id"] == "bash")
            .unwrap();
        assert_eq!(tool["status"], "interrupted");
        actor
            .message(json!({"type":"user","uuid":steer["uuid"],"message":{"role":"user","content":"use the other branch"}}))
            .unwrap();
        assert!(steer_acknowledged(&actor));
        assert_steer_turn_finishes(&mut actor);
    }
}

#[test]
fn redirected_queue_locks_until_echo_and_preserves_independent_draft() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("old".into());
    actor.state.saved.draft = "independent".into();
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 12,
        text: "redirect".into(),
        images: vec![],
    });
    actor.action(Action::SteerQueued(12)).unwrap();
    let sent = read(&mut reader);
    assert!(actor.action(Action::RemoveQueued(12)).is_err());
    assert_eq!(actor.state.queue_steering, vec![12]);
    actor.acknowledge_input(&sent["uuid"]);
    assert!(actor.state.queue_steering.is_empty());
    assert!(actor.state.queued_prompts.is_empty());
    assert_eq!(actor.state.saved.draft, "independent");
}

#[test]
fn deferred_queue_copy_is_locked_before_reconfiguration() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.reconfigure = true;
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 41,
        text: "original".into(),
        images: Vec::new(),
    });
    actor.submit(41, "original".into(), Vec::new()).unwrap();
    let _ = read(&mut reader);
    assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
    assert_eq!(actor.state.queue_sending, vec![41]);
    assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
    assert!(actor.action(Action::RemoveQueued(41)).is_err());
    assert_eq!(actor.state.queued_prompts[0].text, "original");
}

#[test]
fn queued_ack_preserves_independent_same_text_composer() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 41,
        text: "repeat".into(),
        images: Vec::new(),
    });
    actor.submit(41, "repeat".into(), Vec::new()).unwrap();
    let sent = read(&mut reader);
    actor.state.saved.draft = "repeat".into();
    actor.acknowledge_input(&sent["uuid"]);
    assert_eq!(actor.state.saved.draft, "repeat");
    assert!(actor.state.queued_prompts.is_empty());
}

#[test]
fn uncertain_queue_dismissal_keeps_next_paused() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.queued_prompts = vec![
        QueuedPrompt {
            id: 41,
            text: "unknown".into(),
            images: Vec::new(),
        },
        QueuedPrompt {
            id: 42,
            text: "next".into(),
            images: Vec::new(),
        },
    ];
    actor.submit(41, "unknown".into(), Vec::new()).unwrap();
    let _ = read(&mut reader);
    actor.fail("disconnected before echo".into());
    assert_eq!(actor.state.queue_uncertain, vec![41]);
    assert!(actor.action(Action::ResumeQueue).is_err());
    actor.action(Action::DismissUncertainQueued(41)).unwrap();
    assert!(!actor.queue_ready);
    assert_eq!(actor.state.queued_prompts[0].id, 42);
}

#[test]
fn steer_rejection_preserves_draft() {
    let mut actor = actor();
    actor.state.saved.draft = "keep".into();
    assert!(actor
        .action(Action::SubmitWithMode {
            id: 7,
            text: "steer".into(),
            images: Vec::new(),
            mode: SubmissionMode::Steer
        })
        .is_err());
    assert_eq!(actor.state.rejected_submission, Some(7));
    assert_eq!(actor.state.saved.draft, "keep");
}

#[test]
fn activity_reply_rejects_a_changed_turn_without_queuing() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("new-turn".into());
    assert!(actor
        .action(Action::ActivityPrompt {
            id: 53,
            text: "correct this".into(),
            mode: SubmissionMode::Steer,
            expected_turn: Some("old-turn".into())
        })
        .is_err());
    assert_eq!(actor.state.rejected_submission, Some(53));
    assert!(actor.state.queued_prompts.is_empty());
    assert!(actor
        .action(Action::ActivityPrompt {
            id: 54,
            text: "follow-up".into(),
            mode: SubmissionMode::Send,
            expected_turn: None
        })
        .is_err());
    assert!(actor.state.queued_prompts.is_empty());
}

#[test]
fn project_prompt_preserves_composer_and_uses_native_queue() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("busy".into());
    actor.state.saved.draft = "same text".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .action(Action::ProjectPrompt {
            id: 41,
            text: "same text".into(),
        })
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].text, "same text");
    assert_eq!(actor.state.saved.draft, "same text");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    assert!(actor.state.queue_sending.is_empty());
}

#[test]
fn startup_queue_waits_for_handshake_and_sends_images_once_in_order() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.status = Status::Starting;
    let image = "data:image/png;base64,aGVsbG8=".to_string();
    for (id, text, images) in [(41, "first", vec![image.clone()]), (42, "second", vec![])] {
        actor
            .action(Action::SubmitWithMode {
                id,
                text: text.into(),
                images,
                mode: SubmissionMode::Queue,
            })
            .unwrap();
    }
    // Multiple actor ticks during a delayed handshake cannot write a prompt.
    for _ in 0..3 {
        assert!(!actor.drain_queue());
    }
    assert!(actor.in_flight.is_none());
    assert_eq!(actor.state.queued_prompts.len(), 2);
    actor.pending.insert("init".into(), "initialize".into());
    actor.message(json!({"type":"control_response","response":{"request_id":"init","subtype":"success","response":{}}})).unwrap();
    assert!(actor.drain_queue());
    let first = read(&mut reader);
    assert_eq!(
        first["message"]["content"],
        json!([user_content("first", &[image]), vec![json!({"type":"text", "text":crate::review_brief::context(&actor.state.saved.session_review)})]].concat())
    );
    assert!(!actor.drain_queue());
    actor.acknowledge_input(&first["uuid"]);
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert!(actor.drain_queue());
    let second = read(&mut reader);
    assert_eq!(
        second["message"]["content"],
        json!([user_content("second", &[]), vec![json!({"type":"text", "text":crate::review_brief::context(&actor.state.saved.session_review)})]].concat())
    );
    assert!(!actor.drain_queue());
    actor.acknowledge_input(&second["uuid"]);
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert!(actor.state.queued_prompts.is_empty());
    assert!(!actor.drain_queue());
}

#[test]
fn submission_ack_requires_matching_uuid_and_preserves_newer_draft() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.saved.draft = "sent".into();
    actor.submit(4, "sent".into(), Vec::new()).unwrap();
    let sent = read(&mut reader);
    assert!(sent["uuid"].as_str().is_some());
    assert_eq!(actor.state.accepted_submission, None);
    assert_eq!(actor.state.saved.draft, "sent");
    actor
        .message(json!({"type":"user","uuid":"stale","message":{"content":"sent"}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, None);
    assert!(actor
        .action(Action::Approval(json!("stale"), true))
        .is_err());
    assert!(actor.in_flight.is_some());
    actor.state.saved.draft = "new draft".into();
    actor
        .message(json!({"type":"user","uuid":sent["uuid"],"message":{"content":"sent"}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(4));
    assert_eq!(actor.state.saved.items.len(), 1);
    assert_eq!(actor.state.saved.draft, "new draft");
    actor
        .message(json!({"type":"user","uuid":sent["uuid"]}))
        .unwrap();
    assert_eq!(actor.state.saved.items.len(), 1);
}

#[test]
fn interrupt_pauses_queue_and_foreign_session_cannot_complete_turn() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("active".into());
    actor.state.saved.thread_id = Some("primary".into());
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 3,
        text: "later".into(),
        images: Vec::new(),
    });
    actor.action(Action::Interrupt).unwrap();
    assert_eq!(read(&mut reader)["request"]["subtype"], "interrupt");
    assert!(!actor.queue_ready);
    actor
        .message(json!({"type":"result","session_id":"foreign","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("primary"));
    actor.message(json!({"type":"result","session_id":"primary","parent_tool_use_id":"subagent","is_error":false})).unwrap();
    assert_eq!(actor.state.status, Status::Working);
    actor
        .message(json!({"type":"result","session_id":"primary","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Interrupted);
    assert!(!actor.queue_ready);
    assert_eq!(actor.state.queued_prompts.len(), 1);
}

#[test]
fn queue_waits_for_authoritative_idle_after_result() {
    let mut actor = actor();
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn".into());
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.state.turn_id.is_some());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Done);
    assert!(actor.state.turn_id.is_none());
}

#[test]
fn queued_prompt_is_retained_until_echo_even_after_transport_failure() {
    let mut actor = actor();
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 8,
        text: "keep queued".into(),
        images: Vec::new(),
    });
    let prompt = actor.state.queued_prompts.first().cloned().unwrap();
    assert!(actor.submit(prompt.id, prompt.text, prompt.images).is_err());
    assert_eq!(actor.state.queued_prompts.len(), 1);
    let mut reader = transport(&mut actor);
    let prompt = actor.state.queued_prompts[0].clone();
    actor.submit(prompt.id, prompt.text, prompt.images).unwrap();
    let _ = read(&mut reader);
    actor.fail("disconnected before echo".into());
    assert_eq!(actor.state.queued_prompts.len(), 1);
    assert!(!actor.queue_ready);
    assert_eq!(actor.state.queued_prompts[0].text, "keep queued");
}

#[test]
fn interrupt_before_input_echo_preserves_unaccepted_prompt_without_disconnect() {
    for idle_first in [false, true] {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.tracks_session_state = true;
        actor.state.status = Status::Idle;
        actor.state.saved.draft = "keep draft".into();
        actor.state.queued_prompts.push(QueuedPrompt {
            id: 9,
            text: "not yet accepted".into(),
            images: Vec::new(),
        });
        actor
            .submit(9, "not yet accepted".into(), Vec::new())
            .unwrap();
        let sent = read(&mut reader);
        actor.action(Action::Interrupt).unwrap();
        let _ = read(&mut reader);
        if idle_first {
            actor
                .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
                .unwrap();
        }
        actor
            .message(json!({"type":"result","is_error":true,"errors":["interrupted"]}))
            .unwrap();
        if !idle_first {
            actor
                .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
                .unwrap();
        }
        assert_eq!(actor.state.status, Status::Interrupted);
        assert!(actor.state.error.is_none());
        assert_eq!(actor.state.accepted_submission, None);
        assert_eq!(actor.state.rejected_submission, Some(9));
        assert_eq!(actor.state.saved.draft, "keep draft");
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert!(actor.process.is_some());
        assert!(!actor.queue_ready);
        actor
            .message(json!({"type":"user","uuid":sent["uuid"]}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, None);
        assert!(actor.state.saved.items.is_empty());
    }
}

#[test]
fn host_compact_replays_the_observed_claude_sequence_without_a_false_ack_error() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.tracks_session_state = true;
    actor.state.status = Status::Done;
    actor.state.saved.thread_id = Some("session".into());
    assert_eq!(
        actor.action(Action::Compact).unwrap_err(),
        provider::NOTHING_TO_COMPACT
    );
    actor
        .state
        .saved
        .items
        .push(json!({"id":"u","type":"userMessage","content":[{"type":"text","text":"Reply OK"}]}));
    actor.state.saved.context = Some(provider::ContextUsage {
        used: 25_583,
        window: 1_000_000,
    });
    actor.action(Action::Compact).unwrap();
    let sent = read(&mut reader);
    assert_eq!(sent["type"], "user");
    assert_eq!(
        sent["message"]["content"],
        json!([{"type":"text", "text":"/compact"}])
    );
    let uuid = sent["uuid"].clone();
    assert_eq!(
        actor.action(Action::Compact).unwrap_err(),
        provider::COMPACT_BUSY
    );
    // Claude Code 2.1.302, captured verbatim in shape (bulky fields elided).
    for frame in [
        json!({"type":"system","subtype":"session_state_changed","state":"running","session_id":"session"}),
        json!({"type":"command_lifecycle","command_uuid":uuid,"state":"started","session_id":"session"}),
        json!({"type":"system","subtype":"status","status":"compacting","session_id":"session"}),
    ] {
        actor.message(frame).unwrap();
    }
    assert!(actor.state.compacting);
    assert_eq!(actor.state.status, Status::Working);
    actor
        .action(Action::SubmitWithMode {
            id: 41,
            text: "after compact".into(),
            images: vec![],
            mode: SubmissionMode::Queue,
        })
        .unwrap();
    assert!(
        !actor.drain_queue(),
        "compaction must retain the queued input"
    );
    for frame in [
        json!({"type":"system","subtype":"status","status":null,"compact_result":"success","session_id":"session"}),
        json!({"type":"system","subtype":"init","model":"claude-opus-5-5","permissionMode":"auto","session_id":"session"}),
        json!({"type":"system","subtype":"compact_boundary","session_id":"session","uuid":"boundary",
            "compact_metadata":{"trigger":"manual","pre_tokens":25583,"post_tokens":4200}}),
    ] {
        actor.message(frame).unwrap();
    }
    assert!(!actor.state.compacting);
    let probe = read(&mut reader);
    assert_eq!(probe["request"]["subtype"], "get_context_usage");
    for frame in [
        json!({"type":"user","message":{"role":"user","content":"This session is being continued from a previous conversation..."},"session_id":"session","parent_tool_use_id":null}),
        json!({"type":"user","message":{"role":"user","content":"<local-command-stdout>Compacted </local-command-stdout>"},"session_id":"session","parent_tool_use_id":null,"uuid":"stdout","isReplay":true}),
        json!({"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"},"session_id":"session","parent_tool_use_id":null,"uuid":uuid,"isReplay":true}),
        json!({"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","session_id":"session",
            "modelUsage":{"claude-opus-5-5":{"contextWindow":1000000}}}),
        json!({"type":"system","subtype":"session_state_changed","state":"idle","session_id":"session"}),
    ] {
        actor.message(frame).unwrap();
    }
    assert_eq!(actor.state.status, Status::Done);
    assert!(actor.state.error.is_none());
    assert!(actor.process.is_some());
    // The divider stands in for the command; no "/compact" bubble or summary leaks in.
    assert_eq!(actor.state.saved.items.len(), 2);
    let divider = &actor.state.saved.items[1];
    assert_eq!(divider["type"], provider::COMPACTION_ITEM);
    assert_eq!(
        provider::compaction_label(divider),
        "Context compacted \u{b7} manual"
    );
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":probe["request_id"],
        "response":{"totalTokens":23327,"maxTokens":1000000,"percentage":2}}})).unwrap();
    assert_eq!(
        provider::compaction_label(&actor.state.saved.items[1]),
        "Context compacted \u{b7} manual \u{b7} 26k \u{2192} 23k"
    );
    assert_eq!(actor.state.saved.context.unwrap().used, 23_327);
    assert!(
        actor.drain_queue(),
        "idle after compaction must drain the queue"
    );
    let queued = read(&mut reader);
    assert_eq!(queued["type"], "user");
    assert_eq!(
        queued["message"]["content"][0],
        json!({"type":"text","text":"after compact"})
    );
    assert!(
        !actor.drain_queue(),
        "the same queued input must not be sent twice"
    );
    assert_eq!(
        actor.state.queued_prompts[0].id, 41,
        "retain until provider echo"
    );
}

#[test]
fn ordinary_prompt_without_echo_still_reports_unacknowledged_finish() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.submit(5, "hello".into(), Vec::new()).unwrap();
    let _ = read(&mut reader);
    assert!(actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap_err()
        .contains("before acknowledging"));
}
