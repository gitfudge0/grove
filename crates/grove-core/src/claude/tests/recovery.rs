use super::*;

#[test]
fn session_fork_recipe_survives_restart_and_nested_forks_until_input_acknowledgment() {
    let mut original = actor().state.saved.clone();
    original.thread_id = Some("original".into());
    original.items = vec![
        json!({"id":"human", "type":"userMessage", "content":[{"type":"text","text":"first"}]}),
        json!({"id":"answer", "type":"agentMessage", "text":"answer", "claudeAssistantUuid":"assistant"}),
        json!({"id":"later", "type":"userMessage", "content":[{"type":"text","text":"later"}]})].into();
    let fork = original.fork_at(2, false).unwrap();
    let restored: SavedSession =
        serde_json::from_value(serde_json::to_value(&fork).unwrap()).unwrap();
    let nested = restored.fork_at(2, false).unwrap();
    for saved in [restored, nested] {
        let command = launch_command(&[], &saved).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--resume", "original"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--resume-session-at", "assistant"]));
        assert!(args.windows(2).any(|pair| pair[0] == "--session-id"
            && Some(pair[1].as_str()) == saved.thread_id.as_deref()));
        assert!(args.iter().any(|arg| arg == "--fork-session"));
        let mut actor = actor_with_saved(saved);
        actor.input_uuid = Some("accepted".into());
        actor.in_flight = Some(QueuedPrompt {
            id: 1,
            text: "new prompt".into(),
            images: vec![],
        });
        actor.acknowledge_input(&json!("accepted"));
        assert!(actor.state.saved.claude_resume.is_none());
        let resumed = launch_command(&[], &actor.state.saved).unwrap();
        let args = resumed
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--fork-session"));
        assert!(args.windows(2).any(|pair| pair[0] == "--resume"
            && Some(pair[1].as_str()) == actor.state.saved.thread_id.as_deref()));
    }
}

#[test]
fn native_history_stop_reports_archive_failure_separately_from_process_completion() {
    let mut actor = actor();
    actor.state.saved.key = "invalid key".into();
    let (done, completed) = mpsc::channel();
    let (archive_outcome, outcome) = mpsc::channel();
    actor.stop(true, done, Some(archive_outcome));
    completed.recv().unwrap();
    assert!(outcome.recv().unwrap().is_err());
}

#[test]
fn provider_allowed_recovers_usage_but_preserves_other_errors_and_transport_disconnects() {
    let mut actor = actor();
    actor.state.turn_id = Some("turn-1".into());
    actor.message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1234}})).unwrap();
    actor
        .message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}))
        .unwrap();
    assert!(actor.state.error.is_none());
    actor.state.set_provider_error(
        "Claude",
        "Authentication failed".into(),
        false,
        Some("authentication_failed"),
        None,
    );
    actor
        .message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}))
        .unwrap();
    assert_eq!(actor.state.error.as_deref(), Some("Authentication failed"));
    actor.fail("Process exited".into());
    assert_eq!(actor.state.status, Status::Disconnected);
}

#[test]
fn failed_redirect_does_not_disable_redirecting_after_reconnect() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("old".into());
    actor
        .redirect(91, "first correction".into(), vec![])
        .unwrap();
    let _ = read(&mut reader);
    actor.fail("transport disconnected".into());
    assert!(!actor.redirect_pending);
    let mut reconnected = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("new".into());
    actor
        .redirect(92, "second correction".into(), vec![])
        .unwrap();
    assert_eq!(read(&mut reconnected)["priority"], "next");
    // A startup that exits before spawn must also clear transient redirect state.
    actor.stopping.store(true, Ordering::SeqCst);
    assert!(actor.start().is_err());
    assert!(!actor.redirect_pending);
}

#[test]
fn edit_journal_recovers_files_and_only_commits_truncation_after_success() {
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .unwrap()
        .success());
    let root = repo.path().canonicalize().unwrap();
    let file = root.join("work");
    fs_err::write(&file, "current workspace").unwrap();
    let recovery = crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap();
    let mut actor = actor();
    actor.state.saved.key = format!("test-claude-edit-{}", nonce());
    let journal = provider::recovery_dir()
        .unwrap()
        .join(format!("{}.json", actor.state.saved.key));
    actor.state.saved.thread_id = Some("original".into());
    actor.state.saved.items = vec![
        json!({"id":"before","type":"agentMessage","text":"retained","claudeAssistantUuid":"assistant"}),
        json!({"id":"target","type":"userMessage","content":[{"type":"text","text":"original prompt"}],"claudeHumanUuid":"human","claudePreviousAssistantUuid":"assistant"}),
        json!({"id":"later","type":"agentMessage","text":"later"}),
        json!({"id":"next-human","type":"userMessage","content":[{"type":"text","text":"a subsequent turn"}],"claudeHumanUuid":"next-human","claudePreviousAssistantUuid":"later-assistant"}),
    ].into();
    actor.state.saved.pending_edit = Some(PendingEdit {
        target: 1,
        turn_id: "human".into(),
        text: "original prompt".into(),
        images: vec![],
        recovery: Some(recovery.clone()),
        rewind: false,
        composer_draft: Some(("independent draft".into(), vec![])),
        claude_fork_id: Some("fork".into()),
    });
    // Editing in the middle intentionally discards all subsequent turns in
    // the fork. Claude's optional single-turn discard guard rejects this.
    let command = launch_command(&[], &actor.state.saved).unwrap();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for pair in [
        ["--resume", "original"],
        ["--resume-session-at", "assistant"],
        ["--session-id", "fork"],
    ] {
        assert!(args.windows(2).any(|actual| actual == pair));
    }
    assert!(args.iter().any(|arg| arg == "--fork-session"));
    assert!(!args.iter().any(|arg| arg == "--resume-drops-turn"));
    provider::persist(&actor.state.saved).unwrap();
    fs_err::write(&file, "pre target snapshot").unwrap();
    actor.recover_edit().unwrap();
    assert_eq!(fs_err::read_to_string(&file).unwrap(), "current workspace");
    assert_eq!(actor.state.saved.items.len(), 4);
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("original"));
    assert!(actor.state.saved.pending_edit.is_none());
    actor.state.saved.pending_edit = Some(PendingEdit {
        target: 1,
        turn_id: "human".into(),
        text: "original prompt".into(),
        images: vec![],
        recovery: Some(recovery),
        rewind: false,
        composer_draft: Some(("independent draft".into(), vec![])),
        claude_fork_id: Some("fork".into()),
    });
    actor
        .action(Action::Draft("newer independent draft".into()))
        .unwrap();
    let newer_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .action(Action::DraftImages(newer_images.clone()))
        .unwrap();
    actor.finish_edit().unwrap();
    assert_eq!(actor.state.saved.draft_images, newer_images);
    assert_eq!(actor.state.saved.items.len(), 1);
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("fork"));
    assert_eq!(actor.state.saved.draft, "newer independent draft");
    assert_eq!(
        actor
            .state
            .saved
            .claude_resume
            .as_ref()
            .unwrap()
            .original_session
            .as_deref(),
        Some("original")
    );
    // Editing an earlier retained turn before resending must branch from
    // the persisted original, never from the not-yet-created fork.
    actor.state.saved.items.push(json!({"id":"earlier-human","type":"userMessage","claudePreviousAssistantUuid":"older-assistant"}));
    actor.state.saved.pending_edit = Some(PendingEdit {
        target: 1,
        turn_id: "older-human".into(),
        text: "older".into(),
        images: vec![],
        recovery: Some(crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap()),
        rewind: false,
        composer_draft: None,
        claude_fork_id: Some("second-fork".into()),
    });
    let pending_command = launch_command(&[], &actor.state.saved).unwrap();
    let pending_args = pending_command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(pending_args
        .windows(2)
        .any(|pair| pair == ["--resume", "original"]));
    actor.finish_edit().unwrap();
    assert_eq!(
        actor
            .state
            .saved
            .claude_resume
            .as_ref()
            .unwrap()
            .original_session
            .as_deref(),
        Some("original")
    );
    let durable: SavedSession = provider::load_saved_session(&journal).unwrap();
    assert!(durable.pending_edit.is_none());
    assert!(durable.claude_resume.is_some());
    fs_err::remove_file(journal).unwrap();
}

#[test]
fn rewind_keep_files_handoff_and_recovery_preserve_claude_history_and_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let mut actor = actor();
    actor.state.saved.key = format!("test-claude-rewind-{}", nonce());
    actor.state.saved.cwd = directory.path().to_string_lossy().into_owned();
    actor.state.saved.thread_id = Some("original".into());
    actor.state.saved.draft = "independent".into();
    actor.state.saved.draft_images = vec!["independent-image".into()];
    actor.state.saved.items = vec![
        json!({"id":"assistant","type":"agentMessage","claudeAssistantUuid":"anchor","text":"retained"}),
        json!({"id":"activity","type":"groveActivity","text":"turn metadata"}),
        json!({"id":"human","type":"userMessage","claudeHumanUuid":"human-uuid","claudePreviousAssistantUuid":"anchor","content":[{"type":"text","text":"original prompt"}]}),
    ].into();
    actor.state.saved.edit_checkpoints.push(EditCheckpoint {
        start_item: 1,
        turn_id: Some("human-uuid".into()),
        files: None,
    });
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    assert!(actor.state.rewind_target(2).is_ok());
    assert!(actor
        .state
        .rewind_files_unavailable(2)
        .unwrap()
        .contains("No workspace checkpoint"));
    let journal = provider::recovery_dir()
        .unwrap()
        .join(format!("{}.json", actor.state.saved.key));
    let pending = PendingEdit {
        target: 1,
        turn_id: "human-uuid".into(),
        text: "original prompt".into(),
        images: vec!["target-image".into()],
        recovery: None,
        rewind: true,
        composer_draft: None,
        claude_fork_id: Some("fork".into()),
    };
    actor.state.saved.pending_edit = Some(pending.clone());
    let command = launch_command(&[], &actor.state.saved).unwrap();
    let args = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--resume-session-at", "anchor"]),
        "scaffolding before the human must not drop retained context"
    );
    actor.recover_edit().unwrap();
    assert_eq!(actor.state.saved.items.len(), 3);
    assert_eq!(actor.state.saved.draft, "independent");
    actor.state.saved.pending_edit = Some(pending);
    actor.finish_edit().unwrap();
    assert_eq!(actor.state.saved.items.len(), 1);
    assert_eq!(actor.state.saved.draft, "original prompt");
    assert_eq!(actor.state.saved.draft_images, ["target-image"]);
    assert_eq!(actor.state.saved.rewind_drafts[0].text, "independent");
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("fork"));
    assert!(actor.in_flight.is_none());
    let durable: SavedSession = provider::load_saved_session(&journal).unwrap();
    assert_eq!(durable.draft, "original prompt");
    assert_eq!(
        durable.claude_resume.unwrap().assistant_uuid.as_deref(),
        Some("anchor")
    );
    let id = actor.state.saved.rewind_drafts[0].id.clone();
    actor.action(Action::RestoreRewindDraft { id }).unwrap();
    assert_eq!(actor.state.saved.draft, "independent");
    assert_eq!(actor.state.saved.draft_images, ["independent-image"]);
    fs_err::remove_file(journal).unwrap();
}

#[test]
fn rewind_checkpoint_captures_claude_turn_identity_without_git_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.saved.cwd = directory.path().to_string_lossy().into_owned();
    actor
        .submit_inner(1, "non-git turn".into(), vec![])
        .unwrap();
    let sent = read(&mut reader);
    let checkpoint = actor.state.saved.edit_checkpoints.last().unwrap();
    assert_eq!(checkpoint.turn_id.as_deref(), sent["uuid"].as_str());
    assert!(checkpoint.files.is_none());
}

#[test]
fn claude_fork_recipe_survives_save_and_reconnect_before_first_prompt() {
    let mut actor = actor();
    actor.state.saved.thread_id = Some("fork-id".into());
    actor.state.saved.claude_resume = Some(provider::ClaudeResume {
        original_session: Some("original-id".into()),
        assistant_uuid: Some("assistant-id".into()),
        human_uuid: "human-id".into(),
        fork_id: "fork-id".into(),
    });
    let saved: SavedSession =
        serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
    let command = launch_command(&[], &saved).unwrap();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for pair in [
        ["--resume", "original-id"],
        ["--resume-session-at", "assistant-id"],
        ["--session-id", "fork-id"],
    ] {
        assert!(args.windows(2).any(|actual| actual == pair));
    }
    assert!(args.iter().any(|arg| arg == "--fork-session"));
    assert!(!args.iter().any(|arg| arg == "--resume-drops-turn"));
    actor.state.saved.claude_resume = Some(saved.claude_resume.unwrap());
    actor
        .message(json!({"type":"system","subtype":"init","session_id":"fork-id"}))
        .unwrap();
    assert!(actor.state.saved.claude_resume.is_some());
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert!(actor.state.saved.claude_resume.is_some());
    actor.input_uuid = Some("accepted".into());
    actor.in_flight = Some(QueuedPrompt {
        id: 1,
        text: "revised".into(),
        images: vec![],
    });
    actor.acknowledge_input(&json!("unrelated"));
    assert!(actor.state.saved.claude_resume.is_some());
    actor.acknowledge_input(&json!("accepted"));
    assert!(actor.state.saved.claude_resume.is_none());
}

#[test]
fn claude_review_waits_for_authoritative_idle_and_rejects_stale_receipts() {
    let mut actor = actor();
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.state.turn_id = Some("turn".into());
    actor.state.begin_work();
    actor
        .message(json!({"type":"result","subtype":"success","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.review_completion(), None);
    assert_eq!(actor.state.session_status(), "Waiting");
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.session_status(), "Review");
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
    actor.state.begin_work();
    actor.state.status = Status::Working;
    actor
        .message(json!({"type":"result","subtype":"error_during_execution","is_error":true}))
        .unwrap();
    assert_eq!(actor.state.review_completion(), None);
}

#[test]
fn queued_edit_and_remove_respect_provider_sending_lock() {
    let mut actor = actor();
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 41,
        text: "original".into(),
        images: Vec::new(),
    });
    actor.action(Action::BeginQueuedEdit(41)).unwrap();
    actor
        .action(Action::SaveQueuedEdit {
            id: 41,
            text: "revised".into(),
            images: Vec::new(),
        })
        .unwrap();
    assert_eq!(actor.state.queued_prompts[0].text, "revised");
    assert_eq!(actor.state.queue_edit, None);
    assert!(actor.action(Action::SteerQueued(41)).is_err());
    actor.state.queue_sending.push(41);
    assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
    assert!(actor.action(Action::RemoveQueued(41)).is_err());
    assert_eq!(actor.state.queued_prompts.len(), 1);
    actor.state.queue_sending.clear();
    actor.action(Action::RemoveQueued(41)).unwrap();
    assert!(actor.state.queued_prompts.is_empty());
}

#[test]
fn slash_commands_preserve_arguments_without_review_context() {
    for command in ["/compact focus on tests", "/help", "/custom-skill argument"] {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Done;
        actor.submit(41, command.into(), vec![]).unwrap();
        let sent = read(&mut reader);
        assert_eq!(
            sent["message"]["content"],
            json!([{"type":"text", "text":command}])
        );
    }
}

#[test]
fn startup_failure_retains_unsent_queue_for_explicit_resume_after_reconnect() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    actor.state.status = Status::Starting;
    let images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor
        .action(Action::SubmitWithMode {
            id: 51,
            text: "retain".into(),
            images: images.clone(),
            mode: SubmissionMode::Queue,
        })
        .unwrap();
    actor.fail("Claude Code initialization timed out".into());
    assert_eq!(actor.state.queued_prompts[0].text, "retain");
    assert_eq!(actor.state.queued_prompts[0].images, images);
    assert!(actor.state.queue_uncertain.is_empty());
    assert!(!actor.drain_queue());
    let mut reader = transport(&mut actor);
    actor.pending.insert("retry".into(), "initialize".into());
    actor.message(json!({"type":"control_response","response":{"request_id":"retry","subtype":"success","response":{}}})).unwrap();
    assert!(!actor.drain_queue());
    actor.action(Action::ResumeQueue).unwrap();
    assert!(actor.drain_queue());
    let sent = read(&mut reader);
    assert_eq!(
        sent["message"]["content"],
        json!([user_content("retain", &images), vec![json!({"type":"text", "text":crate::review_brief::context(&actor.state.saved.session_review)})]].concat())
    );
    assert!(!actor.drain_queue());
    actor.acknowledge_input(&sent["uuid"]);
    assert!(actor.state.queued_prompts.is_empty());
}

#[test]
fn accepted_prompt_recovery_preserves_newer_text_and_attachments() {
    let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    for newer in [false, true] {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor.initialized = true;
        actor.state.status = Status::Idle;
        actor.state.saved.draft = "sent".into();
        actor.state.saved.draft_images = images.clone();
        actor.submit(42, "sent".into(), images.clone()).unwrap();
        let sent = read(&mut reader);
        if newer {
            actor
                .action(Action::Draft("new unsent draft".into()))
                .unwrap();
        }
        actor
            .message(json!({"type":"user","uuid":sent["uuid"]}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        let recovered = provider::recover_saved_for_test(&actor.state.saved);
        assert_eq!(recovered.draft, if newer { "new unsent draft" } else { "" });
        assert_eq!(
            recovered.draft_images,
            if newer { images.clone() } else { Vec::new() }
        );
    }
}

#[test]
fn queued_prompt_recovery_preserves_independent_draft() {
    let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
    for newer in [false, true] {
        let mut actor = actor();
        actor.state.saved.draft = if newer { "new unsent draft" } else { "sent" }.into();
        actor.state.saved.draft_images = images.clone();
        actor
            .action(Action::SubmitWithMode {
                id: 42,
                text: "sent".into(),
                images: images.clone(),
                mode: SubmissionMode::Queue,
            })
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert_eq!(actor.state.queued_prompts[0].images, images);
        let recovered = provider::recover_saved_for_test(&actor.state.saved);
        assert_eq!(recovered.draft, if newer { "new unsent draft" } else { "" });
        assert_eq!(
            recovered.draft_images,
            if newer { images.clone() } else { Vec::new() }
        );
    }
}

#[test]
fn recovery_restores_launch_configuration() {
    let mut original = actor();
    original.state.saved.config_args = vec!["--model".into(), "sonnet".into(), "--chrome".into()];
    let (_, input) = mpsc::channel();
    let (updates, _) = crate::latest::channel();
    let restored = Actor::new(
        original.state.saved,
        Vec::new(),
        input,
        updates,
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(restored.args, vec!["--model", "sonnet", "--chrome"]);
}

#[test]
fn rich_context_preserves_provider_attribution_and_recovers_old_sessions() {
    let (usage, details) = context_measurement(&rich_context_fixture()).unwrap();
    assert_eq!(usage.used, 128000);
    assert_eq!(usage.window, 200000);
    assert_eq!(details.source, provider::ContextMeasurement::Full);
    assert_eq!(details.categories.len(), 8);
    assert_eq!(details.categories[0].entries[0].label, "Tool results");
    assert!(details.categories[0]
        .entries
        .iter()
        .any(|entry| entry.label == "By tool / Read / results" && entry.tokens == 67000));
    assert!(details.categories[0]
        .entries
        .iter()
        .any(|entry| entry.label == "By attachment / image" && entry.tokens == 20));
    assert_eq!(details.categories[1].entries[0].label, "CLAUDE.md");
    assert_eq!(details.categories[2].entries[0].label, "github / search");
    assert_eq!(details.categories[3].entries[0].label, "grove-ui");
    assert_eq!(details.categories[4].entries[0].label, "reviewer");
    assert_eq!(
        details.categories[5].kind,
        provider::ContextCategoryKind::Free
    );
    assert_eq!(
        details.categories[6].kind,
        provider::ContextCategoryKind::Buffer
    );
    assert_eq!(
        details.categories[7].kind,
        provider::ContextCategoryKind::Deferred
    );
    let mut saved = actor().state.saved;
    saved.context = Some(usage);
    saved.context_details = Some(details);
    let encoded = serde_json::to_value(&saved).unwrap();
    let recovered: SavedSession = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(recovered.context_details, saved.context_details);
    let mut state = Snapshot::new(recovered);
    assert!(!state.context_measurement_live);
    state.record_context(Some(100), Some(200000));
    assert!(state.context_measurement_live);
    state.fail("disconnected".into());
    assert!(!state.context_measurement_live);
    let mut legacy = encoded;
    legacy.as_object_mut().unwrap().remove("context_details");
    let recovered: SavedSession = serde_json::from_value(legacy).unwrap();
    assert!(recovered.context_details.is_none());
}

#[test]
fn mcp_init_inventory_and_recovery_do_not_repeat_old_failures() {
    let mut actor = actor();
    actor.message(json!({"type":"system","subtype":"init","mcp_servers":[{"name":"jira","status":"failed"}]})).unwrap();
    assert_eq!(actor.state.saved.items.len(), 1);
    actor.observe_mcp_servers(&json!([{"name":"jira","status":"failed"}]));
    assert_eq!(actor.state.saved.items.len(), 1);
    actor.observe_mcp_servers(&json!([{"name":"jira","status":"connected"}]));
    actor.observe_mcp_servers(&json!([{"name":"jira","status":"failed","error":"new failure"}]));
    assert_eq!(actor.state.saved.items.len(), 2);
    assert!(actor.state.saved.items.last().unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("new failure"));
}
