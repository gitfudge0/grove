use super::*;

#[test]
#[cfg(unix)]
fn speed_live_control_is_session_scoped_and_reported_separately() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.state.resolved_model = Some("claude-opus-test".into());
    actor.state.models = (vec![
        json!({"model":"opus", "resolvedModel":"claude-opus-test", "supportsFastMode":true}),
    ])
    .into();
    actor.state.saved.draft = "preserve".into();
    actor.state.resolved_access = Some("plan".into());
    actor
        .action(Action::SetSpeed(provider::Speed::Fast))
        .unwrap();
    actor.configure_live().unwrap();
    let request = read(&mut reader);
    assert_eq!(request["request"]["settings"]["fastMode"], true);
    assert!(
        !actor.reset_configuration,
        "Speed applies without restarting or changing model/access"
    );
    actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
    assert_eq!(
        actor.state.resolved_model.as_deref(),
        Some("claude-opus-test")
    );
    assert_eq!(actor.state.resolved_access.as_deref(), Some("plan"));
    assert!(
        actor.state.reported_speed.is_none(),
        "ACK is not effective tier evidence"
    );
    actor.message(json!({"type":"system","subtype":"init","model":"claude-opus-test","fast_mode_state":"off","fast_mode_disabled_reason":"extra_usage_disabled"})).unwrap();
    assert_eq!(actor.state.reported_speed.as_deref(), Some("off"));
    assert_eq!(
        actor.state.saved.composer.speed,
        Some(provider::Speed::Fast)
    );
    actor.action(Action::ResetModelEffort).unwrap();
    assert_eq!(
        actor.state.saved.composer.speed,
        Some(provider::Speed::Fast)
    );
    assert_eq!(actor.state.saved.draft, "preserve");
    actor
        .action(Action::SetSpeed(provider::Speed::Standard))
        .unwrap();
    actor.configure_live().unwrap();
    let request = read(&mut reader);
    assert_eq!(request["request"]["settings"]["fastMode"], false);
    assert!(actor
        .action(Action::SetSpeed(provider::Speed::Ultrafast))
        .is_err());
}

#[test]
fn speed_launch_overlay_preserves_settings_and_never_enables_fast_before_catalog() {
    let mut actor = actor();
    actor.state.saved.composer.speed = Some(provider::Speed::Fast);
    let cmd = launch_command(
        &[
            "--settings".into(),
            r#"{"env":{"CUSTOM":"kept"},"fastMode":true}"#.into(),
        ],
        &actor.state.saved,
    )
    .unwrap();
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    let settings: Value =
        serde_json::from_str(args.windows(2).find(|a| a[0] == "--settings").unwrap()[1].as_str())
            .unwrap();
    assert_eq!(settings["env"]["CUSTOM"], "kept");
    assert_eq!(settings["fastMode"], false);
}

#[test]
fn claude_mode_discovery_requires_advertised_choices() {
    assert_eq!(
        permission_modes_from_help(
            "--permission-mode <mode> Permission mode (choices: \"acceptEdits\", \"manual\", \"plan\")"
        ),
        vec!["acceptEdits", "manual", "plan"]
    );
    assert!(permission_modes_from_help("unknown CLI version").is_empty());
}

#[test]
fn deferred_inline_preserves_identical_composer_after_configuration() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.reconfigure = true;
    actor.state.saved.draft = "same text".into();
    actor
        .action(Action::PromptInline {
            text: "same text".into(),
            images: vec![],
        })
        .unwrap();
    assert!(actor.deferred_inline);
    let _ = read(&mut reader);
    actor.state.status = Status::Idle;
    let prompt = actor.deferred.take().unwrap();
    actor.submit(prompt.id, prompt.text, prompt.images).unwrap();
    let sent = read(&mut reader);
    actor.acknowledge_input(&sent["uuid"]);
    assert_eq!(actor.state.saved.draft, "same text");
    actor.deferred_inline = true;
    actor.fail("failure".into());
    assert!(!actor.deferred_inline);
}

#[test]
fn dismissing_uncertain_deferred_input_prevents_replay_after_configuration_failure() {
    let mut actor = actor();
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.reconfigure = true;
    actor.state.queued_prompts.push(QueuedPrompt {
        id: 41,
        text: "deferred".into(),
        images: Vec::new(),
    });
    // No transport: the configuration control cannot be delivered, leaving a deferred copy.
    assert!(actor.submit(41, "deferred".into(), Vec::new()).is_err());
    assert_eq!(actor.deferred.as_ref().unwrap().id, 41);
    assert_eq!(actor.state.queue_uncertain, vec![41]);
    actor.action(Action::DismissUncertainQueued(41)).unwrap();
    assert!(actor.deferred.is_none());
    assert!(actor.state.queued_prompts.is_empty());
    assert!(!actor.queue_ready);
}

#[test]
fn launch_preserves_config_and_rejects_transport_takeover() {
    assert_eq!(
        launch_options(&["--model=sonnet".into(), "hello".into()]).unwrap(),
        (
            vec!["--model".into(), "sonnet".into()],
            Some("hello".into())
        )
    );
    for flag in [
        "--output-format",
        "--resume",
        "--no-session-persistence",
        "--worktree",
        "--print",
    ] {
        assert!(launch_options(&[flag.into()]).is_err());
    }
    assert!(launch_options(&["--effort".into()]).is_err());
}

#[test]
fn configure_merges_fields_and_applies_native_control_requests() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor
        .action(Action::Configure {
            model: Some("sonnet".into()),
            effort: Some("high".into()),
            access: Some("plan".into()),
        })
        .unwrap();
    actor
        .action(Action::Configure {
            model: None,
            effort: Some("low".into()),
            access: None,
        })
        .unwrap();
    assert_eq!(actor.state.model.as_deref(), Some("sonnet"));
    assert!(actor.state.access.is_none());
    assert_eq!(actor.pending_access.as_deref(), Some("plan"));
    actor.configure_live().unwrap();
    let first = read(&mut reader);
    let second = read(&mut reader);
    let third = read(&mut reader);
    assert_eq!(
        first["request"],
        json!({"subtype":"set_model","model":"sonnet"})
    );
    assert_eq!(
        second["request"],
        json!({"subtype":"set_permission_mode","mode":"plan"})
    );
    assert_eq!(
        third["request"],
        json!({"subtype":"apply_flag_settings","settings":{"effortLevel":"low"}})
    );
    for request in [first, second, third] {
        actor.message(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).unwrap();
    }
    assert_eq!(actor.state.status, Status::Idle);
    actor
        .action(Action::Configure {
            model: Some("haiku".into()),
            effort: None,
            access: None,
        })
        .unwrap();
    assert!(actor.state.effort.is_none());
    assert_eq!(actor.state.access.as_deref(), Some("plan"));
    actor.action(Action::ResetConfiguration).unwrap();
    assert!(actor.state.model.is_none());
    assert!(actor.state.access.is_none());
    assert!(actor.reset_configuration);
}

#[test]
fn advanced_tool_and_mcp_controls_roundtrip_and_cancel() {
    let mut actor = actor();
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("turn".into());
    let mut reader = transport(&mut actor);
    actor.message(json!({"type":"control_request","request_id":"plan","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","input":{"plan":"Build it"},"permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}})).unwrap();
    actor
        .action(Action::RespondApproval {
            id: json!("plan"),
            choice: "claude_suggestion_0".into(),
            input: Some(json!({"plan":"Revised"})),
            feedback: None,
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"plan","response":{"behavior":"allow","updatedInput":{"plan":"Revised"},"updatedPermissions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}}})
    );
    let request = json!({"type":"control_request","request_id":"form","request":{"subtype":"elicitation","mcp_server_name":"trusted-server","message":"Ready?","mode":"form","requested_schema":{"type":"object","properties":{"ready":{"type":"boolean"}},"required":["ready"]}}});
    actor.message(request.clone()).unwrap();
    actor.message(request.clone()).unwrap();
    assert_eq!(actor.state.elicitations.len(), 1);
    assert_eq!(actor.state.elicitations[0].server_name, "trusted-server");
    actor
        .message(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Waiting);
    actor
        .action(Action::RespondElicitation {
            id: json!("form"),
            action: "accept".into(),
            content: Some(json!({"ready":false})),
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"form","response":{"action":"accept","content":{"ready":false}}}})
    );
    actor.message(json!({"type":"control_request","request_id":"url","request":{"subtype":"elicitation","mcp_server_name":"server","message":"Authorize","mode":"url","elicitation_id":"flow","url":"https://example.com/auth"}})).unwrap();
    actor
        .action(Action::RespondElicitation {
            id: json!("url"),
            action: "accept".into(),
            content: None,
        })
        .unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"url","response":{"action":"accept"}}})
    );
    actor.message(request).unwrap();
    actor
        .message(json!({"type":"control_cancel_request","request_id":"form"}))
        .unwrap();
    assert!(actor.state.elicitations.is_empty());
    assert!(actor
        .action(Action::RespondElicitation {
            id: json!("form"),
            action: "accept".into(),
            content: Some(json!({"ready":true}))
        })
        .is_err());
    actor.message(json!({"type":"control_request","request_id":"dialog","request":{"subtype":"request_user_dialog","dialog_kind":"future_dialog","payload":{}}})).unwrap();
    assert_eq!(
        read(&mut reader),
        json!({"type":"control_response","response":{"subtype":"error","request_id":"dialog","error":"Grove does not advertise this dialog kind."}})
    );
    assert!(actor
        .state
        .error
        .as_deref()
        .unwrap()
        .contains("future_dialog"));
    assert!(!serde_json::to_string(&actor.state.saved)
        .unwrap()
        .contains("trusted-server"));
}

#[test]
fn rich_context_full_effective_window_survives_result_capacity_and_model_switches() {
    let mut actor = actor();
    let (usage, details) = context_measurement(&rich_context_fixture()).unwrap();
    actor.state.saved.context = Some(usage);
    actor.state.context_tokens = Some(usage.used);
    actor.state.saved.context_details = Some(details);
    actor.context_model = Some("claude-test".into());
    actor.message(json!({"type":"result","is_error":false,"modelUsage":{"claude-test":{"contextWindow":1000000}}})).unwrap();
    assert_eq!(actor.state.saved.context.unwrap().window, 200000);
    actor.message(json!({"type":"assistant","message":{"id":"new","model":"new-model","content":[],"usage":{"input_tokens":3000}}})).unwrap();
    assert!(actor.state.saved.context.is_none());
    actor.message(json!({"type":"result","is_error":false,"modelUsage":{"child-a":{"contextWindow":1000000},"child-b":{"contextWindow":200000}}})).unwrap();
    assert!(actor.state.saved.context.is_none());
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
fn mcp_status_failure_is_optional_and_visible_without_failing_the_conversation() {
    let mut actor = actor();
    actor.state.status = Status::Idle;
    actor.pending.insert("mcp".into(), "mcp_status".into());
    let event = json!({"type":"control_response","response":{"request_id":"mcp","subtype":"success","response":{"mcpServers":[{"name":"jira","status":"needs-auth","error":"OAuth expired","config":{"headers":{"Authorization":"secret"}}}]}}});
    actor.message(event.clone()).unwrap();
    assert_eq!(actor.state.status, Status::Idle);
    assert!(actor.state.error.is_none());
    assert_eq!(
        actor.state.saved.items.last().unwrap()["type"],
        "providerNotice"
    );
    assert!(!serde_json::to_string(&actor.state.saved.items)
        .unwrap()
        .contains("secret"));
    let count = actor.state.saved.items.len();
    actor.pending.insert("mcp".into(), "mcp_status".into());
    actor.message(event).unwrap();
    assert_eq!(actor.state.saved.items.len(), count);
    actor.pending.insert("old-cli".into(), "mcp_status".into());
    actor.message(json!({"type":"control_response","response":{"request_id":"old-cli","subtype":"error","error":"Unsupported request"}})).unwrap();
    assert_eq!(actor.state.status, Status::Idle);
}

#[test]
fn mcp_health_poll_runs_after_initialization_and_bounds_unanswered_requests() {
    use std::io::BufRead as _;
    let mut actor = actor();
    let mut wire = transport(&mut actor);
    actor.poll_mcp_status();
    assert!(actor.pending.is_empty());
    actor.initialized = true;
    actor.poll_mcp_status();
    let mut line = String::new();
    wire.read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["request"]["subtype"], "mcp_status");
    let count = actor.control_id;
    actor.poll_mcp_status();
    assert_eq!(actor.control_id, count);
    actor.mcp_polled_at = Some(Instant::now().checked_sub(Duration::from_secs(16)).unwrap());
    actor.poll_mcp_status();
    assert!(actor.pending.is_empty());
    assert!(!actor.mcp_status_supported);
    assert!(actor.state.error.is_none());
}

#[test]
fn mcp_health_request_does_not_delay_configuration_acknowledgment() {
    let mut actor = actor();
    actor.initialized = true;
    actor.configuring = true;
    actor.state.status = Status::Starting;
    actor.pending.insert("health".into(), "mcp_status".into());
    actor
        .pending
        .insert("setting".into(), "apply_flag_settings".into());
    actor.message(json!({"type":"control_response","response":{"request_id":"setting","subtype":"success","response":{}}})).unwrap();
    assert!(!actor.configuring);
    assert_eq!(actor.state.status, Status::Idle);
    assert_eq!(actor.pending.len(), 1);
}
