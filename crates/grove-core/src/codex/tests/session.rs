use super::*;

#[test]
fn capabilities_preserve_provider_support_and_gate_active_redirects() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut saved = saved();
        saved.provider = provider;
        let mut state = Snapshot::new(saved);
        state.status = Status::Working;
        state.turn_id = Some("turn".into());
        assert_eq!(
            state.can_redirect_active_turn(),
            provider == Provider::Codex
        );
        state.can_redirect = true;
        assert!(state.can_redirect_active_turn());
        state.compacting = true;
        assert!(!state.can_redirect_active_turn());
        state.compacting = false;
        state.status = Status::Disconnected;
        assert!(!state.can_redirect_active_turn());
        state.edit_supported = true;
        // Remembered support does not authorize acting while disconnected.
        assert!(state.capabilities().edit_messages);
        assert!(!state.can_redirect_active_turn());
    }
}

#[test]
fn session_waiting_requires_explicit_open_parent_wait_tool() {
    let mut snapshot = Snapshot::new(saved());
    snapshot.begin_work();
    snapshot.status = Status::Working;
    snapshot.saved.work_periods[0].subagents.push(Subagent {
        id: "child".into(),
        name: "Child".into(),
        activity: "Working".into(),
        role: None,
        task: None,
        status: SubagentStatus::Working,
    });
    snapshot.saved.items.push(json!({"id":"server","type":"commandExecution","command":"npm run dev","status":"inProgress"}));
    assert_eq!(snapshot.session_status(), "Working");
    snapshot.saved.items.push(
        json!({"id":"wait","type":"collabAgentToolCall","tool":"wait","status":"inProgress"}),
    );
    assert_eq!(snapshot.session_status(), "Waiting");
    snapshot.saved.items[1]["status"] = json!("completed");
    assert_eq!(snapshot.session_status(), "Working");
    snapshot.reconnecting = true;
    snapshot.status = Status::Starting;
    assert_eq!(snapshot.session_status(), "Reconnecting");
    snapshot.fail("connection failed".into());
    assert_eq!(snapshot.session_status(), "Disconnected");
    assert!(!snapshot.reconnecting);
}

/// Real-session clone/update CPU timings; excludes rendering, I/O and provider latency.
#[test]
#[ignore = "manual snapshot payload benchmark; GROVE_PERF_SESSION selects a private fixture"]
fn snapshot_payload_benchmark() {
    use std::hint::black_box;
    let path = std::env::var("GROVE_PERF_SESSION").expect("GROVE_PERF_SESSION");
    let saved = load_saved_session(std::path::Path::new(&path)).unwrap();
    let state = Snapshot::new(saved);
    let expected = state.saved.items.len();
    let serialized = serde_json::to_value(&state.saved).unwrap();
    assert_eq!(serialized["items"].as_array().unwrap().len(), expected);
    let sample = |mutate: bool| {
        let started = Instant::now();
        for _ in 0..50 {
            let mut cloned = black_box(&state).clone();
            if mutate {
                cloned.saved.items.push(
                    json!({"id":"perf-append", "type":"agentMessage", "text":"Completed append."}),
                );
                assert_eq!(cloned.saved.items.len(), expected + 1);
                assert_eq!(
                    cloned.saved.items.last().unwrap()["text"],
                    "Completed append."
                );
            } else {
                assert_eq!(cloned.saved.items.len(), expected);
            }
            black_box(cloned);
        }
        started.elapsed().as_secs_f64() * 1000.0 / 50.0
    };
    for _ in 0..3 {
        black_box(sample(false));
        black_box(sample(true));
    }
    for index in 0..15 {
        let (clone_ms, append_ms) = if index % 2 == 0 {
            (sample(false), sample(true))
        } else {
            let append = sample(true);
            (sample(false), append)
        };
        println!(
            "snapshot_sample={{\"sample\":{index},\"items\":{expected},\"clone_ms\":{clone_ms:.6},\"clone_append_ms\":{append_ms:.6}}}"
        );
    }
    assert_eq!(state.saved.items.len(), expected);
    assert_eq!(serde_json::to_value(&state.saved).unwrap(), serialized);
}

/// Includes full recovery-file serialization, atomic replacement and durability syncs.
#[test]
#[ignore = "manual private recovery persistence benchmark"]
fn snapshot_persistence_benchmark() {
    let path = std::env::var("GROVE_PERF_SESSION").expect("GROVE_PERF_SESSION");
    let saved = load_saved_session(std::path::Path::new(&path)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let expected = serde_json::to_value(&saved).unwrap();
    for sample in 0..11 {
        let start = Instant::now();
        persist_in_dir(&saved, directory.path()).unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let actual = serde_json::to_value(
            load_saved_session(&directory.path().join(format!("{}.json", saved.key))).unwrap(),
        )
        .unwrap();
        assert_eq!(actual, expected);
        if sample >= 2 {
            println!(
                "persistence_sample={{\"sample\":{},\"persist_ms\":{elapsed:.6}}}",
                sample - 2
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn uncertain_delivery_requires_explicit_dismissal_then_resume() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor
        .submit_mode(41, "unknown".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor
        .submit_mode(42, "next".into(), Vec::new(), SubmissionMode::Queue)
        .unwrap();
    actor.action(Action::SteerQueued(41)).unwrap();
    actor
        .message(json!({"id":1,"result":{"turnId":"wrong"}}))
        .unwrap();
    assert_eq!(actor.state.queue_uncertain, vec![41]);
    actor.state.turn_id = None;
    actor.state.status = Status::Idle;
    assert!(actor.action(Action::ResumeQueue).is_err());
    assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
    assert!(actor.action(Action::RemoveQueued(41)).is_err());
    assert!(actor.action(Action::DismissUncertainQueued(42)).is_err());
    actor.action(Action::DismissUncertainQueued(41)).unwrap();
    assert!(actor.queue_paused);
    assert!(!actor.queue_ready);
    assert!(actor.pending.is_empty());
    assert_eq!(actor.state.queued_prompts[0].id, 42);
    actor.action(Action::ResumeQueue).unwrap();
    assert_eq!(actor.state.queue_sending, vec![42]);
}

#[test]
fn composer_overrides_are_explicit_and_native() {
    let mut actor = actor();
    let defaults = actor.turn_params(Vec::new());
    assert!(defaults.get("model").is_none());
    assert!(defaults.get("effort").is_none());
    assert!(defaults.get("approvalPolicy").is_none());
    assert!(defaults.get("sandboxPolicy").is_none());
    actor
        .action(Action::Configure {
            model: Some("test-model".into()),
            effort: Some("low".into()),
            access: Some("read-only".into()),
        })
        .unwrap();
    let params = actor.turn_params(Vec::new());
    assert_eq!(params["model"], "test-model");
    assert_eq!(params["effort"], "low");
    assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
    assert_eq!(params["approvalPolicy"], "on-request");
    actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("full-access".into()),
        })
        .unwrap();
    let params = actor.turn_params(Vec::new());
    assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
    assert_eq!(params["approvalPolicy"], "never");
    assert!(actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("invalid".into())
        })
        .is_err());
}

#[test]
fn pasted_images_validate_and_survive_rejected_turn_until_ack() {
    assert!(validate_prompt("", &["data:image/png;base64,aGVsbG8=".into()]).is_ok());
    assert!(validate_prompt("", &[]).is_err());
    assert!(validate_images(&["file:///private/image.png".into()]).is_err());
    assert!(validate_images(&["https://example.test/image.png".into()]).is_err());
    assert!(validate_images(&["data:image/png;base64,INVALID!".into()]).is_err());
    assert!(validate_images(&["data:image/png;base64,a=AA".into()]).is_err());
    let oversized = base64::engine::general_purpose::STANDARD.encode(vec![0; 20 * 1024 * 1024 + 1]);
    assert!(validate_images(&[format!("data:image/png;base64,{oversized}")]).is_err());
    let mut actor = actor();
    actor.state.saved.draft = "Keep draft".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
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
        .message(json!({"id":9,"error":{"message":"model unavailable"}}))
        .unwrap();
    assert_eq!(actor.state.saved.draft, "Keep draft");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    actor.pending.insert(
        10,
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
        .message(json!({"id":10,"result":{"turn":{"id":"accepted"}}}))
        .unwrap();
    assert!(actor.state.saved.draft.is_empty());
    assert!(actor.state.saved.draft_images.is_empty());
}

#[test]
fn resumed_provider_history_preserves_checklist_chronology() {
    let a = json!({"id":"progress-a","type":"agentMessage","text":"Inspecting"});
    let b = json!({"id":"progress-b","type":"agentMessage","text":"Verifying"});
    let snapshot = |id, first, second| json!({"id":id,"type":"plan","taskSnapshot":true,"plan":[{"step":"Inspect","status":first},{"step":"Verify","status":second}]});
    let saved = vec![
        snapshot("grove-plan-t", "inProgress", "pending"),
        a.clone(),
        snapshot("grove-plan-t-2", "completed", "inProgress"),
        b.clone(),
        snapshot("grove-plan-t-4", "completed", "completed"),
    ];
    let restored = restore_task_snapshots(&saved, vec![a, b]);
    assert_eq!(restored, saved);
    assert_eq!(
        crate::tasks::activity_owner(&restored, 1).unwrap().step,
        "Inspect"
    );
    assert_eq!(
        crate::tasks::activity_owner(&restored, 3).unwrap().step,
        "Verify"
    );
    assert!(crate::tasks::current(&restored).unwrap().all_done());
}

#[test]
fn immutable_plan_ids_do_not_collide_after_history_shortens() {
    let mut state = state();
    state.saved.items = vec![
        json!({"id":"grove-plan-turn-1","type":"plan","plan":[{"step":"First","status":"completed"}]}),
        json!({"id":"grove-plan-turn-1-2","type":"plan","plan":[{"step":"Second","status":"inProgress"}]}),
    ].into();
    state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Third","status":"inProgress"}]}}));
    assert_eq!(state.saved.items.len(), 3);
    assert_eq!(state.saved.items[1]["plan"][0]["step"], "Second");
    assert_eq!(state.saved.items[2]["plan"][0]["step"], "Third");
    let unique: std::collections::HashSet<_> = state
        .saved
        .items
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert_eq!(unique.len(), 3);
}

#[test]
fn routes_only_matching_thread_and_turn() {
    let mut state = state();
    assert!(!state.apply(&json!({"method":"turn/completed","params":{"threadId":"other","turn":{"id":"turn-1","status":"completed"}}})));
    assert!(!state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"old-turn","status":"completed"}}})));
    assert_eq!(state.status, Status::Working);
    assert_eq!(state.turn_id.as_deref(), Some("turn-1"));
}

#[test]
fn completed_item_replaces_stream_without_duplicate_text() {
    let mut state = state();
    for delta in ["Hello ", "world"] {
        assert!(state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":delta}})));
    }
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"item-1","type":"agentMessage","text":"Hello world"}}}));
    assert_eq!(
        state
            .saved
            .items
            .iter()
            .find(|item| item["id"] == "item-1")
            .unwrap()["groveCompleted"],
        true
    );
    assert_eq!(state.saved.items.len(), 1);
    assert_eq!(state.transcript(), "Hello world");
}

#[test]
fn failed_state_with_active_turn_cannot_send_duplicate() {
    let mut actor = actor();
    actor.state.status = Status::Failed;
    assert!(actor
        .prompt("second".into())
        .unwrap_err()
        .contains("current turn"));
}

#[cfg(unix)]
#[test]
fn resume_in_progress_preserves_active_turn() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Thread,
            deadline: Instant::now(),
        },
    );
    actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","status":{"type":"active","activeFlags":[]},"turns":[{"id":"resumed-turn","status":"inProgress","items":[]}]}}})).unwrap();
    assert_eq!(actor.state.turn_id.as_deref(), Some("resumed-turn"));
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.prompt("duplicate".into()).is_err());
}

#[test]
fn resumed_history_retains_known_receipt_time_and_leaves_unknown_time_absent() {
    let mut actor = actor();
    actor.state.saved.items =
        vec![json!({"id":"known", "type":"userMessage", "groveReceivedAt":123})].into();
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Thread,
            deadline: Instant::now(),
        },
    );
    actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","turns":[{"id":"old-turn","status":"completed","items":[
        {"id":"known","type":"userMessage","content":[]},
        {"id":"unknown","type":"agentMessage","text":"historical response"}
    ]}]}}})).unwrap();
    assert_eq!(actor.state.saved.items[0]["groveReceivedAt"], 123);
    assert!(actor.state.saved.items[1].get("groveReceivedAt").is_none());
}

#[test]
fn historical_unfinished_turn_is_not_a_live_process_after_resume() {
    let mut actor = actor();
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Thread,
            deadline: Instant::now(),
        },
    );
    actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","status":{"type":"idle"},"turns":[{"id":"stale","status":"inProgress","items":[]}]}}})).unwrap();
    assert_eq!(actor.state.status, Status::Interrupted);
    assert!(actor.state.turn_id.is_none());
}

#[test]
fn resumed_thread_uses_provider_name_without_changing_stable_identity() {
    let mut actor = actor();
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Thread,
            deadline: Instant::now(),
        },
    );
    actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","name":"Provider restored title","turns":[]}}})).unwrap();
    assert_eq!(
        actor.state.saved.title.as_deref(),
        Some("Provider restored title")
    );
    assert_eq!(actor.state.saved.key, "test-native-1");
    assert_eq!(actor.state.saved.label, "Codex");
}

#[cfg(unix)]
#[test]
fn additional_controls_roundtrip_exact_wire_and_lifecycle() {
    use std::os::unix::process::CommandExt;
    fn read(reader: &mut impl BufRead) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
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
    actor.message(json!({"id":901,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","kind":"writeStdin","command":"yes","proposedExecpolicyAmendment":["echo"]}})).unwrap();
    assert!(actor
        .action(Action::RespondApproval {
            id: json!(901),
            choice: "network_0".into(),
            input: None,
            feedback: None
        })
        .is_err());
    actor
        .action(Action::RespondApproval {
            id: json!(901),
            choice: "execpolicy".into(),
            input: None,
            feedback: None,
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"id":901,"result":{"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["echo"]}}}})
    );
    actor.message(json!({"id":902,"method":"item/permissions/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","permissions":{"network":{"enabled":true}}}})).unwrap();
    actor
        .action(Action::RespondApproval {
            id: json!(902),
            choice: "permission_session".into(),
            input: None,
            feedback: None,
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"id":902,"result":{"permissions":{"network":{"enabled":true}},"scope":"session"}})
    );
    let request = json!({"id":903,"method":"mcpServer/elicitation/request","params":{"threadId":"thread-1","turnId":null,"serverName":"server","mode":"form","message":"Count?","requestedSchema":{"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]}}});
    actor.message(request.clone()).unwrap();
    actor.message(request.clone()).unwrap();
    assert_eq!(actor.state.elicitations.len(), 1);
    actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    assert_eq!(actor.state.status, Status::Waiting);
    assert_eq!(actor.state.elicitations.len(), 1);
    assert!(actor
        .action(Action::RespondElicitation {
            id: json!(903),
            action: "accept".into(),
            content: Some(json!({"count":0}))
        })
        .is_err());
    actor
        .action(Action::RespondElicitation {
            id: json!(903),
            action: "accept".into(),
            content: Some(json!({"count":2})),
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"id":903,"result":{"action":"accept","content":{"count":2}}})
    );
    assert!(actor
        .action(Action::RespondElicitation {
            id: json!(903),
            action: "accept".into(),
            content: Some(json!({"count":2}))
        })
        .is_err());
    actor.message(json!({"id":907,"method":"mcpServer/elicitation/request","params":{"threadId":"thread-1","serverName":"server","mode":"url","message":"Authorize","elicitationId":"flow","url":"https://example.com/auth"}})).unwrap();
    actor
        .action(Action::RespondElicitation {
            id: json!(907),
            action: "accept".into(),
            content: None,
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"id":907,"result":{"action":"accept"}})
    );
    actor.message(request).unwrap();
    actor.state.fail("Disconnected".into());
    assert!(actor.state.elicitations.is_empty());
    actor
        .message(json!({"id":904,"method":"item/tool/call","params":{"tool":"unregistered"}}))
        .unwrap();
    let response = read(&mut reader);
    assert_eq!(response["id"], 904);
    assert_eq!(response["result"]["success"], false);
    for (id, method) in [
        (905, "account/chatgptAuthTokens/refresh"),
        (906, "attestation/generate"),
    ] {
        actor
            .message(json!({"id":id,"method":method,"params":{}}))
            .unwrap();
        assert_eq!(read(&mut reader)["error"]["code"], -32601);
    }
}

#[test]
fn context_token_accounting_preserves_subcounts_and_unknown_fields() {
    let mut state = state();
    let mut event = usage("turn-1", 21_684, Some(475_000));
    event["params"]["tokenUsage"]["last"]["cachedInputTokens"] = json!(7168);
    event["params"]["tokenUsage"]["last"]["cacheWriteInputTokens"] = json!(0);
    assert!(state.apply(&event));
    let accounting = state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .as_ref()
        .unwrap();
    let last = accounting.last.unwrap();
    assert_eq!(last.input_tokens, Some(21_679));
    assert_eq!(last.cached_input_tokens, Some(7168));
    assert_eq!(last.cache_write_input_tokens, Some(0));
    assert_eq!(last.output_tokens, Some(5));
    assert_eq!(last.reasoning_output_tokens, Some(0));
    assert_eq!(last.total_tokens, Some(21_684));
    assert_eq!(
        state.saved.context.unwrap().used,
        21_684,
        "subcounts must not be added again"
    );
    assert_eq!(accounting.total.as_ref().unwrap().cached_input_tokens, None);

    let recovered: SavedSession =
        serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
    assert_eq!(recovered.context_details, state.saved.context_details);
    let mut old = serde_json::to_value(&state.saved).unwrap();
    old["context_details"]
        .as_object_mut()
        .unwrap()
        .remove("accounting");
    let old: SavedSession = serde_json::from_value(old).unwrap();
    assert!(old.context_details.unwrap().accounting.is_none());

    let newer = json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":"turn-1",
        "tokenUsage":{"last":{"totalTokens":0,"inputTokens":0},"modelContextWindow":475000}}});
    assert!(state.apply(&newer));
    let accounting = state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .as_ref()
        .unwrap();
    assert_eq!(accounting.last.unwrap().input_tokens, Some(0));
    assert_eq!(accounting.last.unwrap().cached_input_tokens, None);
    assert!(
        accounting.total.is_none(),
        "a missing accumulated report must not reuse older totals"
    );
    assert_eq!(state.saved.context.unwrap().used, 0);
}
