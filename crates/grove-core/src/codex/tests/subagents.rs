use super::*;

#[test]
fn claude_child_cache_follows_retained_work_periods() {
    let mut saved = saved();
    saved.provider = Provider::Claude;
    for (index, id) in ["earlier", "edited-away"].into_iter().enumerate() {
        let mut period = WorkPeriod {
            start_item: index,
            end_item: Some(index + 1),
            duration_ms: None,
            subagents: Vec::new(),
        };
        record_subagent(&mut period, id, Some(id), None, SubagentStatus::Completed);
        saved.work_periods.push(period);
        saved.claude_subagents.insert(
            id.into(),
            SavedClaudeSubagent {
                items: vec![json!({"id":"message","type":"agentMessage","text":id})].into(),
                ..Default::default()
            },
        );
    }
    saved.work_periods.retain(|period| period.start_item < 1);
    saved.retain_claude_subagents();
    assert_eq!(saved.claude_subagents.len(), 1);
    assert_eq!(
        saved.claude_subagents["earlier"].items[0]["text"],
        "earlier"
    );
    assert!(saved.fresh_conversation().claude_subagents.is_empty());
}

#[test]
fn legacy_child_activity_preserves_existing_provider_metadata() {
    let mut saved = saved();
    saved.items.push(json!({"id":"legacy","type":"subAgentActivity","kind":"started","agentThreadId":"child","agentPath":"/root/verify_empty_directory"}));
    let agent = Subagent {
        id: "child".into(),
        name: "Noether".into(),
        role: Some("explorer".into()),
        task: Some("Inspect files".into()),
        activity: "Final report".into(),
        status: SubagentStatus::Completed,
    };
    saved.work_periods.push(WorkPeriod {
        start_item: 0,
        end_item: Some(1),
        duration_ms: Some(100),
        subagents: vec![agent.clone()],
    });
    let restored = Snapshot::new(saved);
    assert_eq!(
        serde_json::to_value(&restored.saved.work_periods[0].subagents[0]).unwrap(),
        serde_json::to_value(agent).unwrap()
    );
}

#[test]
fn active_subagents_keep_latest_observations_and_terminal_tombstones() {
    let agent = |id: &str, status: SubagentStatus| Subagent {
        id: id.into(),
        name: id.into(),
        role: None,
        task: None,
        activity: "Observed".into(),
        status,
    };
    let periods = vec![
        WorkPeriod {
            start_item: 0,
            end_item: Some(1),
            duration_ms: None,
            subagents: vec![
                agent("done", SubagentStatus::Working),
                agent("live", SubagentStatus::Working),
            ],
        },
        WorkPeriod {
            start_item: 1,
            end_item: Some(2),
            duration_ms: None,
            subagents: vec![
                agent("done", SubagentStatus::Completed),
                agent("live", SubagentStatus::Unknown),
                agent("failed", SubagentStatus::Failed),
            ],
        },
    ];
    let active = active_subagents(&periods);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "live");
    assert_eq!(active[0].status, SubagentStatus::Unknown);
    let restored: Subagent = serde_json::from_value(
        json!({"id":"old", "name":"Agent 1", "activity":"Reading", "status":"working"}),
    )
    .unwrap();
    assert!(restored.role.is_none());
    assert!(restored.task.is_none());
    let mut saved = state().saved;
    saved.work_periods = periods;
    saved.work_periods[1].subagents[1].status = SubagentStatus::Working;
    let recovered = Snapshot::new(saved);
    let active = active_subagents(&recovered.saved.work_periods);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].status, SubagentStatus::Unknown);
    assert!(recovered.saved.fresh_conversation().work_periods.is_empty());
}

#[test]
fn subagent_activity_registers_children_live_and_recovers_legacy_roster() {
    let mut state = state();
    state.begin_work();
    let event = json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"spawn","type":"subAgentActivity","kind":"started","agentThreadId":"child-1","agentPath":"root/reviewer"}}});
    let mut foreign = event.clone();
    foreign["params"]["threadId"] = json!("foreign");
    assert!(!state.apply(&foreign));
    assert!(state.saved.work_periods[0].subagents.is_empty());
    assert!(state.apply(&event));
    assert!(state.apply(&event));
    let agents = &state.saved.work_periods[0].subagents;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "child-1");
    assert_eq!(agents[0].name, "reviewer");
    assert_eq!(agents[0].status, SubagentStatus::Working);
    assert!(state.apply(&json!({"method":"item/started","params":{"threadId":"child-1","item":{"id":"nested","type":"subAgentActivity","kind":"started","agentThreadId":"child-2","agentPath":"root/reviewer/security"}}})));
    assert_eq!(state.saved.work_periods[0].subagents.len(), 2);
    for clear_periods in [false, true] {
        let mut saved = state.saved.clone();
        for period in &mut saved.work_periods {
            period.subagents.clear();
        }
        if clear_periods {
            saved.work_periods.clear();
            saved.session_review.pending = None;
            saved.session_review.last_completed_period = None;
            saved.session_review.reviewed_through = 0;
            saved.session_review.summary.clear();
        }
        let restored = Snapshot::new(saved);
        let agent = &restored.saved.work_periods[0].subagents[0];
        assert_eq!(agent.id, "child-1");
        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.status, SubagentStatus::Unknown);
    }
}

#[test]
fn subagent_collab_counts_children_not_calls_and_isolates_parent_state() {
    let mut state = state();
    state.begin_work();
    let item = json!({"id":"spawn","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","senderThreadId":"thread-1","receiverThreadIds":["child-1"],"prompt":"Inspect the parser","agentRole":"explorer","agentsStates":{"child-1":{"status":"running","message":null}}});
    let event = json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":item}});
    assert!(state.apply(&event));
    assert!(state.apply(&event));
    assert_eq!(state.saved.work_periods[0].subagents.len(), 1);
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Working
    );
    assert_eq!(
        state.saved.work_periods[0].subagents[0].activity,
        "Inspect the parser"
    );
    assert_eq!(
        state.saved.work_periods[0].subagents[0].task.as_deref(),
        Some("Inspect the parser")
    );
    assert_eq!(
        state.saved.work_periods[0].subagents[0].role.as_deref(),
        Some("explorer")
    );
    let parent_items = state.saved.items.clone();
    assert!(!state.apply(&json!({"method":"thread/tokenUsage/updated","params":{"threadId":"child-1","tokenUsage":{"last":{"totalTokens":999},"modelContextWindow":1000}}})));
    assert_eq!(state.context_tokens, None);
    assert_eq!(state.status, Status::Working);
    assert_eq!(state.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(state.saved.items, parent_items);
    assert!(state.apply(&json!({"method":"item/started","params":{"threadId":"child-1","turnId":"child-turn","item":{"id":"cmd","type":"commandExecution","command":"cargo test parser"}}})));
    assert_eq!(
        state.saved.work_periods[0].subagents[0].activity,
        "cargo test parser"
    );
    assert!(state.apply(&json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"failed"}}})));
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Failed
    );
    state.apply(&event); // A duplicate spawn does not resurrect the failed worker.
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Failed
    );
    assert_eq!(state.status, Status::Working);
    let resume = json!({"id":"resume-child", "tool":"resumeAgent", "receiverThreadIds":["child-1"],"agentsStates":{"child-1":{"status":"running"}}});
    let mut empty_started = resume.clone();
    empty_started["agentsStates"] = json!({});
    state.record_collab(&empty_started, 0);
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Failed
    );
    state.record_collab(&resume, 0);
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Working
    );
    state.apply(&json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn-2","status":"completed"}}}));
    state.record_collab(&resume, 0);
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
}

#[test]
fn subagent_reads_find_nested_children_and_preserve_turn_history() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", None, Some("Inspect"), SubagentStatus::Working);
    let read = json!({"id":"child","agentNickname":"Ada","agentRole":"reviewer","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"nested","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","senderThreadId":"child","receiverThreadIds":["nested"],"agentsStates":{"nested":{"status":"running"}}},{"id":"cmd","type":"commandExecution","command":"rg parser"}]}]});
    state.apply_subagent_read("child", 0, &read);
    state.apply_subagent_read("child", 0, &read);
    assert_eq!(state.saved.work_periods[0].subagents.len(), 2);
    assert_eq!(state.saved.work_periods[0].subagents[0].name, "Ada");
    assert_eq!(
        state.saved.work_periods[0].subagents[0].role.as_deref(),
        Some("reviewer")
    );
    assert_eq!(
        state.saved.work_periods[0].subagents[0].activity,
        "rg parser"
    );
    state
        .saved
        .items
        .push(json!({"id":"answer","type":"agentMessage","text":"Done"}));
    state.finish_work();
    // Parent completion is not child completion.
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Working
    );
    state.begin_work();
    state.apply_subagent_read(
        "child",
        0,
        &json!({"id":"child","turns":[{"id":"child-turn","status":"completed","items":[]}]}),
    );
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
    assert!(state.saved.work_periods[1].subagents.is_empty());
    let round_trip: SavedSession =
        serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
    assert_eq!(round_trip.work_periods[0].subagents.len(), 2);
    let old: WorkPeriod =
        serde_json::from_value(json!({"start_item":0,"end_item":1,"duration_ms":2})).unwrap();
    assert!(old.subagents.is_empty());
}

#[test]
fn interrupted_child_is_terminal_for_notifications_reads_and_persistence() {
    for via_read in [false, true] {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, None, SubagentStatus::Working);
        assert_eq!(active_subagents(&state.saved.work_periods).len(), 1);
        if via_read {
            state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"child-turn","status":"interrupted","items":[]}]}));
        } else {
            assert!(state.apply(&json!({"method":"turn/completed","params":{"threadId":"child","turn":{"id":"child-turn","status":"interrupted"}}})));
        }
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Interrupted
        );
        assert!(active_subagents(&state.saved.work_periods).is_empty());
        assert!(state.subagent_terminal("child"));
        let encoded = serde_json::to_value(&state.saved).unwrap();
        assert_eq!(
            encoded["work_periods"][0]["subagents"][0]["status"],
            "interrupted"
        );
        let restored: SavedSession = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            restored.work_periods[0].subagents[0].status,
            SubagentStatus::Interrupted
        );
        assert!(active_subagents(&restored.work_periods).is_empty());
    }
    for value in ["working", "completed", "failed", "unknown"] {
        let parsed: SubagentStatus = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(value));
    }
}

#[test]
fn reused_subagent_belongs_to_new_work_and_old_read_cannot_rewrite_history() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent(
        "child",
        Some("Ada"),
        Some("First task"),
        SubagentStatus::Completed,
    );
    state
        .saved
        .items
        .push(json!({"id":"answer","type":"agentMessage","text":"Done"}));
    state.finish_work();
    state.begin_work();
    state.record_collab(&json!({"receiverThreadIds":["child"],"tool":"followupTask","agentsStates":{"child":{"status":"running","message":"Second task"}}}), 1);
    assert_eq!(
        state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
    assert_eq!(
        state.saved.work_periods[1].subagents[0].status,
        SubagentStatus::Working
    );
    state.apply_subagent_read("child", 0, &json!({"id":"child","agentNickname":"Wrong","turns":[{"id":"new-child-turn","status":"inProgress","items":[]}]}));
    assert_eq!(state.saved.work_periods[0].subagents[0].name, "Ada");
    // An inherited parent turn is not this child's completion or activity.
    state.apply_subagent_read("child", 1, &json!({"id":"child","turns":[{"id":"turn-1","status":"completed","items":[{"type":"commandExecution","command":"parent command"}]}]}));
    assert_eq!(
        state.saved.work_periods[1].subagents[0].activity,
        "Second task"
    );
    assert_eq!(
        state.saved.work_periods[1].subagents[0].status,
        SubagentStatus::Working
    );
}

#[test]
fn subagent_recent_activity_bounds_dedup_and_poll_delta() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", None, None, SubagentStatus::Working);
    for i in 0..40 {
        state.observe_subagent(
            "child",
            &format!("item-{i}"),
            SubagentActivityKind::Tool,
            &"é".repeat(900),
        );
    }
    let activity = &state.subagent_activity["child"];
    assert!(activity.truncated);
    assert!(activity.events.len() <= 20);
    assert!(activity.events.iter().all(|e| e.text.len() <= 1024));
    assert!(
        activity
            .events
            .iter()
            .map(|e| e.id.len() + e.text.len())
            .sum::<usize>()
            <= 16 * 1024
    );
    let thread = json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"agentMessage","text":"First"},{"id":"b","type":"commandExecution","command":"cargo check"}]}]});
    state.apply_subagent_read("child", 0, &thread);
    let before = state.subagent_activity["child"].events.clone();
    state.apply_subagent_read("child", 0, &thread);
    let after = &state.subagent_activity["child"].events;
    assert_eq!(before.len(), after.len());
    assert_eq!(
        before.last().unwrap().observed_at,
        after.last().unwrap().observed_at
    );
    assert_eq!(after.last().unwrap().kind, SubagentActivityKind::Tool);
    state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"turn-1","items":[{"id":"p","type":"agentMessage","text":"Parent secret"}]}]}));
    assert!(!state.subagent_activity["child"]
        .events
        .iter()
        .any(|e| e.text == "Parent secret"));
}

#[test]
fn subagent_recent_activity_poll_refreshes_earlier_retained_tool() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", None, None, SubagentStatus::Working);
    let mut thread = json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"mcpToolCall","tool":"read","progress":"Opening source"},{"id":"b","type":"commandExecution","command":"cargo check"}]}]});
    state.apply_subagent_read("child", 0, &thread);
    let original_b = state.subagent_activity["child"].events[1].observed_at;
    thread["turns"][0]["items"][0]["progress"] = json!("Reading source");
    state.apply_subagent_read("child", 0, &thread);
    let activity = &state.subagent_activity["child"];
    assert_eq!(activity.events.len(), 2);
    assert_eq!(activity.events[0].text, "Reading source");
    assert_eq!(activity.events[1].observed_at, original_b);
    let observed = activity.observed_at;
    let times: Vec<_> = activity.events.iter().map(|e| e.observed_at).collect();
    state.apply_subagent_read("child", 0, &thread);
    assert_eq!(state.subagent_activity["child"].observed_at, observed);
    assert_eq!(
        state.subagent_activity["child"]
            .events
            .iter()
            .map(|e| e.observed_at)
            .collect::<Vec<_>>(),
        times
    );
}

#[test]
fn subagent_recent_activity_session_bound_and_reasoning_exclusion() {
    let mut state = state();
    state.begin_work();
    for i in 0..120 {
        let id = format!("child-{i}");
        state.upsert_subagent(&id, None, None, SubagentStatus::Working);
        for j in 0..20 {
            state.observe_subagent(
                &id,
                &format!("item-{j}"),
                SubagentActivityKind::Commentary,
                &"x".repeat(1024),
            );
        }
    }
    assert_eq!(state.subagent_activity.len(), 100);
    assert!(state.subagent_activity_bytes() <= 1024 * 1024);
    assert!(
        state
            .subagent_activity
            .values()
            .flat_map(|a| &a.events)
            .map(|e| e.id.len() + e.text.len())
            .sum::<usize>()
            <= 1024 * 1024
    );
    state.apply_subagent_notification(&json!({"method":"item/completed","params":{"threadId":"child-119","item":{"id":"reason","type":"reasoning","text":"private"}}}), 0);
    assert!(!state.subagent_activity["child-119"]
        .events
        .iter()
        .any(|e| e.text.contains("private")));
}

#[test]
fn subagent_recent_activity_replacement_and_terminal_eviction() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", None, None, SubagentStatus::Working);
    for i in 0..20 {
        state.observe_subagent(
            "child",
            &format!("item-{i}"),
            SubagentActivityKind::Tool,
            "small",
        );
    }
    for i in 0..20 {
        state.observe_subagent(
            "child",
            &format!("item-{i}"),
            SubagentActivityKind::Tool,
            &"x".repeat(1024),
        );
    }
    assert!(
        state.subagent_activity["child"]
            .events
            .iter()
            .map(|e| e.id.len() + e.text.len())
            .sum::<usize>()
            <= 16 * 1024
    );
    state.upsert_subagent("terminal", None, None, SubagentStatus::Completed);
    state.observe_subagent(
        "terminal",
        "event",
        SubagentActivityKind::Commentary,
        "Done",
    );
    for i in 0..99 {
        let id = format!("new-{i}");
        state.upsert_subagent(&id, None, None, SubagentStatus::Working);
        state.observe_subagent(&id, "event", SubagentActivityKind::Tool, "Read");
    }
    assert!(!state.subagent_activity.contains_key("terminal"));
    assert!(state.subagent_activity_evicted.contains("terminal"));
    assert!(state.subagent_activity.contains_key("child"));
}

#[test]
fn subagent_recent_activity_rejects_stale_generation_success_and_failure() {
    let mut actor = actor();
    actor.state.begin_work();
    actor
        .state
        .upsert_subagent("child", None, None, SubagentStatus::Working);
    actor.state.ensure_subagent_activity("child");
    actor.state.subagent_observation_generation = 1;
    for id in 1..=2 {
        actor.pending.insert(
            id,
            Request {
                kind: Pending::ChildRead {
                    thread: "child".into(),
                    period_start: 0,
                    generation: 0,
                },
                deadline: Instant::now(),
            },
        );
    }
    actor.message(json!({"id":1,"result":{"thread":{"id":"child","turns":[{"id":"old-child-turn","status":"completed","items":[{"type":"agentMessage","text":"Stale"}]}]}}})).unwrap();
    actor
        .message(json!({"id":2,"error":{"message":"Stale failure"}}))
        .unwrap();
    assert!(actor.state.subagent_activity["child"].events.is_empty());
    assert!(!actor.state.subagent_activity["child"].refresh_error);
    assert!(actor.child_read_failures.is_empty());
    assert_eq!(
        actor.state.saved.work_periods.last().unwrap().subagents[0].status,
        SubagentStatus::Working
    );
}

#[test]
fn optional_subagent_read_errors_do_not_fail_parent_or_pause_queue() {
    let mut actor = actor();
    actor.queue_ready = true;
    actor.pending.insert(
        1,
        Request {
            kind: Pending::ChildRead {
                thread: "child".into(),
                period_start: 0,
                generation: 0,
            },
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":1,"error":{"message":"Thread not found"}}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.state.error.is_none());
    assert!(actor.queue_ready);
    assert!(!actor.queue_paused);
    actor.pending.insert(
        2,
        Request {
            kind: Pending::ChildRead {
                thread: "child".into(),
                period_start: 0,
                generation: 0,
            },
            deadline: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
        },
    );
    actor.expire_subagent_reads();
    assert!(actor.pending.is_empty());
    assert_eq!(actor.state.status, Status::Working);
    assert_eq!(actor.child_read_failures.get("child"), Some(&2));
}

#[test]
fn child_read_orders_older_history_before_live_items() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", None, None, SubagentStatus::Working);
    state.upsert_subagent_item("child", json!({"id":"b","type":"agentMessage","text":"B"}));
    state.upsert_subagent_item("child", json!({"id":"c","type":"agentMessage","text":"C"}));
    state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"agentMessage","text":"A"},{"id":"b","type":"agentMessage","text":"B"}]}]}));
    assert_eq!(
        state.subagent_activity["child"]
            .items
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}

#[test]
fn child_transcript_retains_complete_items_and_streams_without_parent_mutation() {
    let mut state = state();
    state.begin_work();
    state.upsert_subagent("child", Some("Ada"), None, SubagentStatus::Working);
    let parent = state.saved.items.clone();
    let text = "full visible commentary ".repeat(150);
    let items: Vec<_> = (0..30)
        .map(|i| json!({"id":format!("item-{i}"),"type":"agentMessage","text":text}))
        .collect();
    state.apply_subagent_read(
        "child",
        0,
        &json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":items}]}),
    );
    assert_eq!(state.subagent_activity["child"].items.len(), 30);
    assert_eq!(state.subagent_activity["child"].items[0]["text"], text);
    state.apply(&json!({"method":"item/started","params":{"threadId":"child","turnId":"child-turn","item":{"id":"cmd","type":"commandExecution","command":"pwd","status":"inProgress"}}}));
    assert!(state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"child","turnId":"child-turn","itemId":"cmd","delta":"full output"}})));
    let projection = state.subagent_snapshot("child");
    assert_eq!(
        projection.saved.items.last().unwrap()["aggregatedOutput"],
        "full output"
    );
    assert_eq!(projection.saved.items.len(), 31);
    assert_eq!(state.saved.items, parent);
    assert!(projection.subagent_activity.is_empty());
}

#[test]
#[cfg(unix)]
fn stop_directly_terminates_running_child_even_after_root_completes() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.message(json!({"method":"thread/started","params":{"thread":{"id":"child","source":{"subAgent":{"thread_spawn":{"parent_thread_id":"thread-1"}}}}}})).unwrap();
    a.message(json!({"method":"turn/started","params":{"threadId":"child","turn":{"id":"child-turn","rootTurnId":"turn-1"}}})).unwrap();
    a.message(json!({"method":"item/started","params":{"threadId":"child","turnId":"child-turn","item":{"type":"commandExecution","id":"cmd","source":"unifiedExecStartup","status":"inProgress","processId":"p"}}})).unwrap();
    a.action(Action::Interrupt).unwrap();
    let id = a.pending.iter().find_map(|(id,p)| matches!(&p.kind, Pending::StopProcess { thread, process, .. } if thread == "child" && process == "p").then_some(*id)).unwrap();
    a.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}})).unwrap();
    a.message(json!({"method":"turn/completed","params":{"threadId":"child","turn":{"id":"child-turn","status":"interrupted"}}})).unwrap();
    a.message(json!({"id":id,"result":{"terminated":false}}))
        .unwrap();
    assert!(a
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("could not confirm"));
    assert!(a
        .stop_tracking
        .processes
        .contains_key(&("child".into(), "cmd".into())));
    // A new submission cannot replace a still-running stop generation.
    assert!(a
        .action(Action::Submit {
            id: 77,
            text: "continue".into(),
            images: vec![]
        })
        .is_err());
    assert_eq!(a.state.rejected_submission, Some(77));
    assert_eq!(a.stop_tracking.root, "turn-1");
    // An acknowledgement from the old stop cannot affect a subsequent turn.
    a.stop_tracking.stopping = false;
    a.message(
        json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}}),
    )
    .unwrap();
    a.pending.insert(
        999,
        Request {
            kind: Pending::StopProcess {
                root: "turn-1".into(),
                thread: "child".into(),
                process: "p".into(),
            },
            deadline: Instant::now(),
        },
    );
    a.message(json!({"id":999,"result":{"terminated":false}}))
        .unwrap();
    assert!(a.state.error.is_none());
    assert_eq!(a.stop_tracking.root, "turn-2");
}

#[test]
#[cfg(unix)]
fn stop_cancels_late_child_after_parentage_is_established() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.action(Action::Interrupt).unwrap();
    a.message(json!({"method":"turn/started","params":{"threadId":"child","turn":{"id":"c","rootTurnId":"turn-1"}}})).unwrap();
    assert!(!a
        .pending
        .values()
        .any(|p| matches!(p.kind, Pending::StopChild { .. })));
    a.message(json!({"method":"thread/started","params":{"thread":{"id":"child","source":{"subAgent":{"thread_spawn":{"parent_thread_id":"thread-1"}}}}}})).unwrap();
    assert!(a.pending.values().any(|p| matches!(&p.kind, Pending::StopChild { thread, turn, .. } if thread == "child" && turn == "c")));
}

#[test]
#[cfg(unix)]
fn stop_reports_child_without_proven_generation() {
    let mut a = actor();
    a.process = Some(fixture_process());
    a.interaction_children
        .register("thread-1", "thread-1", "child", None, None);
    a.interaction_children.start("child", Some("child-turn"));
    a.action(Action::Interrupt).unwrap();
    assert!(a
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("identify every active"));
    assert!(!a
        .pending
        .values()
        .any(|p| matches!(p.kind, Pending::StopChild { .. })));
}
