//! Native input/transport-channel evidence for both provider identities.
use super::*;
use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
use std::sync::mpsc;

fn exercise_behavior_reader(provider: Provider, cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let (handle, actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let mut saved: SavedSession = serde_json::from_str(r#"{
        "key":"behavior-plan-test", "provider":"codex", "project":"Test", "cwd":"/tmp",
        "label":"Test", "roots":[], "thread_id":"test-thread", "items":[],
        "draft":"KEEP THE CONVERSATION DRAFT",
        "rich_plans":{"revisions":[{"message_id":"plan-message", "source":"plan.grove-plan.json", "assets":[],
            "document":{"schema_version":1,"plan_id":"behavior","revision_id":"r1","title":"Pair Grove desktops",
                "presentation":{"outcome":"Control a host and retain your draft.","risks":["Full host access."],"success_criteria":["Verify both providers."]},
                "claims":[{"id":"pairing","text":"You can pair another desktop.",
                    "children":[{"id":"identity","text":"The host verifies the device.",
                        "decisions":[{"id":"access","prompt":"How should desktops connect?","default":"direct","options":[
                            {"id":"direct","label":"Direct connection","description":"Requires a reachable host."},
                            {"id":"relay","label":"Public relay","description":"Adds a relay service."}]}]}]},
                    {"id":"availability","text":"Host access can continue while available.",
                        "decisions":[{"id":"lifecycle","prompt":"When should access run?","default":"open","options":[
                            {"id":"open","label":"While Grove is open","description":"Closing Grove ends access."},
                            {"id":"background","label":"In the background","description":"Adds a service."}]}]}]
            }}]}
    }"#).unwrap();
    saved.provider = provider;
    let mut snapshot = Snapshot::new(saved);
    snapshot.status = Status::Idle;
    let mut plan = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|cx| {
            CodexSession::from_connection(
                snapshot,
                Connection {
                    handle,
                    updates: receive.into(),
                },
                cx,
            )
        });
        let html = cx.new(|_| {
            super::super::inline_html::HtmlSession::new("/tmp".into(), gpui::ScrollHandle::new())
        });
        let view = cx.new(|cx| NativePlan::new(session, html, "behavior".into(), "r1".into(), cx));
        plan = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let plan = plan.unwrap();
    cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(1400.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx
        .debug_bounds("native-plan-claim-content-pairing")
        .is_none());
    let pairing = cx.debug_bounds("native-plan-claim:pairing").unwrap();
    cx.simulate_click(pairing.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx
        .debug_bounds("native-plan-claim-content-pairing")
        .is_some());
    cx.update(|window, cx| {
        let focus = plan
            .read(cx)
            .projection
            .borrow()
            .focus_handle("claim:pairing")
            .unwrap();
        window.focus(&focus, cx);
    });
    for (key, open) in [("space", false), ("enter", true), ("space", false)] {
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        plan.read_with(cx, |view, _| {
            assert_eq!(view.open_claims.contains("pairing"), open)
        });
        assert_eq!(
            cx.debug_bounds("native-plan-claim-content-pairing")
                .is_some(),
            open
        );
    }
    cx.update(|window, cx| {
        plan.update(cx, |view, cx| {
            assert!(all_decisions(&view.snapshot(cx)["document"])
                .iter()
                .all(|d| !decision_answered(d, &view.snapshot(cx)["review"])));
            view.begin_response_review(window, cx);
            assert!(
                view.reviewed_response.is_none(),
                "suggestions never count as answers"
            );
            view.next_decision(window, cx);
            assert!(view.open_claims.contains("pairing"));
            assert!(view.open_claims.contains("identity"));
            assert!(!view.open_claims.contains("availability"));
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        plan.read_with(cx, |view, _| {
            assert!(
                view.decision_focus["access"].is_focused(window),
                "Next unanswered must focus the revealed decision"
            )
        });
    });
    assert!(cx.debug_bounds("native-plan-decision-access").is_some());
    for (width, rem) in [(400., 16.), (320., 24.), (1000., 16.)] {
        cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let claim = cx
            .debug_bounds("native-plan-claim-content-identity")
            .unwrap();
        let decision = cx.debug_bounds("native-plan-decision-access").unwrap();
        assert!(
            decision.origin.x >= claim.origin.x && decision.right() <= claim.right() + gpui::px(1.),
            "nested decision must fit at width {width}, rem {rem}: {decision:?} inside {claim:?}"
        );
    }
    // Expanded reader keeps its complete action footer outside body scrolling.
    cx.update(|window, cx| plan.update(cx, |view, cx| view.toggle_expansion(window, cx)));
    for (width, height, rem) in [(1000., 860., 16.), (640., 600., 20.), (320., 900., 24.)] {
        cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let card = cx.debug_bounds("native-plan-card").unwrap();
        let footer = cx.debug_bounds("native-plan-footer").unwrap();
        let body = cx.debug_bounds("native-plan-body").unwrap();
        let action = cx.debug_bounds("native-plan-update").unwrap();
        assert!(
            footer.bottom() <= card.bottom() + gpui::px(0.5),
            "footer exceeds reader at {width}/{height}/{rem}"
        );
        assert!(
            body.bottom() <= footer.top() + gpui::px(0.5),
            "body covers footer"
        );
        assert!(
            action.bottom() <= footer.bottom() + gpui::px(0.5),
            "action exceeds footer"
        );
    }
    cx.update(|window, _| window.set_rem_size(gpui::px(16.)));
    cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(1400.)));
    cx.update(|window, cx| {
        plan.update(cx, |view, cx| {
            view.choose("access", "direct", cx);
            view.choose("lifecycle", "open", cx);
            view.begin_response_review(window, cx);
            assert!(view.reviewed_response.is_some());
            assert_eq!(
                view.session.read(cx).state.saved.draft,
                "KEEP THE CONVERSATION DRAFT"
            );
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("native-plan-response-review").is_some());
    plan.read_with(cx, |view, _| {
        assert!(
            view.projection
                .borrow()
                .registered_actions()
                .iter()
                .all(|action| !matches!(
                    action,
                    measured::Action::Choose(..)
                        | measured::Action::ToggleClaim(_)
                        | measured::Action::ToggleSection(_)
                )),
            "Response review must not register discarded plan edit controls"
        );
    });
    assert!(
        !actions.try_iter().any(|a| matches!(
            a,
            Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
        )),
        "review must never dispatch"
    );
    cx.update(|window, cx| {
        plan.update(cx, |view, cx| {
            view.activate(&measured::Action::EditResponse, window, cx);
            assert!(view.reviewed_response.is_none());
            view.activate(&measured::Action::SendResponse, window, cx);
            view.begin_response_review(window, cx);
            view.choose("access", "relay", cx);
            assert!(
                view.reviewed_response.is_none(),
                "edited answers invalidate the reviewed snapshot"
            );
            view.activate(&measured::Action::SendResponse, window, cx);
            view.begin_response_review(window, cx);
            // Incoming actor-side answer changes cannot reuse the reviewed snapshot.
            view.local_review
                .as_mut()
                .unwrap()
                .answers
                .insert("access".into(), "direct".into());
            view.activate(&measured::Action::SendResponse, window, cx);
        })
    });
    assert!(
        !actions.try_iter().any(|a| matches!(
            a,
            Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
        )),
        "edit and answer changes must reject stale sends"
    );
    cx.update(|window, cx| {
        plan.update(cx, |view, cx| {
            view.begin_response_review(window, cx);
            view.session.update(cx, |session, _| {
                let mut newer = session.state.saved.rich_plans.revisions[0].clone();
                newer.document.revision_id = "r2".into();
                session.state.saved.rich_plans.revisions.push(newer);
            });
            view.activate(&measured::Action::SendResponse, window, cx);
            assert!(!view.send_pending, "a superseded revision cannot send");
            view.session.update(cx, |session, _| {
                session.state.saved.rich_plans.revisions.pop();
            });
        })
    });
    assert!(
        !actions.try_iter().any(|a| matches!(
            a,
            Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
        )),
        "a stale revision cannot dispatch"
    );
    cx.update(|window, cx| {
        plan.update(cx, |view, cx| {
            view.begin_response_review(window, cx);
            view.activate(&measured::Action::SendResponse, window, cx);
            view.activate(&measured::Action::SendResponse, window, cx);
        })
    });
    let sent: Vec<_> = actions
        .try_iter()
        .filter(|a| {
            matches!(
                a,
                Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
            )
        })
        .collect();
    assert_eq!(sent.len(), 1, "explicit send dispatches once");
    assert!(
        matches!(&sent[0], Action::SendPlanResponse { plan_id, revision_id } if plan_id == "behavior" && revision_id == "r1")
    );
}

#[gpui::test]
fn codex_behavior_reader_disclosure_and_review_boundary(cx: &mut gpui::TestAppContext) {
    exercise_behavior_reader(Provider::Codex, cx);
}

#[gpui::test]
fn claude_behavior_reader_disclosure_and_review_boundary(cx: &mut gpui::TestAppContext) {
    exercise_behavior_reader(Provider::Claude, cx);
}
