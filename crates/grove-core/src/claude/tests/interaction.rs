use super::*;

#[test]
fn permission_probe_reuses_success_and_preserves_cancellation() {
    let cache = PermissionProbeCache::default();
    let stopping = AtomicBool::new(false);
    assert_eq!(
        cache.get("cli".into(), &stopping, || vec!["plan".into()]),
        vec!["plan"]
    );
    assert_eq!(
        cache.get("cli".into(), &stopping, || panic!("cached probe spawned")),
        vec!["plan"]
    );
    stopping.store(true, Ordering::SeqCst);
    assert!(cache
        .get("cli".into(), &stopping, || panic!("canceled probe spawned"))
        .is_empty());
}

#[test]
fn permission_probe_failure_expires_and_overlapping_calls_join() {
    let cache = Arc::new(PermissionProbeCache::default());
    cache.entries.lock().unwrap().insert(
        "failed".into(),
        PermissionProbe::Ready(
            vec![],
            Instant::now().checked_sub(PERMISSION_HELP_RETRY).unwrap(),
        ),
    );
    assert_eq!(
        cache.get("failed".into(), &AtomicBool::new(false), || vec![
            "default".into()
        ]),
        vec!["default"]
    );
    let (started, receive) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let first = std::thread::spawn({
        let cache = cache.clone();
        move || {
            cache.get("shared".into(), &AtomicBool::new(false), || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                vec!["plan".into()]
            })
        }
    });
    receive.recv().unwrap();
    let second = std::thread::spawn({
        let cache = cache.clone();
        move || {
            cache.get("shared".into(), &AtomicBool::new(false), || {
                panic!("duplicate probe")
            })
        }
    });
    release.send(()).unwrap();
    assert_eq!(first.join().unwrap(), vec!["plan"]);
    assert_eq!(second.join().unwrap(), vec!["plan"]);
}

#[test]
fn side_chat_profile_denies_commands_and_permission_requests() {
    let mut actor = actor();
    actor.side_chat = true;
    let mut reader = transport(&mut actor);
    for (name, expected) in [
        ("Read", "allow"),
        ("Bash", "deny"),
        ("Write", "deny"),
        ("ExitPlanMode", "deny"),
    ] {
        actor.message(json!({"type":"control_request","request_id":name,"request":{"subtype":"can_use_tool","tool_name":name,"input":{}}})).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            frame["response"]["response"]["behavior"], expected,
            "{line}"
        );
    }
    assert!(actor.state.approvals.is_empty());
}

#[test]
#[cfg(unix)]
fn permission_mode_discovery_times_out_and_honors_cancellation() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 10"]);
    let started = Instant::now();
    assert!(
        discover_permission_modes(command, &AtomicBool::new(false), Duration::from_millis(50))
            .is_empty()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 10"]);
    assert!(
        discover_permission_modes(command, &AtomicBool::new(true), Duration::from_secs(2))
            .is_empty()
    );
}

#[test]
fn permission_selection_dispatches_before_first_prompt_and_waits_for_ack() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.resolved_access = Some("default".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    assert_eq!(
        actor.permission_changes.len(),
        1,
        "selection must send a permission control request without requiring a prompt"
    );
    let request = read(&mut reader);
    assert_eq!(
        request["request"],
        json!({"subtype":"set_permission_mode","mode":"plan"})
    );
    assert_eq!(actor.state.status, Status::Idle);
    assert!(actor.state.access.is_none());
    assert!(actor.state.saved.composer.access.is_none());
    actor
        .submit(41, "an explicit user prompt".into(), vec![])
        .unwrap();
    assert!(actor.submit(42, "another prompt".into(), vec![]).is_err());
    assert_eq!(actor.state.rejected_submission, Some(42));
    assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
    assert!(actor.in_flight.is_none());
    assert!(!actor.drain_queue());
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
    assert_eq!(actor.state.access.as_deref(), Some("plan"));
    assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
    assert!(actor.drain_queue());
    assert_eq!(read(&mut reader)["type"], "user");
}

#[test]
fn permission_selection_after_restoring_defaults_dispatches_without_a_prompt() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.access = Some("dontAsk".into());
    actor.state.resolved_access = Some("dontAsk".into());
    actor.state.saved.composer.access = Some("dontAsk".into());
    actor.action(Action::ResetConfiguration).unwrap();
    assert!(actor.reset_configuration);
    for (mode, success) in [("plan", false), ("acceptEdits", true)] {
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some(mode.into()),
            })
            .unwrap();
        assert_eq!(
            actor.permission_changes.len(),
            1,
            "restoring defaults must not block later live permission selections"
        );
        let request = read(&mut reader);
        assert_eq!(
            request["request"],
            json!({"subtype":"set_permission_mode","mode":mode})
        );
        assert_eq!(actor.state.resolved_access.as_deref(), Some("dontAsk"));
        assert!(actor.state.saved.composer.access.is_none());
        actor
            .message(json!({"type":"control_response","response":{
                "subtype":if success { "success" } else { "error" },
                "request_id":request["request_id"], "error":"mode unavailable"
            }}))
            .unwrap();
    }
    assert_eq!(actor.state.access.as_deref(), Some("acceptEdits"));
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("acceptEdits")
    );
    assert!(actor.state.error.is_none());
    assert!(
        actor.reset_configuration,
        "model/default restoration still requires restart"
    );
    assert!(actor.in_flight.is_none());
    assert!(actor.deferred.is_none());
}

#[test]
fn permission_selection_requires_launch_capability_for_bypass() {
    let mut actor = actor();
    actor.state.permission_modes = vec!["default".into(), "bypassPermissions".into()];
    let selection = || Action::Configure {
        model: None,
        effort: None,
        access: Some("bypassPermissions".into()),
    };
    assert!(actor.action(selection()).is_err());
    assert!(actor.pending_access.is_none());
    for args in [
        vec!["--append-system-prompt=--dangerously-skip-permissions".into()],
        vec![
            "--append-system-prompt".into(),
            "--allow-dangerously-skip-permissions".into(),
        ],
    ] {
        actor.state.saved.config_args = args;
        assert!(actor.action(selection()).is_err());
    }
    for flag in [
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
    ] {
        actor.state.saved.config_args = vec![flag.into()];
        actor.action(selection()).unwrap();
        assert_eq!(actor.pending_access.as_deref(), Some("bypassPermissions"));
    }
}

#[test]
fn rejected_permission_selection_allows_acknowledged_recovery() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.state.resolved_access = Some("default".into());
    for (mode, success) in [("plan", false), ("acceptEdits", true)] {
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some(mode.into()),
            })
            .unwrap();
        let request = read(&mut reader);
        assert_eq!(request["request"]["mode"], mode);
        assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
        actor
            .message(json!({"type":"control_response","response":{
                "subtype":if success { "success" } else { "error" },
                "request_id":request["request_id"], "error":"mode unavailable"
            }}))
            .unwrap();
    }
    assert_eq!(actor.state.resolved_access.as_deref(), Some("acceptEdits"));
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("acceptEdits")
    );
    assert!(actor.state.error.is_none());
    assert!(actor.permission_changes.is_empty());
    assert_eq!(actor.state.status, Status::Idle);
}

#[test]
fn permission_ack_preserves_a_newer_unrelated_error() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.permission_error = Some("mode unavailable".into());
    actor.state.error = Some("authentication expired".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    let request = read(&mut reader);
    actor
        .message(json!({"type":"control_response","response":{
            "subtype":"success", "request_id":request["request_id"]
        }}))
        .unwrap();
    assert_eq!(actor.state.error.as_deref(), Some("authentication expired"));
    assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
}

#[test]
fn permission_selection_survives_initialization_and_rejection_keeps_effective_access() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initial = Some("perform the launch task".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    assert!(actor.permission_changes.is_empty());
    actor
        .pending
        .insert("initialize".into(), "initialize".into());
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
    assert_eq!(actor.permission_changes.len(), 1);
    let request = read(&mut reader);
    assert_eq!(
        request["request"],
        json!({"subtype":"set_permission_mode","mode":"plan"})
    );
    actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":"mode unavailable"}})).unwrap();
    assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
    assert!(actor.state.saved.composer.access.is_none());
    assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
    assert!(actor.initial.is_none());
    assert!(actor.deferred.is_none());
    assert!(actor.in_flight.is_none());
    assert_eq!(actor.state.status, Status::Idle);
    assert!(!actor.drain_queue());
    assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
}

#[test]
fn startup_launch_prompt_waits_for_successful_permission_selection() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initial = Some("perform the launch task".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    actor
        .pending
        .insert("initialize".into(), "initialize".into());
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
    let request = read(&mut reader);
    assert!(!actor.drain_queue());
    assert_eq!(actor.initial.as_deref(), Some("perform the launch task"));
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
    assert!(actor.drain_queue());
    let prompt = read(&mut reader);
    assert_eq!(prompt["type"], "user");
    assert_eq!(
        prompt["message"]["content"][0]["text"],
        "perform the launch task"
    );
    assert_eq!(actor.state.access.as_deref(), Some("plan"));
    assert!(actor.initial.is_none());
    assert!(!actor.drain_queue());
}

#[test]
fn busy_permission_selection_serializes_changes_without_applying_model_or_effort() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("active".into());
    actor
        .action(Action::Configure {
            model: Some("sonnet".into()),
            effort: Some("low".into()),
            access: Some("plan".into()),
        })
        .unwrap();
    assert_eq!(actor.permission_changes.len(), 1);
    let first = read(&mut reader);
    assert_eq!(actor.pending.len(), 1);
    assert!(actor
        .redirect(42, "steer while the mode is pending".into(), vec![])
        .is_err());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("acceptEdits".into()),
        })
        .unwrap();
    assert_eq!(actor.permission_changes.len(), 1);
    assert_eq!(actor.pending_access.as_deref(), Some("acceptEdits"));
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":first["request_id"]}})).unwrap();
    assert_eq!(actor.state.access.as_deref(), Some("plan"));
    let second = read(&mut reader);
    assert_eq!(
        second["request"],
        json!({"subtype":"set_permission_mode","mode":"acceptEdits"})
    );
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":second["request_id"]}})).unwrap();
    assert_eq!(actor.state.access.as_deref(), Some("acceptEdits"));
    assert_eq!(actor.state.status, Status::Working);
    assert_eq!(actor.state.turn_id.as_deref(), Some("active"));
    assert!(
        actor.reconfigure,
        "model and effort remain pending for the next turn"
    );
    assert!(actor.pending.is_empty());
    assert!(actor.in_flight.is_none());
}

#[test]
fn claude_permission_change_ack_is_bound_to_request_and_fallback_is_authoritative() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.resolved_access = Some("default".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    actor.configure_live().unwrap();
    let request = read(&mut reader);
    let effort = read(&mut reader);
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("acceptEdits".into()),
        })
        .unwrap();
    assert!(actor.state.access.is_none());
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
    assert_eq!(actor.state.access.as_deref(), Some("plan"));
    assert_eq!(actor.pending_access.as_deref(), Some("acceptEdits"));
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":effort["request_id"]}})).unwrap();
    actor
        .message(json!({"type":"system","subtype":"status","permissionMode":"default"}))
        .unwrap();
    assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("default")
    );
}

#[test]
fn pending_access_selection_survives_model_reset_without_claiming_acknowledgement() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.resolved_access = Some("default".into());
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    actor.action(Action::ResetModelEffort).unwrap();
    assert_eq!(actor.pending_access.as_deref(), Some("plan"));
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("default")
    );
    actor
        .pending
        .insert("initialize".into(), "initialize".into());
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"current_permission_mode":"default"}}})).unwrap();
    assert!(actor.reconfigure);
    assert!(actor.pending_access.is_none());
    let request = read(&mut reader);
    assert_eq!(
        request["request"],
        json!({"subtype":"set_permission_mode","mode":"plan"})
    );
    assert_eq!(actor.permission_changes.len(), 1);
    assert_eq!(actor.state.resolved_access.as_deref(), Some("default"));
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("default")
    );
    actor
        .message(json!({"type":"control_response","response":{
            "subtype":"success", "request_id":request["request_id"]
        }}))
        .unwrap();
    assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
    assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
}

#[test]
fn permission_status_before_ack_preserves_acknowledged_explicit_mode_and_fallback() {
    for effective in ["plan", "default"] {
        let mut actor = actor();
        let mut reader = transport(&mut actor);
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("plan".into()),
            })
            .unwrap();
        actor.configure_live().unwrap();
        let request = read(&mut reader);
        let _effort = read(&mut reader);
        actor
            .message(json!({"type":"system","subtype":"status","permissionMode":effective}))
            .unwrap();
        assert!(actor.state.saved.composer.access.is_none());
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
        assert_eq!(
            actor.state.saved.composer.access.as_deref(),
            Some(effective)
        );
        assert_eq!(actor.state.resolved_access.as_deref(), Some(effective));
    }
}

#[test]
fn rejected_claude_permission_change_retains_previous_access_and_stops_deferred_prompt() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.access = Some("bypassPermissions".into());
    actor.state.saved.composer.access = actor.state.access.clone();
    actor.state.resolved_access = actor.state.access.clone();
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("plan".into()),
        })
        .unwrap();
    actor.configure_live().unwrap();
    let request = read(&mut reader);
    let effort = read(&mut reader);
    actor.deferred = Some(QueuedPrompt {
        id: 99,
        text: "do work".into(),
        images: vec![],
    });
    actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":"mode unavailable"}})).unwrap();
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":effort["request_id"]}})).unwrap();
    assert_eq!(
        actor.state.saved.composer.access.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(
        actor.state.resolved_access.as_deref(),
        Some("bypassPermissions")
    );
    assert!(actor.deferred.is_none());
    assert!(!actor.queue_ready);
    assert_eq!(actor.state.rejected_submission, Some(99));
    assert_eq!(actor.state.error.as_deref(), Some("mode unavailable"));
    assert_eq!(actor.state.status, Status::Idle);
}

#[test]
fn claude_child_permission_uses_task_alias_and_parent_completion_preserves_it() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.state.status = Status::Working;
    actor.state.saved.thread_id = Some("primary".into());
    actor.block("agent-tool", &json!({"type":"tool_use","id":"agent-tool","name":"Task","input":{"name":"Reviewer","description":"Review permissions"}}));
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"sdk-task","tool_use_id":"agent-tool","task_type":"local_agent","subagent_type":"Reviewer"})).unwrap();
    actor.message(json!({"type":"control_request","request_id":"child-request","parent_tool_use_id":"agent-tool","request":{"subtype":"can_use_tool","agent_id":"sdk-task","tool_use_id":"bash-1","tool_name":"Bash","input":"cargo test"}})).unwrap();
    assert_eq!(actor.state.approvals.len(), 1);
    assert_eq!(
        actor.state.interaction_agent(&json!("child-request")),
        Some("Reviewer · ent-tool")
    );
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.approvals.len(), 1);
    assert_eq!(actor.state.status, Status::Waiting);
    actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"completed"})).unwrap();
    assert_eq!(actor.state.approvals.len(), 0);
    assert_eq!(actor.state.status, Status::Idle);
}

#[test]
fn failed_or_interrupted_turn_pauses_queue_and_clears_approvals() {
    let mut actor = actor();
    actor.state.approvals.push(json!({"id":"approval"}));
    actor.state.turn_id = Some("turn".into());
    actor.message(json!({"type":"result","is_error":true,"errors":["auth failed"],"session_id":"persisted-id"})).unwrap();
    assert_eq!(actor.state.status, Status::Failed);
    assert!(!actor.queue_ready);
    assert!(actor.state.approvals.is_empty());
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("persisted-id"));
    assert_eq!(actor.state.error.as_deref(), Some("auth failed"));
}

#[test]
fn native_permission_mappings_are_explicit() {
    assert_eq!(permission_mode("read-only").unwrap(), "plan");
    assert_eq!(permission_mode("workspace-write").unwrap(), "acceptEdits");
    assert!(permission_mode("unknown").is_err());
}

#[test]
fn approvals_roundtrip_exact_input_and_reject_expired_ids() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    let input = json!({"command":"printf '%s' '$HOME'","timeout":123});
    actor.message(json!({"type":"control_request","request_id":"approval","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool","input":input}})).unwrap();
    assert_eq!(actor.state.status, Status::Waiting);
    actor
        .action(Action::Approval(json!("approval"), true))
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"approval","response":{"behavior":"allow","updatedInput":input}}})
    );
    assert!(actor
        .action(Action::Approval(json!("approval"), true))
        .is_err());
    assert!(actor.state.approvals.is_empty());
}

#[test]
fn questions_roundtrip_multi_custom_and_preserve_original_input() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    let mut reader = transport(&mut actor);
    let input = json!({"questions":[
        {"header":"Choice","question":"Which?","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}],"multiSelect":true},
        {"header":"Name","question":"Name?","options":[{"label":"Default","description":"Suggested"}],"multiSelect":false}
    ],"metadata":{"source":"fixture"}});
    let frame = json!({"type":"control_request","request_id":"questions","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}});
    actor.message(frame.clone()).unwrap();
    actor.message(frame.clone()).unwrap();
    assert_eq!(actor.state.questions.len(), 1);
    assert_eq!(actor.state.status, Status::Waiting);
    let answers = HashMap::from([
        ("Which?".into(), vec!["A".into(), "B".into()]),
        ("Name?".into(), vec!["Custom name".into()]),
    ]);
    actor
        .action(Action::AnswerQuestions(json!("questions"), Some(answers)))
        .unwrap();
    let mut expected = input;
    expected["answers"] = json!({"Which?":"A, B","Name?":"Custom name"});
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"questions","response":{"behavior":"allow","updatedInput":expected}}})
    );
    assert!(actor.state.questions.is_empty());
    assert_eq!(actor.state.question_answer_acks.len(), 1);
    assert_eq!(actor.state.question_answer_acks[0].id, json!("questions"));
    assert!(actor
        .action(Action::AnswerQuestions(json!("questions"), None))
        .is_err());
    actor.message(frame.clone()).unwrap();
    actor
        .action(Action::AnswerQuestions(json!("questions"), None))
        .unwrap();
    assert_eq!(
        read(&mut reader)["response"]["response"]["behavior"],
        "deny"
    );
    assert_eq!(
        actor.state.question_answer_acks.len(),
        1,
        "denial is not an answer"
    );
    actor.message(frame).unwrap();
    actor
        .message(json!({"type":"control_cancel_request","request_id":"questions"}))
        .unwrap();
    assert!(actor.state.questions.is_empty());
    assert!(actor
        .action(Action::AnswerQuestions(json!("questions"), None))
        .is_err());
}

#[test]
fn extended_questions_roundtrip_text_number_and_reject_invalid_answers() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    let mut reader = transport(&mut actor);
    let input = json!({"title":"Before building", "questions":[
        {"header":"Title","question":"What title?","kind":"text","placeholder":"e.g. Quarterly briefing","description":"Shown on the cover."},
        {"header":"Size","question":"How many slides?","kind":"number","min":1,"max":20,"step":1,"defaultValue":8,"unit":"slides"}
    ]});
    let frame = json!({"type":"control_request","request_id":"extended","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}});
    actor.message(frame.clone()).unwrap();
    assert_eq!(actor.state.questions.len(), 1);
    assert_eq!(actor.state.status, Status::Waiting);
    let answers = |number: &str| {
        HashMap::from([
            ("What title?".into(), vec!["Quarterly briefing".into()]),
            ("How many slides?".into(), vec![number.into()]),
        ])
    };
    for invalid in ["21", "NaN", "eight"] {
        assert!(actor
            .action(Action::AnswerQuestions(
                json!("extended"),
                Some(answers(invalid))
            ))
            .is_err());
        assert_eq!(actor.state.questions.len(), 1);
    }
    actor
        .action(Action::AnswerQuestions(
            json!("extended"),
            Some(answers("8")),
        ))
        .unwrap();
    let mut expected = input;
    expected["answers"] = json!({"What title?":"Quarterly briefing","How many slides?":"8"});
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"extended","response":{"behavior":"allow","updatedInput":expected}}})
    );
    actor.message(frame.clone()).unwrap();
    assert_eq!(
        actor.state.questions.len(),
        1,
        "a resolved provider request id may be reused after the response settles"
    );
    actor
        .action(Action::AnswerQuestions(json!("extended"), None))
        .unwrap();
    assert_eq!(
        read(&mut reader)["response"]["response"]["behavior"],
        "deny"
    );
    actor.message(frame).unwrap();
    actor
        .message(json!({"type":"control_cancel_request","request_id":"extended"}))
        .unwrap();
    assert!(actor.state.questions.is_empty());
    assert!(actor
        .action(Action::AnswerQuestions(
            json!("extended"),
            Some(answers("8"))
        ))
        .is_err());
}

#[test]
fn mcp_questions_reject_invalid_form_answers_and_roundtrip_decline_cancel() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn".into());
    let mut reader = transport(&mut actor);
    let form = json!({"type":"control_request","request_id":"form","request":{
        "subtype":"elicitation","mcp_server_name":"server","message":"Choose settings",
        "requested_schema":{"type":"object","properties":{
            "name":{"type":"string","minLength":2},
            "count":{"type":"integer","minimum":1,"maximum":10},
            "ready":{"type":"boolean"},
            "color":{"type":"string","enum":["blue","green"]},
            "features":{"type":"array","items":{"type":"string","enum":["a","b"]},"minItems":1}
        },"required":["name","count","ready","color","features"]}
    }});
    actor.message(form.clone()).unwrap();
    assert_eq!(actor.state.elicitations.len(), 1);
    let valid = json!({"name":"Deck","count":8,"ready":false,"color":"blue","features":["a","b"]});
    for (field, invalid) in [
        ("name", json!("x")),
        ("count", json!(11)),
        ("ready", json!("yes")),
        ("color", json!("red")),
        ("features", json!([])),
    ] {
        let mut content = valid.clone();
        content[field] = invalid;
        assert!(actor
            .action(Action::RespondElicitation {
                id: json!("form"),
                action: "accept".into(),
                content: Some(content)
            })
            .is_err());
        assert_eq!(actor.state.elicitations.len(), 1);
    }
    actor
        .action(Action::RespondElicitation {
            id: json!("form"),
            action: "accept".into(),
            content: Some(valid.clone()),
        })
        .unwrap();
    assert_eq!(
        read(&mut reader)["response"]["response"],
        json!({"action":"accept","content":valid})
    );
    for action in ["decline", "cancel"] {
        actor.message(form.clone()).unwrap();
        actor
            .action(Action::RespondElicitation {
                id: json!("form"),
                action: action.into(),
                content: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader)["response"]["response"],
            json!({"action":action})
        );
    }
}

#[test]
fn malformed_extended_question_constraints_are_denied_without_waiting() {
    for row in [
        json!({"kind":"number","min":10,"max":1}),
        json!({"kind":"number","min":1}),
        json!({"kind":"number","min":1,"max":10,"step":0}),
        json!({"kind":"number","min":1,"max":10,"defaultValue":11}),
        json!({"kind":"number","min":1,"max":10,"multiSelect":true}),
        json!({"kind":"text","options":[{"label":"A"}]}),
    ] {
        let mut actor = actor();
        actor.state.status = Status::Working;
        let mut reader = transport(&mut actor);
        let mut row = row;
        row["question"] = json!("How many?");
        row["header"] = json!("Size");
        actor.message(json!({"type":"control_request","request_id":"invalid","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[row]}}})).unwrap();
        assert!(actor.state.questions.is_empty());
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(
            read(&mut reader)["response"]["response"]["behavior"],
            "deny"
        );
    }
}

#[test]
fn launch_advertises_only_rendered_extended_question_schema() {
    let actor = actor();
    let command = launch_command(&[], &actor.state.saved).unwrap();
    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDE_CODE_QUESTION_EXTENDED")
            .and_then(|(_, value)| value),
        Some(std::ffi::OsStr::new("1"))
    );
}

#[test]
fn scoped_model_effort_reset_restarts_with_preserved_access() {
    for explicit in [false, true] {
        let mut actor = actor();
        actor.state.resolved_access = Some("plan".into());
        actor
            .action(Action::Configure {
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                access: explicit.then(|| "plan".into()),
            })
            .unwrap();
        actor.action(Action::ResetModelEffort).unwrap();
        assert!(actor.reset_configuration);
        assert!(actor.reconfigure);
        assert!(actor.state.model.is_none());
        assert!(actor.state.effort.is_none());
        assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
        let recovered: SavedSession =
            serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
        assert!(recovered.composer.model.is_none());
        assert!(recovered.composer.effort.is_none());
        assert_eq!(recovered.composer.access.as_deref(), Some("plan"));
        let command = launch_command(&[], &recovered).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|arg| arg == "--model" || arg == "--effort"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "plan"]));
        actor
            .action(Action::Configure {
                model: Some("haiku".into()),
                effort: Some("low".into()),
                access: None,
            })
            .unwrap();
        // A later selection must still restart, rather than configuring the
        // original process with its stale model/effort override.
        assert!(actor.reset_configuration);
        assert_eq!(actor.state.saved.composer.access.as_deref(), Some("plan"));
        assert_eq!(actor.state.saved.composer.model.as_deref(), Some("haiku"));
        assert_eq!(actor.state.saved.composer.effort.as_deref(), Some("low"));
    }
}
