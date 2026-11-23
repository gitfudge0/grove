use super::*;

#[gpui::test]
fn initialized_claude_accepts_first_prompt_before_session_id(cx: &mut gpui::TestAppContext) {
    let (handle, actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let mut state = snapshot();
    state.saved.provider = codex::Provider::Claude;
    state.saved.thread_id = None;
    state.status = Status::Idle;
    let session = cx.new(|cx| {
        CodexSession::from_connection(
            state,
            Connection {
                handle,
                updates: receive.into(),
            },
            cx,
        )
    });
    let result = session.update(cx, |session, cx| {
        session.prompt_with_images("First Claude prompt".into(), Vec::new(), cx)
    });
    assert!(result.is_ok());
    assert!(
        matches!(actions.try_recv().unwrap(), Action::Submit { text, .. } if text == "First Claude prompt")
    );
}

#[gpui::test]
fn dashboard_pending_handoff_locks_mounted_draft_and_releases_on_failure(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _updates) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let reference = codex::ReferenceDraft {
            id: "launch-reference".into(),
            thread_id: None,
            sources: Vec::new(),
            text: "Original reference".into(),
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Original launched prompt", window, cx);
                });
                view.images = vec!["original-image".into()];
                view.reference = Some(reference.clone());
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Starting;
                    session.state.saved.provider = provider;
                    session.state.saved.thread_id = None;
                    session.state.saved.draft = "Original launched prompt".into();
                    session.state.saved.draft_images = view.images.clone();
                    session.state.saved.draft_reference = Some(reference.clone());
                });
                view.launch_dashboard(window, cx);
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("x");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.remove_image(0, window, cx);
                view.remove_reference(window, cx);
                view.attach_paths(vec![std::path::PathBuf::from("/unused")], window, cx);
                view.paste_images(Vec::new(), window, cx);
                view.submit(window, cx);
                assert!(view.dashboard_pending);
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Original launched prompt"
                );
                assert_eq!(view.images, ["original-image"]);
                assert_eq!(view.reference.as_ref(), Some(&reference));
                assert!(!view.pending_attachment_picker);
                assert_eq!(view.pending_image_pastes, 0);
                view.session
                    .update(cx, |session, _| session.state.status = Status::Failed);
                view.flush_dashboard_submission(window, cx);
                assert!(!view.dashboard_pending);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("x");
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.input.read(cx).value().contains('x'),
                    "terminal failure releases editing"
                );
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Idle;
                    session.state.saved.thread_id = Some("late-ready".into());
                    cx.notify();
                });
            });
        });
        cx.run_until_parked();
        assert!(
            !actions
                .try_iter()
                .any(|action| matches!(action, Action::Submit { .. })),
            "late readiness must not automatically submit the recovered draft"
        );
    }
}

#[gpui::test]
fn dashboard_pending_submit_dispatches_once_when_unmounted_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let mut state = snapshot();
        state.saved.provider = provider;
        state.saved.thread_id = None;
        state.status = Status::Starting;
        state.saved.draft = "Launch from the unmounted Dashboard".into();
        let image="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jM1sAAAAASUVORK5CYII=".to_string();
        state.saved.draft_images = vec![image.clone()];
        let reference = codex::ReferenceDraft {
            id: "quoted-source".into(),
            thread_id: Some("source".into()),
            sources: Vec::new(),
            text: "Keep the quoted context".into(),
        };
        state.saved.draft_reference = Some(reference.clone());
        let mut hidden = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    state,
                    Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            hidden = Some(cx.new(|cx| CodexView::new(session, window, cx)));
            let host = cx.new(|_| UnmountedDashboardHost);
            gpui_component::Root::new(host, window, cx).bordered(false)
        });
        let hidden = hidden.unwrap();
        cx.update(|window, cx| hidden.update(cx, |view, cx| view.launch_dashboard(window, cx)));
        assert!(
            !actions
                .try_iter()
                .any(|action| matches!(action, Action::Submit { .. })),
            "not ready must not send"
        );
        cx.update(|_, cx| {
            let session = hidden.read(cx).session.clone();
            session.update(cx, |session, cx| {
                session.state.status = Status::Idle;
                if provider == codex::Provider::Codex {
                    session.state.saved.thread_id = Some("ready-thread".into());
                }
                cx.notify();
            });
        });
        cx.run_until_parked();
        let dispatched = actions.try_iter().collect::<Vec<_>>();
        let submits = dispatched
            .iter()
            .filter_map(|action| match action {
                Action::Submit { text, images, .. } => Some((text, images)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            submits.len(),
            1,
            "{provider:?} unmounted readiness must send exactly once"
        );
        assert!(submits[0].0.contains("Launch from the unmounted Dashboard"));
        assert!(submits[0].0.contains("Keep the quoted context"));
        assert_eq!(submits[0].1, &vec![image]);
        hidden.read_with(cx, |view, cx| {
            assert!(view.reference.as_ref() == Some(&reference));
            assert_eq!(view.images.len(), 1);
            assert!(view
                .input
                .read(cx)
                .value()
                .contains("Launch from the unmounted Dashboard"));
            assert!(!view.dashboard_pending);
        });
        for _ in 0..3 {
            cx.update(|_, cx| {
                let session = hidden.read(cx).session.clone();
                session.update(cx, |_, cx| cx.notify());
            });
            cx.run_until_parked();
        }
        assert!(
            !actions
                .try_iter()
                .any(|action| matches!(action, Action::Submit { .. })),
            "later readiness notifications cannot duplicate the submit"
        );
    }
}

#[gpui::test]
fn describe_command_submit_intercepts_dashboard_before_catalog_or_provider_startup(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            observed.borrow_mut().push(match event {
                CodexViewEvent::DescribeChanges(..) => "description",
                CodexViewEvent::DashboardSubmit { .. } => "provider startup",
                _ => "other",
            });
        })
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for dashboard in [false, true] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, _| {
                        session.state.saved.provider = provider;
                        session.catalog_loading = true;
                    });
                    view.dashboard_tray = dashboard.then(|| {
                        cx.new(|_| {
                            crate::views::sidebar::dashboard::DestinationTray::picker_test_tray(
                                "/missing-native-command-fixture".into(),
                            )
                        })
                    });
                });
            });
            for command in [
                "/describe-changes local",
                "$describe-changes pr",
                "/describe-changes https://github.com/gitfudge0/grove/pull/17",
            ] {
                events.borrow_mut().clear();
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.input
                            .update(cx, |input, cx| input.set_value(command, window, cx));
                        view.submit(window, cx);
                        assert_eq!(view.input.read(cx).value().as_ref(), command);
                        assert!(view.submitted.is_none());
                        assert!(view.session.read(cx).state.error.is_none());
                    });
                });
                assert_eq!(
                    *events.borrow(),
                    ["description"],
                    "{provider:?}, Dashboard={dashboard}, {command}"
                );
            }
            for command in [
                "/describe-changes local changes",
                "/describe-changes https://example.com/pull/17",
            ] {
                events.borrow_mut().clear();
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.input
                            .update(cx, |input, cx| input.set_value(command, window, cx));
                        view.submit(window, cx);
                        assert_eq!(
                            view.session.read(cx).state.error.as_deref(),
                            Some(grove_core::describe_skill::USAGE)
                        );
                        assert_eq!(view.input.read(cx).value().as_ref(), command);
                    });
                });
                assert!(events.borrow().is_empty());
            }
        }
    }
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::Submit { .. }
            | Action::SubmitWithMode { .. }
            | Action::Prompt(_)
            | Action::PromptWithImages { .. }
    )));
}

#[gpui::test]
fn describe_skill_routes_both_providers_without_submitting_or_losing_drafts(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let CodexViewEvent::DescribeChanges(scope, _) = event {
                observed.borrow_mut().push(scope.clone());
            }
        })
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.skills =
                        vec![json!({"name":"describe-changes","enabled":true})].into();
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("/describe-changes local", window, cx);
                });
                assert!(view.run_command(window, cx));
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "/describe-changes local"
                );
                view.pending_image_pastes = 1;
                view.input.update(cx, |input, cx| {
                    input.set_value("$describe-changes pr", window, cx);
                });
                assert!(view.run_command(window, cx));
                assert_eq!(view.pending_image_pastes, 1);
                assert_eq!(view.input.read(cx).value().as_ref(), "$describe-changes pr");
                assert!(view
                    .session
                    .read(cx)
                    .state
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("draft is preserved")));
                view.pending_image_pastes = 0;
                assert!(view.run_command(window, cx));
                view.input.update(cx, |input, cx| {
                    input.set_value("/describe-changes invalid", window, cx);
                });
                assert!(view.run_command(window, cx));
                assert_eq!(
                    view.session.read(cx).state.error.as_deref(),
                    Some(grove_core::describe_skill::USAGE)
                );
                assert_eq!(
                    slash_choices(&view.session.read(cx).state, "describe")
                        .iter()
                        .filter(|(name, _)| name == "/describe-changes")
                        .count(),
                    1
                );
                view.input.update(cx, |input, cx| {
                    input.set_value("$describe", window, cx);
                    input.set_selected_range(9..9, cx);
                });
                let choices = view
                    .suggestion_choices(&view.session.read(cx).state, cx)
                    .unwrap()
                    .0;
                assert_eq!(
                    choices
                        .iter()
                        .filter(|(name, _)| name == "$describe-changes")
                        .count(),
                    1
                );
            });
        });
    }
    assert_eq!(
        *events.borrow(),
        vec![
            grove_core::describe_skill::ScopePreference::Local,
            grove_core::describe_skill::ScopePreference::PullRequest,
            grove_core::describe_skill::ScopePreference::Local,
            grove_core::describe_skill::ScopePreference::PullRequest
        ]
    );
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. })));
}

#[gpui::test]
fn provider_connection_centers_then_reveals_launch_and_failure_recovery(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("Preserved request", window, cx);
            });
            view.session.update(cx, |session, cx| {
                session.state.saved.thread_id = None;
                session.state.status = Status::Starting;
                cx.notify();
            });
        });
    });
    draw(cx);
    let connection = cx.debug_bounds("codex-provider-connection").unwrap();
    assert!((f32::from(connection.center().x) - 500.0).abs() < 1.0);
    assert!((f32::from(connection.center().y) - 400.0).abs() < 1.0);
    for selector in [
        "codex-composer",
        "codex-empty",
        "codex-launch-starters",
        "codex-live-activity",
    ] {
        assert!(cx.debug_bounds(selector).is_none(), "{selector}");
    }
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.state.status = Status::Idle;
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-provider-connection").is_none());
    assert!(cx.debug_bounds("codex-composer").is_some());
    assert!(cx.debug_bounds("codex-launch-starters").is_none());
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.state.status = Status::Disconnected;
            session.state.error = Some("Provider unavailable".into());
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-provider-connection").is_none());
    assert!(cx.debug_bounds("codex-composer").is_some());
    assert!(cx.debug_bounds("codex-error").is_some());
    let reconnect = cx.debug_bounds("codex-reconnect").unwrap();
    cx.simulate_click(reconnect.center(), gpui::Modifiers::default());
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Reconnect)));
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Preserved request");
    });
    view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Starting;
                session.state.error = None;
                session.state.saved.items.push(json!({"type":"userMessage", "id":"existing", "content":[{"type":"text", "text":"Earlier request"}]}));
                cx.notify();
            });
        });
    draw(cx);
    assert!(cx.debug_bounds("codex-provider-connection").is_none());
    assert!(cx.debug_bounds("codex-composer").is_some());
}

#[gpui::test]
fn claude_starting_shows_composer_and_queues_images_without_hiding_history(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("While connecting", window, cx);
            });
            view.images = vec!["data:image/png;base64,aGVsbG8=".into()];
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = codex::Provider::Claude;
                session.state.saved.thread_id = None;
                session.state.status = Status::Starting;
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-provider-connection").is_none());
    assert!(cx.debug_bounds("codex-composer").is_some());
    assert!(cx.debug_bounds("codex-empty").is_none());
    view.read_with(cx, |view, cx| {
        assert!(!view.draft_action_pending(cx));
        assert_eq!(
            live_activity(&view.session.read(cx).state).as_deref(),
            Some("Connecting to Claude Code…")
        );
    });
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    assert!(actions.try_iter().any(|action| matches!(action, Action::SubmitWithMode { text, images, mode: SubmissionMode::Queue, .. } if text == "While connecting" && images == vec!["data:image/png;base64,aGVsbG8="])));
    view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
            session.state.saved.items.push(json!({"type":"userMessage", "id":"earlier", "content":[{"type":"text","text":"Earlier request"}]}));
            cx.notify();
        }));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-body-0").is_some());
    assert!(cx.debug_bounds("codex-composer").is_some());
}

#[gpui::test]
fn dashboard_enter_routes_launch_intent_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = requests.clone();
    let _subscription = cx.update(|_, cx| {
        cx.bind_keys(crate::keymap::shell_bindings());
        cx.subscribe(&view, move |_, event, _| {
            if let CodexViewEvent::DashboardSubmit { open_transcript } = event {
                observed.borrow_mut().push(*open_transcript);
            }
        })
    });
    let shell_enter = format!("{}enter", crate::keymap::platform_mod_prefix());
    let composer_enter = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.catalog_loading = false;
                });
                view.dashboard_tray = Some(cx.new(|_| {
                    crate::views::sidebar::dashboard::DestinationTray::picker_test_tray(
                        "/dashboard-shortcut-fixture".into(),
                    )
                }));
                view.input.update(cx, |input, cx| {
                    input.set_value("Start a session", window, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-enter");
        assert!(requests.borrow().is_empty());
        cx.simulate_keystrokes("enter");
        cx.simulate_keystrokes(composer_enter);
        cx.simulate_keystrokes(&shell_enter);
        assert_eq!(
            *requests.borrow(),
            [false, true, cfg!(target_os = "macos")],
            "{provider:?}"
        );
        requests.borrow_mut().clear();
    }
}
