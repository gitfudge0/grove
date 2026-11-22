use super::*;

#[test]
fn session_age_uses_activity_scale() {
    assert_eq!(elapsed_short(Duration::from_secs(59)), "59s");
    assert_eq!(elapsed_short(Duration::from_mins(1)), "1m");
    assert_eq!(elapsed_short(Duration::from_hours(1)), "1h");
    assert_eq!(elapsed_short(Duration::from_hours(24)), "1d");
    let started = Instant::now();
    assert_eq!(
        session_age_label(started, started + Duration::from_secs(59)),
        "for 59s"
    );
    assert_eq!(
        session_age_label(started, started + Duration::from_mins(1)),
        "for 1m"
    );
}

#[test]
fn diff_status_distinguishes_unknown_clean_and_changed() {
    use grove_core::git::WorktreeGitState;
    assert_eq!(
        session_diff_status(&WorktreeGitState::default()),
        ("clean".into(), false)
    );
    assert_eq!(
        session_diff_status(&WorktreeGitState {
            dirty: true,
            ..Default::default()
        }),
        ("Changes".into(), true)
    );
    assert_eq!(
        session_diff_status(&WorktreeGitState {
            dirty: true,
            added: 3,
            removed: 2,
            ..Default::default()
        }),
        ("+3 -2".into(), true)
    );
}

#[test]
fn structured_sidebar_status_prioritizes_requests_and_preserves_stop_reasons() {
    use grove_core::codex::{Provider, SessionNotice, SessionNoticeKind, Snapshot, Status};
    for provider in [Provider::Codex, Provider::Claude] {
        let saved = serde_json::from_value(serde_json::json!({
            "key":"status-fixture", "project":"grove", "cwd":"/tmp/grove", "label":"agent",
            "title":null, "roots":[], "thread_id":null, "items":[]
        }))
        .unwrap();
        let mut state = Snapshot::new(saved);
        state.saved.provider = provider;
        state.status = Status::Working;
        state
            .approvals
            .push(serde_json::json!({"id":"child-approval", "groveAgent":"child"}));
        assert_eq!(
            structured_session_status(&state, ActivityState::Working),
            "Approval needed"
        );
        state.elicitations.push(grove_core::elicitation::ElicitationRequest::parse_codex(
                serde_json::json!("child-question"),
                &serde_json::json!({"message":"Continue?", "requestedSchema":{"type":"object", "properties":{"answer":{"type":"boolean"}}}}),
            ).unwrap());
        assert_eq!(
            structured_session_status(&state, ActivityState::Working),
            "Approval needed"
        );
        state.approvals.clear();
        assert_eq!(
            structured_session_status(&state, ActivityState::Working),
            "Answer needed"
        );
        state.elicitations.clear();
        for (status, expected) in [
            (Status::Waiting, "Needs you"),
            (Status::Disconnected, "Disconnected"),
            (Status::Interrupted, "Interrupted"),
            (Status::Failed, "Failed"),
        ] {
            state.status = status;
            assert_eq!(
                structured_session_status(&state, ActivityState::Working),
                expected
            );
        }
        state.status = Status::Failed;
        state.error = Some("Provider usage limit".into());
        state.notice = Some(SessionNotice {
            kind: SessionNoticeKind::UsageLimit,
            provider: "fixture",
            body: "Provider usage limit".into(),
            reset_at: None,
            usage_url: None,
        });
        assert_eq!(
            structured_session_status(&state, ActivityState::Exited),
            "Usage limit"
        );
        state.error = Some("Different failure".into());
        assert_eq!(
            structured_session_status(&state, ActivityState::Exited),
            "Failed"
        );
        state.status = Status::Working;
        assert_eq!(
            structured_session_status(&state, ActivityState::Working),
            "Working"
        );
        state.status = Status::Done;
        assert_eq!(
            structured_session_status(&state, ActivityState::Done),
            "Ready"
        );
        assert_eq!(
            structured_session_status(&state, ActivityState::Idle),
            "Ready"
        );
    }
}

#[test]
fn sessions_list_orders_mixed_attention_and_activity_by_the_right_clocks() {
    let now = Instant::now();
    let row = |id, state, status, since_secs, active_secs| SessionListEntry {
        id: SessionId::from_raw(id),
        state,
        status,
        since: Some(
            now.checked_sub(Duration::from_secs(since_secs))
                .expect("fixture clock supports elapsed seconds"),
        ),
        active: Some(
            now.checked_sub(Duration::from_secs(active_secs))
                .expect("fixture clock supports elapsed seconds"),
        ),
    };
    let rows = [
        row(1, ActivityState::Idle, "Idle", 1, 80),
        row(2, ActivityState::Working, "Working", 1, 7),
        row(3, ActivityState::Done, "Done", 40, 2),
        row(4, ActivityState::WaitingForInput, "Needs you", 20, 1),
        row(5, ActivityState::WaitingForInput, "Needs you", 90, 1),
        row(6, ActivityState::Done, "Done", 70, 1),
        row(7, ActivityState::Idle, "Idle", 1, 3),
        row(8, ActivityState::Working, "Working", 1, 1),
        row(9, ActivityState::Exited, "Exited", 1, 1),
    ];
    assert_eq!(
        group_sessions_for_list(&rows, now),
        vec![
            ("Needs you", vec![4, 3]),
            ("Review", vec![5, 2]),
            ("Working", vec![7, 1]),
            ("Ready", vec![6, 0, 8]),
        ]
    );
}

#[test]
fn sessions_list_lifecycle_overrides_activity_without_changing_status() {
    let now = Instant::now();
    let rows = [
        SessionListEntry {
            id: SessionId::from_raw(1),
            state: ActivityState::Working,
            status: "Failed",
            since: None,
            active: None,
        },
        SessionListEntry {
            id: SessionId::from_raw(2),
            state: ActivityState::Done,
            status: "Starting",
            since: None,
            active: None,
        },
    ];
    assert_eq!(
        group_sessions_for_list(&rows, now),
        vec![("Needs you", vec![0]), ("Working", vec![1]),]
    );
    assert_eq!(rows[0].status, "Failed");
    assert_eq!(rows[1].status, "Starting");
}

#[gpui::test]
fn session_status_spinner_advances_only_for_busy_states_and_respects_reduced_motion(
    cx: &mut gpui::TestAppContext,
) {
    struct StatusPreview(&'static str);
    impl gpui::Render for StatusPreview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            session_status_icon(SessionId::from_raw(1), self.0, cx)
        }
    }
    let (preview, cx) = cx.add_window_view(|_, _| StatusPreview("Working"));
    draw(cx);
    assert!(cx.debug_bounds("session-spinner-phase-1-0").is_some());
    // GPUI Animation uses wall-clock Instant, not the executor's virtual clock.
    std::thread::sleep(SESSION_SPINNER_PERIOD / 4);
    assert!(cx.update(Window::simulate_next_frame) > 0);
    draw(cx);
    assert!(
        (1..crate::entities::animation_clock::SPINNER_FRAMES).any(|frame| cx
            .debug_bounds(Box::leak(
                format!("session-spinner-phase-1-{frame}").into_boxed_str()
            ))
            .is_some())
    );
    assert!(cx.debug_bounds("session-spinner-phase-1-0").is_none());
    cx.update(|_, cx| {
        cx.set_reduce_motion(true);
        preview.update(cx, |_, cx| cx.notify());
    });
    cx.update(Window::simulate_next_frame);
    draw(cx);
    assert!(cx.debug_bounds("session-spinner-phase-1-0").is_some());
    assert_eq!(cx.update(Window::simulate_next_frame), 0);
    std::thread::sleep(SESSION_SPINNER_PERIOD / 4);
    draw(cx);
    assert!(cx.debug_bounds("session-spinner-phase-1-0").is_some());
    assert!(cx.debug_bounds("session-spinner-phase-1-3").is_none());
    cx.update(|_, cx| {
        cx.set_reduce_motion(false);
        preview.update(cx, |preview, cx| {
            preview.0 = "Starting";
            cx.notify();
        });
    });
    draw(cx);
    std::thread::sleep(SESSION_SPINNER_PERIOD / 4);
    assert!(cx.update(Window::simulate_next_frame) > 0);
    draw(cx);
    assert!(
        (1..crate::entities::animation_clock::SPINNER_FRAMES).any(|frame| cx
            .debug_bounds(Box::leak(
                format!("session-spinner-phase-1-{frame}").into_boxed_str()
            ))
            .is_some())
    );
    for status in ["Idle", "Done", "Needs you", "Failed", "Exited"] {
        cx.update(|_, cx| {
            preview.update(cx, |preview, cx| {
                preview.0 = status;
                cx.notify();
            });
        });
        cx.update(Window::simulate_next_frame);
        draw(cx);
        assert_eq!(cx.update(Window::simulate_next_frame), 0);
        for frame in 0..crate::entities::animation_clock::SPINNER_FRAMES {
            assert!(cx
                .debug_bounds(Box::leak(
                    format!("session-spinner-phase-1-{frame}").into_boxed_str()
                ))
                .is_none());
        }
    }
}

#[gpui::test]
fn activity_file_links_open_the_owning_codex_and_claude_transcript(cx: &mut gpui::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_str().unwrap();
    let file = root.path().join("review.md");
    fs_err::write(&file, "# Review").unwrap();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Layout fixture".into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_global(crate::theme::ThemeState::new(
            false,
            "tokyonight".into(),
            "tokyonight-day".into(),
        ));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let mut sidebar = Sidebar::new(runtime, window, cx);
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            attach_removal_layout_session(&mut sidebar, path, provider, window, cx);
        }
        sidebar
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let ids: Vec<_> = sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .all()
                .iter()
                .map(|meta| meta.id)
                .collect();
            for id in ids.iter().copied() {
                sidebar.selection = None;
                sidebar.codex_views.remove(&id);
                sidebar.open_activity_link(id, &format!("{}:1", file.display()), window, cx);
                assert_eq!(sidebar.selected_session(), Some(id));
                // Browser events propagate after this update completes.
                let view = sidebar.codex_views[&id].read(cx);
                let browser = view.file_browser.read(cx);
                assert!(browser.visible);
                assert_eq!(
                    browser.target.as_ref().unwrap().path,
                    file.canonicalize().unwrap()
                );
                assert_eq!(browser.target.as_ref().unwrap().line, Some(1));
            }
            let selection = sidebar.selection.clone();
            sidebar.open_activity_link(ids[0], "https://example.com/report", window, cx);
            assert_eq!(sidebar.selection, selection);
        });
    });
    cx.run_until_parked();
    // Exercise the actual browser -> transcript -> sidebar event chain for
    // both providers, with the explorer initially hidden and visible.
    for active in [false, true] {
        let ids = sidebar.read_with(cx, |sidebar, _| {
            sidebar.codex_views.keys().copied().collect::<Vec<_>>()
        });
        for id in ids {
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let browser = sidebar.codex_views[&id].read(cx).file_browser.clone();
                    // Reset activity-link viewers before exercising a fresh
                    // open from this session's actual focused transcript.
                    browser.update(cx, |browser, cx| browser.close(window, cx));
                    sidebar.navigate_to_session(id, window, cx);
                    sidebar.files_active = active;
                    sidebar.files_owner = sidebar.selection.clone();
                    sidebar.checkout_panel.hidden = true;
                    browser.update(cx, |browser, cx| {
                        browser.open(
                            crate::views::file_browser::local_target(
                                &format!("{}:1", file.display()),
                                root.path(),
                            )
                            .unwrap(),
                            window,
                            cx,
                        );
                    });
                });
            });
            cx.run_until_parked();
            sidebar.read_with(cx, |sidebar, _| {
                assert_eq!(sidebar.selected_session(), Some(id));
                assert!(sidebar.files_active);
                assert!(!sidebar.checkout_panel.hidden);
                assert_eq!(sidebar.files_owner, Some(Selection::Session(id)));
            });
            cx.update(|window, cx| {
                let browser = sidebar.read(cx).codex_views[&id]
                    .read(cx)
                    .file_browser
                    .clone();
                browser.update(cx, |browser, cx| browser.close(window, cx));
            });
            cx.run_until_parked();
            sidebar.read_with(cx, |sidebar, _| {
                assert_eq!(sidebar.selected_session(), Some(id));
                assert!(!sidebar.files_active);
                assert!(!sidebar.checkout_panel.hidden);
                assert!(sidebar.checkout_panel.side_chat_owner.is_none());
            });
        }
    }
    // Events from a background session cannot change the visible destination.
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let selected = sidebar.selected_session().unwrap();
            let other = *sidebar
                .codex_views
                .keys()
                .find(|id| **id != selected)
                .unwrap();
            sidebar.files_active = true;
            sidebar.checkout_panel.hidden = true;
            let browser = sidebar.codex_views[&other].read(cx).file_browser.clone();
            browser.update(cx, |browser, cx| browser.close(window, cx));
        });
    });
    cx.run_until_parked();
    sidebar.read_with(cx, |sidebar, _| {
        assert!(sidebar.files_active);
        assert!(sidebar.checkout_panel.hidden);
    });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/report")
    );
}
