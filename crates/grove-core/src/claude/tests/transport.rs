use super::*;

#[test]
fn side_chat_profile_uses_normal_auth_without_workflow_guidance() {
    let mut actor = actor();
    actor.side_chat = true;
    let command = launch_command_profile(&[], &actor.state.saved, true).unwrap();
    let argv = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(!argv.iter().any(|arg| arg == "--bare"));
    let guidance = argv
        .windows(2)
        .find(|pair| pair[0] == "--append-system-prompt")
        .unwrap();
    assert!(guidance[1].contains("read-only side conversation"));
    assert!(!guidance[1].contains(crate::agent_guidance::WORKFLOW_GUIDANCE));
    assert!(!guidance[1].contains(crate::tasks::CLAUDE_GUIDANCE));
    assert!(actor
        .action(Action::Configure {
            model: None,
            effort: None,
            access: Some("full-access".into()),
        })
        .is_err());
    actor.capture_edit_checkpoint();
    assert!(actor.state.saved.edit_checkpoints.is_empty());
    actor.state.saved.key = "invalid key".into();
    actor.save();
    assert!(actor.state.error.is_none());
}

#[test]
fn side_chat_state_is_saved_by_claude_actor() {
    let mut actor = actor();
    actor
        .action(super::super::Action::SideChat {
            checkout: "/checkout".into(),
            chat: crate::provider::SavedSideChat {
                session: None,
                draft: "draft".into(),
                messages: vec![("user".into(), "question".into())],
            },
        })
        .unwrap();
    assert_eq!(actor.state.saved.side_chats["/checkout"].draft, "draft");
}

#[test]
fn provider_errors_keep_claude_result_details() {
    let mut actor = actor();
    actor.state.turn_id = Some("turn-1".into());
    actor
        .message(json!({"type":"assistant","error":"rate_limit","message":{"content":[]}}))
        .unwrap();
    assert_eq!(
        actor.state.effective_notice().unwrap().kind,
        provider::SessionNoticeKind::Stopped
    );
    assert!(actor.state.effective_notice().unwrap().usage_url.is_none());
    actor.state.error = None;
    actor.state.notice = None;
    actor
        .message(json!({"type":"result","is_error":true,"errors":["First detail","Second detail"]}))
        .unwrap();
    assert_eq!(
        actor.state.error.as_deref(),
        Some("First detail\nSecond detail")
    );
    assert_eq!(
        actor.state.effective_notice().unwrap().kind,
        provider::SessionNoticeKind::Stopped
    );
}

#[test]
fn work_period_waits_for_claude_idle_and_closes_on_disconnect() {
    let mut actor = actor();
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.state.turn_id = Some("turn".into());
    actor.state.begin_work();
    actor
        .message(json!({"type":"result","subtype":"success","is_error":false}))
        .unwrap();
    assert!(actor.state.work_started.is_some());
    assert!(actor.state.saved.work_periods[0].end_item.is_none());
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .unwrap();
    assert!(actor.state.work_started.is_none());
    assert!(actor.state.saved.work_periods[0].duration_ms.is_some());
    actor.state.begin_work();
    actor.fail("disconnected".into());
    assert!(actor.state.work_started.is_none());
    assert!(actor.state.saved.work_periods[1].end_item.is_some());
}

#[test]
fn launch_enables_claude_ai_artifact_tools_outside_side_chats() {
    let actor = actor();
    for side_chat in [false, true] {
        let command = launch_command_profile(&[], &actor.state.saved, side_chat).unwrap();
        for name in ["CLAUDE_CODE_ARTIFACT", "CLAUDE_CODE_ARTIFACT_VERIFY"] {
            let value = command
                .get_envs()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| value);
            let expected = (!side_chat).then(|| std::ffi::OsStr::new("1"));
            assert_eq!(value, expected, "{name} side_chat={side_chat}");
        }
        let guided = command
            .get_args()
            .any(|arg| arg.to_string_lossy().contains(CLAUDE_AI_ARTIFACT_GUIDANCE));
        assert_eq!(guided, !side_chat);
    }
}

#[test]
fn api_error_terminal_reason_cannot_report_success() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn".into());
    actor.message(json!({"type":"result", "subtype":"success", "terminal_reason":"api_error", "is_error":false, "errors":["Login expired"]})).unwrap();
    assert_eq!(actor.state.status, Status::Failed);
    assert!(actor
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("Login expired"));
}

#[test]
fn rich_context_query_errors_and_timeouts_never_fail_the_session() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Done;
    actor.action(Action::RefreshContext).unwrap();
    let request = read(&mut reader);
    actor
        .message(
            json!({"type":"control_response","response":{"subtype":"error",
        "request_id":request["request_id"],"error":"not supported"}}),
        )
        .unwrap();
    assert_eq!(
        actor.state.context_query_error.as_deref(),
        Some("not supported")
    );
    assert_eq!(actor.state.status, Status::Done);
    assert!(actor.state.error.is_none());
    actor.action(Action::RefreshContext).unwrap();
    let _ = read(&mut reader);
    actor.context_query_started =
        Some(Instant::now().checked_sub(Duration::from_secs(16)).unwrap());
    actor.poll_context_query();
    assert!(!actor.state.context_query_pending);
    assert!(actor
        .state
        .context_query_error
        .as_deref()
        .unwrap()
        .contains("timed out"));
    assert!(actor.state.error.is_none());
}

#[test]
fn automatic_boundary_fills_after_from_next_usage_and_probe_errors_are_harmless() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.turn_id = Some("turn".into());
    actor.state.status = Status::Working;
    actor
        .message(json!({"type":"system","subtype":"status","status":"compacting"}))
        .unwrap();
    assert!(actor.state.compacting);
    actor.message(json!({"type":"system","subtype":"compact_boundary","uuid":"b","compact_metadata":{"trigger":"auto","pre_tokens":182000}})).unwrap();
    let probe = read(&mut reader);
    actor.message(json!({"type":"control_response","response":{"subtype":"error","request_id":probe["request_id"],"error":"unsupported"}})).unwrap();
    assert!(actor.state.error.is_none());
    assert!(actor.process.is_some());
    assert_eq!(
        provider::compaction_label(&actor.state.saved.items[0]),
        "Context compacted \u{b7} auto"
    );
    actor
        .message(
            json!({"type":"assistant","message":{"id":"m","model":"claude-opus-5-5","content":[],
        "usage":{"input_tokens":4000,"cache_read_input_tokens":20000,"output_tokens":10}}}),
        )
        .unwrap();
    assert_eq!(
        provider::compaction_label(&actor.state.saved.items[0]),
        "Context compacted \u{b7} auto \u{b7} 182k \u{2192} 24k"
    );
}
