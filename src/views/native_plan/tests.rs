use super::*;
use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
use std::{cell::RefCell, rc::Rc, sync::mpsc};

fn exercise_native_overview(provider: Provider, cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let (handle, _actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let mut saved = long_decision_fixture(provider);
    let document = &mut saved.rich_plans.revisions[0].document;
    document.markdown = Some("## Implementation\nHidden but copyable technical detail.\n## Verification\nRun the provider checks.".into());
    document.presentation = Some(serde_json::from_value(json!({"outcome":"Keep the draft intact.","flow":[{"title":"Choose","detail":"Select the provider"},{"title":"Return","detail":"Verify the account"}],"included":["Sign in"],"deferred":["API keys"],"risks":["Verify provider response"],"success_criteria":["Cancel preserves drafts"],"revision_changes":[]})).unwrap());
    let mut plan = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|cx| {
            CodexSession::from_connection(
                Snapshot::new(saved),
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
        let view = cx.new(|cx| NativePlan::new(session, html, "plan".into(), "r1".into(), cx));
        plan = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let plan = plan.unwrap();
    cx.simulate_resize(gpui::size(gpui::px(600.), gpui::px(1600.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("native-plan-overview").is_some());
    assert!(
        cx.debug_bounds("native-plan-decision-access").is_some(),
        "Decisions cannot be hidden by folded details"
    );
    assert!(cx.debug_bounds("native-plan-detail-content-0").is_none());
    let header = cx.debug_bounds("native-plan-detail-0").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    plan.read_with(cx, |view, cx| {
        assert!(view.open_sections.contains(&0));
        assert!(view
            .markdown(cx)
            .contains("Hidden but copyable technical detail."));
        assert!(view.markdown(cx).contains("Keep the draft intact."));
    });
    assert!(cx.debug_bounds("native-plan-detail-content-0").is_some());
    cx.update(|window, cx| {
        let focus = plan
            .read(cx)
            .projection
            .borrow()
            .focus_handle("section:0")
            .unwrap();
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    plan.read_with(cx, |view, _| assert!(!view.open_sections.contains(&0)));
    assert!(cx.debug_bounds("native-plan-detail-content-0").is_none());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("native-plan-detail-content-0").is_some());
    for (width, rem) in [(400., 16.), (1000., 16.), (320., 32.)] {
        cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1600.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let body = cx.debug_bounds("native-plan-overview").unwrap();
        let flow = cx.debug_bounds("native-plan-flow").unwrap();
        assert!(flow.size.width <= body.size.width);
        for selector in ["native-plan-flow-step-0", "native-plan-flow-step-1"] {
            let step = cx.debug_bounds(selector).unwrap();
            assert!(
                step.origin.x >= flow.origin.x && step.right() <= flow.right(),
                "Flow child overflows at width {width}, rem {rem}: {step:?}, parent {flow:?}"
            );
        }
    }
}
#[gpui::test]
fn codex_native_overview_preserves_decisions_copy_and_disclosure(cx: &mut gpui::TestAppContext) {
    exercise_native_overview(Provider::Codex, cx);
}
#[gpui::test]
fn claude_native_overview_preserves_decisions_copy_and_disclosure(cx: &mut gpui::TestAppContext) {
    exercise_native_overview(Provider::Claude, cx);
}

#[test]
fn markdown_sections_preserve_fences_and_associate_headings() {
    let source = "# Outcome\n\nSummary\n\n## Delivery\nUse a flow.\n```text\n## Not a section\n```\n\n## Checks\nVerify.\n";
    let sections = markdown_sections(source);
    assert_eq!(
        sections
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        ["Overview", "Delivery", "Checks"]
    );
    assert_eq!(
        sections
            .iter()
            .map(|(_, body)| body.as_str())
            .collect::<String>(),
        source
    );
    assert!(sections[1].1.contains("## Not a section"));
}

#[test]
fn unanswered_decisions_include_markdown_and_legacy_without_accepting_recommendations() {
    let document = json!({"decisions":[{"id":"one","default":"yes","options":[{"id":"yes","label":"Yes"}]}],"claims":[{"children":[{"decisions":[{"id":"two","options":[]}]}]}]});
    let decisions = all_decisions(&document);
    assert_eq!(decisions.len(), 2);
    assert!(!decision_answered(&decisions[0], &json!({})));
    assert!(!decision_answered(
        &decisions[0],
        &json!({"answers":{"one":"unknown"}})
    ));
    let review = json!({"answers":{"one":"yes"},"own_answers":{"two":"My custom answer"}});
    assert!(decisions.iter().all(|d| decision_answered(d, &review)));
    assert_eq!(resolved_answer(&decisions[0], &review), "Yes");
    assert_eq!(resolved_answer(&decisions[1], &review), "My custom answer");
}

#[test]
fn repository_authoring_example_retains_legacy_assets() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut store = grove_core::rich_plan::PlanStore::default();
    store
        .import(
            "example",
            "tests/fixtures/native-plans/example.grove-plan.json",
            root.to_str().unwrap(),
            std::slice::from_ref(&root),
        )
        .unwrap();
    let revision = &store.revisions[0];
    assert_eq!(revision.document.plan_id, "native-review-example");
    assert!(revision.assets.iter().any(|a| a.mime == "image/png"));
    assert!(revision.assets.iter().any(|a| a.mime == "text/html"));
}

fn long_decision_fixture(provider: Provider) -> SavedSession {
    serde_json::from_value(json!({
        "key":"native-plan-test","provider":provider,"project":"Test","cwd":"/tmp","label":"Test","roots":[],"thread_id":"test-thread","items":[],"draft":"KEEP THIS DRAFT",
        "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],"document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Invitations","markdown":"# Invitations\n\n## Access\nChoose who can invite.","decisions":[{"id":"access","section":"Access","prompt":"QA wrote “When a case date has no tasks... the Tasks and Surveys tab should show No tasks”. Which empty states become the single line?","default":"admin","options":[{"id":"admin","label":"Admins","description":"Limit invitations to admins."}]}]}}]}
    })).unwrap()
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires macOS Metal renderer; captures native plan layout"]
fn long_decision_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".grove-artifacts/long-plan-decision");
    std::fs::create_dir_all(&output).unwrap();
    let previous_theme = grove_core::theme::current().name.clone();
    for appearance in ["dark", "light"] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&format!(
                "grove-{appearance}"
            )));
            c::sync_component_theme(cx);
        });
        for (width, rem) in [(1000., 16.), (400., 16.), (400., 24.)] {
            for provider in [Provider::Codex, Provider::Claude] {
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(1000.)),
                        |window, cx| {
                            window.set_rem_size(gpui::px(rem));
                            let session = cx.new(|cx| {
                                CodexSession::from_connection(
                                    Snapshot::new(long_decision_fixture(provider)),
                                    Connection {
                                        handle,
                                        updates: receive.into(),
                                    },
                                    cx,
                                )
                            });
                            let html = cx.new(|_| {
                                super::super::inline_html::HtmlSession::new(
                                    "/tmp".into(),
                                    gpui::ScrollHandle::new(),
                                )
                            });
                            let plan = cx.new(|cx| {
                                NativePlan::new(session, html, "plan".into(), "r1".into(), cx)
                            });
                            cx.new(|cx| {
                                gpui_component::Root::new(plan, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                for _ in 0..3 {
                    cx.advance_clock(std::time::Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                let path =
                    output.join(format!("{appearance}-{provider:?}-{width:.0}-{rem:.0}.png"));
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native long decision fixture: {}", path.display());
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
    cx.update(|cx| {
        assert!(grove_core::theme::set_by_name(&previous_theme));
        c::sync_component_theme(cx);
    });
}

fn exercise_decision_composer(provider: Provider, cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let (handle, actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let saved = long_decision_fixture(provider);
    let mut snapshot = Snapshot::new(saved);
    snapshot.status = Status::Idle;
    let mut plan = None;
    let events: Rc<RefCell<Vec<NativePlanEvent>>> = Rc::default();
    let events_out = events.clone();
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
        let view = cx.new(|cx| NativePlan::new(session, html, "plan".into(), "r1".into(), cx));
        cx.subscribe(&view, move |_, _, event: &NativePlanEvent, _| {
            events_out.borrow_mut().push(event.clone());
        })
        .detach();
        plan = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let plan = plan.unwrap();
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(1000.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    // Warm rendering reuses the exact snapshot and prepared document. A
    // local answer invalidates review presentation without reparsing sections.
    cx.update(|_, cx| {
        plan.update(cx, |view, cx| {
            let first = view.snapshot(cx);
            let sections = view.prepared.borrow().sections.clone();
            assert!(Rc::ptr_eq(&first, &view.snapshot(cx)));
            view.local_review = Some(grove_core::rich_plan::ReviewState {
                answers: HashMap::from([("access".into(), "admin".into())]),
                ..Default::default()
            });
            let answered = view.snapshot(cx);
            assert!(!Rc::ptr_eq(&first, &answered));
            assert_eq!(answered["review"]["answers"]["access"], "admin");
            assert!(Rc::ptr_eq(&sections, &view.prepared.borrow().sections));
            view.local_review = None;
            assert!(view.snapshot(cx)["review"]["answers"]
                .as_object()
                .unwrap()
                .is_empty());
            let original = view.session.read(cx).state.saved.rich_plans.revisions[0]
                .document
                .markdown
                .clone();
            view.session.update(cx, |session, _| {
                session.state.saved.rich_plans.revisions[0]
                    .document
                    .markdown = Some("## Replacement\nFresh document".into());
            });
            // Same plan/revision IDs and no observer notification: mutable
            // session access must still invalidate the identity fast path.
            let replaced = view.snapshot(cx);
            assert_eq!(
                replaced["document"]["markdown"],
                "## Replacement\nFresh document"
            );
            assert!(!Rc::ptr_eq(&sections, &view.prepared.borrow().sections));
            assert!(Rc::ptr_eq(&replaced, &view.snapshot(cx)));
            view.session.update(cx, |session, _| {
                session.state.saved.rich_plans.revisions[0]
                    .document
                    .markdown = original;
            });
            assert!(!Rc::ptr_eq(&replaced, &view.snapshot(cx)));
        });
    });
    for width in [1000., 400.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let prompt = cx.debug_bounds("decision-prompt-access-line-0").unwrap();
        let decision = cx.debug_bounds("native-plan-decision-access").unwrap();
        assert!(
            prompt.size.width > decision.size.width * 0.9,
            "prompt must retain the decision width: {prompt:?} {decision:?}"
        );
        let discuss = cx
            .debug_bounds("native-plan-decision-discuss-access")
            .unwrap();
        assert!(
            discuss.origin.y >= prompt.bottom(),
            "discussion action must follow prompt"
        );
        assert!(
            discuss.right() <= decision.right() + gpui::px(1.),
            "discussion action must fit its decision: {discuss:?} {decision:?}"
        );
        assert!(
            prompt.size.height < gpui::px(300.),
            "prompt must wrap as prose rather than a character column: {prompt:?}"
        );
    }
    assert!(cx.debug_bounds("native-plan-update").is_some());
    assert!(cx.debug_bounds("native-plan-approve").is_none());
    assert!(cx
        .debug_bounds("native-plan-input-well-own_answers:access")
        .is_none());
    let notifications = Rc::new(std::cell::Cell::new(0));
    let observed = notifications.clone();
    let subscription = cx.update(|_, cx| {
        cx.observe(&plan, move |_, cx| {
            let _ = cx;
            observed.set(observed.get() + 1);
        })
    });
    let session = cx.update(|_, cx| plan.read(cx).session.clone());
    cx.update(|_, cx| {
        session.update(cx, |session, cx| {
            session.state.model = Some("unrelated-model-update".into());
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        0,
        "unrelated updates must not invalidate plans"
    );
    cx.update(|_, cx| {
        session.update(cx, |session, cx| {
            session.state.saved.rich_plans.revisions[0]
                .review
                .answer_drafts
                .insert("access".into(), "Draft from actor".into());
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(
        notifications.get() > 0,
        "review updates must invalidate the plan"
    );
    drop(subscription);
    let button = cx.debug_bounds("native-plan-custom-access").unwrap();
    cx.simulate_click(button.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        matches!(&events.borrow()[0], NativePlanEvent::Compose { target: PlanComposerTarget::CustomAnswer { decision_id, .. }, .. } if decision_id == "access")
    );
    cx.update(|_, cx| {
        plan.update(cx, |view, cx| {
            view.save_custom_draft("access", "Team leads".into(), cx)
                .unwrap();
            assert_eq!(view.custom_answer_draft("access", cx), "Team leads");
            assert!(!decision_answered(
                &all_decisions(&view.snapshot(cx)["document"])[0],
                &view.snapshot(cx)["review"]
            ));
            view.save_custom_answer("access", "Team leads".into(), cx)
                .unwrap();
            assert!(decision_answered(
                &all_decisions(&view.snapshot(cx)["document"])[0],
                &view.snapshot(cx)["review"]
            ));
            assert_eq!(view.session.read(cx).state.saved.draft, "KEEP THIS DRAFT");
            view.set_composer_pending(true, cx);
            view.dispatch(false, cx);
        });
    });
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::SendPlanResponse { .. } | Action::ApprovePlan { .. }
    )));
}

#[gpui::test]
fn codex_plan_custom_answer_uses_composer_event(cx: &mut gpui::TestAppContext) {
    exercise_decision_composer(Provider::Codex, cx);
}
#[gpui::test]
fn claude_plan_custom_answer_uses_composer_event(cx: &mut gpui::TestAppContext) {
    exercise_decision_composer(Provider::Claude, cx);
}
