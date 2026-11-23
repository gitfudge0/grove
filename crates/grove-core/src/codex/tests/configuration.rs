use super::*;

#[test]
#[cfg(unix)]
fn side_chat_profile_has_no_workflow_or_inherited_mcp_tools() {
    use std::os::unix::process::CommandExt;
    let mut actor = actor();
    actor.side_chat = true;
    actor.side_mcp_servers = vec!["inherited".into()];
    let mut child = Command::new("cat")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
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
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    let params = &request["params"];
    assert_eq!(params["config"]["mcp_servers.inherited.enabled"], false);
    assert!(params["config"].get("mcp_servers.grove_activity").is_none());
    assert_eq!(params["config"]["tools.update_plan.enabled"], false);
    assert!(!params["developerInstructions"]
        .as_str()
        .unwrap()
        .contains(crate::agent_guidance::WORKFLOW_GUIDANCE));
    assert_eq!(params["sandbox"], "read-only");
    assert_eq!(params["approvalPolicy"], "never");
    actor
        .message(json!({"id":"permission","method":"item/permissions/requestApproval","params":{}}))
        .unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert!(response.get("error").is_some());
    assert!(actor.state.approvals.is_empty());
}

#[test]
fn composer_discovery_reads_configuration_without_starting_a_thread() {
    let (_handle, actions) = Handle::channel();
    let (updates, _output) = crate::latest::channel();
    let mut actor = Actor::new(saved(), actions, updates, Vec::new());
    actor.discovery_only = true;
    actor.state.saved.thread_id = None;
    actor.pending.insert(
        1,
        Request {
            kind: Pending::Config,
            deadline: Instant::now() + Duration::from_secs(30),
        },
    );
    // There is deliberately no process. The ordinary Config path would
    // attempt thread/start and fail; discovery must only read metadata.
    actor.message(json!({"id":1,"result":{"config":{"model":"configured-model","model_reasoning_effort":"high","sandbox_mode":"read-only","approval_policy":"on-request"}}})).unwrap();
    assert!(actor.pending.is_empty());
    assert!(actor.state.saved.thread_id.is_none());
    assert!(actor.process.is_none());
    assert_eq!(
        actor.state.resolved_model.as_deref(),
        Some("configured-model")
    );
    assert_eq!(actor.state.resolved_effort.as_deref(), Some("high"));
    assert_eq!(actor.state.resolved_access.as_deref(), Some("read-only"));
}

#[test]
#[cfg(unix)]
fn initialization_reuses_models_but_still_reads_workspace_skills_and_config() {
    for cached in [false, true] {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.pending.insert(
            99,
            Request {
                kind: Pending::Initialize,
                deadline: Instant::now() + Duration::from_secs(30),
            },
        );
        if cached {
            actor.state.models = (vec![
                json!({"model":"cached-model", "isDefault":true, "defaultReasoningEffort":"high"}),
            ])
            .into();
            actor.state.resolved_model = None;
        } else {
            actor.state.models.clear();
        }
        actor.message(json!({"id":99,"result":{}})).unwrap();
        assert_eq!(
            actor
                .pending
                .values()
                .any(|pending| matches!(pending.kind, Pending::Models)),
            !cached
        );
        assert!(actor
            .pending
            .values()
            .any(|pending| matches!(pending.kind, Pending::Skills)));
        assert!(actor
            .pending
            .values()
            .any(|pending| matches!(pending.kind, Pending::Config)));
        if cached {
            assert_eq!(actor.state.resolved_model.as_deref(), Some("cached-model"));
        }
    }
}

#[test]
fn speed_persistence_migration_and_catalog_request_fields() {
    let mut actor = actor();
    assert_eq!(ComposerSettings::default().speed, Some(Speed::Standard));
    let mut legacy = serde_json::to_value(saved()).unwrap();
    legacy.as_object_mut().unwrap().remove("composer");
    assert!(serde_json::from_value::<SavedSession>(legacy.clone())
        .unwrap()
        .composer
        .speed
        .is_none());
    legacy["composer"] = json!({"model":null,"effort":null,"access":null});
    assert!(serde_json::from_value::<SavedSession>(legacy)
        .unwrap()
        .composer
        .speed
        .is_none());
    actor.state.model = Some("catalog-model".into());
    actor.state.models = (vec![
        json!({"model":"catalog-model","serviceTiers":[{"id":"priority"},{"id":"ultrafast"}]}),
    ])
    .into();
    actor.state.saved.draft = "untouched".into();
    actor.state.saved.draft_images = vec!["attachment".into()];
    actor.action(Action::SetSpeed(Speed::Fast)).unwrap();
    assert_eq!(actor.turn_params(vec![])["serviceTierForTurn"], "priority");
    assert!(actor.state.reported_speed.is_none());
    actor.action(Action::ResetModelEffort).unwrap();
    assert_eq!(actor.state.saved.composer.speed, Some(Speed::Fast));
    let restored: SavedSession =
        serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
    assert_eq!(restored.composer.speed, Some(Speed::Fast));
    assert_eq!(restored.draft, "untouched");
    assert_eq!(restored.draft_images, ["attachment"]);
    actor.action(Action::SetSpeed(Speed::Standard)).unwrap();
    assert_eq!(actor.turn_params(vec![])["serviceTierForTurn"], "default");
    actor.state.models.clear();
    assert!(actor.action(Action::SetSpeed(Speed::Ultrafast)).is_err());
    assert_eq!(actor.state.saved.composer.speed, Some(Speed::Standard));
}

#[test]
fn live_catalog_ultra_effort_is_sent_and_persisted() {
    let mut actor = actor();
    // Codex CLI 0.163.0 model/list advertises these stops for GPT-6.1 Sol.
    actor.state.models = (vec![json!({
        "id":"gpt-6.1-sol", "model":"gpt-6.1-sol",
        "defaultReasoningEffort":"medium",
        "supportedReasoningEfforts":[
            {"reasoningEffort":"low"}, {"reasoningEffort":"medium"},
            {"reasoningEffort":"high"}, {"reasoningEffort":"xhigh"},
            {"reasoningEffort":"max"}, {"reasoningEffort":"ultra"}
        ]
    })])
    .into();
    actor
        .action(Action::Configure {
            model: Some("gpt-6.1-sol".into()),
            effort: Some("ultra".into()),
            access: None,
        })
        .unwrap();
    assert_eq!(actor.turn_params(vec![])["effort"], "ultra");
    let recovered =
        serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
    let (_, input) = mpsc::channel();
    let (updates, _) = crate::latest::channel();
    let mut restored = Actor::new(recovered, input, updates, Vec::new());
    assert_eq!(restored.turn_params(vec![])["effort"], "ultra");
    assert!(restored
        .action(Action::Configure {
            model: None,
            effort: Some("bogus".into()),
            access: None,
        })
        .is_err());
    assert_eq!(restored.turn_params(vec![])["effort"], "ultra");
}

#[test]
fn changing_models_explicitly_replaces_previous_effort_with_catalog_default() {
    let mut actor = actor();
    actor.state.models =
        (vec![json!({"id":"new-id","model":"new-model","defaultReasoningEffort":"low"})]).into();
    actor.state.effort = Some("xhigh".into());
    actor
        .action(Action::Configure {
            model: Some("new-model".into()),
            effort: None,
            access: None,
        })
        .unwrap();
    assert_eq!(actor.turn_params(vec![])["effort"], "low");
    assert_eq!(actor.state.saved.composer.effort.as_deref(), Some("low"));
    actor
        .action(Action::Configure {
            model: Some("new-model".into()),
            effort: Some("medium".into()),
            access: None,
        })
        .unwrap();
    assert_eq!(actor.turn_params(vec![])["effort"], "medium");
}

#[test]
fn skill_mentions_use_catalog_paths_and_optional_catalog_errors_are_nonfatal() {
    let mut actor = actor();
    actor.state.skills = (vec![
        json!({"name":"review","path":"/skills/review/SKILL.md","enabled":true}),
        json!({"name":"disabled","path":"/disabled","enabled":false}),
    ])
    .into();
    let input = actor.input("Use $review, then $disabled and $reviewer", vec![]);
    assert_eq!(input.len(), 2);
    assert_eq!(
        input[1],
        json!({"type":"skill","name":"review","path":"/skills/review/SKILL.md"})
    );
    actor.pending.insert(
        99,
        Request {
            kind: Pending::Models,
            deadline: Instant::now(),
        },
    );
    actor
        .message(json!({"id":99,"error":{"message":"unsupported"}}))
        .unwrap();
    assert_eq!(actor.state.status, Status::Working);
    assert!(actor.state.error.is_none());
}

#[test]
fn grove_workflow_guidance_preserves_config_in_start_and_resume_requests() {
    for resume in [false, true] {
        for custom in [None, Some("Follow the team's instructions")] {
            let (mut actor, mut reader) = idle_actor();
            if !resume {
                actor.state.saved.thread_id = None;
            }
            actor
                .request("config/read", json!({}), Pending::Config)
                .unwrap();
            let config_request = read_line(&mut reader);
            actor.message(json!({"id":config_request["id"],"result":{"config":{"developer_instructions":custom}}})).unwrap();
            let request = read_line(&mut reader);
            assert_eq!(
                request["method"],
                if resume {
                    "thread/resume"
                } else {
                    "thread/start"
                }
            );
            let guidance = request["params"]["developerInstructions"].as_str().unwrap();
            assert!(guidance.contains("You are running inside Grove."));
            assert!(guidance.contains("even when a skill prescribes a different format or handoff"));
            assert!(guidance.contains("Markdown link to the .grove-plan.json artifact"));
            assert!(guidance.contains("Grove's Review response and Send response controls"));
            assert!(guidance.contains("does not override explicit user preferences"));
            assert!(guidance.contains(crate::tasks::CODEX_GUIDANCE));
            assert!(!guidance.contains(crate::tasks::CLAUDE_GUIDANCE));
            assert!(guidance.contains(crate::reports::REPORT_GUIDANCE));
            assert!(guidance.contains(crate::describe_skill::SKILL));
            assert!(guidance.contains(crate::review_skill::SKILL));
            assert!(guidance.ends_with(crate::rich_plan::GUIDANCE));
            if let Some(custom) = custom {
                assert!(guidance.starts_with(&format!("{custom}\n\n")));
            }
        }
    }
}

#[test]
fn context_token_accounting_is_retired_when_model_changes() {
    let mut actor = actor();
    actor.state.resolved_model = Some("old-model".into());
    actor.message(usage("turn-1", 76000, Some(200000))).unwrap();
    assert!(actor
        .state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .is_some());
    actor
        .action(Action::Configure {
            model: Some("new-model".into()),
            effort: None,
            access: None,
        })
        .unwrap();
    assert!(actor
        .state
        .saved
        .context_details
        .as_ref()
        .unwrap()
        .accounting
        .is_none());
}
