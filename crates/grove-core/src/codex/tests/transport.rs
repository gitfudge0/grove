use super::*;

#[test]
fn side_chat_profile_can_retry_a_stopped_turn_after_resume() {
    for side_chat in [false, true] {
        let mut actor = actor();
        actor.side_chat = side_chat;
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now() + Duration::from_secs(5),
            },
        );
        actor
            .message(json!({"id":1,"result":{
                "model":"gpt-6-luna", "sandbox":{"type":"readOnly"},
                "approvalPolicy":"never", "thread":{
                    "id":"side-thread", "status":{"type":"idle"},
                    "turns":[{"id":"stopped-turn","status":"inProgress","items":[]}]
                }
            }}))
            .unwrap();
        assert!(actor.state.turn_id.is_none());
        if side_chat {
            assert_eq!(actor.state.status, Status::Idle);
            assert!(actor.state.error.is_none());
            assert!(actor.pending.is_empty()); // no automatic retry prompt
        } else {
            assert_eq!(actor.state.status, Status::Interrupted);
            assert!(actor
                .state
                .error
                .as_deref()
                .unwrap()
                .contains("previous turn stopped"));
        }
    }
}

#[test]
fn side_chat_retention_is_scoped_to_session_and_checkout() {
    use super::super::*;
    let mut session = saved();
    session.side_chats.insert(
        "/checkout/a".into(),
        SavedSideChat {
            session: None,
            draft: "unsent".into(),
            messages: vec![
                ("user".into(), "question".into()),
                ("assistant".into(), "answer".into()),
            ],
        },
    );
    session.side_chats.insert(
        "/checkout/b".into(),
        SavedSideChat {
            session: None,
            draft: "other".into(),
            messages: Vec::new(),
        },
    );
    let dir = tempfile::tempdir().unwrap();
    persist_in_dir(&session, dir.path()).unwrap();
    let path = dir.path().join(format!("{}.json", session.key));
    let decoded = load_saved_session(&path).unwrap();
    assert_eq!(decoded.side_chats["/checkout/a"].draft, "unsent");
    assert_eq!(decoded.side_chats["/checkout/a"].messages.len(), 2);
    assert_eq!(decoded.side_chats["/checkout/b"].draft, "other");
    assert!(decoded.fresh_conversation().side_chats.is_empty());
    let mut old = serde_json::to_value(&session).unwrap();
    old.as_object_mut().unwrap().remove("side_chats");
    assert!(serde_json::from_value::<SavedSession>(old)
        .unwrap()
        .side_chats
        .is_empty());
    let mut actor = actor();
    actor
        .action(Action::SideChat {
            checkout: "/checkout/a".into(),
            chat: session.side_chats["/checkout/a"].clone(),
        })
        .unwrap();
    assert_eq!(actor.state.saved.side_chats["/checkout/a"].draft, "unsent");
    actor
        .action(Action::SideChatDraft {
            checkout: "/checkout/a".into(),
            draft: "edited".into(),
        })
        .unwrap();
    assert_eq!(actor.state.saved.side_chats["/checkout/a"].draft, "edited");
    assert_eq!(
        actor.state.saved.side_chats["/checkout/a"].messages.len(),
        2
    );
}

#[test]
fn provider_notice_does_not_revive_after_clear_and_identical_raw_error() {
    let mut state = state();
    state.set_provider_error("Codex", "Connection interrupted".into(), true, None, None);
    state.clear_error();
    state.error = Some("Connection interrupted".into());
    assert!(state.effective_notice().is_none());
}

#[test]
fn provider_warning_is_historical_and_foreign_errors_are_ignored() {
    let mut state = state();
    assert!(state.apply(&json!({"method":"configWarning","params":{"summary":"Configuration warning","details":"Unsupported setting"}})));
    assert_eq!(state.saved.items.last().unwrap()["type"], "providerNotice");
    let count = state.saved.items.len();
    state.apply(&json!({"method":"configWarning","params":{"summary":"Configuration warning","details":"Unsupported setting"}}));
    assert_eq!(state.saved.items.len(), count);
    assert!(state.error.is_none());
    assert_eq!(state.status, Status::Working);
    assert!(!state.apply(
        &json!({"method":"error","params":{"threadId":"foreign","error":{"message":"Not ours"}}})
    ));
    assert!(state.error.is_none());
}

#[test]
#[cfg(unix)]
fn stop_targets_observed_generation_and_reports_rejected_process() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1","rootTurnId":"turn-1"}}})).unwrap();
    a.message(json!({"method":"thread/started","params":{"thread":{"id":"child","source":{"subAgent":{"thread_spawn":{"parent_thread_id":"thread-1"}}}}}})).unwrap();
    a.message(json!({"method":"turn/started","params":{"threadId":"child","turn":{"id":"child-turn","rootTurnId":"turn-1"}}})).unwrap();
    a.message(json!({"method":"thread/started","params":{"thread":{"id":"nested","source":{"subAgent":{"thread_spawn":{"parent_thread_id":"child"}}}}}})).unwrap();
    a.message(json!({"method":"turn/started","params":{"threadId":"nested","turn":{"id":"nested-turn","rootTurnId":"turn-1"}}})).unwrap();
    a.message(json!({"method":"item/started","params":{"threadId":"nested","turnId":"nested-turn","item":{"type":"commandExecution","id":"command","source":"unifiedExecStartup","processId":"p1"}}})).unwrap();
    // An unrelated turn cannot establish process ownership.
    a.message(json!({"method":"item/started","params":{"threadId":"nested","turnId":"old","item":{"type":"commandExecution","id":"old-command","source":"unifiedExecStartup","processId":"old-p"}}})).unwrap();
    a.action(Action::Interrupt).unwrap();
    assert!(a.queue_paused);
    assert!(a
        .pending
        .values()
        .any(|p| matches!(&p.kind, Pending::Interrupt { turn, .. } if turn == "turn-1")));
    let children: HashSet<_> = a
        .pending
        .values()
        .filter_map(|p| match &p.kind {
            Pending::StopChild { thread, turn, .. } => Some((thread.as_str(), turn.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        children,
        HashSet::from([("child", "child-turn"), ("nested", "nested-turn")])
    );
    let list = a
        .pending
        .iter()
        .find_map(|(id, p)| {
            matches!(&p.kind, Pending::StopList { thread, .. } if thread == "nested").then_some(*id)
        })
        .unwrap();
    a.message(json!({"id":list,"result":{"data":[{"itemId":"command","processId":"p1"},{"itemId":"old-command","processId":"old-p"}]}})).unwrap();
    let process = a
        .pending
        .iter()
        .find_map(|(id, p)| match &p.kind {
            Pending::StopProcess {
                thread, process, ..
            } if thread == "nested" && process == "p1" => Some(*id),
            _ => None,
        })
        .unwrap();
    assert!(!a
        .pending
        .values()
        .any(|p| matches!(&p.kind, Pending::StopProcess { process, .. } if process == "old-p")));
    // A provider's false result is not a successful cancellation.
    a.message(json!({"id":process,"result":{"terminated":false}}))
        .unwrap();
    assert!(a
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("could not confirm"));
    // Duplicate notifications cannot issue another request.
    let before = a.seq;
    a.message(json!({"method":"item/started","params":{"threadId":"nested","turnId":"nested-turn","item":{"type":"commandExecution","id":"command","source":"unifiedExecStartup","processId":"p1"}}})).unwrap();
    assert_eq!(a.seq, before);
    // A late new process belonging to the same active turn is cancelled.
    a.message(json!({"method":"item/started","params":{"threadId":"nested","turnId":"nested-turn","item":{"type":"commandExecution","id":"late","source":"unifiedExecStartup","processId":"p2"}}})).unwrap();
    let list = a
        .pending
        .iter()
        .find_map(|(id, p)| {
            matches!(&p.kind, Pending::StopList { thread, .. } if thread == "nested").then_some(*id)
        })
        .unwrap();
    a.message(json!({"id":list,"result":{"data":[{"itemId":"late","processId":"p2"}]}}))
        .unwrap();
    assert!(a
        .pending
        .values()
        .any(|p| matches!(&p.kind, Pending::StopProcess { process, .. } if process == "p2")));
}

#[test]
#[cfg(unix)]
fn stop_ignores_completed_terminal_absent_from_provider_list() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"commandExecution","id":"done","source":"unifiedExecStartup","processId":"p"}}})).unwrap();
    a.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"commandExecution","id":"done","source":"unifiedExecStartup","processId":"p","status":"completed","exitCode":0}}})).unwrap();
    a.action(Action::Interrupt).unwrap();
    let list = a
        .pending
        .iter()
        .find_map(|(id, p)| matches!(p.kind, Pending::StopList { .. }).then_some(*id))
        .unwrap();
    a.message(json!({"id":list,"result":{"data":[]}})).unwrap();
    assert!(!a
        .pending
        .values()
        .any(|p| matches!(p.kind, Pending::StopProcess { .. })));
    assert!(a.state.error.is_none());
}

#[test]
#[cfg(unix)]
fn stop_reports_previously_observed_missing_process_id() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"commandExecution","id":"unknown","source":"unifiedExecStartup","processId":null}}})).unwrap();
    a.action(Action::Interrupt).unwrap();
    assert!(a
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("background terminal"));
}

#[test]
#[cfg(unix)]
fn stop_reads_all_background_pages_and_matches_owned_items() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"commandExecution","id":"mine","source":"unifiedExecStartup","processId":"p"}}})).unwrap();
    a.action(Action::Interrupt).unwrap();
    let list = a
        .pending
        .iter()
        .find_map(|(id, p)| matches!(p.kind, Pending::StopList { .. }).then_some(*id))
        .unwrap();
    a.message(json!({"id":list,"result":{"data":[{"itemId":"unrelated","processId":"x"}],"nextCursor":"page2"}})).unwrap();
    assert!(!a
        .pending
        .values()
        .any(|p| matches!(p.kind, Pending::StopProcess { .. })));
    let next = a
        .pending
        .iter()
        .find_map(|(id, p)| matches!(p.kind, Pending::StopList { .. }).then_some(*id))
        .unwrap();
    a.message(json!({"id":next,"result":{"data":[{"itemId":"mine","processId":"p"}]}}))
        .unwrap();
    assert!(a
        .pending
        .values()
        .any(|p| matches!(&p.kind, Pending::StopProcess { process, .. } if process == "p")));
}

#[test]
#[cfg(unix)]
fn stop_request_expiry_is_scoped_and_late_responses_are_ignored() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.stop_tracking.root = "turn-1".into();
    a.stop_tracking.stopping = true;
    let now = Instant::now();
    let expired = now.checked_sub(Duration::from_secs(1)).unwrap();
    let future = now + Duration::from_secs(30);
    a.pending.insert(
        10,
        Request {
            kind: Pending::StopList {
                root: "turn-1".into(),
                thread: "child".into(),
                version: 1,
            },
            deadline: expired,
        },
    );
    a.pending.insert(
        11,
        Request {
            kind: Pending::StopChild {
                root: "turn-1".into(),
                thread: "child".into(),
                turn: "child-turn".into(),
            },
            deadline: expired,
        },
    );
    a.pending.insert(
        12,
        Request {
            kind: Pending::StopProcess {
                root: "turn-1".into(),
                thread: "child".into(),
                process: "p".into(),
            },
            deadline: expired,
        },
    );
    a.pending.insert(
        13,
        Request {
            kind: Pending::StopProcess {
                root: "turn-1".into(),
                thread: "child".into(),
                process: "next".into(),
            },
            deadline: future,
        },
    );
    a.pending.insert(
        14,
        Request {
            kind: Pending::Models,
            deadline: expired,
        },
    );
    a.expire_stop_requests(now);
    assert!(
        !a.pending.contains_key(&10)
            && !a.pending.contains_key(&11)
            && !a.pending.contains_key(&12)
    );
    assert!(a.pending.contains_key(&13) && a.pending.contains_key(&14));
    assert!(a.process.is_some());
    assert!(a
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("could not confirm"));
    let error = a.state.error.clone();
    a.message(json!({"id":10,"result":{"data":[{"itemId":"x","processId":"p"}]}}))
        .unwrap();
    a.message(json!({"id":11,"error":{"message":"late"}}))
        .unwrap();
    a.message(json!({"id":12,"result":{"terminated":false}}))
        .unwrap();
    assert_eq!(a.state.error, error);
    assert!(a.pending.contains_key(&13) && a.pending.contains_key(&14));
    assert!(a.process.is_some());
}

#[test]
fn settled_background_commands_keep_streaming_during_next_turn() {
    let mut state = state();
    state.apply(&json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"background","type":"commandExecution","command":"sleep 1; echo done","aggregatedOutput":""}}}));
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}));
    state.apply(
        &json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}}),
    );
    assert!(state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"background","delta":"done"}})));
    assert!(state.transcript().contains("done"));
    assert!(!state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"stale-message","delta":"stale"}})));
    assert_eq!(state.turn_id.as_deref(), Some("turn-2"));
    assert_eq!(state.status, Status::Working);
}

#[test]
fn command_output_stream_merges_final_output() {
    let mut state = state();
    state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"cmd","delta":"output"}}));
    state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"cmd","type":"commandExecution","command":"pwd","aggregatedOutput":"output"}}}));
    assert_eq!(state.saved.items.len(), 1);
    assert_eq!(state.transcript(), "pwd\noutput");
}

#[test]
fn early_interrupt_error_retries_but_has_fixed_bound() {
    let mut actor = actor();
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Interrupt {
                attempt: 0,
                turn: "turn-1".into(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":1,"error":{"message":"no active turn to interrupt"}}))
        .unwrap();
    assert_eq!(actor.interrupt_retry.as_ref().unwrap().1, 1);
    actor.interrupt_retry = None;
    actor.pending.insert(
        2,
        Request {
            kind: Pending::Interrupt {
                attempt: 4,
                turn: "turn-1".into(),
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":2,"error":{"message":"no active turn to interrupt"}}))
        .unwrap();
    assert!(actor.interrupt_retry.is_none());
    assert!(actor.state.error.is_some());
}

#[test]
fn stop_before_spawn_prevents_process_creation() {
    let mut actor = actor();
    actor.stopping.store(true, Ordering::SeqCst);
    assert!(actor.start().unwrap_err().contains("closing"));
    assert!(actor.process.is_none());
}

#[cfg(unix)]
#[test]
fn stop_terminates_process_even_with_retained_handle_clones() {
    let mut process = fixture_process();
    let (handle, _actions) = Handle::channel();
    handle.1.store(process.child.id(), Ordering::SeqCst);
    let retained = handle.clone();
    handle.stop(false);
    assert!(retained.2.load(Ordering::SeqCst));
    assert!(!process.child.wait().unwrap().success());
}

#[test]
fn clear_conversation_persists_fresh_identity_and_fences_old_transport() {
    let (mut actor, _) = idle_actor();
    actor.args = vec![
        "-c".into(),
        "model=chosen".into(),
        "old launch prompt".into(),
    ];
    actor.state.saved.draft = "old draft".into();
    actor.state.saved.draft_images = vec!["old image".into()];
    actor.state.saved.context = Some(ContextUsage {
        used: 99,
        window: 100,
    });
    actor.state.saved.composer.model = Some("chosen".into());
    let before = actor.state.saved.clone();
    let dir = tempfile::tempdir().unwrap();
    persist_in_dir(&before, dir.path()).unwrap();
    actor
        .clear_conversation(|saved| persist_in_dir(saved, dir.path()))
        .unwrap();
    let restored = load_from_dir(dir.path()).unwrap().sessions.remove(0);
    assert_eq!(restored.key, before.key);
    assert_eq!(restored.cwd, before.cwd);
    assert_eq!(restored.composer.model, before.composer.model);
    assert!(restored.thread_id.is_none());
    assert!(restored.items.is_empty());
    assert!(restored.title.is_none());
    assert!(restored.context.is_none());
    assert!(restored.draft.is_empty());
    assert!(restored.draft_images.is_empty());
    assert!(actor.process.is_none());
    assert!(actor.pending.is_empty());
    assert_eq!(actor.state.cleared_conversations, 1);
    assert_eq!(launch_options(&actor.args).unwrap().1, None);
    actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","item":{"id":"late","type":"agentMessage","text":"old"}}})).unwrap();
    assert!(actor.state.saved.items.is_empty());
    // Use a fresh fake transport and inspect the actual provider boundary.
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
    actor.start_thread(None).unwrap();
    let request = read_line(&mut reader);
    assert_eq!(request["method"], "thread/start");
    assert!(request["params"].get("threadId").is_none());
    actor
        .message(json!({"id":request["id"],"result":{"thread":{"id":"fresh-thread","turns":[]}}}))
        .unwrap();
    actor.prompt("new question".into()).unwrap();
    let turn = read_line(&mut reader);
    assert_eq!(turn["method"], "turn/start");
    assert_eq!(turn["params"]["threadId"], "fresh-thread");
    assert_eq!(turn["params"]["input"][0]["text"], "new question");
}

#[test]
fn clear_conversation_rejects_busy_disconnected_and_save_failure_without_mutation() {
    let mut actor = actor();
    assert_eq!(
        actor.action(Action::ClearConversation).unwrap_err(),
        CLEAR_BUSY
    );
    actor.state.status = Status::Done;
    actor.state.turn_id = None;
    assert_eq!(
        actor.action(Action::ClearConversation).unwrap_err(),
        CLEAR_DISCONNECTED
    );
    let (mut actor, _) = idle_actor();
    let old = actor.state.saved.thread_id.clone();
    assert!(actor
        .clear_conversation(|_| Err("disk full".into()))
        .unwrap_err()
        .contains("disk full"));
    assert_eq!(actor.state.saved.thread_id, old);
    assert!(actor.process.is_some());
    assert_eq!(actor.state.cleared_conversations, 0);
}
