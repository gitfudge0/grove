use super::*;

#[test]
fn established_forks_claim_current_identity_before_resume() {
    let mut saved = saved();
    saved.fork_origin = Some(ForkOrigin {
        source_thread: Some("source".into()),
        last_turn: None,
        users: 0,
    });
    assert!(crate::provider_ownership::native_write_target(&saved).is_none());
    saved.fork_origin.as_mut().unwrap().source_thread = None;
    assert_eq!(
        crate::provider_ownership::native_write_target(&saved),
        Some("thread-1")
    );
    saved.provider = Provider::Claude;
    saved.claude_resume = Some(ClaudeResume {
        original_session: Some("source".into()),
        assistant_uuid: None,
        human_uuid: "human".into(),
        fork_id: "fork".into(),
    });
    assert_eq!(
        crate::provider_ownership::native_write_target(&saved),
        Some("fork")
    );
}

#[test]
fn forks_preserve_destination_prefix_and_draft_for_both_providers() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut original = saved();
        original.provider = provider;
        original.items = vec![
            json!({"id":"u1","type":"userMessage","content":[{"type":"text","text":"first"}]}),
            json!({"id":"a1","type":"agentMessage","text":"answer", "claudeAssistantUuid":"assistant-1"}),
            json!({"id":"u2","type":"userMessage","content":[{"type":"text","text":"second"},{"type":"image","url":"data:image/png;base64,abc"}]}),
            json!({"id":"a2","type":"agentMessage","text":"later"})].into();
        original.edit_checkpoints.push(EditCheckpoint {
            start_item: 0,
            turn_id: Some("turn-1".into()),
            files: None,
        });
        let fork = original.fork_at(2, true).unwrap();
        assert_eq!(fork.items.len(), 2);
        assert_eq!(fork.draft, "second");
        assert_eq!(fork.draft_images, vec!["data:image/png;base64,abc"]);
        assert_eq!(fork.cwd, original.cwd);
        assert_eq!(fork.project, original.project);
        assert_ne!(fork.key, original.key);
        assert!(fork.title.unwrap().contains("Forked"));
        assert_eq!(original.items.len(), 4);
        let first = original.fork_at(0, true).unwrap();
        assert!(first.items.is_empty());
        assert_eq!(first.draft, "first");
        let mut active = Snapshot::new(original);
        active.saved.work_periods.push(WorkPeriod {
            start_item: 0,
            end_item: Some(2),
            duration_ms: None,
            subagents: Vec::new(),
        });
        active.saved.work_periods.push(WorkPeriod {
            start_item: 2,
            end_item: None,
            duration_ms: None,
            subagents: Vec::new(),
        });
        active.status = Status::Starting;
        assert!(!active.can_fork_at(2, false));
        for status in [Status::Working, Status::Waiting] {
            active.status = status;
            assert!(active.can_fork_at(2, false));
            assert!(!active.can_fork_at(2, true));
            assert!(!active.can_fork_at(4, false));
        }
        let response = active.saved.fork_at(2, false).unwrap();
        assert!(response.draft.is_empty());
        assert!(response.draft_images.is_empty());
        if provider == Provider::Codex {
            assert_eq!(
                response.fork_origin.unwrap().last_turn.as_deref(),
                Some("turn-1")
            );
        } else {
            assert_eq!(
                response.claude_resume.unwrap().assistant_uuid.as_deref(),
                Some("assistant-1")
            );
        }
    }
}

#[test]
fn review_briefs_accumulate_until_review_and_survive_restore() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut state = Snapshot::new(saved());
        state.saved.provider = provider;
        let complete = |state: &mut Snapshot, text: &str| {
            state.begin_work();
            state
                .saved
                .items
                .push(json!({"type":"agentMessage", "phase":"final_answer", "text":text}));
            state.finish_work();
            state.status = Status::Done;
            state.completed_for_review();
        };
        complete(
            &mut state,
            "<grove-review-summary>Fixed theme hover colors; native checks pending.</grove-review-summary>",
        );
        let first = state.review_completion().unwrap();
        complete(&mut state, "Committed as abc123.");
        assert_eq!(
            state.saved.session_review.summary,
            "Fixed theme hover colors; native checks pending."
        );
        assert!(state.mark_reviewed(first).is_err());
        complete(
            &mut state,
            "<grove-review-summary>Fixed hover and focus colors. Checked both themes.</grove-review-summary>",
        );
        let mut restored = Snapshot::new(
            serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap(),
        );
        restored.status = Status::Idle;
        assert!(restored
            .saved
            .session_review
            .summary
            .contains("hover and focus"));
        restored
            .mark_reviewed(restored.review_completion().unwrap())
            .unwrap();
        assert_eq!(
            restored.saved.session_review.reviewed_through,
            restored.saved.items.len()
        );
        assert!(restored.saved.session_review.summary.is_empty());
        complete(
            &mut restored,
            "<grove-review-summary>Added keyboard navigation.</grove-review-summary>",
        );
        assert_eq!(
            restored.saved.session_review.summary,
            "Added keyboard navigation."
        );
        let fresh = restored.saved.fresh_conversation();
        assert_eq!(fresh.session_review.reviewed_through, 0);
        assert!(fresh.session_review.summary.is_empty());
    }
}

#[test]
fn session_review_receipts_survive_restore_and_reject_stale_turn_actions() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut session = saved();
        session.provider = provider;
        let mut snapshot = Snapshot::new(session);
        snapshot.apply(&json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"first"}}}));
        snapshot.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","item":{"id":"reply","type":"agentMessage","text":"Finished"}}}));
        snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"first","status":"completed"}}}));
        assert_eq!(snapshot.session_status(), "Review");
        let first = snapshot.review_completion().unwrap();
        let mut restored = Snapshot::new(
            serde_json::from_value(serde_json::to_value(&snapshot.saved).unwrap()).unwrap(),
        );
        restored.status = Status::Idle;
        assert_eq!(restored.review_completion(), Some(first));
        restored.mark_reviewed(first).unwrap();
        restored.completed_for_review();
        assert_eq!(
            restored.session_status(),
            "Ready",
            "duplicate completion must not re-open review"
        );
        restored.begin_work();
        restored.status = Status::Working;
        assert!(restored.mark_reviewed(first).is_err());
        restored.finish_work();
        restored.status = Status::Done;
        restored.completed_for_review();
        let second = restored.review_completion().unwrap();
        assert_ne!(first, second);
        assert!(restored.mark_reviewed(first).is_err());
        assert_eq!(restored.review_completion(), Some(second));
        let mut fresh = Snapshot::new(restored.saved.fresh_conversation());
        fresh.status = Status::Idle;
        assert_eq!(fresh.review_completion(), None);
        fresh.begin_work();
        fresh.finish_work();
        fresh.status = Status::Done;
        fresh.completed_for_review();
        assert_ne!(fresh.review_completion(), Some(first));
    }
}

#[test]
fn stale_completion_cannot_finish_new_work_or_recreate_review() {
    let mut snapshot = Snapshot::new(saved());
    assert!(snapshot.apply(
        &json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"first"}}})
    ));
    assert!(snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"first","status":"completed"}}})));
    let first = snapshot.review_completion().unwrap();
    snapshot.mark_reviewed(first).unwrap();
    assert!(!snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"first","status":"completed"}}})));
    assert_eq!(snapshot.session_status(), "Ready");
    assert!(snapshot.apply(
        &json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"second"}}})
    ));
    assert!(!snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"first","status":"completed"}}})));
    assert_eq!(snapshot.session_status(), "Working");
    assert_eq!(snapshot.turn_id.as_deref(), Some("second"));
    assert!(snapshot.work_started.is_some());
    assert!(snapshot.saved.work_periods[1].end_item.is_none());
    assert_eq!(snapshot.review_completion(), None);
    assert!(snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"second","status":"completed"}}})));
    assert_eq!(snapshot.session_status(), "Review");
    assert_ne!(snapshot.review_completion(), Some(first));
    // No active identity after reconnect/rewind cannot be proof of success.
    snapshot.saved.session_review.pending = None;
    snapshot.saved.session_review.last_completed_period = None;
    snapshot.saved.session_review.reviewed_through = 0;
    snapshot.saved.session_review.summary.clear();
    snapshot.status = Status::Idle;
    assert!(!snapshot.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"second","status":"completed"}}})));
    assert_eq!(snapshot.review_completion(), None);
}

#[test]
fn failed_and_interrupted_periods_never_become_review_results() {
    for status in ["failed", "interrupted"] {
        let mut snapshot = Snapshot::new(saved());
        snapshot.apply(
            &json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn"}}}),
        );
        snapshot.apply(
            &json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn","status":status}}}),
        );
        assert_eq!(
            snapshot.status,
            if status == "failed" {
                Status::Failed
            } else {
                Status::Interrupted
            }
        );
        assert_eq!(snapshot.saved.work_periods.len(), 1);
        assert_eq!(snapshot.review_completion(), None);
        let mut restored = Snapshot::new(snapshot.saved);
        restored.status = Status::Idle;
        assert_eq!(restored.review_completion(), None);
    }
}

#[test]
fn codex_review_action_updates_only_its_current_completed_result() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.state.finish_work();
    actor.state.status = Status::Done;
    actor.state.completed_for_review();
    let receipt = actor.state.review_completion().unwrap();
    assert!(actor
        .action(Action::MarkReviewed {
            work_period: receipt + 1
        })
        .is_err());
    actor
        .action(Action::MarkReviewed {
            work_period: receipt,
        })
        .unwrap();
    assert_eq!(actor.state.session_status(), "Ready");
}

#[test]
fn session_history_close_preserves_codex_and_claude_transcripts_separately_from_recovery() {
    for provider in [Provider::Codex, Provider::Claude] {
        let root = tempfile::tempdir().unwrap();
        let active = root.path().join("active");
        let history = root.path().join("history");
        let mut session = saved();
        session.provider = provider;
        session.draft = "unsent draft".into();
        session.items.push(
            serde_json::json!({"id":"reply", "type":"agentMessage", "text":"Retained answer"}),
        );
        session.config_args = vec!["--example-preserved-option".into()];
        persist_in_dir(&session, &active).unwrap();
        archive_closed_in_dirs(&session, &active, &history).unwrap();
        assert!(load_from_dir(&active).unwrap().sessions.is_empty());
        let retained = load_from_dir(&history).unwrap().sessions.remove(0);
        assert_eq!(
            serde_json::to_value(&retained).unwrap(),
            serde_json::to_value(&session).unwrap()
        );
        // Reopen writes recovery before archive cleanup, so interrupted adoption
        // retains both copies rather than losing the conversation.
        persist_in_dir(&retained, &active).unwrap();
        assert_eq!(load_from_dir(&active).unwrap().sessions.len(), 1);
        assert_eq!(load_from_dir(&history).unwrap().sessions.len(), 1);
    }
}

#[test]
fn session_history_archive_failure_preserves_active_recovery() {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("active");
    let blocked = root.path().join("blocked");
    let session = saved();
    persist_in_dir(&session, &active).unwrap();
    fs_err::write(&blocked, "not a directory").unwrap();
    assert!(archive_closed_in_dirs(&session, &active, &blocked).is_err());
    assert_eq!(load_from_dir(&active).unwrap().sessions.len(), 1);
    let mut unsafe_key = session;
    unsafe_key.key = "../escape".into();
    assert!(archive_closed_in_dirs(&unsafe_key, &active, &root.path().join("history")).is_err());
    assert_eq!(load_from_dir(&active).unwrap().sessions.len(), 1);
}

#[test]
#[ignore = "manual transcript existing-row mutation benchmark"]
fn transcript_existing_row_edit_benchmark() {
    use std::hint::black_box;
    let path = std::env::var("GROVE_PERF_SESSION").expect("GROVE_PERF_SESSION");
    let saved = load_saved_session(std::path::Path::new(&path)).unwrap();
    let shared = saved.items;
    let legacy: Vec<Value> = shared.iter().map(|item| item.as_ref().clone()).collect();
    let index = legacy
        .iter()
        .rposition(|item| item["type"] == "agentMessage")
        .unwrap();
    let original = legacy[index]["text"].as_str().unwrap_or_default();
    let replacement = format!("{original} streamed continuation");
    let sample = |old: bool| {
        let start = Instant::now();
        for _ in 0..50 {
            if old {
                let mut next = black_box(&legacy).clone();
                next[index]["text"] = json!(replacement);
                assert_eq!(next[index]["text"], replacement);
                assert_eq!(next.len(), legacy.len());
                black_box(next);
            } else {
                let mut next = black_box(&shared).clone();
                next[index]["text"] = json!(replacement);
                assert_eq!(next[index]["text"], replacement);
                assert_eq!(next.len(), legacy.len());
                black_box(next);
            }
        }
        start.elapsed().as_secs_f64() * 1000.0 / 50.0
    };
    for _ in 0..3 {
        black_box(sample(true));
        black_box(sample(false));
    }
    for index in 0..15 {
        let (legacy_ms, shared_ms) = if index % 2 == 0 {
            (sample(true), sample(false))
        } else {
            let shared = sample(false);
            (sample(true), shared)
        };
        println!(
            "existing_edit_sample={{\"sample\":{index},\"legacy_ms\":{legacy_ms:.6},\"shared_ms\":{shared_ms:.6}}}"
        );
    }
    assert_eq!(shared[index]["text"], original);
    assert_eq!(
        serde_json::to_value(&shared).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
}

#[test]
fn claude_recovery_removes_shell_tools_misidentified_as_children() {
    for provider in [Provider::Claude, Provider::Codex] {
        let mut saved = saved();
        saved.provider = provider;
        saved.items = vec![
            json!({"id":"shell","type":"commandExecution","command":"sleep 2"}),
            json!({"id":"read","type":"mcpToolCall","tool":"Read"}),
            json!({"id":"agent","type":"mcpToolCall","tool":"Agent"}),
            json!({"id":"task","type":"mcpToolCall","tool":"Task"}),
            json!({"id":"legacy","type":"subAgentActivity","kind":"started","agentThreadId":"shell"}),
        ].into();
        let mut period = WorkPeriod {
            start_item: 0,
            end_item: Some(5),
            duration_ms: None,
            subagents: Vec::new(),
        };
        for id in ["shell", "read", "agent", "task", "unmatched"] {
            record_subagent(&mut period, id, None, None, SubagentStatus::Unknown);
        }
        saved.work_periods.push(period);
        let recovered = Snapshot::new(saved);
        let ids = recovered.saved.work_periods[0]
            .subagents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            if provider == Provider::Claude {
                vec!["agent", "task", "unmatched"]
            } else {
                vec!["shell", "read", "agent", "task", "unmatched"]
            }
        );
    }
}

#[test]
#[cfg(unix)]
fn reference_prefix_looking_ordinary_prompt_restores_after_rejection() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    let text = "Referenced transcript selection (quoted data):\nnot JSON\n\nUser reply:\nexplain this format";
    actor
        .action(Action::Submit {
            id: 91,
            text: text.into(),
            images: vec![],
        })
        .unwrap();
    actor.action(Action::Draft(String::new())).unwrap();
    actor
        .message(json!({"id":1,"error":{"message":"rejected"}}))
        .unwrap();
    assert_eq!(actor.state.saved.draft, text);
    assert!(actor.state.saved.draft_reference.is_none());
    assert!(ReferenceDraft::is_canonical_prompt(
        &reference("valid").serialize("reply")
    ));
    let mut invalid = reference("");
    assert!(!ReferenceDraft::is_canonical_prompt(
        &invalid.serialize("reply")
    ));
    invalid.id = "valid".into();
    invalid.sources.clear();
    assert!(!ReferenceDraft::is_canonical_prompt(
        &invalid.serialize("reply")
    ));
}

#[test]
fn work_periods_follow_turn_boundaries_and_recover_without_inventing_time() {
    let mut state = state();
    let start =
        json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}});
    state.apply(&start);
    state.apply(&start);
    assert_eq!(state.saved.work_periods.len(), 1);
    state.work_started = Some(Instant::now().checked_sub(Duration::from_secs(3)).unwrap());
    state
        .saved
        .items
        .push(json!({"id":"answer", "type":"agentMessage", "text":"Done"}));
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"other","status":"completed"}}}));
    assert!(state.work_started.is_some());
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}}));
    let period = &state.saved.work_periods[0];
    assert_eq!(period.start_item, 0);
    assert_eq!(period.end_item, Some(1));
    assert!(period.duration_ms.unwrap() >= 3000);
    assert!(state.work_started.is_none());
    let mut saved = state.saved;
    saved.work_periods.push(WorkPeriod {
        start_item: 1,
        end_item: None,
        duration_ms: None,
        subagents: Vec::new(),
    });
    let recovered = Snapshot::new(saved);
    assert_eq!(recovered.saved.work_periods[1].end_item, Some(1));
    assert_eq!(recovered.saved.work_periods[1].duration_ms, None);
    assert!(recovered.work_started.is_none());
    assert!(recovered.saved.fresh_conversation().work_periods.is_empty());
}

#[test]
fn old_recovery_defaults_to_codex_and_claude_round_trips() {
    let mut record = serde_json::to_value(saved()).unwrap();
    record.as_object_mut().unwrap().remove("provider");
    let restored: SavedSession = serde_json::from_value(record).unwrap();
    assert_eq!(restored.provider, Provider::Codex);
    let mut claude = restored;
    claude.provider = Provider::Claude;
    let restored: SavedSession =
        serde_json::from_slice(&serde_json::to_vec(&claude).unwrap()).unwrap();
    assert_eq!(restored.provider, Provider::Claude);
    assert_eq!(restored.provider.agent(), crate::agent::Agent::Claude);
}

#[test]
fn provider_notices_preserve_retry_recovery_and_raw_error_ownership() {
    let mut state = state();
    state.apply(&json!({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","error":{"message":"Connection interrupted"},"willRetry":true}}));
    assert_eq!(state.status, Status::Working);
    assert_eq!(
        state.effective_notice().unwrap().kind,
        SessionNoticeKind::Retrying
    );
    state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"tool","delta":"Local output"}}));
    assert_eq!(
        state.effective_notice().unwrap().kind,
        SessionNoticeKind::Retrying
    );
    state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"message","delta":"Recovered"}}));
    assert!(state.error.is_none());
    state.apply(&json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"resetsAt":1234}}}}));
    assert!(state.error.is_none());
    state.apply(&json!({"method":"error","params":{"threadId":"thread-1","error":{"message":"Limit reached","codexErrorInfo":"usageLimitExceeded"},"willRetry":false}}));
    assert_eq!(state.effective_notice().unwrap().reset_at, Some(1234));
    assert!(state.apply(&json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"resetsAt":5678}}}})));
    assert_eq!(state.effective_notice().unwrap().reset_at, Some(5678));
    assert_eq!(
        state.effective_notice().unwrap().kind,
        SessionNoticeKind::UsageLimit
    );
    state.error = Some("A different error".into());
    assert!(state.effective_notice().is_none());
}

#[test]
fn native_history_stop_reports_archive_failure_separately_from_process_completion() {
    let mut actor = actor();
    actor.state.saved.key = "invalid key".into();
    let (done, completed) = mpsc::channel();
    let (archive_outcome, outcome) = mpsc::channel();
    assert!(!actor
        .action(Action::Stop {
            forget: true,
            done,
            archive_outcome: Some(archive_outcome)
        })
        .unwrap());
    completed.recv().unwrap();
    assert!(outcome.recv().unwrap().is_err());
}

#[test]
fn queue_edit_preserves_identity_position_and_attachments() {
    let mut actor = actor();
    actor.state.queued_prompts = vec![
        QueuedPrompt {
            id: 41,
            text: "first".into(),
            images: Vec::new(),
        },
        QueuedPrompt {
            id: 42,
            text: "second".into(),
            images: Vec::new(),
        },
    ];
    actor.action(Action::BeginQueuedEdit(42)).unwrap();
    assert_eq!(actor.state.queue_edit, Some(42));
    assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
    assert!(actor
        .action(Action::SaveQueuedEdit {
            id: 41,
            text: "wrong".into(),
            images: Vec::new()
        })
        .is_err());
    assert!(actor
        .action(Action::SaveQueuedEdit {
            id: 42,
            text: String::new(),
            images: Vec::new()
        })
        .is_err());
    assert_eq!(actor.state.queue_edit, Some(42));
    let images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .action(Action::SaveQueuedEdit {
            id: 42,
            text: "revised".into(),
            images: images.clone(),
        })
        .unwrap();
    assert_eq!(actor.state.queue_edit, None);
    assert_eq!(actor.state.queued_prompts[0].id, 41);
    assert_eq!(
        actor.state.queued_prompts[1],
        QueuedPrompt {
            id: 42,
            text: "revised".into(),
            images
        }
    );
    actor.action(Action::BeginQueuedEdit(42)).unwrap();
    actor.action(Action::CancelQueuedEdit(42)).unwrap();
    assert_eq!(actor.state.queued_prompts[1].text, "revised");
}

#[test]
fn composer_settings_survive_recovery_and_reset_restores_protocol_defaults() {
    let mut actor = actor();
    actor.configured_turn = json!({"model":"configured","effort":"medium","sandboxPolicy":{"type":"readOnly"},"approvalPolicy":"on-request"});
    actor
        .action(Action::Configure {
            model: Some("selected".into()),
            effort: Some("high".into()),
            access: Some("full-access".into()),
        })
        .unwrap();
    let recovered: SavedSession =
        serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
    let state = Snapshot::new(recovered);
    assert_eq!(state.model.as_deref(), Some("selected"));
    assert_eq!(state.effort.as_deref(), Some("high"));
    assert_eq!(state.access.as_deref(), Some("full-access"));
    actor.action(Action::ResetConfiguration).unwrap();
    let params = actor.turn_params(vec![]);
    assert_eq!(params["model"], "configured");
    assert_eq!(params["effort"], "medium");
    assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
    assert_eq!(params["approvalPolicy"], "on-request");
    assert!(actor.state.saved.composer.model.is_none());
    assert_eq!(actor.state.resolved_model.as_deref(), Some("configured"));
    assert_eq!(actor.state.resolved_effort.as_deref(), Some("medium"));
    assert_eq!(actor.state.resolved_access.as_deref(), Some("read-only"));
}

#[test]
#[cfg(unix)]
fn stop_finishes_after_matching_completion_and_grace_or_reports_timeout() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.action(Action::Interrupt).unwrap();
    let now = Instant::now();
    a.advance_stop(now);
    assert!(a.stop_tracking.stopping);
    a.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}})).unwrap();
    a.advance_stop(now);
    a.advance_stop(now + Duration::from_millis(499));
    assert!(a.stop_tracking.stopping);
    a.advance_stop(now + Duration::from_millis(501));
    assert!(!a.stop_tracking.stopping);
    let mut b = actor();
    b.process = Some(fixture_process());
    b.action(Action::Interrupt).unwrap();
    b.advance_stop(Instant::now() + Duration::from_secs(31));
    assert!(!b.stop_tracking.stopping);
    assert!(b
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("could not confirm"));
}

#[test]
#[cfg(unix)]
fn accepted_steering_clears_only_submitted_draft_after_recovery() {
    let sent_images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    for newer in [
        None,
        Some(("new unsent draft", sent_images.clone())),
        Some(("sent", Vec::new())),
    ] {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft = "sent".into();
        actor.state.saved.draft_images = sent_images.clone();
        actor
            .submit_mode(
                42,
                "sent".into(),
                sent_images.clone(),
                SubmissionMode::Steer,
            )
            .unwrap();
        let awaiting = recover_saved_for_test(&actor.state.saved);
        assert_eq!(awaiting.draft, "sent");
        assert_eq!(awaiting.draft_images, sent_images);
        if let Some((text, images)) = &newer {
            actor.action(Action::Draft((*text).into())).unwrap();
            actor.action(Action::DraftImages(images.clone())).unwrap();
        }
        actor
            .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        let recovered = recover_saved_for_test(&actor.state.saved);
        let (text, images) = newer.unwrap_or(("", Vec::new()));
        assert_eq!(recovered.draft, text);
        assert_eq!(recovered.draft_images, images);
    }
}

#[test]
#[cfg(unix)]
fn queued_acceptance_clears_only_matching_composer_after_recovery() {
    let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    for draft in ["sent", "new unsent draft"] {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft = draft.into();
        actor.state.saved.draft_images = images.clone();
        actor
            .submit_mode(42, "sent".into(), images.clone(), SubmissionMode::Queue)
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert_eq!(actor.state.queued_prompts[0].text, "sent");
        assert_eq!(actor.state.queued_prompts[0].images, images);
        let recovered = recover_saved_for_test(&actor.state.saved);
        assert_eq!(recovered.draft, if draft == "sent" { "" } else { draft });
        assert_eq!(
            recovered.draft_images,
            if draft == "sent" {
                Vec::new()
            } else {
                images.clone()
            }
        );
    }
}

#[test]
#[cfg(unix)]
fn rejected_steering_preserves_retry_draft_after_recovery() {
    let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    for response in [
        json!({"id":1,"error":{"message":"no active turn"}}),
        json!({"id":1,"result":{"turnId":"wrong-turn"}}),
    ] {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft = "retry".into();
        actor.state.saved.draft_images = images.clone();
        actor
            .submit_mode(42, "retry".into(), images.clone(), SubmissionMode::Steer)
            .unwrap();
        actor.message(response).unwrap();
        assert_eq!(actor.state.rejected_submission, Some(42));
        let recovered = recover_saved_for_test(&actor.state.saved);
        assert_eq!(recovered.draft, "retry");
        assert_eq!(recovered.draft_images, images);
    }
}

#[test]
#[cfg(unix)]
fn queue_is_fifo_and_waits_for_exact_completion_and_ack() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor
        .submit_mode(42, "second".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(42));
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"wrong","status":"completed"}}})).unwrap();
    assert!(actor.pending.is_empty());
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert_eq!(actor.pending.len(), 1);
    assert_eq!(actor.state.queued_prompts.len(), 2);
    actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}})).unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-2","status":"completed"}}})).unwrap();
    assert_eq!(actor.pending.len(), 1);
    actor.state.saved.draft = "independent draft".into();
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].id, 42);
    assert_eq!(actor.state.saved.draft, "independent draft");
    assert_eq!(actor.pending.len(), 1);
    actor
        .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
        .unwrap();
    assert_eq!(actor.state.queued_prompts.len(), 1);
}

#[test]
fn resumed_unseen_prose_remains_unassigned_across_reconnect_and_completion() {
    let known = json!({"id":"known","type":"agentMessage","text":"Observed progress"});
    let unseen = json!({"id":"unseen","type":"agentMessage","phase":"commentary","text":"Progress while disconnected"});
    let saved = vec![
        json!({"id":"grove-plan-t","type":"plan","taskSnapshot":true,"plan":[{"step":"Inspect","status":"inProgress"}]}),
        known.clone(),
    ];
    let first = restore_task_snapshots(&saved, vec![known.clone(), unseen.clone()]);
    assert_eq!(
        crate::tasks::activity_owner(&first, 1).unwrap().step,
        "Inspect"
    );
    assert!(crate::tasks::activity_owner(&first, 2).is_none());
    assert_eq!(first[2]["groveTaskOwnershipUnavailable"], true);
    let second = restore_task_snapshots(&first, vec![known, unseen]);
    assert!(crate::tasks::activity_owner(&second, 2).is_none());
    assert_eq!(second[2]["groveTaskOwnershipUnavailable"], true);
    let mut state = state();
    state.saved.items = second.into();
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"unseen","type":"agentMessage","phase":"commentary","text":"Completed old stream"}}}));
    assert!(crate::tasks::activity_owner(&state.saved.items, 2).is_none());
    state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"new-live","delta":"New observed progress"}}));
    assert_eq!(
        crate::tasks::activity_owner(&state.saved.items, 3)
            .unwrap()
            .step,
        "Inspect"
    );
}

#[test]
fn recovery_is_private_atomic_and_isolates_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    let saved = saved();
    persist_in_dir(&saved, dir.path()).unwrap();
    fs_err::write(dir.path().join("broken.json"), "{").unwrap();
    let result = load_from_dir(dir.path()).unwrap();
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.warnings.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs_err::metadata(dir.path().join("test-native-1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn unsafe_recovery_key_cannot_escape_directory() {
    let dir = tempfile::tempdir().unwrap();
    let mut saved = saved();
    saved.key = "../escape".into();
    assert!(persist_in_dir(&saved, dir.path()).is_err());
}

#[test]
fn old_recovery_without_new_optional_fields_is_compatible() {
    let mut value = serde_json::to_value(saved()).unwrap();
    for key in ["draft", "config_args", "title", "context", "work_periods"] {
        value.as_object_mut().unwrap().remove(key);
    }
    let restored: SavedSession = serde_json::from_value(value).unwrap();
    assert!(restored.draft.is_empty());
    assert!(restored.config_args.is_empty());
    assert!(restored.context.is_none());
    assert!(restored.work_periods.is_empty());
}

#[test]
fn reconnect_start_failure_is_retryable_disconnected_state() {
    let mut actor = actor();
    actor.state.status = Status::Failed;
    actor.state.turn_id = None;
    actor.args = vec!["--unsupported".into()];
    actor.action(Action::Reconnect).unwrap();
    assert_eq!(actor.state.status, Status::Disconnected);
    assert!(!actor.state.reconnecting);
    assert!(actor.process.is_none());
    assert!(actor.state.error.is_some());
}

#[test]
fn provider_title_overrides_prompt_fallback_and_survives_recovery() {
    let mut state = state();
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"user-1","type":"userMessage","content":[{"type":"text","text":"Fix session search\nPreserve keyboard navigation"}]}}}));
    assert_eq!(state.saved.title.as_deref(), Some("Fix session search"));
    state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":"Search keyboard navigation"}}));
    assert_eq!(
        state.saved.title.as_deref(),
        Some("Search keyboard navigation")
    );
    let restored: SavedSession =
        serde_json::from_slice(&serde_json::to_vec(&state.saved).unwrap()).unwrap();
    assert_eq!(restored.title, state.saved.title);
    assert!(!state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"other","threadName":"Wrong session"}})));
    assert_eq!(
        state.saved.title.as_deref(),
        Some("Search keyboard navigation")
    );
    state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"thread-1"}}));
    assert_eq!(state.saved.title.as_deref(), Some("Fix session search"));
}

#[test]
fn token_usage_tracks_last_turn_context_and_survives_recovery() {
    let mut state = state();
    assert!(state.apply(&usage("turn-1", 76_000, Some(200_000))));
    assert_eq!(
        state.saved.context,
        Some(ContextUsage {
            used: 76_000,
            window: 200_000
        })
    );
    assert_eq!(state.saved.context.unwrap().percent(), 38);
    // A missing window keeps the last known one; cumulative totals are never used.
    assert!(state.apply(&usage("turn-1", 90_000, None)));
    assert_eq!(state.saved.context.unwrap().used, 90_000);
    assert_eq!(state.saved.context.unwrap().window, 200_000);
    let recovered: SavedSession =
        serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
    assert_eq!(Snapshot::new(recovered).saved.context, state.saved.context);
    // An unknown window leaves the meter unknown.
    let mut fresh = Snapshot::new(saved());
    fresh.apply(&usage("turn-1", 10_000, None));
    assert!(fresh.saved.context.is_none());
    assert_eq!(fresh.context_tokens, Some(10_000));
}

#[test]
#[cfg(unix)]
fn manual_compaction_preserves_recovery_and_edit_guards() {
    for guard in [
        "old_head",
        "uncertain",
        "failed",
        "interrupted",
        "stop",
        "reconnect",
        "rejected",
        "editing",
    ] {
        let (mut actor, mut reader) = idle_actor();
        actor.queue_paused = true;
        if guard == "old_head" {
            actor.state.queued_prompts.push(QueuedPrompt {
                id: 40,
                text: "held".into(),
                images: vec![],
            });
        }
        actor.action(Action::Compact).unwrap();
        assert_eq!(read_line(&mut reader)["method"], "thread/compact/start");
        actor.message(json!({"id":1,"result":{}})).unwrap();
        actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"inProgress"}}})).unwrap();
        actor
            .submit_mode(41, "fresh".into(), vec![], SubmissionMode::Queue)
            .unwrap();
        // Unrelated completions cannot release this compaction's inputs.
        actor.message(json!({"method":"turn/completed","params":{"threadId":"other","turn":{"id":"compact-turn","status":"completed"}}})).unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"old-turn","status":"completed"}}})).unwrap();
        assert!(actor.state.queue_sending.is_empty());
        match guard {
            "uncertain" => actor.state.queue_uncertain.push(41),
            "stop" => {
                actor.action(Action::Interrupt).unwrap();
                let _ = read_line(&mut reader);
            }
            "reconnect" => {
                // Exercise startup revocation without launching an external CLI.
                actor.stopping.store(true, Ordering::SeqCst);
                assert!(actor.start().is_err());
            }
            "rejected" => {
                actor.pending.insert(
                    99,
                    Request {
                        kind: Pending::Compact,
                        deadline: Instant::now(),
                    },
                );
                actor
                    .message(json!({"id":99,"error":{"message":"rejected"}}))
                    .unwrap();
            }
            "editing" => {
                actor.action(Action::BeginQueuedEdit(41)).unwrap();
            }
            _ => {}
        }
        let status = if matches!(guard, "failed" | "interrupted") {
            guard
        } else {
            "completed"
        };
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":status}}})).unwrap();
        assert!(actor.state.queue_sending.is_empty(), "guard={guard}");
        assert!(actor.state.queued_prompts.iter().any(|p| p.id == 41));
        if guard == "editing" {
            actor.action(Action::CancelQueuedEdit(41)).unwrap();
            assert_eq!(actor.state.queue_sending, vec![41]);
            assert_eq!(read_line(&mut reader)["method"], "turn/start");
        } else {
            assert!(actor.queue_paused, "guard={guard}");
        }
    }
}

#[test]
#[cfg(unix)]
fn edit_older_message_restores_pre_turn_then_retains_earlier_history_and_populates_draft() {
    let mut fixture = edit_fixture();
    let earlier = fixture.actor.state.saved.items[..2].to_vec();
    fixture.actor.edit_message(2, "target".into()).unwrap();
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "before target"
    );
    assert_eq!(fixture.actor.state.saved.items.len(), 5);
    assert!(edit_disk(&fixture).pending_edit.is_some());
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
        .unwrap();
    assert_eq!(fixture.actor.state.saved.items, earlier);
    assert_eq!(fixture.actor.state.saved.draft, "target prompt");
    assert_eq!(
        fixture.actor.state.saved.draft_images,
        vec!["data:image/png;base64,aGVsbG8="]
    );
    assert_eq!(fixture.actor.state.saved.edit_checkpoints.len(), 1);
    assert_eq!(
        fixture.actor.state.saved.edit_checkpoints[0]
            .turn_id
            .as_deref(),
        Some("earlier-turn")
    );
    assert!(edit_disk(&fixture).pending_edit.is_none());
    assert_eq!(edit_disk(&fixture).items, earlier);
    assert_eq!(fixture.actor.state.status, Status::Idle);
}

#[test]
#[cfg(unix)]
fn inline_edit_preserves_composer_through_restore_and_submission_ack() {
    let mut fixture = edit_fixture();
    let image = "data:image/png;base64,aGVsbG8=".to_owned();
    fixture.actor.state.saved.draft = "Unrelated draft".into();
    fixture.actor.state.saved.draft_images = vec![image.clone()];
    fixture
        .actor
        .action(Action::EditMessageInline {
            index: 2,
            message_id: "target".into(),
        })
        .unwrap();
    assert_eq!(
        edit_disk(&fixture)
            .pending_edit
            .unwrap()
            .composer_draft
            .unwrap()
            .0,
        "Unrelated draft"
    );
    fixture
        .actor
        .action(Action::Draft("Newer composer draft".into()))
        .unwrap();
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "Newer composer draft");
    assert_eq!(fixture.actor.state.saved.draft_images, vec![image.clone()]);
    fixture
        .actor
        .action(Action::PromptInline {
            text: "Edited target".into(),
            images: vec![image.clone()],
        })
        .unwrap();
    fixture
        .actor
        .action(Action::Draft("Typing during send".into()))
        .unwrap();
    fixture
        .actor
        .action(Action::DraftImages(Vec::new()))
        .unwrap();
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"result":{"turn":{"id":"edited-turn","status":"inProgress"}}}))
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "Typing during send");
    assert!(fixture.actor.state.saved.draft_images.is_empty());
    assert_eq!(fixture.actor.state.accepted_prompts, 1);
    assert_eq!(fixture.actor.state.saved.items.len(), 2);
}

#[test]
#[cfg(unix)]
fn inline_edit_failure_preserves_composer_and_original_history() {
    let mut fixture = edit_fixture();
    let items = fixture.actor.state.saved.items.clone();
    fixture.actor.state.saved.draft = "Keep composer".into();
    fixture
        .actor
        .action(Action::EditMessageInline {
            index: 2,
            message_id: "target".into(),
        })
        .unwrap();
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"error":{"message":"Rejected restore"}}))
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "Keep composer");
    assert_eq!(fixture.actor.state.saved.items, items);
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
}

#[test]
#[cfg(unix)]
fn edit_provider_failure_recovers_current_workspace_and_complete_conversation() {
    let mut fixture = edit_fixture();
    let items = fixture.actor.state.saved.items.clone();
    fixture.actor.edit_message(2, "target".into()).unwrap();
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"error":{"message":"revert refused"}}))
        .unwrap();
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
    assert_eq!(fixture.actor.state.saved.items, items);
    assert_eq!(fixture.actor.state.saved.edit_checkpoints.len(), 3);
    assert!(fixture.actor.state.saved.pending_edit.is_none());
    assert!(edit_disk(&fixture).pending_edit.is_none());
    assert_eq!(fixture.actor.state.error.as_deref(), Some("revert refused"));
}

#[test]
#[cfg(unix)]
fn edit_busy_queued_and_stale_message_guards_leave_workspace_and_history_untouched() {
    let mut fixture = edit_fixture();
    let items = fixture.actor.state.saved.items.clone();
    fixture.actor.state.status = Status::Working;
    assert!(fixture.actor.edit_message(2, "target".into()).is_err());
    fixture.actor.state.status = Status::Idle;
    fixture.actor.state.queued_prompts.push(QueuedPrompt {
        id: 77,
        text: "queued".into(),
        images: Vec::new(),
    });
    assert!(fixture.actor.edit_message(2, "target".into()).is_err());
    fixture.actor.state.queued_prompts.clear();
    assert!(fixture.actor.edit_message(2, "stale-id".into()).is_err());
    assert_eq!(fixture.actor.state.saved.items, items);
    assert!(fixture.actor.state.saved.pending_edit.is_none());
    assert!(fixture.actor.pending.is_empty());
    assert!(!fixture.journal.exists());
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
}

#[test]
#[cfg(unix)]
fn edit_reconnect_recovers_durable_journal_only_after_paginated_provider_history() {
    for target_present in [true, false] {
        let mut fixture = edit_fixture();
        let original = fixture.actor.state.saved.items.clone();
        fixture.actor.edit_message(2, "target".into()).unwrap();
        let saved = edit_disk(&fixture);
        fixture.actor.state = Snapshot::new(saved);
        fixture.actor.pending.clear();
        fixture.actor.pending.insert(
            900,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        fixture
            .actor
            .message(json!({"id":900,"result":{"thread":{"id":"thread-1","turns":[]}}}))
            .unwrap();
        assert!(fixture.actor.state.saved.pending_edit.is_some());
        let first_page = edit_pending(&fixture.actor);
        fixture.actor.message(json!({"id":first_page,"result":{"data":[{"id":"newer-turn"}],"nextCursor":"older-page"}})).unwrap();
        assert!(fixture.actor.state.saved.pending_edit.is_some());
        assert_eq!(fixture.actor.state.saved.items, original);
        let second_page = edit_pending(&fixture.actor);
        let turns = if target_present {
            json!([{"id":"target-turn"}])
        } else {
            json!([{"id":"earlier-turn"}])
        };
        fixture
            .actor
            .message(json!({"id":second_page,"result":{"data":turns,"nextCursor":null}}))
            .unwrap();
        assert!(fixture.actor.state.saved.pending_edit.is_none());
        assert!(edit_disk(&fixture).pending_edit.is_none());
        if target_present {
            assert_eq!(fixture.actor.state.saved.items, original);
            assert_eq!(
                fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
                "latest workspace"
            );
        } else {
            assert_eq!(fixture.actor.state.saved.items, original[..2]);
            assert_eq!(fixture.actor.state.saved.draft, "target prompt");
            assert_eq!(
                fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
                "before target"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn edit_transport_failure_keeps_durable_journal_and_history_for_reconnect() {
    let mut fixture = edit_fixture();
    let items = fixture.actor.state.saved.items.clone();
    let process = fixture.actor.process.as_mut().unwrap();
    // Reap the entire fixture process group so the pipe has no remaining reader.
    unsafe {
        libc::kill(-(process.child.id() as i32), libc::SIGKILL);
    }
    process.child.wait().unwrap();
    let error = fixture.actor.edit_message(2, "target".into()).unwrap_err();
    assert!(error.contains("uncertain"), "{error}");
    assert_eq!(fixture.actor.state.status, Status::Disconnected);
    assert!(fixture.actor.process.is_none());
    assert_eq!(fixture.actor.state.saved.items, items);
    assert!(fixture.actor.state.saved.pending_edit.is_some());
    let durable = edit_disk(&fixture);
    assert!(durable.pending_edit.is_some());
    assert_eq!(durable.items, items);
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "before target"
    );
    assert_eq!(fixture.actor.state.edited_messages, 0);
    durable
        .pending_edit
        .unwrap()
        .recovery
        .unwrap()
        .restore()
        .unwrap();
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
}

#[test]
#[cfg(unix)]
fn rewind_without_files_works_without_git_and_preserves_complete_drafts_durably() {
    let mut fixture = edit_fixture();
    fixture.actor.state.saved.edit_checkpoints[1].files = None;
    let non_git = tempfile::tempdir().unwrap();
    fixture.actor.state.saved.cwd = non_git.path().to_string_lossy().into_owned();
    let reference = ReferenceDraft {
        id: "quote".into(),
        thread_id: Some("thread-1".into()),
        sources: vec![ReferenceSource {
            message_id: "earlier-answer".into(),
            role: "assistant".into(),
        }],
        text: "literal quote".into(),
    };
    fixture.actor.state.saved.draft = "Unrelated draft".into();
    fixture.actor.state.saved.draft_images = vec!["original-image".into()];
    fixture.actor.state.saved.draft_reference = Some(reference.clone());
    fixture
        .actor
        .action(Action::RewindMessage {
            index: 2,
            message_id: "target".into(),
            restore_files: false,
        })
        .unwrap();
    let durable = edit_disk(&fixture);
    assert!(durable.pending_edit.as_ref().unwrap().rewind);
    assert!(durable.pending_edit.as_ref().unwrap().recovery.is_none());
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "target prompt");
    assert_eq!(fixture.actor.state.saved.items.len(), 2);
    assert!(
        fixture.actor.pending.is_empty(),
        "rewind must never submit a turn"
    );
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
    let durable = edit_disk(&fixture);
    assert_eq!(durable.rewind_drafts.len(), 1);
    let saved_id = durable.rewind_drafts[0].id.clone();
    fixture.actor.state = Snapshot::new(durable);
    fixture
        .actor
        .action(Action::RestoreRewindDraft {
            id: saved_id.clone(),
        })
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "Unrelated draft");
    assert_eq!(fixture.actor.state.saved.draft_images, ["original-image"]);
    assert_eq!(fixture.actor.state.saved.draft_reference, Some(reference));
    assert_eq!(
        fixture.actor.state.saved.rewind_drafts[0].text,
        "target prompt"
    );
    fixture
        .actor
        .action(Action::RestoreRewindDraft { id: saved_id })
        .unwrap();
    assert_eq!(fixture.actor.state.saved.draft, "target prompt");
    assert_eq!(
        fixture.actor.state.saved.draft_images,
        ["data:image/png;base64,aGVsbG8="]
    );
    assert!(fixture.actor.state.saved.draft_reference.is_none());
}

#[test]
#[cfg(unix)]
fn rewind_restore_rejects_main_checkout_and_stale_identity_before_mutation() {
    let mut fixture = edit_fixture();
    assert!(fixture.actor.state.rewind_target(2).is_ok());
    assert!(fixture
        .actor
        .state
        .rewind_files_unavailable(2)
        .unwrap()
        .contains("linked worktree"));
    let error = fixture
        .actor
        .action(Action::RewindMessage {
            index: 2,
            message_id: "target".into(),
            restore_files: true,
        })
        .unwrap_err();
    assert!(error.contains("linked worktree"), "{error}");
    assert_eq!(
        fixture.actor.state.rewind_attempts, 1,
        "rejected dispatch must be acknowledged"
    );
    assert!(fixture.actor.state.saved.pending_edit.is_none());
    assert!(fixture
        .actor
        .action(Action::RewindMessage {
            index: 2,
            message_id: "stale".into(),
            restore_files: false
        })
        .is_err());
    assert!(!fixture.journal.exists());
    assert_eq!(fixture.actor.state.rewind_attempts, 2);
    assert_eq!(
        fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
        "latest workspace"
    );
}

#[test]
#[cfg(unix)]
fn rewind_linked_worktree_restores_files_and_recovers_provider_rejection() {
    let mut fixture = edit_fixture();
    let main = fixture.repo.path();
    for args in [
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(main)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let linked = main.join("linked");
    assert!(Command::new("git")
        .arg("-C")
        .arg(main)
        .args(["worktree", "add", "-qb", "rewind-test"])
        .arg(&linked)
        .status()
        .unwrap()
        .success());
    let root = linked.canonicalize().unwrap();
    fixture.actor.state.saved.cwd = root.to_string_lossy().into_owned();
    fs_err::write(root.join("work"), "before target").unwrap();
    fixture.actor.state.saved.edit_checkpoints[1].files =
        Some(crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap());
    fs_err::write(root.join("work"), "later unrelated edit").unwrap();
    assert!(fixture.actor.state.rewind_files_unavailable(2).is_none());
    // A checkpoint must never mutate a root no longer owned by this session.
    fixture.actor.state.saved.cwd = main.to_string_lossy().into_owned();
    assert!(fixture
        .actor
        .state
        .rewind_files_unavailable(2)
        .unwrap()
        .contains("worktrees changed"));
    assert!(fixture
        .actor
        .action(Action::RewindMessage {
            index: 2,
            message_id: "target".into(),
            restore_files: true
        })
        .unwrap_err()
        .contains("worktrees changed"));
    assert_eq!(
        fs_err::read_to_string(root.join("work")).unwrap(),
        "later unrelated edit"
    );
    assert!(fixture.actor.state.saved.pending_edit.is_none());
    fixture.actor.state.saved.cwd = root.to_string_lossy().into_owned();
    let reservation = crate::checkpoint::acquire_restore(std::slice::from_ref(&root)).unwrap();
    fixture
        .actor
        .action(Action::RewindMessageReserved {
            index: 2,
            message_id: "target".into(),
            reservation: reservation.clone(),
        })
        .unwrap();
    assert!(crate::checkpoint::restore_blocked(std::slice::from_ref(
        &root
    )));
    drop(reservation); // The actor now owns the final guard through recovery.
    assert_eq!(
        fs_err::read_to_string(root.join("work")).unwrap(),
        "before target"
    );
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"error":{"message":"provider rejected rewind"}}))
        .unwrap();
    assert_eq!(
        fs_err::read_to_string(root.join("work")).unwrap(),
        "later unrelated edit"
    );
    assert_eq!(fixture.actor.state.saved.items.len(), 5);
    assert!(fixture.actor.state.saved.pending_edit.is_none());
    assert!(!crate::checkpoint::restore_blocked(std::slice::from_ref(
        &root
    )));
    fixture
        .actor
        .action(Action::RewindMessage {
            index: 2,
            message_id: "target".into(),
            restore_files: true,
        })
        .unwrap();
    let id = edit_pending(&fixture.actor);
    fixture
        .actor
        .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
        .unwrap();
    assert_eq!(
        fs_err::read_to_string(root.join("work")).unwrap(),
        "before target"
    );
    assert_eq!(fixture.actor.state.saved.draft, "target prompt");
    assert!(fixture.actor.pending.is_empty());
    assert!(!crate::checkpoint::restore_blocked(std::slice::from_ref(
        &root
    )));
}

#[test]
#[cfg(unix)]
fn rewind_keep_files_reconciles_uncertain_delivery_without_a_workspace_snapshot() {
    for provider_kept_target in [true, false] {
        let mut fixture = edit_fixture();
        fixture.actor.state.saved.edit_checkpoints[1].files = None;
        fixture.actor.state.saved.draft = "independent".into();
        fixture
            .actor
            .action(Action::RewindMessage {
                index: 2,
                message_id: "target".into(),
                restore_files: false,
            })
            .unwrap();
        fixture.actor.state = Snapshot::new(edit_disk(&fixture));
        fixture.actor.pending.clear();
        fixture.actor.pending.insert(
            800,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        fixture
            .actor
            .message(json!({"id":800,"result":{"thread":{"id":"thread-1","turns":[]}}}))
            .unwrap();
        let id = edit_pending(&fixture.actor);
        let turns = if provider_kept_target {
            json!([{"id":"target-turn"}])
        } else {
            json!([{"id":"earlier-turn"}])
        };
        fixture
            .actor
            .message(json!({"id":id,"result":{"data":turns,"nextCursor":null}}))
            .unwrap();
        assert!(fixture.actor.state.saved.pending_edit.is_none());
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "latest workspace"
        );
        assert!(fixture.actor.pending.is_empty());
        if provider_kept_target {
            assert_eq!(fixture.actor.state.saved.items.len(), 5);
            assert_eq!(fixture.actor.state.saved.draft, "independent");
            assert!(fixture.actor.state.saved.rewind_drafts.is_empty());
        } else {
            assert_eq!(fixture.actor.state.saved.items.len(), 2);
            assert_eq!(fixture.actor.state.saved.draft, "target prompt");
            assert_eq!(
                fixture.actor.state.saved.rewind_drafts[0].text,
                "independent"
            );
        }
    }
}

#[test]
fn rewind_durable_recovery_ownership_survives_archiving_and_allows_only_owner_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    let nested = root.join("nested");
    let other = directory.path().join("other");
    fs_err::create_dir_all(&nested).unwrap();
    fs_err::create_dir(&other).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    fs_err::write(root.join("work"), "before recovery").unwrap();
    let recovery = crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap();
    let active = directory.path().join("active");
    let archived = directory.path().join("archived");
    fs_err::create_dir(&active).unwrap();
    fs_err::create_dir(&archived).unwrap();
    let dirs = [active.clone(), archived.clone()];
    let mut session = saved();
    session.key = "durable-owner".into();
    session.pending_edit = Some(PendingEdit {
        target: 0,
        turn_id: "turn".into(),
        text: "prompt".into(),
        images: vec![],
        recovery: Some(recovery),
        rewind: true,
        composer_draft: None,
        claude_fork_id: None,
    });
    persist_in_dir(&session, &active).unwrap();
    assert!(workspace_recovery_blocked_in_dirs(std::slice::from_ref(&root), None, &dirs).unwrap());
    assert!(
        workspace_recovery_blocked_in_dirs(std::slice::from_ref(&nested), None, &dirs).unwrap()
    );
    assert!(
        !workspace_recovery_blocked_in_dirs(std::slice::from_ref(&other), None, &dirs).unwrap()
    );
    assert!(!workspace_recovery_blocked_in_dirs(
        std::slice::from_ref(&root),
        Some("durable-owner"),
        &dirs
    )
    .unwrap());
    archive_closed_in_dirs(&session, &active, &archived).unwrap();
    assert!(workspace_recovery_blocked_in_dirs(std::slice::from_ref(&root), None, &dirs).unwrap());
    assert!(!workspace_recovery_blocked_in_dirs(
        std::slice::from_ref(&root),
        Some("durable-owner"),
        &dirs
    )
    .unwrap());
    session.pending_edit = None;
    persist_in_dir(&session, &archived).unwrap();
    assert!(!workspace_recovery_blocked_in_dirs(std::slice::from_ref(&root), None, &dirs).unwrap());
    fs_err::write(active.join("corrupt.json"), "not json").unwrap();
    assert!(workspace_recovery_blocked_in_dirs(std::slice::from_ref(&root), None, &dirs).is_err());
}

#[test]
fn rewind_restores_reference_envelope_as_a_literal_composer_reference() {
    let mut saved = saved();
    let reference = ReferenceDraft {
        id: "r".into(),
        thread_id: Some("old-thread".into()),
        sources: vec![ReferenceSource {
            message_id: "m".into(),
            role: "assistant".into(),
        }],
        text: "/clear is quoted data".into(),
    };
    saved.finish_rewind_draft(reference.serialize("reply"), vec![]);
    assert_eq!(saved.draft, "reply");
    assert_eq!(saved.draft_reference, Some(reference));
}

#[test]
fn edit_capability_tracks_paginated_and_legacy_thread_responses() {
    let mut actor = actor();
    actor.state.turn_id = None;
    actor.state.saved.items = vec![
        json!({"id":"sent","type":"userMessage","content":[{"type":"text","text":"sent prompt"}]}),
    ]
    .into();
    for (id, history_mode, supported) in [
        (901, None, false),
        (902, Some("paginated"), true),
        (903, Some("legacy"), false),
    ] {
        actor.pending.insert(
            id,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        let mut thread = json!({"id":"thread-1","turns":[]});
        if let Some(mode) = history_mode {
            thread["historyMode"] = json!(mode);
        }
        actor
            .message(json!({"id":id,"result":{"thread":thread}}))
            .unwrap();
        assert_eq!(actor.state.edit_supported, supported);
        let error = actor.state.editable_message(0).unwrap_err();
        if supported {
            assert!(error.contains("checkpoint"), "{error}");
        } else {
            assert!(error.contains("rewind support"), "{error}");
        }
    }
}
