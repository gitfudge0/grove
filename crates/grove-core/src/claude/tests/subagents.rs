use super::*;

#[test]
fn provider_rate_limit_events_preserve_continue_and_child_boundaries() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn-1".into());
    actor
        .message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning"}}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.state.error.is_none());
    assert_eq!(
        actor.state.account_usage.as_ref().unwrap().status,
        crate::account_usage::UsageStatus::Warning
    );
    assert_eq!(
        actor.state.saved.items.last().unwrap()["severity"],
        "warning"
    );
    actor.message(json!({"type":"rate_limit_event","parent_tool_use_id":"child","rate_limit_info":{"status":"rejected"}})).unwrap();
    assert!(actor.state.error.is_none());
    assert_eq!(
        actor.state.account_usage.as_ref().unwrap().status,
        crate::account_usage::UsageStatus::Warning
    );
    actor.message(json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1234}})).unwrap();
    let notice = actor.state.effective_notice().unwrap();
    assert_eq!(notice.kind, provider::SessionNoticeKind::UsageLimit);
    assert_eq!(notice.reset_at, Some(1234));
    assert_eq!(actor.state.status, Status::Working);
    actor
        .message(json!({"type":"assistant","error":"rate_limit","message":{"content":[]}}))
        .unwrap();
    assert_eq!(actor.state.effective_notice().unwrap().reset_at, Some(1234));
    actor.message(json!({"type":"result","is_error":true,"subtype":"error_during_execution","errors":["Request rejected"]})).unwrap();
    assert_eq!(
        actor.state.effective_notice().unwrap().kind,
        provider::SessionNoticeKind::UsageLimit
    );
    assert_eq!(actor.state.effective_notice().unwrap().reset_at, Some(1234));
}

#[test]
fn child_authoritative_handback_report_is_visible_and_deduplicated() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.register_subagent(&json!({"id":"child","name":"Agent","input":{"prompt":"Inspect"}}));
    let report = "Full child final report. ".repeat(100);
    let structured = json!({"agentId":"provider-child","status":"completed","handback":"send","handbackReport":{"text":report},"content":[{"type":"text","text":"Report delivered elsewhere, not repeated here"}]});
    let result = json!({"type":"tool_result","tool_use_id":"child","content":"Report delivered elsewhere, not repeated here"});
    actor.subagent_tool_result(&result, Some(&structured));
    actor.subagent_tool_result(&result, Some(&structured));
    let items = &actor.state.subagent_activity["child"].items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "agentMessage");
    assert_eq!(items[0]["phase"], "final_answer");
    assert_eq!(items[0]["text"], report);
    assert_eq!(actor.state.subagent_activity["child"].events.len(), 1);
    let mut child = ChildTranscript::default();
    child.message(&json!({"type":"assistant","message":{"id":"message","content":[{"type":"text","text":report}]}}), "/repo");
    child.message(&json!({"type":"result","result":report}), "/repo");
    assert_eq!(child.items.len(), 1);
    assert_eq!(child.items[0]["phase"], "final_answer");
}

#[test]
fn child_fragmented_final_frames_preserve_stream_block_identity() {
    let mut child = ChildTranscript::default();
    child.message(&json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"fragmented"}}}), "/repo");
    for (index, text) in [(0, "First response"), (2, "Second response")] {
        child.message(&json!({"type":"stream_event","event":{"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}}), "/repo");
        child.message(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}}), "/repo");
    }
    for text in ["First response", "Second response"] {
        child.message(&json!({"type":"assistant","message":{"id":"fragmented","content":[{"type":"text","text":text}]}}), "/repo");
    }
    assert_eq!(child.items.len(), 2);
    assert_eq!(child.items[0]["id"], "fragmented-0");
    assert_eq!(child.items[0]["text"], "First response");
    assert_eq!(child.items[1]["id"], "fragmented-2");
    assert_eq!(child.items[1]["text"], "Second response");
}

#[test]
fn child_transcript_preserves_full_streamed_messages_and_tool_results() {
    let mut child = ChildTranscript::default();
    let text = "Detailed child output. ".repeat(160);
    child.message(
        &json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"message"}}}),
        "/repo",
    );
    child.message(&json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}), "/repo");
    child.message(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}}), "/repo");
    assert_eq!(child.items[0]["text"], text);
    child.message(&json!({"type":"assistant","message":{"id":"message","content":[{"type":"text","text":text},{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"cat file"}},{"type":"thinking","thinking":"private"}]}}), "/repo");
    assert_eq!(child.items.len(), 2);
    assert_eq!(child.items[1]["type"], "commandExecution");
    assert_eq!(child.items[1]["status"], "inProgress");
    child.message(&json!({"type":"user","uuid":"user","message":{"content":[{"type":"text","text":text},{"type":"tool_result","tool_use_id":"bash","content":text,"is_error":true}]}}), "/repo");
    assert_eq!(child.items.len(), 3);
    assert_eq!(child.items[1]["aggregatedOutput"], text);
    assert_eq!(child.items[1]["result"]["content"][0]["text"], text);
    assert_eq!(child.items[1]["status"], "failed");
    assert_eq!(child.items[2]["content"][0]["text"], text);
    child.message(&json!({"type":"assistant","message":{"id":"edit","content":[{"type":"tool_use","id":"write","name":"Write","input":{"file_path":"/repo/file","content":"new"}}]}}), "/repo");
    child.message(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"write","content":"written"}]},"tool_use_result":{"type":"create","filePath":"/repo/file","content":"new"}}), "/repo");
    assert_eq!(child.items[3]["type"], "fileChange");
    assert_eq!(child.items[3]["changes"][0]["diff"], "+new\n");
    assert_eq!(child.items[3]["status"], "completed");
}

#[test]
fn subagent_prompt_is_task_fallback_and_activity_does_not_replace_it() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.register_subagent(&json!({"id":"child", "name":"Agent", "input":{"subagent_type":"Researcher", "prompt":"Inspect the parser"}}));
    actor.subagent_block("child", &json!({"type":"text", "text":"Reading source"}));
    let child = &actor.state.saved.work_periods[0].subagents[0];
    assert_eq!(child.role.as_deref(), Some("Researcher"));
    assert_eq!(child.task.as_deref(), Some("Inspect the parser"));
    assert_eq!(child.activity, "Reading source");
}

#[test]
fn background_child_reports_and_terminal_patches_reach_the_roster() {
    let mut actor = actor();
    actor.state.begin_work();
    let spawn = json!({"id":"child","name":"Agent","input":{"subagent_type":"general-purpose","description":"Inspect directory"}});
    actor.register_subagent(&spawn);
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"child","run_id":"run","task_type":"local_agent"})).unwrap();
    let done = json!({"type":"system","subtype":"task_notification","task_id":"task","tool_use_id":"child","run_id":"run","status":"completed","handback_report":{"text":"FINAL_REPORT"}});
    actor.message(done.clone()).unwrap();
    actor.message(done).unwrap();
    assert_eq!(
        actor.state.subagent_activity["child"]
            .items
            .iter()
            .filter(|item| item["type"] == "agentMessage" && item["text"] == "FINAL_REPORT")
            .count(),
        1
    );
    actor.register_subagent(&json!({"id":"stopped","name":"Task","input":{}}));
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"stopped-task","tool_use_id":"stopped","run_id":"stopped-run","task_type":"local_agent"})).unwrap();
    actor.message(json!({"type":"system","subtype":"task_updated","task_id":"stopped-task","run_id":"stopped-run","patch":{"status":"killed"}})).unwrap();
    actor.register_subagent(&json!({"id":"stopped","name":"Task","input":{}}));
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[1].status,
        SubagentStatus::Interrupted
    );
    actor.subagent_tool_result(&json!({"tool_use_id":"stopped","is_error":false}), None);
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[1].status,
        SubagentStatus::Interrupted
    );
    actor.message(json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":"old-run","status":"failed","handback_report":{"text":"STALE"}})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
    assert!(!actor.state.subagent_activity["child"]
        .items
        .iter()
        .any(|item| item["text"] == "STALE"));
    actor
        .message(json!({"type":"result","parent_tool_use_id":"stopped","is_error":false}))
        .unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[1].status,
        SubagentStatus::Interrupted
    );
    for (index, (status, expected)) in [
        ("completed", SubagentStatus::Completed),
        ("failed", SubagentStatus::Failed),
        ("killed", SubagentStatus::Interrupted),
        ("stopped", SubagentStatus::Interrupted),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("patch-{index}");
        actor.register_subagent(&json!({"id":id,"name":"Agent","input":{}}));
        actor.message(json!({"type":"system","subtype":"task_started","task_id":id,"tool_use_id":id,"run_id":id,"task_type":"local_agent"})).unwrap();
        let subtype = if status == "stopped" {
            "task_notification"
        } else {
            "task_updated"
        };
        actor.message(json!({"type":"system","subtype":subtype,"task_id":id,"run_id":id,"status":status,"patch":{"status":status}})).unwrap();
        assert_eq!(
            actor.state.saved.work_periods[0].subagents[index + 2].status,
            expected
        );
    }
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"nested","tool_use_id":"nested-tool","parent_task_id":"task","run_id":"nested-run","task_type":"local_agent","subagent_type":"Explore","description":"Nested scan"})).unwrap();
    let nested_report = json!({"type":"system","subtype":"task_notification","task_id":"nested","run_id":"nested-run","status":"completed","handback_report":{"text":"NESTED_FINAL"}});
    actor.message(nested_report.clone()).unwrap();
    actor.message(nested_report).unwrap();
    assert_eq!(
        actor.state.subagent_activity["nested-tool"]
            .items
            .iter()
            .filter(|item| item["type"] == "agentMessage" && item["text"] == "NESTED_FINAL")
            .count(),
        1
    );
}

#[test]
fn child_history_roundtrip_restores_full_messages_tools_and_alias_updates() {
    use crate::provider::SubagentTranscriptAvailability as Availability;
    let mut actor = actor();
    actor.state.begin_work();
    actor.register_subagent(&json!({"id":"child","name":"Agent","input":{"name":"Ada","subagent_type":"Explore","description":"Inspect directory"}}));
    assert_eq!(
        actor.state.subagent_transcript_availability("child"),
        Availability::Awaiting
    );
    actor.message(json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"child","run_id":"run","task_type":"local_agent"})).unwrap();
    let text = "Full provider-visible report. ".repeat(200);
    actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message","content":[{"type":"text","text":text},{"type":"thinking","thinking":"PRIVATE"},{"type":"tool_use","id":"read","name":"Read","input":{"file_path":"file"}}]}})).unwrap();
    let pending = actor_with_saved(
        serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
    );
    assert_eq!(
        pending.state.subagent_activity["child"].items[1]["status"],
        "inProgress"
    );
    assert_eq!(
        pending.state.subagent_snapshot("child").status,
        Status::Idle
    );
    actor.message(json!({"type":"user","parent_tool_use_id":"child","message":{"content":[{"type":"tool_result","tool_use_id":"read","content":"full tool output"}]}})).unwrap();
    assert_eq!(
        actor.state.subagent_transcript_availability("child"),
        Availability::Live
    );
    let bytes = serde_json::to_vec(&actor.state.saved).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));
    let mut restored = actor_with_saved(provider::recover_saved_for_test(&actor.state.saved));
    assert_eq!(
        restored.state.subagent_transcript_availability("child"),
        Availability::Restored
    );
    assert_eq!(
        restored.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Unknown
    );
    assert_eq!(
        restored.state.saved.work_periods[0].subagents[0].name,
        "Ada"
    );
    assert_eq!(
        restored.state.subagent_activity["child"].items,
        actor.state.subagent_activity["child"].items
    );
    assert!(restored.state.subagent_activity["child"]
        .observed_at
        .is_none());
    assert!(restored.state.subagent_activity["child"].events.is_empty());
    restored.message(json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":"run","status":"completed","handback_report":{"text":text}})).unwrap();
    assert_eq!(restored.state.subagent_activity["child"].items.len(), 2);
    assert_eq!(
        restored.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
    let complete = actor_with_saved(provider::recover_saved_for_test(&restored.state.saved));
    assert_eq!(
        complete.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
    assert_eq!(complete.state.subagent_activity["child"].items.len(), 2);
    assert!(complete
        .state
        .saved
        .fresh_conversation()
        .claude_subagents
        .is_empty());
}

#[test]
fn subagents_track_live_nested_work_without_primary_state_pollution() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("primary-turn".into());
    actor.state.saved.thread_id = Some("primary".into());
    let spawn = json!({"type":"tool_use","id":"child","name":"Agent","input":{"name":"Explorer","subagent_type":"Explore","description":"Find auth paths","prompt":"Search all auth entry points"}});
    actor.block("child", &spawn);
    actor.block("child", &spawn);
    assert_eq!(actor.state.saved.work_periods[0].subagents.len(), 1);
    let item_count = actor.state.saved.items.len();
    actor.message(json!({"type":"assistant","session_id":"primary","parent_tool_use_id":"child","message":{"model":"child-model","usage":{"input_tokens":900},"content":[{"type":"tool_use","id":"read","name":"Read","input":{}},{"type":"tool_use","id":"nested","name":"Task","input":{"subagent_type":"Researcher","description":"Inspect tests"}}]}})).unwrap();
    assert_eq!(actor.state.saved.items.len(), item_count);
    assert_eq!(actor.state.saved.thread_id.as_deref(), Some("primary"));
    assert!(actor.context_model.is_none());
    assert_eq!(actor.state.status, Status::Working);
    let children = &actor.state.saved.work_periods[0].subagents;
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].name, "Explorer");
    assert_eq!(children[0].role.as_deref(), Some("Explore"));
    assert_eq!(children[0].task.as_deref(), Some("Find auth paths"));
    assert_eq!(children[0].activity, "Using Task");
    actor.message(json!({"type":"user","parent_tool_use_id":"child","message":{"content":[{"type":"tool_result","tool_use_id":"nested","is_error":true,"content":"failed"}]}})).unwrap();
    actor
        .message(json!({"type":"result","parent_tool_use_id":"child","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    let children = &actor.state.saved.work_periods[0].subagents;
    assert_eq!(children[0].status, SubagentStatus::Completed);
    assert_eq!(children[1].status, SubagentStatus::Failed);
    actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"content":[{"type":"text","text":"Late activity"}]}})).unwrap();
    assert_eq!(
        actor.state.saved.work_periods[0].subagents[0].status,
        SubagentStatus::Completed
    );
}

#[test]
fn subagent_recent_activity_claude_attribution_dedup_and_reasoning_exclusion() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.register_subagent(
        &json!({"id":"child", "name":"Agent", "input":{"description":"Inspect"}}),
    );
    let message = json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message-1","content":[{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"src/main.rs"}},{"type":"text","text":"Found the entry point"},{"type":"thinking","thinking":"private chain"}]}});
    actor.message(message.clone()).unwrap();
    actor.message(message).unwrap();
    let activity = &actor.state.subagent_activity["child"];
    assert_eq!(activity.events.len(), 2);
    assert_eq!(
        activity.events[0].kind,
        crate::provider::SubagentActivityKind::Tool
    );
    assert!(activity.events[0].text.contains("src/main.rs"));
    assert_eq!(
        activity.events[1].kind,
        crate::provider::SubagentActivityKind::Commentary
    );
    assert!(!activity
        .events
        .iter()
        .any(|e| e.text.contains("private chain")));
    assert_eq!(actor.state.subagent_activity.len(), 1);
    actor.message(json!({"type":"assistant","parent_tool_use_id":"child","message":{"id":"message-2","content":[{"type":"text","text":"Found the entry point"}]}})).unwrap();
    assert_eq!(actor.state.subagent_activity["child"].events.len(), 3);
    actor
        .message(json!({"type":"result","parent_tool_use_id":"child","is_error":false}))
        .unwrap();
    assert_eq!(actor.state.subagent_activity["child"].events.len(), 3);
}

#[test]
fn claude_task_output_wait_is_not_inferred_from_background_tools() {
    let mut actor = actor();
    actor.state.begin_work();
    actor.state.status = Status::Working;
    actor.block("output", &json!({"type":"tool_use","id":"output","name":"TaskOutput","input":{"task_id":"task","block":false}}));
    assert_eq!(actor.state.session_status(), "Working");
    actor.state.saved.items[0]["arguments"]["block"] = json!(true);
    assert_eq!(actor.state.session_status(), "Waiting");
    actor.state.saved.items[0]["status"] = json!("completed");
    assert_eq!(actor.state.session_status(), "Working");
}
