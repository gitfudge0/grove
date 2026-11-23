use super::*;

#[test]
fn claude_task_notifications_deduplicate_tool_lifecycle_and_ignore_shell_tasks() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.block("agent", &json!({"type":"tool_use","id":"agent","name":"Task","input":{"description":"Review changes"}}));
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"sdk-task","tool_use_id":"agent","task_type":"local_agent","subagent_type":"Reviewer","description":"Review changes"})).unwrap();
    actor.message(json!({"type":"system","subtype":"task_progress","task_id":"sdk-task","tool_use_id":"agent","summary":"Checking error handling"})).unwrap();
    actor.message(json!({"type":"system","subtype":"task_progress","task_id":"sdk-task","tool_use_id":"agent","description":"Running List files tool"})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0]
            .task
            .as_deref(),
        Some("Review changes")
    );
    actor.block("shell-tool", &json!({"type":"tool_use","id":"shell-tool","name":"Bash","input":{"command":"sleep 2","description":"Run a two-second timing probe"}}));
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"shell","run_id":"shell-run","tool_use_id":"shell-tool","task_type":"local_bash","is_backgrounded":true,"description":"Run a two-second timing probe"})).unwrap();
    actor.message(json!({"type":"system","subtype":"task_progress","task_id":"shell","tool_use_id":"shell-tool","summary":"Running shell"})).unwrap();
    actor.message(json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed","summary":"Shell completed"})).unwrap();
    assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].activity,
        "Running List files tool"
    );
    actor.message(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"agent","content":"Launched agent","is_error":false}]},"tool_use_result":{"isAsync":true,"status":"async_launched"}})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Working
    );
    actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"failed","summary":"Review failed"})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Failed
    );
    actor.block("agent", &json!({"type":"tool_use","id":"agent","name":"Task","input":{"description":"Late replay"}}));
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Failed
    );
    actor.message(json!({"type":"system","subtype":"task_notification","task_id":"sdk-task","status":"stopped"})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Interrupted
    );
}

#[test]
fn naturally_completed_redirect_keeps_the_correction_until_its_echo() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("old".into());
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.redirect(81, "correction".into(), vec![]).unwrap();
    let sent = read(&mut reader);
    actor
        .message(json!({"type":"result","terminal_reason":"completed","queued_turn_count":0}))
        .unwrap();
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert!(actor.state.status.busy());
    assert_eq!(actor.in_flight.as_ref().unwrap().id, 81);
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
        .unwrap();
    actor.acknowledge_input(&sent["uuid"]);
    actor
        .message(json!({"type":"result","terminal_reason":"completed","queued_turn_count":0}))
        .unwrap();
    assert!(actor.state.status.busy());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Done);
}

#[test]
fn redirect_intermediate_result_retains_input_and_waits_for_authoritative_idle() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("old".into());
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.state.saved.draft = "correction".into();
    actor
        .action(Action::SubmitWithMode {
            id: 91,
            text: "correction".into(),
            images: vec![],
            mode: SubmissionMode::Steer,
        })
        .unwrap();
    let sent = read(&mut reader);
    assert_eq!(sent["priority"], "next");
    assert_eq!(sent["origin"]["kind"], "human");
    actor.message(json!({"type":"result","terminal_reason":"aborted_tools","queued_turn_count":0,"is_error":false})).unwrap();
    assert_eq!(actor.in_flight.as_ref().unwrap().id, 91);
    assert!(actor.state.status.busy());
    actor.acknowledge_input(&sent["uuid"]);
    actor.acknowledge_input(&sent["uuid"]);
    assert_eq!(actor.state.saved.items.len(), 1);
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    assert!(actor.state.status.busy());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Done);
}

#[test]
fn historical_claude_messages_require_new_anchors_before_editing() {
    let mut actor = actor();
    actor.state.saved.provider = provider::Provider::Claude;
    actor.state.status = Status::Idle;
    actor
        .state
        .saved
        .items
        .push(json!({"id":"old","type":"userMessage","content":[]}));
    assert!(actor
        .state
        .editable_message(0)
        .unwrap_err()
        .contains("anchor"));
    actor.state.saved.items[0]["claudeHumanUuid"] = json!("old");
    assert!(actor
        .state
        .editable_message(0)
        .unwrap_err()
        .contains("checkpoint"));
}

#[test]
fn clear_conversation_drops_resume_identity_and_old_event_receiver() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    actor.state.status = Status::Done;
    actor.state.saved.thread_id = Some("old-session".into());
    actor
        .state
        .saved
        .items
        .push(json!({"type":"agentMessage","text":"old"}));
    actor.state.saved.title = Some("Old title".into());
    actor.state.saved.draft = "draft".into();
    actor.args = vec![
        "--model".into(),
        "sonnet".into(),
        "old launch prompt".into(),
    ];
    actor
        .state
        .saved
        .claude_subagents
        .insert("old-child".into(), provider::SavedClaudeSubagent::default());
    actor
        .subagent_tasks
        .insert("old-task".into(), "old-child".into());
    actor
        .child_transcripts
        .insert("old-child".into(), ChildTranscript::default());
    let before = actor.state.saved.clone();
    let mut restored = None;
    actor
        .clear_conversation(|saved| {
            restored = Some(provider::recover_saved_for_test(saved));
            Ok(())
        })
        .unwrap();
    assert!(actor.subagent_tasks.is_empty());
    assert!(actor.child_transcripts.is_empty());
    assert!(actor.state.saved.claude_subagents.is_empty());
    let restored = restored.unwrap();
    assert_eq!(restored.key, before.key);
    assert_eq!(restored.cwd, before.cwd);
    assert!(restored.thread_id.is_none());
    assert!(restored.items.is_empty());
    assert!(restored.title.is_none());
    assert!(restored.draft.is_empty());
    assert!(actor.process.is_none());
    assert_eq!(actor.state.cleared_conversations, 1);
    assert_eq!(launch_options(&actor.args).unwrap().1, None);
    let command = launch_command(&actor.args, &restored).unwrap();
    let args = command
        .get_args()
        .map(|v| v.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(!args
        .iter()
        .any(|arg| arg == "--resume" || arg == "old-session" || arg == "old launch prompt"));
    // A new CLI's identity is accepted rather than filtered by the old one.
    actor
        .message(json!({"type":"system","subtype":"init","session_id":"fresh-session"}))
        .unwrap();
    assert_eq!(
        actor.state.saved.thread_id.as_deref(),
        Some("fresh-session")
    );
    actor.message(json!({"type":"assistant","session_id":"old-session","message":{"content":[{"type":"text","text":"late old text"}]}})).unwrap();
    assert!(actor.state.saved.items.is_empty());
}

#[test]
fn clear_conversation_rejections_preserve_existing_claude_context() {
    let mut actor = actor();
    actor.state.saved.thread_id = Some("old-session".into());
    actor.state.status = Status::Working;
    assert_eq!(
        actor.action(Action::ClearConversation).unwrap_err(),
        provider::CLEAR_BUSY
    );
    actor.state.status = Status::Done;
    assert_eq!(
        actor.action(Action::ClearConversation).unwrap_err(),
        provider::CLEAR_DISCONNECTED
    );
    let _reader = transport(&mut actor);
    assert!(actor
        .clear_conversation(|_| Err("disk full".into()))
        .is_err());
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("old-session"));
    assert!(actor.process.is_some());
    assert_eq!(actor.state.cleared_conversations, 0);
}

#[test]
fn images_use_anthropic_content_blocks() {
    assert_eq!(
        user_content("look", &["data:image/png;base64,aGVsbG8=".into()])[1],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}})
    );
}

#[test]
fn streams_replace_final_text_without_duplication_and_preserve_tool_results() {
    let mut actor = actor();
    for event in [
        json!({"type":"message_start","message":{"id":"m"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}),
    ] {
        actor.stream(&event);
    }
    actor.message(json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"Hello!"}]}})).unwrap();
    assert_eq!(actor.state.saved.items.len(), 1);
    assert_eq!(actor.state.saved.items[0]["text"], "Hello!");
    assert_eq!(actor.state.saved.items[0]["groveCompleted"], true);
    let tool = json!({"type":"tool_use","id":"tool1","name":"Bash","input":{"command":"pwd"}});
    actor.block("m-1", &tool);
    actor.tool_result(&json!({"tool_use_id":"tool1","content":"/tmp","is_error":false}));
    actor.block("m-1", &tool);
    assert_eq!(actor.state.saved.items[1]["aggregatedOutput"], "/tmp");
    assert_eq!(actor.state.saved.items[1]["status"], "completed");
}

#[test]
fn protocol_batches_yield_with_backlog_and_preserve_pending_order() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    let (send, lines) = mpsc::channel();
    actor.process.as_mut().unwrap().lines = lines;
    // Unknown future protocol events are deliberately ignored by message(),
    // but must still be bounded and consumed in transport order.
    for sequence in 0..600 {
        send.send(Ok(json!({"type":"future_event", "sequence":sequence})))
            .unwrap();
    }
    assert!(actor.receive_batch());
    assert!(actor.dirty);
    let next = actor
        .process
        .as_ref()
        .unwrap()
        .lines
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(next["sequence"], 256);
    assert!(actor.receive_batch());
    let next = actor
        .process
        .as_ref()
        .unwrap()
        .lines
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(next["sequence"], 513);
    assert!(actor.receive_batch());
    assert!(!actor.receive_batch());
}

#[test]
fn pending_tools_wait_for_idle_and_keep_late_results() {
    let mut actor = actor();
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn".into());
    actor.state.saved.items.push(json!({"id":"late", "type":"commandExecution", "status":"inProgress", "aggregatedOutput":""}));
    actor
        .state
        .saved
        .items
        .push(json!({"id":"unresolved", "type":"commandExecution", "status":"inProgress"}));
    actor
        .message(json!({"type":"result", "is_error":false}))
        .unwrap();
    assert_eq!(actor.state.saved.items[0]["status"], "inProgress");
    actor.tool_result(&json!({"type":"tool_result", "tool_use_id":"late", "is_error":true, "content":"late failure output"}));
    actor
        .message(json!({"type":"system", "subtype":"session_state_changed", "state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Done);
    assert_eq!(actor.state.saved.items[0]["status"], "failed");
    assert_eq!(
        actor.state.saved.items[0]["aggregatedOutput"],
        "late failure output"
    );
    assert_eq!(actor.state.saved.items[1]["status"], "completed");
}

#[test]
fn idle_before_result_retains_foreground_and_latest_failure_wins() {
    let mut actor = actor();
    actor.tracks_session_state = true;
    actor.state.turn_id = Some("turn".into());
    actor.state.status = Status::Working;
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.state.turn_id.is_some());
    actor
        .message(json!({"type":"result","is_error":true,"errors":["failure"]}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Failed);
    assert!(actor.state.turn_id.is_none());
    actor.session_idle = false;
    actor.state.turn_id = Some("next".into());
    actor
        .message(json!({"type":"result","is_error":false}))
        .unwrap();
    actor
        .message(json!({"type":"result","is_error":true,"errors":["continuation failed"]}))
        .unwrap();
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Failed);
    assert_eq!(actor.state.error.as_deref(), Some("continuation failed"));
}

#[test]
fn fragmented_final_frames_preserve_original_stream_block_identity() {
    let mut actor = actor();
    actor.stream(&json!({"type":"message_start","message":{"id":"mixed"}}));
    let tool = json!({"type":"tool_use","id":"pwd-tool","name":"Bash","input":{"command":"pwd"}});
    actor.stream(&json!({"type":"content_block_start","index":0,"content_block":tool}));
    actor
        .message(json!({"type":"assistant","message":{"id":"mixed","content":[tool]}}))
        .unwrap();
    actor.stream(
        &json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
    );
    actor.stream(&json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"# MARIGOLD\n\nResult."}}));
    let text = json!({"type":"text","text":"# MARIGOLD\n\nResult."});
    actor
        .message(json!({"type":"assistant","message":{"id":"mixed","content":[text]}}))
        .unwrap();
    assert_eq!(actor.state.saved.items.len(), 2);
    assert_eq!(actor.state.saved.items[1]["id"], "mixed-1");
    actor.stream(
        &json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
    );
    actor.stream(&json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Second block."}}));
    let second = json!({"type":"text","text":"Second block."});
    actor
        .message(json!({"type":"assistant","message":{"id":"mixed","content":[second]}}))
        .unwrap();
    actor
        .message(json!({"type":"assistant","message":{"id":"mixed","content":[tool,text,second]}}))
        .unwrap();
    assert_eq!(actor.state.saved.items.len(), 3);
    assert_eq!(
        actor
            .state
            .saved
            .items
            .iter()
            .filter(|item| item["type"] == "agentMessage")
            .count(),
        2
    );
    assert_eq!(actor.state.saved.items[1]["text"], text["text"]);
    assert_eq!(actor.state.saved.items[2]["text"], second["text"]);
}

#[test]
fn rich_context_missing_optional_fields_and_invalid_measurements_are_distinct() {
    let (usage, details) =
        context_measurement(&json!({"totalTokens":0,"maxTokens":200000})).unwrap();
    assert_eq!(usage.used, 0);
    assert!(details.categories.is_empty());
    assert_eq!(details.auto_compact_enabled, None);
    for data in [
        json!({"totalTokens":12}),
        json!({"maxTokens":200000}),
        json!({"totalTokens":12,"maxTokens":0}),
    ] {
        assert!(context_measurement(&data).is_none());
    }
    let (_, details) = context_measurement(&json!({"totalTokens":20,"maxTokens":100,
        "categories":[{"name":"Future category","tokens":20}]}))
    .unwrap();
    assert_eq!(
        details.categories[0].kind,
        provider::ContextCategoryKind::Unknown
    );
}

#[test]
fn rich_context_query_preserves_old_measurement_and_rejects_late_results() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Done;
    actor.state.record_context(Some(100), Some(200000));
    actor.action(Action::RefreshContext).unwrap();
    assert!(actor.state.context_query_pending);
    assert_eq!(actor.state.saved.context.unwrap().used, 100);
    actor.action(Action::RefreshContext).unwrap(); // coalesced
    let request = read(&mut reader);
    actor
        .message(
            json!({"type":"control_response","response":{"subtype":"success",
        "request_id":request["request_id"],"response":rich_context_fixture()}}),
        )
        .unwrap();
    assert_eq!(
        actor
            .state
            .saved
            .context_details
            .as_ref()
            .unwrap()
            .categories
            .len(),
        8
    );
    actor.action(Action::RefreshContext).unwrap();
    let stale = read(&mut reader);
    actor.state.record_context(Some(160000), Some(200000));
    actor
        .message(
            json!({"type":"control_response","response":{"subtype":"success",
        "request_id":stale["request_id"],"response":rich_context_fixture()}}),
        )
        .unwrap();
    assert!(!actor.state.context_query_pending);
    assert_eq!(actor.state.saved.context.unwrap().used, 160000);
    assert!(actor
        .state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .categories
        .is_empty());
}

#[test]
fn rich_context_legacy_categories_and_deferred_tools_keep_their_meaning() {
    let (_, details) = context_measurement(&json!({"totalTokens":40,"maxTokens":100,
        "categories":[{"name":"Skills","tokens":40},{"name":"Free space","tokens":50},
            {"name":"Autocompact buffer","tokens":10},
            {"id":"mcp_tools","name":"MCP tools","tokens":3,"kind":"used"},
            {"id":"mcp_tools_deferred","name":"Deferred tools","tokens":7,"kind":"deferred"}],
        "mcpTools":[{"name":"loaded","tokens":3,"isLoaded":true},
            {"name":"deferred","tokens":7,"isLoaded":false}]}))
    .unwrap();
    assert_eq!(
        details.categories[0].kind,
        provider::ContextCategoryKind::Used
    );
    assert_eq!(
        details.categories[1].kind,
        provider::ContextCategoryKind::Free
    );
    assert_eq!(
        details.categories[2].kind,
        provider::ContextCategoryKind::Buffer
    );
    assert_eq!(details.categories[3].entries[0].label, "loaded");
    assert_eq!(details.categories[4].entries[0].label, "deferred");
}

#[test]
fn assistant_usage_and_result_window_feed_the_context_meter() {
    let mut actor = actor();
    actor.state.turn_id = Some("turn".into());
    actor.state.status = Status::Working;
    actor.message(json!({"type":"assistant","parent_tool_use_id":null,"message":{"id":"m","model":"claude-opus-5-5","content":[{"type":"text","text":"OK."}],
        "usage":{"input_tokens":2,"cache_creation_input_tokens":12922,"cache_read_input_tokens":12654,"output_tokens":5}}})).unwrap();
    // The window is unknown until the result reports it.
    assert!(actor.state.saved.context.is_none());
    // Subagent traffic never moves the primary meter.
    actor.message(json!({"type":"assistant","parent_tool_use_id":"task","message":{"id":"s","model":"claude-haiku","content":[],
        "usage":{"input_tokens":900000,"output_tokens":1}}})).unwrap();
    actor
        .message(json!({"type":"result","is_error":false,"modelUsage":{
        "claude-haiku":{"contextWindow":200000},
        "claude-opus-5-5":{"contextWindow":1000000}}}))
        .unwrap();
    assert_eq!(
        actor.state.saved.context,
        Some(provider::ContextUsage {
            used: 25_583,
            window: 1_000_000
        })
    );
}
