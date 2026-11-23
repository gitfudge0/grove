use super::*;
use grove_core::codex::{Provider, QuestionRequest, SavedSession};
use serde_json::json;

fn snapshot(provider: Provider) -> Snapshot {
    Snapshot::new(SavedSession::empty(
        "session".into(),
        provider,
        "project".into(),
        "/tmp".into(),
    ))
}

#[test]
fn live_provider_requests_have_independent_identities() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut state = snapshot(provider);
        state.status = Status::Waiting;
        state.approvals = vec![json!({
            "id": 42,
            "method": "item/commandExecution/requestApproval",
            "params": {"command": "pwd"},
        })];
        let question = |id| {
            QuestionRequest::from_codex(
                json!(id),
                &json!({"questions": [{"id": "what", "question": "Which option?"}]}),
            )
            .unwrap()
        };
        state.questions = vec![question("a"), question("b")];
        let alerts = Alert::structured(&state);
        assert!(matches!(alerts.as_slice(), [
            Alert::Approval(..), Alert::Question(a), Alert::Question(b)
        ] if a != b));

        state.questions.remove(0);
        assert_eq!(
            Alert::structured(&state),
            vec![
                Alert::Approval(state.interaction_epoch, "42".into()),
                Alert::Question(state.questions[0].instance()),
            ]
        );

        state.reconnecting = true;
        assert!(Alert::structured(&state).is_empty());
    }
}

#[test]
fn retry_and_idle_are_quiet_but_blocked_activity_has_a_recovery_alert() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut state = snapshot(provider);
        state.status = Status::Working;
        assert!(Alert::structured(&state).is_empty());

        state.status = Status::Disconnected;
        state.error = Some("Connection lost".into());
        state.queue_uncertain.push(13);
        assert_eq!(Alert::structured(&state), vec![Alert::Disconnected]);

        state.status = Status::Idle;
        state.error = None;
        assert_eq!(
            Alert::structured(&state),
            vec![Alert::UncertainDelivery(13)]
        );
        state.queue_uncertain.clear();
        assert!(Alert::structured(&state).is_empty());

        state.status = Status::Failed;
        state.error = Some("Quota exhausted".into());
        state.notice = Some(grove_core::codex::SessionNotice {
            kind: SessionNoticeKind::UsageLimit,
            provider: "provider",
            body: "Quota exhausted".into(),
            reset_at: None,
            usage_url: None,
        });
        assert_eq!(Alert::structured(&state), vec![Alert::UsageLimit]);
    }
}

#[test]
fn visible_requests_stay_quiet_and_only_live_banners_can_open_a_session() {
    let first = SessionId::from_raw(1);
    let second = SessionId::from_raw(2);
    let mut pending = PendingAlerts::default();
    let mut shown = Vec::new();
    let mut closed = Vec::new();
    let mut record = |event| match event {
        Delivery::Show(id, title) => shown.push((id, title)),
        Delivery::Close(id) => closed.push(id),
    };

    pending.observe(first, vec![Alert::Question(1)], true, &mut record);
    pending.observe(first, vec![Alert::Question(1)], false, &mut record);
    pending.observe(
        first,
        vec![Alert::Question(1), Alert::Question(2)],
        false,
        &mut record,
    );
    pending.observe(
        second,
        vec![Alert::Approval(1, "1".into())],
        false,
        &mut record,
    );
    pending.observe(first, vec![Alert::Question(1)], false, &mut record);
    assert_eq!(
        pending.take_target(1),
        None,
        "resolved between delivery and click"
    );
    assert_eq!(pending.take_target(2), Some(second));
    assert_eq!(pending.take_target(2), None, "duplicate click");
    pending.observe(
        second,
        vec![Alert::Approval(1, "1".into())],
        false,
        &mut record,
    );
    pending.observe(
        second,
        vec![Alert::Approval(2, "1".into())],
        false,
        &mut record,
    );

    assert_eq!(
        shown,
        [
            (1, "Answer needed"),
            (2, "Approval needed"),
            (3, "Approval needed")
        ]
    );
    assert_eq!(closed, [1]);
}
