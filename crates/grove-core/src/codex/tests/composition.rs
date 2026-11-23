use super::*;

#[test]
fn reference_draft_roundtrip_legacy_and_acceptance_identity() {
    let mut saved = saved();
    let mut legacy = serde_json::to_value(&saved).unwrap();
    legacy.as_object_mut().unwrap().remove("draft_reference");
    assert!(serde_json::from_value::<SavedSession>(legacy)
        .unwrap()
        .draft_reference
        .is_none());
    saved.draft = "explain".into();
    saved.draft_reference = Some(reference("old"));
    let canonical = saved
        .draft_reference
        .as_ref()
        .unwrap()
        .serialize(&saved.draft);
    let encoded = canonical.lines().nth(1).unwrap();
    assert_eq!(
        serde_json::from_str::<ReferenceDraft>(encoded).unwrap(),
        reference("old")
    );
    let restored: SavedSession =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    assert_eq!(restored.draft_reference, saved.draft_reference);
    saved.draft_reference = Some(reference("new"));
    saved.clear_submitted_draft(&canonical, &[]);
    assert_eq!(saved.draft, "explain");
    assert_eq!(saved.draft_reference, Some(reference("new")));
    let canonical = saved
        .draft_reference
        .as_ref()
        .unwrap()
        .serialize(&saved.draft);
    saved.clear_submitted_draft(&canonical, &[]);
    assert!(saved.draft.is_empty());
    assert!(saved.draft_reference.is_none());
    saved.draft_reference = Some(reference("fresh"));
    assert!(saved.fresh_conversation().draft_reference.is_none());
}

#[test]
#[cfg(unix)]
fn reference_codex_dispatch_literal_once_and_late_acceptance_preserves_replacement() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let mut actor = actor();
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let input = child.stdin.take().unwrap();
    let (_, lines) = mpsc::channel();
    actor.process = Some(Process {
        activity: crate::activity_mcp::Server::start().unwrap(),
        visuals: crate::visuals_mcp::Server::start().unwrap(),
        child,
        input,
        lines,
        capture: crate::provider_diagnostics::Capture::default(),
        ownership: None,
    });
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.state.saved.draft = "explain".into();
    actor.state.saved.draft_reference = Some(reference("old"));
    actor.state.skills = (vec![json!({"name":"skill","path":"/tmp/skill"})]).into();
    let canonical = reference("old").serialize("explain");
    actor
        .action(Action::Submit {
            id: 91,
            text: canonical.clone(),
            images: vec![],
        })
        .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let sent: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(sent["params"]["input"][0]["text"], canonical);
    assert_eq!(sent["params"]["input"].as_array().unwrap().len(), 2);
    assert!(crate::review_brief::is_context(
        sent["params"]["input"][1]["text"].as_str().unwrap()
    ));
    assert_eq!(actor.state.saved.draft, "explain");
    actor
        .action(Action::DraftReference(Some(reference("new"))))
        .unwrap();
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"turn-new"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(91));
    assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
    assert_eq!(actor.state.saved.draft, "explain");
}

#[test]
#[cfg(unix)]
fn reference_attached_after_plain_submit_preserves_newer_draft_on_acceptance() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.state.saved.draft = "original plain message".into();
    actor
        .action(Action::Submit {
            id: 91,
            text: "original plain message".into(),
            images: vec![],
        })
        .unwrap();
    actor
        .action(Action::Draft("new unsent text".into()))
        .unwrap();
    let images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor.action(Action::DraftImages(images.clone())).unwrap();
    actor
        .action(Action::DraftReference(Some(reference("new"))))
        .unwrap();
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"accepted-turn"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(91));
    assert_eq!(actor.state.saved.draft, "new unsent text");
    assert_eq!(actor.state.saved.draft_images, images);
    assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
}

#[test]
#[cfg(unix)]
fn reference_attached_after_plain_submit_survives_rejection_without_restoring_old_text() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor
        .action(Action::Submit {
            id: 91,
            text: "old submitted text".into(),
            images: vec![],
        })
        .unwrap();
    actor.action(Action::Draft(String::new())).unwrap();
    actor
        .action(Action::DraftReference(Some(reference("new"))))
        .unwrap();
    actor
        .message(json!({"id":1,"error":{"message":"rejected"}}))
        .unwrap();
    assert_eq!(actor.state.rejected_submission, Some(91));
    assert!(actor.state.saved.draft.is_empty());
    assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
}

#[test]
#[cfg(unix)]
fn reference_queue_and_steer_snapshots_are_immutable_and_rejections_retain_draft() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.saved.draft_reference = Some(reference("old"));
    let canonical = reference("old").serialize("");
    actor
        .submit_mode(91, canonical.clone(), vec![], SubmissionMode::Steer)
        .unwrap();
    actor
        .message(json!({"id":1,"error":{"message":"rejected"}}))
        .unwrap();
    assert_eq!(actor.state.saved.draft_reference, Some(reference("old")));
    actor
        .submit_mode(92, canonical.clone(), vec![], SubmissionMode::Queue)
        .unwrap();
    assert!(actor.state.saved.draft_reference.is_none());
    assert_eq!(actor.state.queued_prompts.last().unwrap().text, canonical);
    actor
        .action(Action::DraftReference(Some(reference("new"))))
        .unwrap();
    assert_eq!(
        actor.state.queued_prompts.last().unwrap().text,
        reference("old").serialize("")
    );
}

#[test]
fn speed_model_switch_retains_preference_without_unadvertised_tier() {
    let mut actor = actor();
    actor.state.model = Some("fast-capable".into());
    actor.state.models = (vec![
        json!({"model":"fast-capable","additionalSpeedTiers":["fast"]}),
        json!({"model":"other"}),
    ])
    .into();
    actor.action(Action::SetSpeed(Speed::Fast)).unwrap();
    actor
        .action(Action::Configure {
            model: Some("other".into()),
            effort: None,
            access: None,
        })
        .unwrap();
    assert_eq!(actor.state.saved.composer.speed, Some(Speed::Fast));
    assert!(actor.state.speed_unavailable(Speed::Fast).is_some());
    assert_eq!(actor.turn_params(vec![])["serviceTierForTurn"], "default");
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
    assert!(actor
        .input("$newly-installed", Vec::new())
        .contains(&json!({
            "type":"skill", "name":"newly-installed", "path":"/skills/new/SKILL.md"
        })));
    assert_eq!(
        actor.state.saved.thread_id.as_deref(),
        Some("retained-thread")
    );
    assert_eq!(actor.state.saved.draft, "unsent draft");
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.process.is_none());
}

#[test]
fn submission_receipt_tracks_turn_identity_before_or_after_normalized_echo() {
    for echo_first in [true, false] {
        let mut actor = actor();
        actor.state.turn_id = None;
        actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"receipt-turn"}}})).unwrap();
        let echo = json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"receipt-turn","item":{"id":"receipt-input","type":"userMessage","content":[{"type":"text","text":"normalized input"}]}}});
        actor.pending.insert(
            77,
            Request {
                kind: Pending::Turn {
                    submission: Some(91),
                    text: "raw input ".into(),
                    images: vec![],
                },
                deadline: Instant::now() + Duration::from_mins(1),
            },
        );
        if echo_first {
            actor.message(echo.clone()).unwrap();
        }
        actor
            .message(json!({"id":77,"result":{"turn":{"id":"receipt-turn"}}}))
            .unwrap();
        if !echo_first {
            assert_eq!(actor.state.submission_item_id(91), None);
            actor.message(echo).unwrap();
        }
        assert_eq!(actor.state.submission_item_id(91), Some("receipt-input"));
        assert!(actor.state.submission_accepted(91));
        // Updating completion without a turn ID keeps ownership intact.
        actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","item":{"id":"receipt-input","type":"userMessage","content":[{"text":"normalized input"}]}}})).unwrap();
        assert_eq!(actor.state.submission_item_id(91), Some("receipt-input"));
        assert_eq!(actor.state.submission_item_id(92), None);
    }
}

#[test]
#[cfg(unix)]
fn activity_reply_queue_and_steer_preserve_identical_main_draft() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("busy".into());
    actor.state.saved.draft = "same text".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor.state.saved.draft_reference = Some(ReferenceDraft {
        id: "reference".into(),
        thread_id: Some("source-thread".into()),
        sources: vec![ReferenceSource {
            message_id: "source-message".into(),
            role: "assistant".into(),
        }],
        text: "quoted selection".into(),
    });
    let original_reference = actor.state.saved.draft_reference.clone();
    actor
        .action(Action::ActivityPrompt {
            id: 41,
            text: "same text".into(),
            mode: SubmissionMode::Queue,
            expected_turn: Some("busy".into()),
        })
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(41));
    assert_eq!(actor.state.saved.draft, "same text");
    actor
        .action(Action::ActivityPrompt {
            id: 42,
            text: "same text".into(),
            mode: SubmissionMode::Steer,
            expected_turn: Some("busy".into()),
        })
        .unwrap();
    let request = actor
        .pending
        .iter()
        .find(|(_, p)| matches!(p.kind, Pending::Steer { .. }))
        .map(|(id, _)| *id)
        .unwrap();
    actor
        .message(json!({"id":request,"result":{"turnId":"busy"}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(42));
    assert_eq!(actor.state.saved.draft, "same text");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    assert_eq!(actor.state.saved.draft_reference, original_reference);
    assert!(!actor.state.queued_prompts.iter().any(|p| p.id == 42));
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
#[cfg(unix)]
fn project_prompt_preserves_composer_and_uses_native_queue() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("busy".into());
    actor.state.saved.draft = "same text".into();
    actor.state.saved.draft_reference = Some(reference("preserved"));
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .action(Action::ProjectPrompt {
            id: 41,
            text: "same text".into(),
        })
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].text, "same text");
    assert_eq!(actor.state.saved.draft, "same text");
    assert_eq!(
        actor.state.saved.draft_reference,
        Some(reference("preserved"))
    );
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    assert!(actor.state.queue_sending.is_empty());
    actor.state.turn_id = None;
    actor.state.status = Status::Idle;
    actor.queue_ready = true;
    actor.dispatch_queue().unwrap();
    assert_eq!(actor.state.queue_sending, vec![41]);
    assert_eq!(actor.state.saved.draft, "same text");
    assert_eq!(
        actor.state.saved.draft_reference,
        Some(reference("preserved"))
    );
    assert_eq!(actor.state.saved.draft_images.len(), 1);
}

#[test]
#[cfg(unix)]
fn held_queue_head_blocks_fifo_until_released() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor
        .submit_mode(42, "second".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor.action(Action::BeginQueuedEdit(41)).unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert!(actor.pending.is_empty());
    actor.action(Action::CancelQueuedEdit(41)).unwrap();
    assert_eq!(actor.state.queue_sending, vec![41]);
    assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
    assert!(actor.action(Action::RemoveQueued(41)).is_err());
    // Editing a later row does not prevent the earlier request's acknowledgement.
    actor.action(Action::BeginQueuedEdit(42)).unwrap();
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].id, 42);
    assert_eq!(actor.state.queue_edit, Some(42));
    assert!(actor.state.queue_sending.is_empty());
}

#[test]
#[cfg(unix)]
fn queued_steering_is_atomic_and_preserves_row_on_rejection_or_uncertainty() {
    for response in [
        json!({"id":1,"error":{"message":"rejected"}}),
        json!({"id":1,"result":{"turnId":"wrong"}}),
        json!({"id":1,"result":{"turnId":"turn-1"}}),
    ] {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft = "unrelated draft".into();
        actor
            .submit_mode(41, "queued".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor.action(Action::SteerQueued(41)).unwrap();
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert_eq!(actor.state.queue_sending, vec![41]);
        assert!(actor.action(Action::SteerQueued(41)).is_err());
        assert!(actor.action(Action::RemoveQueued(41)).is_err());
        actor.message(response.clone()).unwrap();
        assert_eq!(actor.state.saved.draft, "unrelated draft");
        if response["result"]["turnId"] == "turn-1" {
            assert!(actor.state.queued_prompts.is_empty());
            assert!(actor.state.queue_sending.is_empty());
        } else {
            assert_eq!(actor.state.queued_prompts[0].id, 41);
            assert_eq!(
                actor.state.queue_editable(41),
                response.get("error").is_some()
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn pending_steer_blocks_fifo_after_turn_completes_before_ack() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor
        .submit_mode(42, "steering".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor.action(Action::SteerQueued(42)).unwrap();
    assert_eq!(actor.state.queue_steering, vec![42]);
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert_eq!(actor.pending.len(), 1);
    assert_eq!(actor.state.queue_sending, vec![42]);
    actor
        .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
        .unwrap();
    assert_eq!(actor.pending.len(), 1);
    assert_eq!(actor.state.queue_sending, vec![41]);
    assert!(actor.state.queue_steering.is_empty());
    assert_eq!(actor.state.queued_prompts.len(), 1);
    assert_eq!(actor.state.queued_prompts[0].id, 41);
}

#[test]
#[cfg(unix)]
fn composer_steer_has_visible_locked_row_until_acknowledged() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.saved.draft = "steer".into();
    actor
        .submit_mode(41, "steer".into(), Vec::new(), SubmissionMode::Steer)
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].id, 41);
    assert!(!actor.state.queue_editable(41));
    actor
        .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
        .unwrap();
    assert!(actor.state.queued_prompts.is_empty());
    assert!(actor.state.queue_sending.is_empty());
    assert!(actor.state.saved.draft.is_empty());
}

#[test]
#[cfg(unix)]
fn malformed_queued_ack_and_steer_failure_keep_queue_paused() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "retained".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor
        .submit_mode(42, "steer".into(), Vec::new(), SubmissionMode::Steer)
        .unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert!(actor.queue_ready);
    assert_eq!(actor.pending.len(), 1);
    actor
        .message(json!({"id":1,"error":{"message":"too late"}}))
        .unwrap();
    assert!(actor.queue_paused);
    assert!(!actor.queue_ready);
    assert!(actor.pending.is_empty());
    actor.action(Action::ResumeQueue).unwrap();
    actor.message(json!({"id":2,"result":{}})).unwrap();
    assert_eq!(actor.state.queued_prompts[0].text, "retained");
    assert!(actor.queue_paused);
    assert_eq!(actor.state.status, Status::Failed);
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert!(actor.pending.is_empty());
}

#[test]
#[cfg(unix)]
fn interrupted_queue_requires_explicit_resume_and_survives_rejection() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "keep me".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}})).unwrap();
    assert!(actor.pending.is_empty());
    actor.action(Action::ResumeQueue).unwrap();
    actor
        .message(json!({"id":1,"error":{"message":"rejected"}}))
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].text, "keep me");
    assert!(actor.pending.is_empty());
    actor.action(Action::RemoveQueued(41)).unwrap();
    assert!(actor.state.queued_prompts.is_empty());
}

#[test]
fn rejected_submission_is_exact_and_survives_retry_until_new_outcome() {
    let mut actor = actor();
    actor.state.saved.draft = "Retry my draft".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    // The actor may reject after the UI queued against a stale idle snapshot.
    assert!(actor
        .action(Action::Submit {
            id: 41,
            text: "Retry my draft".into(),
            images: actor.state.saved.draft_images.clone()
        })
        .is_err());
    assert_eq!(actor.state.rejected_submission, Some(41));
    assert_eq!(actor.state.saved.draft, "Retry my draft");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    let stale_failure = actor.state.clone();
    actor.pending.insert(
        10,
        Request {
            kind: Pending::Turn {
                submission: Some(42),
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    // Queued/retrying state keeps the prior outcome; it cannot match token42.
    assert_eq!(stale_failure.rejected_submission, Some(41));
    assert_ne!(actor.state.rejected_submission, Some(42));
    actor
        .message(json!({"id":10,"error":{"message":"image temporarily unavailable"}}))
        .unwrap();
    assert_eq!(actor.state.rejected_submission, Some(42));
    assert_eq!(actor.state.saved.draft, "Retry my draft");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    actor.pending.insert(
        11,
        Request {
            kind: Pending::Turn {
                submission: Some(43),
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    assert_eq!(actor.state.rejected_submission, Some(42));
    actor
        .message(json!({"id":11,"result":{"turn":{"id":"retried"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(43));
    assert_eq!(actor.state.rejected_submission, Some(42));
    assert!(actor.state.saved.draft.is_empty());
    assert!(actor.state.saved.draft_images.is_empty());
}

#[test]
fn unrelated_turn_ack_cannot_acknowledge_a_ui_submission() {
    let mut actor = actor();
    actor.state.saved.draft = "UI draft".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Turn {
                submission: None,
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"cli-turn"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, None);
    actor.pending.insert(
        2,
        Request {
            kind: Pending::Turn {
                submission: Some(41),
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor.state.saved.draft = "UI draft".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .message(json!({"id":2,"error":{"message":"busy"}}))
        .unwrap();
    assert_ne!(actor.state.accepted_submission, Some(41));
    assert_eq!(actor.state.saved.draft, "UI draft");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    actor.pending.insert(
        3,
        Request {
            kind: Pending::Turn {
                submission: Some(42),
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":3,"result":{"turn":{"id":"ui-turn"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(42));
    assert_ne!(actor.state.accepted_submission, Some(41));
    // An untracked CLI response must not erase the UI acknowledgement before
    // the main thread coalesces queued snapshots.
    actor.pending.insert(
        4,
        Request {
            kind: Pending::Turn {
                submission: None,
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":4,"result":{"turn":{"id":"later-cli-turn"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(42));
}

#[test]
fn prompt_acknowledgement_changes_only_after_successful_turn_response() {
    let mut actor = actor();
    let before_submission = actor.state.clone();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor.pending.insert(
        8,
        Request {
            kind: Pending::Turn {
                submission: None,
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}})).unwrap();
    assert_eq!(actor.state.accepted_prompts, 0);
    assert_eq!(before_submission.accepted_prompts, 0);
    actor
        .message(json!({"id":8,"error":{"message":"image rejected"}}))
        .unwrap();
    assert_eq!(actor.state.accepted_prompts, 0);
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    actor.pending.insert(
        9,
        Request {
            kind: Pending::Turn {
                submission: None,
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":9,"result":{"turn":{"id":"turn-accepted"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_prompts, 1);
    assert!(actor.state.saved.draft_images.is_empty());
    // Duplicate/unmatched responses cannot acknowledge another submission.
    actor
        .message(json!({"id":9,"result":{"turn":{"id":"turn-accepted"}}}))
        .unwrap();
    assert_eq!(actor.state.accepted_prompts, 1);
}

#[test]
fn pending_start_blocks_second_prompt_without_turn_identity() {
    let mut actor = actor();
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Turn {
                submission: None,
                text: actor.state.saved.draft.clone(),
                images: actor.state.saved.draft_images.clone(),
            },
            deadline: Instant::now(),
        },
    );
    assert!(actor.prompt("second".into()).is_err());
}

#[cfg(unix)]
#[test]
fn failed_prompt_preserves_draft_and_is_never_replayed() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.prompt("keep this text".into()).unwrap();
    actor.action(Action::Draft(String::new())).unwrap();
    actor
        .message(json!({"id":1,"error":{"message":"provider rejected prompt"}}))
        .unwrap();
    assert_eq!(actor.state.saved.draft, "keep this text");
    assert!(actor.initial.is_none());
    assert!(actor.pending.is_empty());
}

#[test]
fn old_history_derives_a_bounded_unicode_prompt_title() {
    let mut saved = saved();
    saved.items.push(json!({"id":"user","type":"userMessage","content":[{"type":"text","text":format!("\n{}\nmore", "界".repeat(100))}]}));
    let state = Snapshot::new(saved);
    assert_eq!(state.saved.title.as_ref().unwrap().chars().count(), 80);
    assert_eq!(state.saved.title.as_deref(), Some("界".repeat(80).as_str()));
}

#[test]
fn context_token_accounting_is_retired_during_compaction() {
    let mut state = state();
    assert!(state.apply(&usage("turn-1", 76000, Some(200000))));
    assert!(state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .is_some());
    assert!(state.apply(
        &json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1",
        "item":{"id":"compact","type":COMPACTION_ITEM}}})
    ));
    assert!(state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .is_none());
    assert!(state.apply(&usage("turn-1", 12000, Some(200000))));
    assert_eq!(
        state
            .saved
            .context_details
            .unwrap()
            .accounting
            .unwrap()
            .last
            .unwrap()
            .total_tokens,
        Some(12000)
    );
}

#[test]
fn compact_tokens_and_labels_are_compact() {
    assert_eq!(compact_tokens(950), "950");
    assert_eq!(compact_tokens(4_200), "4.2k");
    assert_eq!(compact_tokens(5_000), "5k");
    assert_eq!(compact_tokens(24_400), "24k");
    assert_eq!(compact_tokens(182_000), "182k");
    assert_eq!(compact_tokens(999_700), "1M");
    assert_eq!(compact_tokens(1_260_000), "1.3M");
    let item =
        json!({"type":COMPACTION_ITEM,"trigger":"auto","preTokens":182_000,"postTokens":24_000});
    assert_eq!(
        compaction_label(&item),
        "Context compacted \u{b7} auto \u{b7} 182k \u{2192} 24k"
    );
    assert_eq!(item_text(&item), compaction_label(&item));
    let pending =
        json!({"type":COMPACTION_ITEM,"trigger":"manual","preTokens":182_000,"postTokens":null});
    assert_eq!(
        compaction_label(&pending),
        "Context compacted \u{b7} manual"
    );
    assert_eq!(
        ContextUsage {
            used: 300,
            window: 200
        }
        .percent(),
        100
    );
}

#[test]
#[cfg(unix)]
fn manual_compaction_resumes_fresh_queue_after_old_pause() {
    for paused in [false, true] {
        for early_queue in [false, true] {
            for late_ack in [false, true] {
                let (mut actor, mut reader) = idle_actor();
                actor.queue_paused = paused;
                actor.action(Action::Compact).unwrap();
                assert_eq!(read_line(&mut reader)["method"], "thread/compact/start");
                if !late_ack {
                    actor.message(json!({"id":1,"result":{}})).unwrap();
                }
                if early_queue {
                    actor
                        .submit_mode(41, "first".into(), vec![], SubmissionMode::Queue)
                        .unwrap();
                }
                actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"inProgress"}}})).unwrap();
                actor.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"id":"c1","type":"contextCompaction"}}})).unwrap();
                if !early_queue {
                    actor
                        .submit_mode(41, "first".into(), vec![], SubmissionMode::Queue)
                        .unwrap();
                }
                actor
                    .submit_mode(42, "second".into(), vec![], SubmissionMode::Queue)
                    .unwrap();
                actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"id":"c1","type":"contextCompaction"}}})).unwrap();
                assert!(
                    actor.state.queue_sending.is_empty(),
                    "item completion must not dispatch"
                );
                actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"completed"}}})).unwrap();
                if late_ack {
                    assert!(actor.state.queue_sending.is_empty());
                    actor.message(json!({"id":1,"result":{}})).unwrap();
                }
                assert_eq!(
                    actor.state.queue_sending,
                    vec![41],
                    "paused={paused}, early_queue={early_queue}, late_ack={late_ack}"
                );
                let start = read_line(&mut reader);
                assert_eq!(start["method"], "turn/start");
                assert_eq!(start["params"]["input"][0]["text"], "first");
                assert_eq!(
                    actor.state.queued_prompts.len(),
                    2,
                    "retain rows until acknowledgment"
                );
                actor
                    .message(json!({"id":2,"result":{"turn":{"id":"next"}}}))
                    .unwrap();
                actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"completed"}}})).unwrap();
                assert!(
                    actor.state.queue_sending.is_empty(),
                    "duplicate completion cannot dispatch second input"
                );
                actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"next","status":"completed"}}})).unwrap();
                assert_eq!(actor.state.queue_sending, vec![42]);
                assert_eq!(
                    read_line(&mut reader)["params"]["input"][0]["text"],
                    "second"
                );
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn manual_compaction_requests_thread_compact_and_normalizes_its_divider() {
    let (mut actor, mut reader) = idle_actor();
    actor
        .message(usage("turn-0", 182_000, Some(475_000)))
        .unwrap();
    actor.action(Action::Compact).unwrap();
    assert_eq!(
        read_line(&mut reader),
        json!({"id":1,"method":"thread/compact/start","params":{"threadId":"thread-1"}})
    );
    // While the request is pending, ordinary prompts and a second compaction wait.
    assert!(actor.prompt("next".into()).is_err());
    assert_eq!(actor.action(Action::Compact).unwrap_err(), COMPACT_BUSY);
    actor.message(json!({"id":1,"result":{}})).unwrap();
    // Observed codex-cli 0.163.0 sequence: a fresh turn wraps the compaction item.
    actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"inProgress"}}})).unwrap();
    actor.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"type":"contextCompaction","id":"c1"}}})).unwrap();
    assert!(actor.state.compacting);
    assert_eq!(actor.state.status, Status::Working);
    assert_eq!(actor.state.saved.items[1]["status"], "inProgress");
    assert_ne!(
        item_text(&actor.state.saved.items[1]),
        "contextCompaction: inProgress"
    );
    let mut post = usage("compact-turn", 24_000, Some(475_000));
    post["params"]["tokenUsage"]["last"]["inputTokens"] = json!(0);
    post["params"]["tokenUsage"]["last"]["outputTokens"] = json!(0);
    actor.message(post).unwrap();
    actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"type":"contextCompaction","id":"c1"}}})).unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"completed"}}})).unwrap();
    assert!(!actor.state.compacting);
    assert_eq!(actor.state.status, Status::Done);
    let divider = &actor.state.saved.items[1];
    assert_eq!(divider["type"], COMPACTION_ITEM);
    assert_eq!(divider["status"], "completed");
    assert_eq!(
        compaction_label(divider),
        "Context compacted \u{b7} manual \u{b7} 182k \u{2192} 24k"
    );
    assert_eq!(actor.state.saved.context.unwrap().used, 24_000);
    // Resumed history carries no Grove metadata; the known divider survives it.
    actor.pending.insert(
        9,
        Request {
            kind: Pending::Thread,
            deadline: Instant::now() + Duration::from_secs(30),
        },
    );
    actor.message(json!({"id":9,"result":{"thread":{"id":"thread-1","turns":[{"id":"t","status":"completed","items":[
        {"id":"u1","type":"userMessage","content":[{"type":"text","text":"hi"}]},
        {"id":"c1","type":"contextCompaction"},
        {"id":"old","type":"contextCompaction"}
    ]}]}}})).unwrap();
    assert_eq!(
        compaction_label(&actor.state.saved.items[1]),
        "Context compacted \u{b7} manual \u{b7} 182k \u{2192} 24k"
    );
    assert_eq!(item_text(&actor.state.saved.items[2]), "Context compacted");
}

#[test]
fn automatic_compaction_is_labelled_auto_and_waits_for_post_usage() {
    let mut state = state();
    state.apply(&usage("turn-1", 150_000, Some(200_000)));
    state.apply(&json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"contextCompaction","id":"auto"}}}));
    assert!(state.compacting);
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"contextCompaction","id":"auto"}}}));
    assert!(!state.compacting);
    let divider = state.saved.items.last().unwrap().clone();
    assert_eq!(compaction_label(&divider), "Context compacted \u{b7} auto");
    state.apply(&usage("turn-1", 30_000, Some(200_000)));
    assert_eq!(
        compaction_label(state.saved.items.last().unwrap()),
        "Context compacted \u{b7} auto \u{b7} 150k \u{2192} 30k"
    );
    // Only the waiting figure is filled; later usage never rewrites it.
    state.apply(&usage("turn-1", 40_000, Some(200_000)));
    assert_eq!(state.saved.items.last().unwrap()["postTokens"], 30_000);
}

#[test]
#[cfg(unix)]
fn compaction_is_rejected_while_busy_or_empty_and_failure_is_inline() {
    let (mut actor, mut reader) = idle_actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn-1".into());
    assert_eq!(actor.action(Action::Compact).unwrap_err(), COMPACT_BUSY);
    assert!(actor.pending.is_empty());
    actor.state.status = Status::Done;
    actor.state.turn_id = None;
    actor.state.saved.items.clear();
    assert_eq!(
        actor.action(Action::Compact).unwrap_err(),
        NOTHING_TO_COMPACT
    );
    actor.state.saved.thread_id = None;
    assert_eq!(
        actor.action(Action::Compact).unwrap_err(),
        NOTHING_TO_COMPACT
    );
    actor.state.saved.thread_id = Some("thread-1".into());
    actor.state.saved.items = vec![json!({"id":"a","type":"agentMessage","text":"hello"})].into();
    actor.action(Action::Compact).unwrap();
    let sent = read_line(&mut reader);
    actor
        .message(json!({"id":sent["id"],"error":{"code":-32600,"message":"no thread"}}))
        .unwrap();
    assert_eq!(actor.state.error.as_deref(), Some("no thread"));
    assert_eq!(actor.state.status, Status::Done);
    assert!(!actor.state.compact_requested);
    actor.process = None;
    assert_eq!(
        actor.action(Action::Compact).unwrap_err(),
        COMPACT_DISCONNECTED
    );
}
