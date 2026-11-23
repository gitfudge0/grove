use super::*;

#[test]
fn plan_ready_uses_latest_actionable_revision_in_the_completed_turn() {
    let mut snapshot = Snapshot::new(saved());
    snapshot.begin_work();
    snapshot
        .saved
        .items
        .push(json!({"id":"plan-message","type":"agentMessage","text":"Plan"}));
    snapshot.finish_work();
    snapshot.status = Status::Done;
    snapshot.completed_for_review();
    let plan: crate::rich_plan::PlanRevision = serde_json::from_value(json!({
        "message_id":"plan-message","source":"plan.grove-plan.json","assets":[],
        "document":{"schema_version":1,"plan_id":"status-plan","revision_id":"r1","title":"Plan","claims":[{"id":"result","text":"Implement status"}]}
    })).unwrap();
    snapshot.saved.rich_plans.revisions.push(plan.clone());
    assert_eq!(snapshot.session_status(), "Plan ready");
    assert!(snapshot
        .mark_reviewed(snapshot.review_completion().unwrap())
        .is_err());
    let mut latest = plan;
    latest.document.revision_id = "r2".into();
    latest.review.delivery = crate::rich_plan::ResponseDelivery::Sent { id: 1 };
    snapshot.saved.rich_plans.revisions.push(latest);
    assert_eq!(
        snapshot.session_status(),
        "Review",
        "superseded draft must not keep Plan ready alive"
    );
}

#[test]
fn live_reasoning_tools_and_plan_updates_remain_readable() {
    let mut state = state();
    for delta in ["Check ", "repository"] {
        assert!(state.apply(&json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"reason","summaryIndex":0,"delta":delta}})));
    }
    assert!(state.transcript().contains("Check repository"));
    assert!(state.apply(&json!({"method":"item/mcpToolCall/progress","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"tool","message":"Fetching documentation"}})));
    assert!(state.transcript().contains("Fetching documentation"));
    assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Inspect repository","status":"completed"},{"step":"Implement","status":"inProgress"}]}})));
    assert!(state.transcript().contains("[x] Inspect repository"));
    assert!(state.transcript().contains("[ ] Implement"));
}

#[test]
fn plan_updates_store_a_structured_checklist() {
    let mut state = state();
    assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","explanation":"Fix the bug","plan":[{"step":"Inspect","status":"completed"},{"step":"Implement","status":"in_progress"},{"step":"Test","status":"pending"}]}})));
    let item = state
        .saved
        .items
        .iter()
        .find(|item| item["id"] == "grove-plan-turn-1")
        .unwrap();
    assert_eq!(item["type"], "plan");
    assert_eq!(item["explanation"], "Fix the bug");
    assert_eq!(
        item["plan"],
        json!([
            {"id":"step-0","step":"Inspect","status":"completed"},
            {"id":"step-1","step":"Implement","status":"inProgress"},
            {"id":"step-2","step":"Test","status":"pending"}
        ])
    );
    assert!(item["text"].as_str().unwrap().contains("- [x] Inspect"));
    assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Inspect","status":"completed"},{"step":"Implement","status":"inProgress"}]}})));
    let item = state
        .saved
        .items
        .iter()
        .rfind(|item| crate::tasks::is_checklist(item))
        .unwrap();
    assert!(item["explanation"].is_null());
    assert_eq!(item["plan"][1]["status"], "inProgress");
    let list = crate::tasks::current(&state.saved.items).unwrap();
    assert_eq!(list.done(), 1);
    assert_eq!(list.now_label(), Some("Implement"));
}

#[test]
fn plan_updates_retain_task_ownership_within_one_turn() {
    let mut state = state();
    let update = |a, b| json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Inspect","status":a},{"step":"Verify","status":b}]}});
    state.apply(&update("inProgress", "pending"));
    state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"progress-first","delta":"Inspecting repository"}}));
    state.apply(&update("completed", "inProgress"));
    state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"progress-second","delta":"Running checks"}}));
    state.apply(&update("completed", "completed"));
    let items = &state.saved.items;
    assert_eq!(
        items
            .iter()
            .filter(|item| crate::tasks::is_checklist(item))
            .count(),
        3
    );
    let first = items
        .iter()
        .position(|item| item["id"] == "progress-first")
        .unwrap();
    let second = items
        .iter()
        .position(|item| item["id"] == "progress-second")
        .unwrap();
    assert_eq!(
        crate::tasks::activity_owner(items, first).unwrap().step,
        "Inspect"
    );
    assert_eq!(
        crate::tasks::activity_owner(items, second).unwrap().step,
        "Verify"
    );
    assert!(crate::tasks::current(items).unwrap().all_done());
}

#[test]
fn native_plan_tool_is_enabled_for_new_and_resumed_threads_with_roots() {
    for resume in [false, true] {
        for roots in [None, Some(vec![json!("/tmp/grove-worktree")])] {
            let (mut actor, mut reader) = idle_actor();
            if !resume {
                actor.state.saved.thread_id = None;
            }
            actor.start_thread(roots.clone()).unwrap();
            let request = read_line(&mut reader);
            assert_eq!(
                request["method"],
                if resume {
                    "thread/resume"
                } else {
                    "thread/start"
                }
            );
            assert_eq!(
                request["params"]["config"]["tools.update_plan.enabled"],
                true
            );
            match roots {
                Some(roots) => assert_eq!(
                    request["params"]["config"]["sandbox_workspace_write.writable_roots"],
                    json!(roots)
                ),
                None => assert!(request["params"]["config"]
                    .get("sandbox_workspace_write.writable_roots")
                    .is_none()),
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn native_plan_approval_stale_queue_never_reaches_provider() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
    actor.state.saved.draft = "independent composer".into();

    actor.deliver_plan("plan", "r1", true, |_| Ok(())).unwrap();
    let mut next = actor.state.saved.rich_plans.revisions[0].clone();
    next.document.revision_id = "r2".into();
    next.review = crate::rich_plan::ReviewState::default();
    actor.state.saved.rich_plans.revisions.push(next);
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.queue_ready = true;
    assert!(actor.dispatch_queue().unwrap_err().contains("outdated"));
    assert!(actor.pending.is_empty());
    assert!(actor.state.queued_prompts.is_empty());
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.approval,
        crate::rich_plan::ResponseDelivery::Failed { .. }
    ));
}

#[test]
#[cfg(unix)]
fn native_plan_actor_journals_before_dispatch_and_never_duplicates_feedback() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
    actor.state.saved.draft = "independent composer".into();

    assert!(actor
        .send_plan_response("plan", "r1", |_| Err("disk full".into()))
        .is_err());
    assert!(actor.state.queued_prompts.is_empty());
    assert!(actor.pending.is_empty());
    let journal = std::cell::RefCell::new(Vec::new());
    for _ in 0..2 {
        actor
            .send_plan_response("plan", "r1", |saved| {
                journal
                    .borrow_mut()
                    .push(serde_json::to_vec(saved).unwrap());
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(journal.borrow().len(), 1);
    assert_eq!(actor.state.queued_prompts.len(), 1);
    assert!(actor.pending.is_empty());
    let id = actor.state.queued_prompts[0].id;
    assert!(!actor.state.queue_editable(id));
    assert!(actor.state.begin_queue_edit(id).is_err());
    actor.state.remove_queued(id).unwrap();
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.delivery,
        crate::rich_plan::ResponseDelivery::Draft
    ));
    actor.send_plan_response("plan", "r1", |_| Ok(())).unwrap();
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.queue_ready = true;
    actor.dispatch_queue().unwrap();
    assert_eq!(actor.pending.len(), 1);
    actor
        .message(json!({"id":actor.seq,"result":{"turn":{"id":"feedback-turn"}}}))
        .unwrap();
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.delivery,
        crate::rich_plan::ResponseDelivery::Sent { .. }
    ));
    let seq = actor.seq;
    actor
        .send_plan_response("plan", "r1", |_| {
            panic!("confirmed feedback must not be journaled again")
        })
        .unwrap();
    assert_eq!(actor.seq, seq);
    assert_eq!(actor.state.saved.draft, "independent composer");
    let mut restarted = Snapshot::new(
        serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
    );
    assert!(restarted
        .saved
        .rich_plans
        .prepare_delivery("plan", "r1")
        .unwrap()
        .is_none());
}

#[test]
#[cfg(unix)]
fn native_plan_approval_journals_queues_and_confirms_without_touching_draft() {
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
    actor.state.saved.draft = "independent composer".into();

    assert!(actor
        .deliver_plan("plan", "r1", true, |_| Err("disk full".into()))
        .is_err());
    assert!(actor.state.queued_prompts.is_empty());
    assert!(actor.pending.is_empty());
    let journal = std::cell::RefCell::new(Vec::new());
    for _ in 0..2 {
        actor
            .deliver_plan("plan", "r1", true, |saved| {
                journal
                    .borrow_mut()
                    .push(serde_json::to_vec(saved).unwrap());
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(journal.borrow().len(), 1);
    assert_eq!(actor.state.queued_prompts.len(), 1);
    assert!(actor.pending.is_empty());
    let id = actor.state.queued_prompts[0].id;
    assert!(!actor.state.queue_editable(id));
    assert!(actor.state.begin_queue_edit(id).is_err());
    actor.state.remove_queued(id).unwrap();
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.approval,
        crate::rich_plan::ResponseDelivery::Draft
    ));
    actor.deliver_plan("plan", "r1", true, |_| Ok(())).unwrap();
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.queue_ready = true;
    actor.dispatch_queue().unwrap();
    assert_eq!(actor.pending.len(), 1);
    actor
        .message(json!({"id":actor.seq,"result":{"turn":{"id":"feedback-turn"}}}))
        .unwrap();
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.approval,
        crate::rich_plan::ResponseDelivery::Sent { .. }
    ));
    let seq = actor.seq;
    actor
        .deliver_plan("plan", "r1", true, |_| {
            panic!("confirmed feedback must not be journaled again")
        })
        .unwrap();
    assert_eq!(actor.seq, seq);
    assert_eq!(actor.state.saved.draft, "independent composer");
    let mut restarted = Snapshot::new(
        serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
    );
    assert!(restarted
        .saved
        .rich_plans
        .prepare_approval("plan", "r1")
        .unwrap()
        .is_none());
}
