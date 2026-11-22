use super::*;

#[test]
fn side_chat_profile_cannot_inherit_or_change_write_permissions() {
    let mut actor = actor();
    actor.side_chat = true;
    actor.state.model = Some("larger-parent-model".into());
    actor.state.access = Some("full-access".into());
    let params = actor.turn_params(vec![]);
    assert_eq!(params["model"], "gpt-6-luna");
    assert_eq!(params["approvalPolicy"], "never");
    assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
    assert!(actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("full-access".into()),
        })
        .is_err());
    actor.capture_edit_checkpoint();
    assert!(actor.state.saved.edit_checkpoints.is_empty());
    // An invalid recovery key would fail normal persistence. Side snapshots
    // are exclusively the parent session's responsibility.
    actor.state.saved.key = "invalid key".into();
    actor.save();
    assert!(actor.state.error.is_none());
}

#[test]
fn answer_echo_text_is_only_for_nonsecret_async_questions() {
    let params = serde_json::json!({"questions":[{"id":"scope", "question":"Which scope?"}]});
    let mut request =
        super::super::QuestionRequest::from_codex(serde_json::json!("ask"), &params).unwrap();
    let answers = std::collections::HashMap::from([("scope".into(), vec!["All".into()])]);
    assert_eq!(request.answer_echo_text(&answers), None);
    request.source = super::super::QuestionSource::AsyncMessage;
    assert_eq!(
        request.answer_echo_text(&answers).as_deref(),
        Some("Which scope?\nAll")
    );
    request.questions[0].secret = true;
    assert_eq!(request.answer_echo_text(&answers), None);
}

#[test]
fn fresh_yolo_options_survive_saved_session_resume_for_both_providers() {
    for provider in [Provider::Codex, Provider::Claude] {
        let args = provider.agent().launch_args_for_backend(true, true, false);
        let options = if provider == Provider::Codex {
            launch_options(&args).unwrap().0
        } else {
            crate::claude::launch_options(&args).unwrap().0
        };
        assert!(!options.is_empty());
        let mut session = saved();
        session.provider = provider;
        session.native_config_version = 1;
        session.config_args = options.clone();
        session.composer.access = Some(
            if provider == Provider::Codex {
                "full-access"
            } else {
                "bypassPermissions"
            }
            .into(),
        );
        let mut recovered = recover_saved_for_test(&session);
        recovered.migrate_native_config();
        assert_eq!(recovered.config_args, options);
        assert_eq!(recovered.composer.access, session.composer.access);
    }
}

#[test]
fn legacy_native_bypass_migration_preserves_explicit_access_and_custom_options() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut legacy = serde_json::to_value(saved()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("native_config_version");
        let mut session: SavedSession = serde_json::from_value(legacy).unwrap();
        session.provider = provider;
        session.composer.access = Some("full-access".into());
        session.config_args = if provider == Provider::Codex {
            vec![
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"danger-full-access\"",
                "-c",
                "model=\"custom\"",
            ]
        } else {
            vec![
                "--dangerously-skip-permissions",
                "--chrome",
                "--allowedTools",
                "Read",
            ]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let explicit = session.config_args.clone();
        let mut native = session.clone();
        native.native_config_version = 1;
        native.migrate_native_config();
        assert_eq!(native.config_args, explicit);
        session.migrate_native_config();
        assert_eq!(session.composer.access.as_deref(), Some("full-access"));
        assert_eq!(
            session.config_args,
            if provider == Provider::Codex {
                vec!["-c", "model=\"custom\""]
            } else {
                vec!["--chrome", "--allowedTools", "Read"]
            }
        );
        let mut recovered: SavedSession =
            serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
        recovered.migrate_native_config();
        assert_eq!(recovered.config_args, session.config_args);
        assert_eq!(recovered.native_config_version, 1);
    }
    let mut custom = saved();
    custom.native_config_version = 0;
    custom.config_args = vec!["-c".into(), "approval_policy=\"never\"".into()];
    let expected = custom.config_args.clone();
    custom.migrate_native_config();
    assert_eq!(custom.config_args, expected);
}

#[test]
fn effective_codex_access_separates_sandbox_approvals_and_preserves_richer_reset() {
    let mut actor = actor();
    let sandbox = json!({"type":"workspaceWrite","writableRoots":["/custom"],"networkAccess":false,"excludeSlashTmp":true,"readOnlyAccess":{"type":"restricted","readableRoots":["/read"]}});
    actor.state.sandbox = Some(json!({"type":"dangerFullAccess"}));
    actor.state.approval_policy = Some(json!("on-request"));
    assert_eq!(
        actor.state.configured_access_summary(),
        "No sandbox · ask on request"
    );
    actor.configured_turn =
        json!({"sandboxPolicy":sandbox,"approvalPolicy":{"reject":{"sandbox_approval":true}}});
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("full-access".into()),
        })
        .unwrap();
    actor.action(Action::ResetConfiguration).unwrap();
    assert_eq!(actor.turn_params(vec![])["sandboxPolicy"], sandbox);
    assert_eq!(actor.state.sandbox, Some(sandbox));
    assert_eq!(
        actor.state.approval_policy,
        Some(json!({"reject":{"sandbox_approval":true}}))
    );
    assert_eq!(
        actor.state.configured_access_summary(),
        "Workspace access · configured approvals"
    );
}

#[test]
fn scoped_model_effort_reset_preserves_access_and_survives_recovery() {
    for explicit in [false, true] {
        let mut actor = actor();
        actor.configured_turn = json!({"model":"configured","effort":"medium","sandboxPolicy":{"type":"readOnly"},"approvalPolicy":"on-request"});
        let effective_access = if explicit {
            "full-access"
        } else {
            "workspace-write"
        };
        actor.state.resolved_access = Some(effective_access.into());
        let inherited_sandbox = json!({"type":"workspaceWrite","writableRoots":["/custom"],"networkAccess":true,"excludeTmpdirEnvVar":true});
        actor.state.sandbox = Some(inherited_sandbox.clone());
        actor
            .action(Action::Configure {
                model: Some("selected".into()),
                effort: Some("high".into()),
                access: explicit.then(|| "full-access".into()),
            })
            .unwrap();
        actor.action(Action::ResetModelEffort).unwrap();
        assert_eq!(
            actor.state.resolved_access.as_deref(),
            Some(effective_access)
        );
        assert_eq!(actor.state.sandbox, Some(inherited_sandbox));
        let params = actor.turn_params(vec![]);
        assert_eq!(params["model"], "configured");
        assert_eq!(params["effort"], "medium");
        if explicit {
            assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
            assert_eq!(params["approvalPolicy"], "never");
        } else {
            assert!(params.get("sandboxPolicy").is_none());
            assert!(params.get("approvalPolicy").is_none());
            assert!(actor.state.saved.composer.access.is_none());
        }
        let recovered =
            serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
        let (_, input) = mpsc::channel();
        let (updates, _) = crate::latest::channel();
        let mut restored = Actor::new(recovered, input, updates, Vec::new());
        restored.configured_turn = actor.configured_turn.clone();
        assert!(restored.state.saved.composer.reset_model_effort_only);
        assert_eq!(restored.turn_params(vec![]), params);
        restored
            .action(Action::Configure {
                model: None,
                effort: Some("low".into()),
                access: None,
            })
            .unwrap();
        let params = restored.turn_params(vec![]);
        assert!(restored.state.saved.composer.reset_model_effort_only);
        assert_eq!(params["model"], "configured");
        assert_eq!(params["effort"], "low");
        if explicit {
            assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
        } else {
            assert!(params.get("sandboxPolicy").is_none());
            assert!(params.get("approvalPolicy").is_none());
        }
        restored.action(Action::ResetConfiguration).unwrap();
        assert!(!restored.state.saved.composer.reset_model_effort_only);
        assert_eq!(
            restored.turn_params(vec![])["sandboxPolicy"]["type"],
            "readOnly"
        );
    }
}

#[test]
#[cfg(unix)]
fn steer_rejection_preserves_active_turn_and_approval() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.approvals.push(json!({"id":99}));
    actor
        .submit_mode(7, "adjust".into(), Vec::new(), SubmissionMode::Steer)
        .unwrap();
    assert!(
        matches!(&actor.pending[&1].kind, Pending::Steer { submission: 7, turn, .. } if turn == "turn-1")
    );
    assert_eq!(actor.state.accepted_submission, None);
    actor
        .message(json!({"id":1,"error":{"message":"no active turn"}}))
        .unwrap();
    assert_eq!(actor.state.rejected_submission, Some(7));
    assert!(actor.queue_paused);
    assert!(!actor.queue_ready);
    assert_eq!(actor.state.status, Status::Working);
    assert_eq!(actor.state.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(actor.state.approvals.len(), 1);
    actor
        .submit_mode(8, "adjust again".into(), Vec::new(), SubmissionMode::Steer)
        .unwrap();
    assert_eq!(actor.state.error, None);
    actor
        .message(json!({"id":2,"result":{"turnId":"turn-1"}}))
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(8));
    assert_eq!(actor.state.error, None);
}

#[test]
fn completed_turn_expires_approvals_and_clears_active_turn() {
    let mut state = state();
    state.approvals.push(json!({"id":1}));
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}}));
    assert_eq!(state.status, Status::Interrupted);
    assert!(state.approvals.is_empty());
    assert!(state.turn_id.is_none());
}

#[test]
fn disconnect_invalidates_all_permissions() {
    let mut state = state();
    state.approvals.push(json!({"id":1}));
    state.fail("lost connection".into());
    assert_eq!(state.status, Status::Disconnected);
    assert!(state.approvals.is_empty());
    assert!(state.turn_id.is_none());
}

#[test]
fn expired_approval_is_rejected() {
    let mut actor = actor();
    assert!(actor
        .action(Action::Approval(json!("old"), true))
        .unwrap_err()
        .contains("expired"));
}

#[cfg(unix)]
#[test]
fn approval_routes_exact_turn_and_unsupported_request_fails_closed() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.message(json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"old"}})).unwrap();
    assert!(actor.state.approvals.is_empty());
    actor.message(json!({"id":8,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1"}})).unwrap();
    assert!(actor.state.approvals.is_empty());
    assert!(actor
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("unsupported"));
    actor.message(json!({"id":9,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1"}})).unwrap();
    assert_eq!(actor.state.approvals.len(), 1);
    actor.action(Action::Approval(json!(9), false)).unwrap();
    assert!(actor.state.approvals.is_empty());
    assert_eq!(actor.state.status, Status::Working);
}

#[test]
fn child_approval_requires_its_exact_turn_and_survives_parent_completion() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .interaction_children
        .register("thread-1", "thread-1", "child-1", Some("Reviewer"), None);
    actor
        .interaction_children
        .start("child-1", Some("child-turn"));
    actor.message(json!({"id":70,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1"}})).unwrap();
    assert!(actor.state.approvals.is_empty());
    actor.message(json!({"id":71,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"stale-turn"}})).unwrap();
    assert!(actor.state.approvals.is_empty());
    actor.message(json!({"id":72,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"child-turn"}})).unwrap();
    assert_eq!(actor.state.approvals.len(), 1);
    assert_eq!(
        actor.state.interaction_agent(&json!(72)),
        Some("Reviewer · child-1")
    );
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert_eq!(actor.state.approvals.len(), 1);
    assert_eq!(actor.state.status, Status::Waiting);
    actor.message(json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"completed"}}})).unwrap();
    assert!(actor.state.approvals.is_empty());
    assert!(actor.state.interaction_owners.is_empty());
    assert_eq!(actor.state.status, Status::Idle);
}

#[test]
fn async_question_metadata_survives_completion_and_history_without_duplicates() {
    let mut state = state();
    let message = async_question_message();
    state.apply(&message);
    state.apply(&message);
    assert_eq!(state.questions.len(), 1);
    let request = &state.questions[0];
    assert_eq!(request.questions.len(), 2);
    assert_eq!(request.questions[0].options[1].label, "Composer");
    assert!(request
        .questions
        .iter()
        .all(|question| question.allow_custom));
    assert!(request.questions[1].options.is_empty());
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}));
    assert_eq!(state.status, Status::Waiting);
    assert_eq!(state.questions.len(), 1);
    state.questions.clear();
    state.restore_async_questions();
    assert_eq!(state.questions.len(), 1);
    state.resolve_async_question(&json!("async-question"));
    state.apply(&message);
    state.restore_async_questions();
    assert!(state.questions.is_empty());
}

#[test]
fn async_questions_reject_invalid_metadata_and_survive_unrelated_user_reply() {
    let mut state = state();
    let mut message = async_question_message();
    message["params"]["item"]["questions"][0]["options"] = json!(["All", "All"]);
    state.apply(&message);
    assert!(state.questions.is_empty());
    state.apply(&async_question_message());
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"userMessage","id":"reply","content":[{"type":"text","text":"All"}]}}}));
    assert_eq!(state.questions.len(), 1);
    state.questions.clear();
    state.restore_async_questions();
    state.apply(&async_question_message());
    assert_eq!(state.questions.len(), 1);
}

#[cfg(unix)]
#[test]
fn async_question_answers_use_user_input_preserve_draft_and_retry() {
    use std::os::unix::process::CommandExt;
    let mut actor = actor();
    let mut child = Command::new("cat")
        .process_group(0)
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
    actor.state.saved.draft = "Unsent draft".into();
    actor.state.saved.draft_images = vec!["draft.png".into()];
    actor.message(async_question_message()).unwrap();
    let answers = HashMap::from([
        ("0".into(), vec!["All".into()]),
        ("1".into(), vec!["Keep it tidy".into()]),
    ]);
    actor
        .action(Action::AnswerQuestions(
            json!("async-question"),
            Some(answers.clone()),
        ))
        .unwrap();
    assert!(actor
        .action(Action::AnswerQuestions(
            json!("async-question"),
            Some(answers.clone())
        ))
        .is_err());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let sent: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(sent["method"], "turn/start");
    assert_eq!(sent["params"]["threadId"], "thread-1");
    assert_eq!(
        sent["params"]["input"][0]["text"],
        "Which changes?\nAll\n\nAny details?\nKeep it tidy"
    );
    assert_eq!(
        actor.state.questions.len(),
        1,
        "retain until acknowledgement"
    );
    // An echoed user message can arrive before the request acknowledgement.
    // It must neither dismiss this card nor turn a rejection into data loss.
    actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"userMessage","id":"echo","content":[{"type":"text","text":"Which changes?\nAll"}]}}})).unwrap();
    actor
        .message(json!({"id":sent["id"],"error":{"message":"Try again"}}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Waiting);
    assert_eq!(actor.state.questions.len(), 1);
    assert!(actor.state.question_answer_acks.is_empty());
    actor.state.turn_id = None;
    actor
        .action(Action::AnswerQuestions(
            json!("async-question"),
            Some(answers),
        ))
        .unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let retry: Value = serde_json::from_str(&line).unwrap();
    actor
        .message(json!({"id":retry["id"],"result":{"turn":{"id":"turn-2"}}}))
        .unwrap();
    assert!(actor.state.questions.is_empty());
    assert_eq!(actor.state.turn_id.as_deref(), Some("turn-2"));
    assert_eq!(actor.state.question_answer_acks.len(), 1);
    let ack = &actor.state.question_answer_acks[0];
    assert_eq!(ack.id, json!("async-question"));
    assert_eq!(ack.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(
        ack.turn_id, None,
        "capture submission scope, not acknowledged turn"
    );
    assert_eq!(actor.state.saved.draft, "Unsent draft");
    assert_eq!(actor.state.saved.draft_images, vec!["draft.png"]);
    actor.state.turn_id = Some("turn-1".into());
    let mut new_question = async_question_message();
    new_question["params"]["item"]["id"] = json!("dismiss");
    actor.message(new_question.clone()).unwrap();
    actor
        .action(Action::AnswerQuestions(json!("dismiss"), None))
        .unwrap();
    actor.message(new_question).unwrap();
    assert!(actor.state.questions.is_empty());
    assert!(
        actor.pending.is_empty(),
        "dismissal must not send a provider RPC"
    );
    let mut cancelled = async_question_message();
    cancelled["params"]["item"]["id"] = json!("cancelled-in-flight");
    actor.message(cancelled).unwrap();
    actor
        .action(Action::AnswerQuestions(
            json!("cancelled-in-flight"),
            Some(HashMap::from([
                ("0".into(), vec!["All".into()]),
                ("1".into(), vec!["Details".into()]),
            ])),
        ))
        .unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let submitted: Value = serde_json::from_str(&line).unwrap();
    actor
        .state
        .resolve_async_question(&json!("cancelled-in-flight"));
    // A provider can reuse its wire id for a new request before the old reply arrives.
    let mut replacement_item = async_question_message()["params"]["item"].clone();
    replacement_item["id"] = json!("cancelled-in-flight");
    let replacement = QuestionRequest::from_async_message(&replacement_item).unwrap();
    assert!(!replacement.turn_bound());
    actor.state.questions.push(replacement);
    actor
        .message(json!({"id":submitted["id"],"result":{"turn":{"id":"turn-3"}}}))
        .unwrap();
    assert_eq!(
        actor.state.question_answer_acks.len(),
        1,
        "late success after cancellation is not an answer receipt"
    );
    assert_eq!(
        actor.state.questions.len(),
        1,
        "late success must not remove the replacement request"
    );
    assert_eq!(actor.state.questions[0].id, json!("cancelled-in-flight"));
}

#[cfg(unix)]
#[test]
fn question_requests_roundtrip_grouped_answers_and_expire() {
    use std::os::unix::process::CommandExt;
    let mut actor = actor();
    let mut child = Command::new("cat")
        .process_group(0)
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
    let request = json!({"id":81,"method":"item/tool/requestUserInput","params":{
    "threadId":"thread-1","turnId":"turn-1","itemId":"tool","isBlocking":true,
    "questions":[
        {"id":"size","header":"Size","question":"Which size?","options":[{"label":"Small","description":"Compact"}]},
        {"id":"token","header":"Token","question":"Enter token","isSecret":true,"options":null}
    ]}});
    actor.message(request.clone()).unwrap();
    actor.message(request).unwrap();
    assert_eq!(actor.state.questions.len(), 1);
    assert_eq!(actor.state.status, Status::Waiting);
    assert!(actor.state.approvals.is_empty());
    assert!(actor
        .action(Action::AnswerQuestions(
            json!(81),
            Some(HashMap::from([("size".into(), vec!["Small".into()])]))
        ))
        .is_err());
    assert_eq!(actor.state.questions.len(), 1);
    let answers = HashMap::from([
        ("size".into(), vec!["Small".into()]),
        ("token".into(), vec!["private-token".into()]),
    ]);
    actor
        .action(Action::AnswerQuestions(json!(81), Some(answers)))
        .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"id":81,"result":{"answers":{"size":{"answers":["Small"]},"token":{"answers":["private-token"]}}}})
    );
    assert!(!serde_json::to_string(&actor.state.saved)
        .unwrap()
        .contains("private-token"));
    assert_eq!(actor.state.question_answer_acks.len(), 1);
    assert_eq!(actor.state.question_answer_acks[0].id, json!(81));
    assert_eq!(actor.state.question_answer_acks[0].sequence, 1);
    assert!(!format!("{:?}", actor.state.question_answer_acks).contains("private-token"));
    assert!(!serde_json::to_string(&actor.state.saved)
        .unwrap()
        .contains("question_answer_acks"));
    assert!(actor.state.questions.is_empty());
    assert!(actor
        .action(Action::AnswerQuestions(json!(81), None))
        .is_err());
    actor.message(json!({"id":82,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
    actor
        .action(Action::AnswerQuestions(json!(82), None))
        .unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"id":82,"result":{"answers":{}}})
    );
    assert_eq!(
        actor.state.question_answer_acks.len(),
        1,
        "cancellation is not an answer"
    );
    actor.message(json!({"id":83,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
    actor.message(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":83}})).unwrap();
    assert!(actor
        .action(Action::AnswerQuestions(json!(83), None))
        .is_err());
    actor.message(json!({"id":84,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert!(actor.state.questions.is_empty());
    assert!(actor
        .action(Action::AnswerQuestions(json!(84), None))
        .is_err());
}

#[test]
fn failed_question_transport_does_not_acknowledge_or_remove_request() {
    let mut actor = actor();
    actor.state.questions.push(
        QuestionRequest::from_codex(
            json!(91),
            &json!({"questions":[{"id":"q","question":"Name?"}]}),
        )
        .unwrap(),
    );
    assert!(actor
        .action(Action::AnswerQuestions(
            json!(91),
            Some(HashMap::from([("q".into(), vec!["Alice".into()]),]))
        ))
        .is_err());
    assert!(actor.state.question_answer_acks.is_empty());
    assert_eq!(actor.state.questions.len(), 1);
    assert!(actor.state.questions[0].turn_bound());
}

#[test]
fn question_acknowledgements_are_ephemeral_across_restart_and_new_conversation() {
    let mut snapshot = state();
    snapshot
        .question_answer_acks
        .push(snapshot.question_answer_ack(json!(91)));
    let serialized = serde_json::to_value(&snapshot.saved).unwrap();
    assert!(serialized.get("question_answer_acks").is_none());
    let restored: SavedSession = serde_json::from_value(serialized).unwrap();
    assert!(Snapshot::new(restored).question_answer_acks.is_empty());
    assert!(Snapshot::new(snapshot.saved.fresh_conversation())
        .question_answer_acks
        .is_empty());
}

#[test]
fn question_validation_rejects_ambiguous_ids_and_unoffered_answers() {
    for questions in [
        json!([]),
        json!([{"id":"","question":"q"}]),
        json!([{"id":"q","question":"q"},{"id":"q","question":"other"}]),
        json!([{"id":"q","question":"q","options":[{"label":"A"},{"label":"A"}]}]),
    ] {
        assert!(QuestionRequest::from_codex(json!(1), &json!({"questions":questions})).is_err());
    }
    let request = QuestionRequest::from_codex(
        json!(1),
        &json!({"questions":[{"id":"q","question":"q","options":[{"label":"A"}]}]}),
    )
    .unwrap();
    assert!(request
        .validate(&HashMap::from([("q".into(), vec!["unoffered".into()])]))
        .is_err());
    assert!(request
        .validate(&HashMap::from([("q".into(), vec!["A".into(), "A".into()])]))
        .is_err());
}

#[test]
fn launch_preserves_defaults_and_only_maps_explicit_permission_bypass() {
    assert_eq!(launch_options(&[]).unwrap(), (Vec::new(), None));
    let args = crate::agent::Agent::Codex.launch_args(false, false);
    assert!(launch_options(&args).unwrap().0.is_empty());
    let args = crate::agent::Agent::Codex.launch_args(true, false);
    let (args, _) = launch_options(&args).unwrap();
    assert!(args.contains(&"sandbox_mode=\"danger-full-access\"".into()));
    assert!(args.contains(&"approval_policy=\"never\"".into()));
    assert!(launch_options(&["--add-dir".into(), "/extra".into()]).is_ok());
    assert!(launch_options(&["--add-dir".into()]).is_err());
}

#[test]
fn resolved_server_request_expires_approval_immediately() {
    let mut state = state();
    state.status = Status::Waiting;
    state.approvals.push(json!({"id":7}));
    state.apply(
        &json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":7}}),
    );
    assert!(state.approvals.is_empty());
    assert_eq!(state.status, Status::Working);
}
