use super::*;

#[test]
fn native_plan_approval_stale_queue_never_reaches_provider() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
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
    assert!(actor.drain_queue());
    assert!(actor.state.error.as_deref().unwrap().contains("outdated"));
    assert!(actor.in_flight.is_none());
    assert!(actor.state.queue_sending.is_empty());
    assert!(actor.state.queued_prompts.is_empty());
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.approval,
        crate::rich_plan::ResponseDelivery::Failed { .. }
    ));
}

#[test]
fn native_plan_actor_journals_queues_and_confirms_one_review_without_touching_draft() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
    actor.state.saved.draft = "independent composer".into();

    assert!(actor
        .send_plan_response("plan", "r1", |_| Err("disk full".into()))
        .is_err());
    assert!(actor.state.queued_prompts.is_empty());
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
    assert!(!actor.drain_queue());
    let id = actor.state.queued_prompts[0].id;
    assert!(!actor.state.queue_editable(id));
    actor.state.status = Status::Idle;
    actor.queue_ready = true;
    assert!(actor.drain_queue());
    let wire = read(&mut reader);
    assert_eq!(wire["type"], "user");
    assert!(wire["message"]["content"]
        .to_string()
        .contains("Plan review: Native"));
    actor.acknowledge_input(&wire["uuid"]);
    assert!(actor.state.queued_prompts.is_empty());
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.delivery,
        crate::rich_plan::ResponseDelivery::Sent { .. }
    ));
    actor
        .send_plan_response("plan", "r1", |_| {
            panic!("confirmed feedback must not be journaled again")
        })
        .unwrap();
    assert!(actor.state.queued_prompts.is_empty());
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
fn native_plan_approval_journals_queues_and_confirms_without_touching_draft() {
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
    actor.state.saved.draft = "independent composer".into();

    assert!(actor
        .deliver_plan("plan", "r1", true, |_| Err("disk full".into()))
        .is_err());
    assert!(actor.state.queued_prompts.is_empty());
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
    assert!(!actor.drain_queue());
    let id = actor.state.queued_prompts[0].id;
    assert!(!actor.state.queue_editable(id));
    actor.state.status = Status::Idle;
    actor.queue_ready = true;
    assert!(actor.drain_queue());
    let wire = read(&mut reader);
    assert_eq!(wire["type"], "user");
    assert!(wire["message"]["content"]
        .to_string()
        .contains("Explicit plan approval: Native"));
    actor.acknowledge_input(&wire["uuid"]);
    assert!(actor.state.queued_prompts.is_empty());
    assert!(matches!(
        actor.state.saved.rich_plans.revisions[0].review.approval,
        crate::rich_plan::ResponseDelivery::Sent { .. }
    ));
    actor
        .deliver_plan("plan", "r1", true, |_| {
            panic!("confirmed feedback must not be journaled again")
        })
        .unwrap();
    assert!(actor.state.queued_prompts.is_empty());
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
