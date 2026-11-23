use super::*;

#[gpui::test]
fn activity_triage_shows_mixed_session_facts_and_routes_the_pending_request(
    cx: &mut gpui::TestAppContext,
) {
    use grove_core::codex::{Provider, ReviewCompletion, Status, WorkPeriod};
    use serde_json::json;

    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_reduce_motion(true);
    });
    let mut ids = Vec::new();
    let mut actions = Vec::new();
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.open_dashboard(window, cx);
        let registry = sidebar.runtime.read(cx).registry.clone();
        let base = sidebar.dashboard.as_ref().unwrap().session.read(cx).state.saved.clone();
        for (kind, provider, title) in [
            (0, Provider::Codex, "QA snapshot cleanup"),
            (1, Provider::Claude, "QHIN health data integration research"),
            (2, Provider::Codex, "Verify installer builds"),
            (3, Provider::Claude, "Update integration fixtures"),
            (4, Provider::Codex, "Explore Activity layouts"),
        ] {
            let mut saved = base.clone();
            saved.key = format!("activity-triage-{kind}");
            saved.provider = provider;
            saved.title = Some(title.into());
            let mut state = grove_core::codex::Snapshot::new(saved);
            match kind {
                0 => {
                    state.status = Status::Working;
                    state.approvals.push(json!({"id":"qa","method":"item/commandExecution/requestApproval","params":{"command":"pwd","reason":"Confirm the QA directory."}}));
                }
                1 | 2 => {
                    state.status = Status::Done;
                    state.saved.items = vec![
                        json!({"type":"plan","plan":[{"id":"older","step":"Complete earlier work","status":"completed"}]}),
                        json!({"type":"userMessage","text":"Open the explainer folder"}),
                        json!({"type":"agentMessage","text":"Done."}),
                    ].into();
                    state.saved.work_periods = vec![WorkPeriod {
                        start_item: 1, end_item: Some(3), duration_ms: None, subagents: vec![],
                    }];
                    state.saved.session_review.pending = Some(ReviewCompletion { id: 1, period: 0 });
                    state.saved.session_review.summary = if kind == 1 {
                        "Opened the motion explainer folder.\nDeliverable: [Open explainer](https://example.com/explainer)".into()
                    } else {
                        "Linux installer passed.\nVerification: Windows still needs a live run.".into()
                    };
                }
                3 => {
                    state.status = Status::Working;
                    state.turn_id = Some("turn".into());
                    state.saved.activity_reporting = true;
                    let report = json!({"id":"fixtures","task_id":"","status":"in_progress","label":"Running integration fixtures"});
                    state.saved.items = vec![
                        json!({"type":"userMessage","text":"Update fixtures"}),
                        json!({"type":"mcpToolCall","server":"grove_activity","tool":"report_activity","status":"completed","arguments":report,"result":{"structuredContent":{"accepted":true,"report":report,"turn_id":"turn"}}}),
                    ].into();
                }
                _ => state.status = Status::Idle,
            }
            let (handle, receive_actions) = grove_core::codex::Handle::channel();
            actions.push(receive_actions);
            let (_updates, receive) = std::sync::mpsc::channel();
            let session = cx.new(|cx| CodexSession::from_connection(state, grove_core::codex::Connection { handle: handle.clone(), updates: receive.into() }, cx));
            let cwd = session.read(cx).state.saved.cwd.clone();
            ids.push(registry.update(cx, |registry, _| {
                let id = registry.insert_meta(String::new(), cwd, provider.agent());
                registry.attach_codex(id, session, handle);
                id
            }));
        }
        sidebar.sync(window, cx);
        sidebar
    });
    let [request, deliverable, review, working, ready] = ids.as_slice() else {
        panic!()
    };
    for (width, rem) in [(1100., crate::zoom::REM_BASE), (560., 20.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
        cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for selector in [
            format!("dashboard-review-request-{}", request.raw()),
            format!("dashboard-review-summary-{}-link-0", deliverable.raw()),
            format!("dashboard-review-summary-{}-verification", review.raw()),
            format!("dashboard-mark-reviewed-{}", review.raw()),
            format!("dashboard-current-action-{}", working.raw()),
            format!("dashboard-open-session-{}", ready.raw()),
        ] {
            let bounds = cx
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .expect("Activity fixture control is visible");
            let activity = cx.debug_bounds("dashboard-recent").unwrap();
            assert!(
                bounds.left() >= activity.left() && bounds.right() <= activity.right(),
                "{bounds:?} escapes {activity:?}"
            );
        }
    }
    let request_button = cx
        .debug_bounds(Box::leak(
            format!("dashboard-review-request-{}", request.raw()).into_boxed_str(),
        ))
        .unwrap();
    cx.simulate_click(request_button.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(sidebar.read(cx).selected_session(), Some(*request));
        assert!(sidebar.read(cx).codex_views.contains_key(request));
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.open_dashboard(window, cx));
        let _ = window.draw(cx);
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let focus =
                sidebar
                    .dashboard_timelines
                    .session_focus(sidebar.active_workspace, *request, cx);
            assert!(focus.is_focused(window));
        });
    });
    // Activate the first review action by keyboard, then deliver the
    // actor's acknowledgment through the observed snapshot.
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert_eq!(
                sidebar.dashboard_timelines.review_order,
                [*deliverable, *review]
            );
            sidebar
                .dashboard_timelines
                .review_action_focus((sidebar.active_workspace, *deliverable))
                .unwrap()
                .focus(window, cx);
        });
    });
    cx.simulate_keystrokes("enter");
    assert!(actions[1].try_iter().any(|action| matches!(
        action,
        grove_core::codex::Action::MarkReviewed { work_period: 1 }
    )));
    cx.update(|_, cx| {
        let session = sidebar
            .read(cx)
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(*deliverable)
            .unwrap()
            .clone();
        session.update(cx, |session, cx| {
            session.state.mark_reviewed(1).unwrap();
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar
                .dashboard_timelines
                .session_focus(sidebar.active_workspace, *review, cx)
                .is_focused(window));
        });
    });
}

#[test]
fn worktree_cache_reuses_only_fresh_matching_results_or_current_inflight() {
    let scope = (1, "/project".to_owned(), 7);
    let mut discovery = WorktreeDiscovery {
        scope: Some(scope.clone()),
        fetched_at: Some(std::time::Instant::now()),
        in_flight: None,
    };
    assert!(discovery.reusable(&scope, 4, true));
    assert!(!discovery.reusable(&scope, 4, false));
    assert!(!discovery.reusable(&(1, "/other".into(), 7), 4, true));
    assert!(!discovery.reusable(&(1, "/project".into(), 8), 4, true));
    discovery.fetched_at = Some(
        std::time::Instant::now()
            .checked_sub(WORKTREE_CACHE_FRESHNESS)
            .unwrap(),
    );
    assert!(!discovery.reusable(&scope, 4, true));
    discovery.in_flight = Some(4);
    assert!(discovery.reusable(&scope, 4, false));
    assert!(!discovery.reusable(&scope, 5, true));
    discovery.fetched_at = None;
    discovery.in_flight = None;
    assert!(
        !discovery.reusable(&scope, 4, true),
        "failures remain retryable"
    );
}

#[gpui::test]
async fn dashboard_revisit_reuses_worktrees_and_retry_forces_refresh(
    cx: &mut gpui::TestAppContext,
) {
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let path = repo.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Cached worktrees".into(),
            path: path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&path);
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.open_dashboard(window, cx);
            let tray = sidebar.dashboard.as_ref().unwrap().tray.clone();
            // Seed a successful result, then exercise the real navigation
            // and request boundary synchronously so no clock or CLI timing is needed.
            tray.update(cx, |tray, _| {
                tray.worktrees = vec![(path.clone(), "Main worktree".into())];
                tray.worktree_discovery.scope = Some((0, path.clone(), sidebar.active_workspace));
                tray.worktree_discovery.fetched_at = Some(std::time::Instant::now());
                tray.worktree_discovery.in_flight = None;
                tray.loading = false;
            });
            let generation = tray.read(cx).generation;
            let rows = tray.read(cx).worktrees.clone();
            sidebar.open_dashboard(window, cx);
            assert_eq!(
                tray.read(cx).generation,
                generation,
                "fresh revisit must not launch Git"
            );
            tray.update(cx, |tray, _| {
                tray.worktree_discovery.fetched_at = Some(
                    std::time::Instant::now()
                        .checked_sub(WORKTREE_CACHE_FRESHNESS)
                        .unwrap(),
                );
            });
            sidebar.open_dashboard(window, cx);
            let refreshing = tray.read(cx).generation;
            assert_ne!(refreshing, generation);
            assert_eq!(tray.read(cx).worktrees, rows);
            assert!(
                !tray.read(cx).loading,
                "retained rows remain usable during refresh"
            );
            sidebar.open_dashboard(window, cx);
            assert_eq!(
                tray.read(cx).generation,
                refreshing,
                "concurrent revisit must join refresh"
            );
            tray.update(cx, |tray, _| tray.error = Some("retry me".into()));
            tray.update(cx, DestinationTray::retry_worktrees);
        });
    });
    // Retry is delivered through Sidebar's real subscription. A parked UI
    // scheduler does not imply the real Git subprocess has finished.
    cx.run_until_parked();
    let tray = cx.update(|_, cx| sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone());
    cx.cx
        .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
        .await;
    cx.update(|_, cx| {
        let tray = &sidebar.read(cx).dashboard.as_ref().unwrap().tray;
        assert!(tray.read(cx).error.is_none());
        assert!(tray.read(cx).worktree_discovery.fetched_at.is_some());
        assert!(!tray.read(cx).worktrees.is_empty());
    });
    let (workspace, discarded_generation) = cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let workspace = sidebar.active_workspace;
            sidebar.force_refresh_dashboard_worktrees(cx);
            let generation = sidebar.dashboard.as_ref().unwrap().tray.read(cx).generation;
            // Change the workspace before the actual Git result is delivered.
            cx.global_mut::<SettingsState>().store.workspaces.active = workspace.wrapping_add(1);
            (workspace, generation)
        })
    });
    cx.cx
        .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
        .await;
    cx.update(|window, cx| {
        cx.global_mut::<SettingsState>().store.workspaces.active = workspace;
        sidebar.update(cx, |sidebar, cx| {
            let tray = sidebar.dashboard.as_ref().unwrap().tray.clone();
            assert!(tray.read(cx).worktree_discovery.in_flight.is_none());
            assert!(tray.read(cx).worktree_discovery.fetched_at.is_none());
            sidebar.sync(window, cx);
            assert!(sidebar.open_dashboard_for_project(0, &path, window, cx));
            assert_ne!(
                tray.read(cx).generation,
                discarded_generation,
                "returning must start a new request after discarding a hidden result"
            );
        });
    });
    cx.cx
        .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
        .await;
    cx.update(|_, cx| {
        let tray = &sidebar.read(cx).dashboard.as_ref().unwrap().tray;
        assert!(tray.read(cx).worktree_discovery.fetched_at.is_some());
        assert!(tray.read(cx).error.is_none());
    });
}

// Exercise the view emitted event through Sidebar's real subscription and runtime.
// The invalid boundary deliberately stops before any provider process is launched.
fn adopted_session_routes_fork_event(
    cx: &mut gpui::TestAppContext,
    provider: grove_core::codex::Provider,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let view = cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.open_dashboard(window, cx);
            let mut saved = sidebar
                .dashboard
                .as_ref()
                .unwrap()
                .session
                .read(cx)
                .state
                .saved
                .clone();
            saved.provider = provider;
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let mut state = grove_core::codex::Snapshot::new(saved);
            state.status = grove_core::codex::Status::Idle;
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    state,
                    grove_core::codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
            sidebar.adopt_dashboard_launch(NO_PROJECT, session, view.clone(), false, window, cx);
            view
        })
    });
    view.update(cx, |_, cx| {
        cx.emit(CodexViewEvent::ForkRequested(usize::MAX, false));
    });
    cx.run_until_parked();
    sidebar.read_with(cx, |sidebar, cx| {
        let toast = sidebar.runtime.read(cx).toast.read(cx).current();
        assert_eq!(
            toast.map(|toast| toast.message.as_str()),
            Some("This conversation boundary is unavailable."),
            "the adopted view must route Fork to the runtime instead of silently dropping it"
        );
    });
}

#[gpui::test]
fn dashboard_adopted_codex_routes_fork_event(cx: &mut gpui::TestAppContext) {
    adopted_session_routes_fork_event(cx, grove_core::codex::Provider::Codex);
}
#[gpui::test]
fn dashboard_adopted_claude_routes_fork_event(cx: &mut gpui::TestAppContext) {
    adopted_session_routes_fork_event(cx, grove_core::codex::Provider::Claude);
}

#[gpui::test]
fn activity_includes_projectless_codex_and_claude_without_other_workspace_sessions(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let (visible, hidden) = cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.open_dashboard(window, cx);
            let registry = sidebar.runtime.read(cx).registry.clone();
            let ids = registry.update(cx, |registry, cx| {
                let visible = [Agent::Codex, Agent::Claude]
                    .map(|agent| registry.insert_meta(String::new(), dashboard_home_dir(), agent));
                let hidden = registry.insert_meta(
                    "Other workspace".into(),
                    "/tmp/other-workspace".into(),
                    Agent::Codex,
                );
                cx.notify();
                (visible, hidden)
            });
            sidebar.sync(window, cx);
            ids
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    for id in visible {
        assert!(cx
            .debug_bounds(Box::leak(
                format!("dashboard-terminal-session-{}", id.raw()).into_boxed_str()
            ))
            .is_some());
    }
    assert!(cx
        .debug_bounds(Box::leak(
            format!("dashboard-terminal-session-{}", hidden.raw()).into_boxed_str()
        ))
        .is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let canvas = sidebar.active_canvas_sessions(cx);
            assert!(visible.iter().all(|id| canvas.contains(&(*id, false))));
            assert!(!canvas.contains(&(hidden, false)));
            sidebar.select(Selection::Session(visible[0]), cx);
            sidebar.sync(window, cx);
            assert_eq!(sidebar.selection, Some(Selection::Session(visible[0])));
        });
    });
}

#[gpui::test]
fn project_picker_destination_preserves_codex_and_claude_composer(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        for (name, path) in [
            ("First", "/tmp/picker-first"),
            ("Second", "/tmp/picker-second"),
        ] {
            store.projects.push(grove_core::storage::Project {
                name: name.into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(path);
        }
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let settings = cx.new(|cx| {
                crate::views::settings_panel::SettingsPanel::new(sidebar.runtime.clone(), cx)
            });
            sidebar.set_settings_panel(settings.clone(), cx);
            sidebar.open_dashboard(window, cx);
            for provider in [
                grove_core::codex::Provider::Codex,
                grove_core::codex::Provider::Claude,
            ] {
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                let session = dashboard.session.clone();
                let view = dashboard.view.clone();
                session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.saved.draft = "Retain literal draft".into();
                    session.state.saved.composer.model = Some("retained-model".into());
                    session.state.saved.composer.effort = Some("high".into());
                    session.state.saved.config_args = vec!["retained-config".into()];
                });
                assert!(!sidebar.open_dashboard_for_project(1, "/stale", window, cx));
                assert!(sidebar.open_dashboard_for_project(1, "/tmp/picker-second", window, cx));
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                assert_eq!(dashboard.session, session);
                assert_eq!(dashboard.view, view);
                let saved = &session.read(cx).state.saved;
                assert_eq!(saved.provider, provider);
                assert_eq!(saved.draft, "Retain literal draft");
                assert_eq!(saved.composer.model.as_deref(), Some("retained-model"));
                assert_eq!(saved.composer.effort.as_deref(), Some("high"));
                assert_eq!(saved.config_args, ["retained-config"]);
                assert_eq!(
                    sidebar.dashboard_destination_for_test(cx),
                    Some((1, "/tmp/picker-second".into()))
                );
                assert!(sidebar.dashboard_open);
                settings.update(cx, |settings, cx| {
                    settings.open_projects(Some("/tmp/picker-second".into()), window, cx);
                });
                assert!(settings.read(cx).is_open());
                sidebar.open_project_checkout_from_settings(
                    "/tmp/picker-second",
                    "/tmp/picker-second",
                    false,
                    window,
                    cx,
                );
                assert!(!settings.read(cx).is_open());
                sidebar.manage_project_checkouts("/tmp/picker-second", window, cx);
                sidebar.open_destination(
                    super::super::global_navigation::Destination::Home,
                    window,
                    cx,
                );
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                assert_eq!(dashboard.session, session);
                assert_eq!(dashboard.view, view);
                assert_eq!(session.read(cx).state.saved.draft, "Retain literal draft");
                assert_eq!(session.read(cx).state.saved.provider, provider);
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
            }
        });
    });
    cx.update(|_, cx| {
        SettingsState::update(cx, |store| {
            store.projects.remove(0);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            sidebar.read(cx).dashboard_destination_for_test(cx),
            Some((0, "/tmp/picker-second".into()))
        );
        SettingsState::update(cx, |store| store.projects[0].archived = true);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let dashboard = sidebar.read(cx).dashboard.as_ref().unwrap();
        assert_eq!(dashboard.tray.read(cx).project, NO_PROJECT);
        assert_eq!(
            dashboard.session.read(cx).state.saved.draft,
            "Retain literal draft"
        );
    });
}

struct InsetDashboardFixture(Entity<Sidebar>);
impl Render for InsetDashboardFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().pt(gpui::px(100.)).child(self.0.clone())
    }
}

#[gpui::test]
fn dashboard_position_uses_its_own_inset_viewport(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        let path = "/tmp/grove-dashboard-inset-fixture";
        store.projects.push(grove_core::storage::Project {
            name: "Dashboard fixture".into(),
            path: path.into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(path);
        cx.set_global(SettingsState::new(store));
    });
    let (fixture, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        InsetDashboardFixture(sidebar)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let sidebar = fixture.read(cx).0.clone();
        let session = sidebar.read(cx).dashboard.as_ref().unwrap().session.clone();
        session.update(cx, |session, cx| {
            session.catalog_loading = false;
            session.catalog_error = Some("Provider options unavailable".into());
            session.state.error = Some("Launch failed".into());
            cx.notify();
        });
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let page = cx.debug_bounds("dashboard-page").unwrap();
    let composer = cx.debug_bounds("codex-composer").unwrap();
    assert_eq!(page.top(), gpui::px(100.));
    assert!(
        (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION).abs()
            < gpui::px(2.),
        "page: {page:?}, composer: {composer:?}"
    );
}

#[gpui::test]
fn dashboard_adoption_keeps_composer_anchored_on_first_frame(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        let path = "/tmp/grove-dashboard-inset-adoption";
        store.projects.push(grove_core::storage::Project {
            name: "Dashboard fixture".into(),
            path: path.into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(path);
        cx.set_global(SettingsState::new(store));
    });
    let (fixture, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        InsetDashboardFixture(sidebar)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let before = cx.debug_bounds("codex-composer").unwrap().top();
    let (_updates, receive) = std::sync::mpsc::channel();
    let (handle, _actions) = grove_core::codex::Handle::channel();
    cx.update(|window, cx| {
        let sidebar = fixture.read(cx).0.clone();
        sidebar.update(cx, |sidebar, cx| {
            let saved = sidebar
                .dashboard
                .as_ref()
                .unwrap()
                .session
                .read(cx)
                .state
                .saved
                .clone();
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    grove_core::codex::Snapshot::new(saved),
                    grove_core::codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
            let measured = sidebar.dashboard.as_ref().unwrap().viewport_height.get();
            assert_eq!(measured, gpui::px(800.));
            assert_eq!(window.viewport_size().height, gpui::px(900.));
            sidebar.adopt_dashboard_launch(0, session, view, false, window, cx);
            assert_eq!(
                sidebar.dashboard.as_ref().unwrap().viewport_height.get(),
                measured,
                "the replacement must have the measured height before its first layout"
            );
        });
        let _ = window.draw(cx);
    });
    assert_eq!(
        cx.debug_bounds("codex-composer").unwrap().top(),
        before,
        "the first replacement frame must use the measured dashboard height"
    );
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1000.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let page = cx.debug_bounds("dashboard-page").unwrap();
    let composer = cx.debug_bounds("codex-composer").unwrap();
    assert!(
        (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION).abs()
            < gpui::px(2.)
    );
}

// Exercise the real adoption/fresh-draft owner with mocked connected sessions.
// Live provider normalization and native submit are verified separately.
fn sequential_dashboard_adoption(
    cx: &mut gpui::TestAppContext,
    provider: grove_core::codex::Provider,
    reduced_motion: bool,
    after_session_start: grove_core::storage::AfterSessionStart,
    open_first_transcript: bool,
) {
    let repo = tempfile::tempdir().unwrap();
    let first = repo.path().join("first");
    let selected = repo.path().join("selected");
    let checkout = repo.path().join("selected-checkout");
    for path in [&first, &selected, &checkout] {
        fs_err::create_dir(path).unwrap();
    }
    let checkout = checkout.to_string_lossy().into_owned();
    cx.update(|cx| {
        cx.set_reduce_motion(reduced_motion);
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store {
            after_session_start,
            ..Default::default()
        };
        for (name, path) in [("First", &first), ("Selected", &selected)] {
            let path = path.to_string_lossy().into_owned();
            store.projects.push(grove_core::storage::Project {
                name: name.into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(&path);
        }
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.run_until_parked();
    let mut receivers = Vec::new();
    let mut launched_sessions = Vec::new();
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.open_dashboard(window, cx);
            let initial_key = sidebar
                .dashboard
                .as_ref()
                .unwrap()
                .session
                .read(cx)
                .state
                .saved
                .key
                .clone();
            let mut fresh_keys = vec![initial_key];
            let mut adopted_ids = Vec::new();
            for turn in 1..=2 {
                let mut saved = sidebar
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .session
                    .read(cx)
                    .state
                    .saved
                    .clone();
                saved.provider = provider;
                saved.project = "Selected".into();
                saved.cwd = checkout.clone();
                saved.key = format!("mock-dashboard-turn-{turn}");
                saved.draft = format!("Independent prompt {turn}");
                saved.composer.model = Some("fixture-model".into());
                saved.composer.effort = Some("high".into());
                saved.composer.access = Some("fixture-permission".into());
                saved.composer.speed = Some(grove_core::codex::Speed::Fast);
                saved.config_args = vec!["fixture-configuration".into()];
                sidebar
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .tray
                    .update(cx, |tray, _| {
                        tray.project = 1;
                        tray.path = checkout.clone();
                        tray.worktrees = vec![(checkout.clone(), "feature-branch".into())];
                        tray.loading = false;
                        tray.error = None;
                    });
                let (handle, actions) = grove_core::codex::Handle::channel();
                receivers.push(actions);
                let (_updates, receive) = std::sync::mpsc::channel();
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        grove_core::codex::Snapshot::new(saved),
                        grove_core::codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                launched_sessions.push(session.clone());
                let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
                let open_transcript = open_first_transcript && turn == 1;
                sidebar.adopt_dashboard_launch(
                    1,
                    session.clone(),
                    view.clone(),
                    open_transcript,
                    window,
                    cx,
                );
                let open_session = open_transcript
                    || after_session_start == grove_core::storage::AfterSessionStart::OpenSession;
                assert_eq!(sidebar.dashboard_open, !open_session);
                if open_session {
                    let id = sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .all()
                        .last()
                        .unwrap()
                        .id;
                    assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                    assert!(view.focus_handle(cx).is_focused(window));
                }
                let d = sidebar.dashboard.as_ref().unwrap();
                let next = &d.session.read(cx).state.saved;
                assert_ne!(d.session, session);
                assert_ne!(d.view, view);
                assert!(d.session.read(cx).is_deferred());
                assert_eq!(next.provider, provider);
                assert_eq!(next.cwd, checkout);
                assert_eq!(d.tray.read(cx).project, 1);
                assert_eq!(d.tray.read(cx).path, checkout);
                assert_eq!(
                    d.tray.read(cx).worktrees,
                    [(checkout.clone(), "feature-branch".into())]
                );
                assert!(
                    !d.tray.read(cx).loading,
                    "cached destination remains visible during refresh"
                );
                assert_eq!(next.composer.model.as_deref(), Some("fixture-model"));
                assert_eq!(next.composer.effort.as_deref(), Some("high"));
                assert_eq!(next.composer.access.as_deref(), Some("fixture-permission"));
                assert_eq!(next.composer.speed, Some(grove_core::codex::Speed::Fast));
                assert_eq!(next.config_args, ["fixture-configuration"]);
                assert!(next.draft.is_empty());
                assert!(next.draft_images.is_empty());
                assert!(next.draft_reference.is_none());
                assert!(next.thread_id.is_none());
                assert!(next.items.is_empty());
                assert!(!fresh_keys.contains(&next.key));
                fresh_keys.push(next.key.clone());
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                assert_eq!(registry.all().len(), turn);
                adopted_ids.push(registry.all().last().unwrap().id);
                assert_eq!(
                    sidebar
                        .dashboard_timelines
                        .is_entering(sidebar.active_workspace, *adopted_ids.last().unwrap()),
                    !reduced_motion,
                    "only adopted sessions receive entrance state; reduced motion skips it"
                );
                assert_eq!(
                    session.read(cx).state.saved.draft,
                    format!("Independent prompt {turn}")
                );
                sidebar.open_dashboard(window, cx);
            }
            assert_ne!(adopted_ids[0], adopted_ids[1]);
            sidebar.open_dashboard(window, cx);
            for id in adopted_ids {
                assert!(!sidebar
                    .dashboard_timelines
                    .is_entering(sidebar.active_workspace, id));
            }
        });
    });
    cx.update(|_, cx| {
        for session in &launched_sessions {
            session.update(cx, |session, cx| {
                session.state.status = grove_core::codex::Status::Idle;
                session.state.saved.thread_id = Some("ready-thread".into());
                cx.notify();
            });
        }
    });
    cx.run_until_parked();
    for (turn, actions) in receivers.iter().enumerate() {
        let submissions = actions
            .try_iter()
            .filter_map(|action| match action {
                grove_core::codex::Action::Submit { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            submissions,
            [format!("Independent prompt {}", turn + 1)],
            "each adopted session sends exactly once after readiness"
        );
    }
}
#[gpui::test]
fn dashboard_sequential_codex_submits_keep_fresh_composer_and_destination(
    cx: &mut gpui::TestAppContext,
) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Codex,
        false,
        grove_core::storage::AfterSessionStart::StayOnDashboard,
        false,
    );
}
#[gpui::test]
fn dashboard_sequential_claude_submits_keep_fresh_composer_and_destination(
    cx: &mut gpui::TestAppContext,
) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Claude,
        false,
        grove_core::storage::AfterSessionStart::StayOnDashboard,
        false,
    );
}
#[gpui::test]
fn dashboard_codex_reduced_motion_skips_entrance(cx: &mut gpui::TestAppContext) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Codex,
        true,
        grove_core::storage::AfterSessionStart::StayOnDashboard,
        false,
    );
}
#[gpui::test]
fn dashboard_claude_reduced_motion_skips_entrance(cx: &mut gpui::TestAppContext) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Claude,
        true,
        grove_core::storage::AfterSessionStart::StayOnDashboard,
        false,
    );
}
#[gpui::test]
fn dashboard_codex_opens_adopted_session_and_preserves_fresh_composer(
    cx: &mut gpui::TestAppContext,
) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Codex,
        false,
        grove_core::storage::AfterSessionStart::OpenSession,
        false,
    );
}
#[gpui::test]
fn dashboard_claude_opens_adopted_session_and_preserves_fresh_composer(
    cx: &mut gpui::TestAppContext,
) {
    sequential_dashboard_adoption(
        cx,
        grove_core::codex::Provider::Claude,
        false,
        grove_core::storage::AfterSessionStart::OpenSession,
        false,
    );
}
#[gpui::test]
fn dashboard_transcript_override_is_per_launch_for_both_providers(cx: &mut gpui::TestAppContext) {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        sequential_dashboard_adoption(
            cx,
            provider,
            false,
            grove_core::storage::AfterSessionStart::StayOnDashboard,
            true,
        );
    }
}
fn worktree_round_trip(cx: &mut gpui::TestAppContext, provider: grove_core::codex::Provider) {
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().join("repo");
    fs_err::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "Fixture",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let worktree_root = grove_core::git::worktrees_root().unwrap();
    fs_err::create_dir_all(&worktree_root).unwrap();
    let worktree_fixture = tempfile::Builder::new()
        .prefix("grove-dashboard-roundtrip-")
        .tempdir_in(worktree_root)
        .unwrap();
    let worktree_dir = worktree_fixture
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let project_path = repo.to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Round trip".into(),
            path: project_path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: Some(worktree_dir),
        });
        store.assign_project_to_active_workspace(&project_path);
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.open_dashboard(window, cx);
            let d = sidebar.dashboard.as_ref().unwrap();
            d.session.update(cx, |session, cx| {
                session.state.saved.provider = provider;
                cx.notify();
            });
        });
        let _ = window.draw(cx);
    });
    cx.simulate_input("Keep this Dashboard draft");
    cx.run_until_parked();
    let (session_id, view_id) = cx.update(|_, cx| {
        let d = sidebar.read(cx).dashboard.as_ref().unwrap();
        assert_eq!(
            d.session.read(cx).state.saved.draft,
            "Keep this Dashboard draft"
        );
        (d.session.entity_id(), d.view.entity_id())
    });
    for cancel in [true, false] {
        cx.update(|_, cx| {
            let tray = sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone();
            tray.update(cx, |_, cx| cx.emit(DestinationEvent::NewWorktree));
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(sidebar.read(cx).pending_new_worktree, Some(0)));
        cx.simulate_input(if cancel {
            "discarded-worktree"
        } else {
            "dashboard-round-trip"
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                if cancel {
                    sidebar.act(Action::Cancel, window, cx);
                } else {
                    sidebar.submit_worktree(cx);
                    assert!(
                        sidebar.worktree_errors.iter().all(Option::is_none),
                        "{:?}",
                        sidebar.worktree_errors
                    );
                }
            });
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while sidebar.read_with(cx, |sidebar, _| sidebar.worktree_creating)
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(
                    !sidebar.worktree_creating,
                    "worktree creation did not finish"
                );
                assert!(sidebar.pending_new_worktree.is_none());
                assert!(sidebar.dashboard_open);
                let d = sidebar.dashboard.as_ref().unwrap();
                assert_eq!(d.session.entity_id(), session_id);
                assert_eq!(d.view.entity_id(), view_id);
                assert_eq!(
                    d.session.read(cx).state.saved.draft,
                    "Keep this Dashboard draft"
                );
                assert!(d.session.read(cx).is_deferred());
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
                if cancel {
                    assert_eq!(d.tray.read(cx).path, project_path);
                } else {
                    let expected = worktree_fixture
                        .path()
                        .join("dashboard-round-trip")
                        .to_string_lossy()
                        .into_owned();
                    assert_eq!(d.tray.read(cx).path, expected);
                    assert_eq!(d.session.read(cx).state.saved.cwd, expected);
                    assert!(std::path::Path::new(&expected).join(".git").exists());
                }
                d.view.focus_handle(cx).focus(window, cx);
            });
        });
    }
    cx.simulate_input(" retained after creation");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let d = sidebar.read(cx).dashboard.as_ref().unwrap();
        assert_eq!(
            d.session.read(cx).state.saved.draft,
            "Keep this Dashboard draft retained after creation"
        );
    });
}
#[gpui::test]
fn codex_dashboard_worktree_round_trip_retains_composer(cx: &mut gpui::TestAppContext) {
    worktree_round_trip(cx, grove_core::codex::Provider::Codex);
}
#[gpui::test]
fn claude_dashboard_worktree_round_trip_retains_composer(cx: &mut gpui::TestAppContext) {
    worktree_round_trip(cx, grove_core::codex::Provider::Claude);
}

#[gpui::test]
fn activity_growth_keeps_heading_and_composer_anchored(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        let path = "/tmp/grove-dashboard-anchor-fixture";
        store.projects.push(grove_core::storage::Project {
            name: "Dashboard visual fixture".into(),
            path: path.into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(path);
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    for (width, height) in [(1100., 900.), (560., 500.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let summary = cx.debug_bounds("dashboard-summary").unwrap().top();
        let composer = cx.debug_bounds("codex-composer").unwrap().top();
        let id = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                seed_activity_fixture(sidebar, grove_core::codex::Provider::Codex, window, cx)
            })
        });
        for expanded in [false, true, false] {
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    if sidebar
                        .dashboard_timelines
                        .is_open(sidebar.active_workspace, id)
                        != expanded
                    {
                        sidebar
                            .dashboard_timelines
                            .toggle((sidebar.active_workspace, id));
                    }
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(summary, cx.debug_bounds("dashboard-summary").unwrap().top());
            assert_eq!(composer, cx.debug_bounds("codex-composer").unwrap().top());
        }
        cx.update(|_,cx| {
            let session=sidebar.read(cx).runtime.read(cx).registry.read(cx).codex(id).unwrap().clone();
            session.update(cx,|session,cx| {
                session.state.saved.items.push(serde_json::json!({"type":"agentMessage","text":"A new observed activity step with enough text to wrap at narrow window widths."}));
                cx.notify();
            });
            sidebar.update(cx,|sidebar,cx| {
                let items=&session.read(cx).state.saved.items;
                sidebar.task_glances.get_mut(&id).unwrap().refresh_dashboard(items,grove_core::codex::Status::Working);
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(summary, cx.debug_bounds("dashboard-summary").unwrap().top());
        assert_eq!(composer, cx.debug_bounds("codex-composer").unwrap().top());
    }
}

#[gpui::test]
fn dashboard_opens_at_startup_and_groups_content(cx: &mut gpui::TestAppContext) {
    let repo = tempfile::tempdir().unwrap();
    let path = repo.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Test project".into(),
            path: path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&path);
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let session = sidebar.read(cx).dashboard.as_ref().unwrap().session.clone();
        session.update(
            cx,
            crate::entities::codex_session::CodexSession::settle_draft_catalog_for_test,
        );
    });
    cx.update(|_, cx| {
        let tray = sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone();
        tray.update(cx, |tray, cx| {
            tray.set_preparation_for_test(false, None, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.update(|_, cx| {
        assert!(sidebar.read(cx).dashboard_open);
        assert!(sidebar.read(cx).dashboard.is_some());
        assert_eq!(sidebar.read(cx).runtime.read(cx).registry.read(cx).len(), 0);
    });
    let summary = cx.debug_bounds("dashboard-summary").unwrap();
    let composer = cx.debug_bounds("codex-composer").unwrap();
    let recent = cx.debug_bounds("dashboard-recent").unwrap();
    let tray = cx.debug_bounds("dashboard-destination").unwrap();
    let hints = cx.debug_bounds("codex-composer-hints").unwrap();
    let page = cx.debug_bounds("dashboard-page").unwrap();
    assert!(
        (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION).abs()
            < gpui::px(2.),
        "page: {page:?}, composer: {composer:?}"
    );
    assert!(composer.top() >= summary.bottom());
    assert!(composer.top() - summary.bottom() < gpui::px(100.));
    assert_eq!(tray.top(), composer.bottom());
    assert!(hints.top() >= tray.bottom());
    let heading = cx.debug_bounds("dashboard-activity-heading").unwrap();
    assert_eq!(heading.left(), recent.left());
    assert!(heading.bottom() < recent.top());
    assert_eq!(recent.left(), composer.left());
    assert_eq!(recent.size.width, composer.size.width);
    assert!(recent.top() >= hints.bottom());
    assert!(
        heading.top() - hints.bottom() < gpui::px(50.),
        "heading={heading:?}; hints={hints:?}"
    );
    assert!(composer.size.height > gpui::px(50.));
    assert!(
        composer.bottom() <= page.bottom(),
        "composer must fit beside checkout tools"
    );
    if let Some(checkout) = cx.debug_bounds("checkout-panel") {
        assert!(composer.right() <= checkout.left());
    }

    cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(500.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let summary = cx.debug_bounds("dashboard-summary").unwrap();
    let composer = cx.debug_bounds("codex-composer").unwrap();
    assert!(composer.top() >= summary.bottom());
    assert!(composer.size.width > gpui::px(100.));
    let recent = cx.debug_bounds("dashboard-recent").unwrap();
    let heading = cx.debug_bounds("dashboard-activity-heading").unwrap();
    assert_eq!(heading.left(), recent.left());
    assert!(heading.bottom() < recent.top());
    assert_eq!(recent.left(), composer.left());
    assert_eq!(recent.size.width, composer.size.width);
    assert!(composer.top() - summary.bottom() < gpui::px(100.));
    // The compact layout fits at 500px; use a short viewport to exercise scrolling.
    cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(200.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let page = cx.debug_bounds("dashboard-page").unwrap();
    let recent_top = cx.debug_bounds("dashboard-recent").unwrap().top();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: page.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-300.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("dashboard-recent").unwrap().top() < recent_top);

    // A workspace switch lands on the actual Dashboard even from grid mode.
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.toggle_grid(window, cx);
        });
        let store = &mut cx.global_mut::<SettingsState>().store;
        store.workspaces.create("Other workspace").unwrap();
        store.assign_project_to_active_workspace(&path);
        sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(sidebar.read(cx).mode, ViewMode::Grid);
        assert!(sidebar.read(cx).dashboard_open);
        assert_eq!(sidebar.read(cx).runtime.read(cx).registry.read(cx).len(), 0);
    });
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("codex-composer").is_some());
    assert!(cx.debug_bounds("dashboard-destination").is_some());
}

#[gpui::test]
fn destination_popup_rise_cleanup_cannot_unmount_a_new_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (tray, cx) = cx.add_window_view(|_, _| {
        DestinationTray::new(
            vec![(0, "Project".into(), "/checkout".into())],
            0,
            "/checkout".into(),
        )
    });
    cx.update(|window, cx| {
        cx.set_reduce_motion(false);
        tray.update(cx, |tray, cx| {
            tray.open(true, window, cx);
            tray.close_popup(cx);
            assert!(tray.popup.is_none());
            assert!(tray.closing_popup.is_some());
            tray.open(false, window, cx);
        });
    });
    cx.executor()
        .advance_clock(motion::PopupMotion::exit_duration() * 2);
    cx.run_until_parked();
    cx.update(|window, cx| {
        tray.update(cx, |tray, cx| {
            assert!(tray.popup.is_some());
            assert!(tray.open_selector == Some(1));
            cx.set_reduce_motion(true);
            tray.close_popup(cx);
            assert!(tray.closing_popup.is_none());
            tray.open(true, window, cx);
            tray.close_popup(cx);
            assert!(tray.closing_popup.is_none());
        });
    });
}

#[gpui::test]
fn dashboard_destination_controls_size_to_content(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (tray, cx) = cx.add_window_view(|_, _| {
        let mut tray = DestinationTray::new(
            vec![(
                0,
                "A substantially longer project".into(),
                "/checkout".into(),
            )],
            0,
            "/checkout".into(),
        );
        tray.worktrees = vec![("/checkout".into(), "main".into())];
        tray
    });
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(300.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.update(|_, cx| {
        let tray = tray.read(cx);
        let project = tray.bounds[0].get();
        let worktree = tray.bounds[1].get();
        assert!(
            project.size.width > worktree.size.width,
            "long project grows independently of the short worktree"
        );
        assert!(project.size.width < gpui::px(400.));
        assert!(worktree.size.width < gpui::px(200.));
    });
    let strip = cx.debug_bounds("dashboard-destination").unwrap();
    assert_eq!(
        strip.size.width,
        gpui::px(1000. * COMPOSER_CONTEXT_WIDTH_FRACTION)
    );
    let bounds = cx.update(|_, cx| {
        let tray = tray.read(cx);
        [tray.bounds[0].get(), tray.bounds[1].get()]
    });
    assert_eq!(bounds[0].left(), strip.left() + gpui::px(SPACE_2XL + 1.));
    assert_eq!(bounds[1].right(), strip.right() - gpui::px(SPACE_2XL + 1.));
    for (idx, bounds) in bounds.iter().enumerate() {
        assert!(
            (f32::from(bounds.center().y - strip.center().y)).abs() <= 1.0,
            "selector is vertically centered in the strip"
        );
        cx.simulate_mouse_move(bounds.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let tray = tray.read(cx);
            assert!(tray.hovered[idx]);
            assert!(!tray.hovered[1 - idx]);
            assert_eq!(
                tray.bounds[idx].get(),
                *bounds,
                "hover must not shift layout"
            );
        });
    }
    cx.simulate_mouse_move(
        gpui::point(strip.left() - gpui::px(1.), strip.center().y),
        None,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(tray.read(cx).hovered, [false; 2]);
    });

    cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(300.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let strip = cx.debug_bounds("dashboard-destination").unwrap();
    cx.update(|_, cx| {
        let tray = tray.read(cx);
        let project = tray.bounds[0].get();
        let worktree = tray.bounds[1].get();
        assert!(
            worktree.top() >= project.bottom() || worktree.left() >= project.right(),
            "narrow selectors remain separate whether they fit or wrap"
        );
        assert_eq!(worktree.right(), strip.right() - gpui::px(SPACE_2XL + 1.));
    });
}
#[gpui::test]
fn dashboard_status_follows_adopted_codex_and_claude_sessions(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let source = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let saved = sidebar
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .session
                    .read(cx)
                    .state
                    .saved
                    .clone();
                let source = cx.new(|cx| {
                    let mut session = CodexSession::history(saved, cx);
                    session.state.saved.provider = provider;
                    session.state.status = grove_core::codex::Status::Starting;
                    session.begin_dashboard_handoff_for_test();
                    session
                });
                let old_view = cx.new(|cx| CodexView::new(source.clone(), window, cx));
                sidebar.adopt_dashboard_launch(
                    NO_PROJECT,
                    source.clone(),
                    old_view.clone(),
                    false,
                    window,
                    cx,
                );
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                assert_ne!(dashboard.view, old_view);
                assert_ne!(dashboard.session, source);
                dashboard.session.update(
                    cx,
                    crate::entities::codex_session::CodexSession::settle_draft_catalog_for_test,
                );
                assert_eq!(
                    dashboard
                        .view
                        .read(cx)
                        .dashboard_handoff_sessions(cx)
                        .as_slice(),
                    std::slice::from_ref(&source)
                );
                source
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("dashboard-status-entry-0").is_some(),
            "the replacement Dashboard shows connection progress"
        );
        // Clear the actor-owned handoff, without touching the fresh draft.
        cx.update(|_, cx| {
            source.update(cx, |session, cx| {
                session.cancel_dashboard_title_waiting();
                session.state.status = grove_core::codex::Status::Done;
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("dashboard-status-entry-0").is_none(),
            "completion removes the observed handoff"
        );
        sidebar.read_with(cx, |sidebar, cx| {
            let dashboard = sidebar.dashboard.as_ref().unwrap();
            assert!(dashboard.session.read(cx).state.saved.draft.is_empty());
            assert!(dashboard
                .view
                .read(cx)
                .dashboard_handoff_sessions(cx)
                .is_empty());
        });
    }
}
