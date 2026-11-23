mod project_tree;

use super::*;

#[gpui::test]
fn sidebar_session_press_cannot_start_transcript_selection(cx: &mut gpui::TestAppContext) {
    let cx = navigation_selection_fixture(cx);
    let session = cx.debug_bounds("selection-session").unwrap();
    let transcript = cx.debug_bounds("selection-transcript").unwrap();
    cx.simulate_mouse_down(
        session.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_move(
        transcript.origin + gpui::point(gpui::px(100.), gpui::px(8.)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert_eq!(
        cx.update(gpui_component::WindowExt::selected_text),
        "",
        "session navigation owns its press, including small pointer slips"
    );
    cx.simulate_mouse_up(
        transcript.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
}

#[gpui::test]
fn empty_expanded_sidebar_hides_cross_project_section(cx: &mut gpui::TestAppContext) {
    assert_empty_cross_project_launch(cx, false, false, false);
}

#[gpui::test]
fn empty_compact_sidebar_hides_cross_project_section(cx: &mut gpui::TestAppContext) {
    assert_empty_cross_project_launch(cx, true, false, false);
}

#[gpui::test]
fn populated_expanded_sidebar_hides_cross_project_section(cx: &mut gpui::TestAppContext) {
    assert_empty_cross_project_launch(cx, false, true, false);
}

#[gpui::test]
fn populated_compact_sidebar_hides_cross_project_section(cx: &mut gpui::TestAppContext) {
    assert_empty_cross_project_launch(cx, true, true, false);
}

#[gpui::test]
fn hidden_cross_project_sections_preserve_workspace_session_inventory(
    cx: &mut gpui::TestAppContext,
) {
    use grove_core::session_meta::ContextRoot;
    let paths = ["/grove-multi-alpha", "/grove-multi-beta"];
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store {
            projects: paths
                .iter()
                .enumerate()
                .map(|(index, path)| grove_core::storage::Project {
                    name: ["alpha", "beta"][index].into(),
                    path: (*path).into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect(),
            sidebar_width: Some(300.0),
            ..Default::default()
        };
        store.assign_project_to_active_workspace(paths[0]);
        store.workspaces.create("Other").unwrap();
        store.assign_project_to_active_workspace(paths[1]);
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            // Additional projects do not widen the primary session's workspace.
            registry.insert_meta_with_context(
                "alpha".into(),
                paths[0].into(),
                Agent::Codex,
                vec![ContextRoot {
                    project: "beta".into(),
                    wt_path: paths[1].into(),
                }],
                None,
            );
            // Two roots in one project remain ordinary project sessions.
            registry.insert_meta_with_context(
                "alpha".into(),
                paths[0].into(),
                Agent::Claude,
                vec![ContextRoot {
                    project: "alpha".into(),
                    wt_path: format!("{}/feature", paths[0]),
                }],
                None,
            );
            registry.insert_meta_with_context(
                "beta".into(),
                paths[1].into(),
                Agent::Codex,
                vec![ContextRoot {
                    project: "alpha".into(),
                    wt_path: paths[0].into(),
                }],
                None,
            );
        });
        for (index, path) in paths.iter().enumerate() {
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    index,
                    vec![grove_core::git::Worktree {
                        path: (*path).into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
        }
        Sidebar::new(runtime, window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    assert!(cx.debug_bounds("multi-project-sessions").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.mode = ViewMode::Project;
            assert_eq!(
                sidebar.visible_session_order(cx),
                vec![SessionId::from_raw(2), SessionId::from_raw(1)]
            );
            sidebar.mode = ViewMode::List;
            assert_eq!(
                sidebar.visible_session_order(cx),
                vec![SessionId::from_raw(2), SessionId::from_raw(1)]
            );
            sidebar.mode = ViewMode::Project;
            assert_eq!(sidebar.multi_project_sessions(cx).len(), 1);
            sidebar.toggle_sidebar(window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("multi-project-anchor").is_none());
    cx.update(|_, cx| {
        assert_eq!(sidebar.read(cx).multi_project_sessions(cx).len(), 1);
    });
}

#[gpui::test]
fn workspace_switches_retain_project_and_grid_mode(cx: &mut gpui::TestAppContext) {
    assert_workspace_switches_retain_view_mode(cx, ViewMode::Project);
}

#[gpui::test]
fn workspace_switches_retain_list_and_grid_mode(cx: &mut gpui::TestAppContext) {
    assert_workspace_switches_retain_view_mode(cx, ViewMode::List);
}

#[test]
fn zen_target_uses_selection_or_first_grid_session() {
    let first = SessionId::from_raw(1);
    let second = SessionId::from_raw(2);
    let sessions = [(first, false), (second, true)];
    assert_eq!(zen_target(ViewMode::Project, None, &sessions), None);
    assert_eq!(zen_target(ViewMode::List, None, &sessions), None);
    assert_eq!(
        zen_target(ViewMode::Grid, None, &sessions),
        Some((first, false))
    );
    assert_eq!(
        zen_target(ViewMode::Grid, Some(&Selection::Home(second)), &sessions),
        Some((second, true))
    );
    assert_eq!(zen_target(ViewMode::Grid, None, &[]), None);
}

#[test]
fn pending_reattached_session_is_starting_in_sidebar() {
    assert_eq!(
        managed_session_status(false, true, ActivityState::Idle).0,
        "Starting"
    );
    assert_eq!(
        managed_session_status(true, true, ActivityState::Idle).0,
        "Failed"
    );
    assert_eq!(
        managed_session_status(false, false, ActivityState::Working).0,
        "Working"
    );
}

#[gpui::test]
fn final_home_close_stays_empty_until_explicitly_opened_in_its_workspace(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store::default();
        store.workspaces.create("Other").unwrap();
        cx.set_global(SettingsState::new(store));
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
        Sidebar::new(runtime, window, cx)
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert_eq!(sidebar.active_workspace, 2);
            sidebar.add_terminal(window, cx);
            let Some(Selection::Home(original)) = sidebar.selection else {
                panic!("terminal action should select a shell")
            };
            assert_eq!(sidebar.terminal_owners.get(&original), Some(&2));
            sidebar.act(Action::CloseHome(original), window, cx);
            sidebar.act(Action::ConfirmHome(original), window, cx);
            assert_eq!(
                sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal_count(),
                0
            );
            assert!(!sidebar.terminal_owners.contains_key(&original));
            assert_ne!(sidebar.selection, Some(Selection::Home(original)));
            sidebar.sync(window, cx);
            assert_eq!(
                sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal_count(),
                0
            );
            sidebar.add_terminal(window, cx);
            let Some(Selection::Home(reopened)) = sidebar.selection else {
                panic!("explicit terminal action should select a shell")
            };
            assert_ne!(reopened, original);
            assert_eq!(sidebar.terminal_owners.get(&reopened), Some(&2));
            assert_eq!(
                sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal_count(),
                1
            );
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("home-title-2").is_some());
    let row = cx.debug_bounds("home-2").unwrap();
    let title = cx.debug_bounds("home-title-2").unwrap();
    assert!(title.left() >= row.left() && title.right() <= row.right());
}

#[test]
fn sessions_list_groups_specific_requests_and_stop_reasons() {
    let now = Instant::now();
    let rows: Vec<_> = [
        "Approval needed",
        "Answer needed",
        "Usage limit",
        "Disconnected",
        "Failed",
        "Interrupted",
        "Ready",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, status)| SessionListEntry {
        id: SessionId::from_raw(index as u64 + 1),
        state: ActivityState::Idle,
        status,
        since: Some(now),
        active: None,
    })
    .collect();
    assert_eq!(
        group_sessions_for_list(&rows, now),
        vec![("Needs you", vec![0, 1, 2, 3, 4]), ("Ready", vec![6, 5])]
    );
}

#[test]
fn sessions_list_ties_and_idle_dwell_have_stable_boundaries() {
    let now = Instant::now();
    let rows = [
        SessionListEntry {
            id: SessionId::from_raw(4),
            state: ActivityState::WaitingForInput,
            status: "Needs you",
            since: None,
            active: None,
        },
        SessionListEntry {
            id: SessionId::from_raw(2),
            state: ActivityState::WaitingForInput,
            status: "Needs you",
            since: None,
            active: None,
        },
        SessionListEntry {
            id: SessionId::from_raw(5),
            state: ActivityState::Idle,
            status: "Idle",
            since: None,
            active: Some(
                now.checked_sub(crate::activity::IDLE_DWELL)
                    .expect("fixture clock supports idle dwell")
                    + Duration::from_nanos(1),
            ),
        },
        SessionListEntry {
            id: SessionId::from_raw(7),
            state: ActivityState::Idle,
            status: "Idle",
            since: None,
            active: Some(
                now.checked_sub(crate::activity::IDLE_DWELL)
                    .expect("fixture clock supports idle dwell"),
            ),
        },
        SessionListEntry {
            id: SessionId::from_raw(3),
            state: ActivityState::Working,
            status: "Working",
            since: None,
            active: None,
        },
        SessionListEntry {
            id: SessionId::from_raw(9),
            state: ActivityState::Exited,
            status: "Exited",
            since: None,
            active: Some(now),
        },
    ];
    assert_eq!(
        group_sessions_for_list(&rows, now),
        vec![
            ("Needs you", vec![1, 0]),
            ("Working", vec![4]),
            ("Ready", vec![2, 3, 5]),
        ]
    );
}

#[gpui::test]
fn session_view_cross_project_cards_align_and_keep_shortcut_order(cx: &mut gpui::TestAppContext) {
    assert_session_list_cards(cx, true);
}

#[gpui::test]
fn sessions_list_cards_keep_inset_and_controls_inside_narrow_rail(cx: &mut gpui::TestAppContext) {
    assert_session_list_cards(cx, false);
}

#[gpui::test]
fn palette_session_focus_reaches_new_terminal_in_list_and_grid(cx: &mut gpui::TestAppContext) {
    let path = env!("CARGO_MANIFEST_DIR").to_string();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "current".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            tmux_enabled: Some(false),
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
        sidebar.mode = ViewMode::List;
        sidebar
    });
    draw(cx);

    for mode in [ViewMode::List, ViewMode::Grid] {
        let id = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.mode = mode;
                let prior_focus = cx.focus_handle();
                prior_focus.focus(window, cx);
                let runtime = sidebar.runtime.clone();
                let did_spawn = runtime.update(cx, |runtime, cx| {
                    runtime.spawn_session_in_with_context(
                        "current".into(),
                        path.clone(),
                        Agent::Terminal,
                        vec!["\0".into()],
                        Vec::new(),
                        None,
                        cx,
                    )
                });
                assert!(
                    did_spawn,
                    "the session is registered before asynchronous startup resolves"
                );
                let id = runtime
                    .read(cx)
                    .state
                    .read(cx)
                    .active_session()
                    .expect("failed session is registered");
                sidebar.focus_active_session_after_palette(window, cx);
                assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                assert!(sidebar.focus.is_focused(window));
                assert_eq!(sidebar.pending_canvas_focus, Some(id));
                assert_eq!(
                    sidebar.pending_grid_workspace_focus,
                    (mode == ViewMode::Grid).then_some(id)
                );
                id
            })
        });
        draw(cx);
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            assert!(sidebar.terminal_views[&id]
                .focus_handle(cx)
                .is_focused(window));
        });
        let terminal = sidebar.read_with(cx, |sidebar, cx| {
            sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .session(id)
                .unwrap()
                .clone()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal.read_with(cx, |terminal, _| terminal.spawn_error().is_none()) {
            cx.run_until_parked();
            assert!(
                Instant::now() < deadline,
                "startup failure must arrive without replacing the registered session"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            sidebar.read_with(cx, |sidebar, cx| sidebar
                .runtime
                .read(cx)
                .state
                .read(cx)
                .active_session()),
            Some(id)
        );
    }
}

#[gpui::test]
fn run_script_excluded_from_sidebar_and_normal_canvas_lifecycle(cx: &mut gpui::TestAppContext) {
    struct ScriptRepo(std::path::PathBuf);
    impl Drop for ScriptRepo {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let repo = ScriptRepo(std::env::temp_dir().join(format!(
        "grove-sidebar-script-test-{}-{unique}",
        std::process::id()
    )));
    fs_err::create_dir(&repo.0).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&repo.0)
        .status()
        .unwrap()
        .success());
    let path = repo
        .0
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "script demo".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts {
                    run: Some(" \n ".into()),
                    ..Default::default()
                },
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
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let run_selector: &'static str = Box::leak(format!("run-script-{path}").into_boxed_str());
    assert!(cx.debug_bounds(run_selector).is_none());
    let actions_selector: &'static str =
        Box::leak(format!("worktree-actions-{path}").into_boxed_str());
    let worktree_selector: &'static str = Box::leak(format!("worktree-{path}").into_boxed_str());
    let count_selector: &'static str = Box::leak(format!("worktree-count-{path}").into_boxed_str());
    let actions_right = cx.debug_bounds(actions_selector).unwrap().right();
    assert_eq!(
        f32::from(cx.debug_bounds(actions_selector).unwrap().size.width),
        HIERARCHY_TRAILING_W
    );
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Selection::Project(0), cx);
            sidebar.focus.focus(window, cx);
            let focus = window.focused(cx);
            sidebar.act(Action::RunScript(path.clone(), path.clone()), window, cx);
            sidebar.act(
                Action::RunScript(path.clone(), format!("{path}/missing")),
                window,
                cx,
            );
            assert_eq!(sidebar.selection, Some(Selection::Project(0)));
            assert_eq!(window.focused(cx), focus);
            assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
        });
    });

    cx.update(|window, cx| {
        cx.global_mut::<SettingsState>().store.projects[0]
            .scripts
            .run = Some("\0".into());
        sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
    });
    draw(cx);
    let worktree = cx.debug_bounds(worktree_selector).unwrap();
    cx.simulate_mouse_move(worktree.center(), None, gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds(run_selector).is_none());
    assert!(cx.debug_bounds(actions_selector).unwrap().right() <= actions_right);
    assert!(cx.debug_bounds(count_selector).is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert_eq!(sidebar.selection, Some(Selection::Project(0)));
            assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
            let focus = window.focused(cx);
            sidebar.act(Action::RunScript(path.clone(), path.clone()), window, cx);
            assert_eq!(window.focused(cx), focus);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("canvas-overview").is_some());

    // Invalid script bytes produce no PTY reader. These registered sessions
    // exercise the normal canvas and close path without racing GPUI's test scheduler.
    let (first_id, second_id) = cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let registry = sidebar.runtime.read(cx).registry.clone();
            let mut ids = Vec::new();
            for _ in 0..2 {
                let session = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::spawn_script(
                        "\0", &path, cx,
                    )
                });
                ids.push(registry.update(cx, |registry, cx| {
                    let id =
                        registry.insert_meta("script demo".into(), path.clone(), Agent::Terminal);
                    registry.attach(id, session, None);
                    cx.notify();
                    id
                }));
            }
            sidebar.sync(window, cx);
            let snap = sidebar.snapshot.clone();
            sidebar
                .runtime
                .read(cx)
                .state
                .clone()
                .update(cx, |state, cx| {
                    state.select_session(ids[0], &snap);
                    cx.notify();
                });
            assert_eq!(sidebar.select_active_session(cx), Some(ids[0]));
            (ids[0], ids[1])
        })
    });
    draw(cx);
    let first_header: &'static str =
        Box::leak(format!("terminal-header-{}", first_id.raw()).into_boxed_str());
    assert!(cx.debug_bounds(first_header).is_some());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar.terminal_views[&first_id]
                .focus_handle(cx)
                .contains_focused(window, cx));
            let snap = sidebar.snapshot.clone();
            sidebar
                .runtime
                .read(cx)
                .state
                .clone()
                .update(cx, |state, cx| {
                    state.select_session(second_id, &snap);
                    cx.notify();
                });
            assert_eq!(
                sidebar.selection,
                Some(Selection::Session(first_id)),
                "Hidden source cannot steal canvas focus"
            );
            sidebar.pending_canvas_focus = sidebar.select_active_session(cx);
            assert_eq!(sidebar.selection, Some(Selection::Session(second_id)));
            assert_eq!(sidebar.pending_canvas_focus, Some(second_id));
        });
    });
    draw(cx);
    let second_header: &'static str =
        Box::leak(format!("terminal-header-{}", second_id.raw()).into_boxed_str());
    assert!(cx.debug_bounds(second_header).is_some());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar.terminal_views[&second_id]
                .focus_handle(cx)
                .contains_focused(window, cx));
            sidebar.act(Action::Select(Selection::Session(first_id)), window, cx);
            assert_eq!(sidebar.selection, Some(Selection::Session(first_id)));
            sidebar.act(Action::Close(first_id), window, cx);
            sidebar.act(Action::ConfirmClose(first_id), window, cx);
            assert!(!sidebar.dashboard_open);
            assert_eq!(
                sidebar.destination,
                global_navigation::Destination::Projects
            );
            assert!(sidebar.selection.is_none());
            let registry = sidebar.runtime.read(cx).registry.read(cx);
            assert!(registry.meta(first_id).is_none());
            assert!(registry.meta(second_id).is_some());
            sidebar.act(Action::Select(Selection::Session(second_id)), window, cx);
            assert_eq!(sidebar.selection, Some(Selection::Session(second_id)));
            sidebar.act(Action::Close(second_id), window, cx);
            sidebar.act(Action::ConfirmClose(second_id), window, cx);
            assert!(!sidebar.dashboard_open);
            assert_eq!(
                sidebar.destination,
                global_navigation::Destination::Projects
            );
            assert!(sidebar.selection.is_none());
            assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
        });
    });
}

#[test]
fn sidebar_session_titles_flatten_hard_breaks_without_losing_content() {
    for title in [
        "Image 1 \n\nWhen running something via Claude",
        "Image 1\r\nWhen running something via Claude",
        "Image 1\u{0085}When running something via Claude",
        "Image 1\u{2028}When running something via Claude",
        "Image 1\u{2029}When running something via Claude",
    ] {
        assert_eq!(
            single_line_session_title(title),
            "Image 1 When running something via Claude"
        );
    }
    assert_eq!(
        single_line_session_title("Keep  deliberate spacing"),
        "Keep  deliberate spacing"
    );
}

#[gpui::test]
fn workspace_restoration_lands_on_dashboard_and_retains_cached_terminal(
    cx: &mut gpui::TestAppContext,
) {
    let paths = ["/grove-focus-one", "/grove-focus-two"];
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store {
            projects: paths
                .iter()
                .enumerate()
                .map(|(index, path)| grove_core::storage::Project {
                    name: format!("project-{index}"),
                    path: (*path).into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect(),
            ..Default::default()
        };
        store.workspaces.create("Other").unwrap();
        store.assign_project_to_active_workspace(paths[1]);
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
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
        let registry = runtime.read(cx).registry.clone();
        for (index, path) in paths.iter().enumerate() {
            let session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::attach_existing(
                    "grove-focus-test-not-attached",
                    24,
                    80,
                    cx,
                )
            });
            registry.update(cx, |registry, cx| {
                let id =
                    registry.insert_meta(format!("project-{index}"), (*path).into(), Agent::Codex);
                registry.attach(id, session, None);
                cx.notify();
            });
        }
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            for (index, path) in paths.iter().enumerate() {
                tree.set_active_worktrees(
                    index,
                    vec![grove_core::git::Worktree {
                        path: (*path).into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            }
        });
        let picker =
            cx.new(|cx| crate::views::workspace_manager::WorkspaceManager::new(window, cx));
        let shell_focus = cx.focus_handle();
        shell_focus.focus(window, cx);
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.set_workspace_selector(picker);
        sidebar.set_shell_focus(shell_focus);
        sidebar
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            if let Some(selection) = sidebar.selection.clone() {
                sidebar.act(Action::Select(selection), window, cx);
            }
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        let first = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
        assert_eq!(sidebar.selection, Some(Selection::Session(first)));
        assert!(sidebar.terminal_views[&first]
            .focus_handle(cx)
            .is_focused(window));
    });
    cx.update(|window, cx| {
        let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
        picker.focus_handle(cx).focus(window, cx);
        cx.global_mut::<SettingsState>().store.workspaces.select(2);
    });
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        let second = sidebar.runtime.read(cx).registry.read(cx).all()[1].id;
        assert_eq!(sidebar.selection, Some(Selection::Session(second)));
        assert!(sidebar.dashboard_open);
        assert!(sidebar
            .dashboard
            .as_ref()
            .unwrap()
            .view
            .focus_handle(cx)
            .is_focused(window));
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let second = sidebar.runtime.read(cx).registry.read(cx).all()[1].id;
            sidebar.select_session_id(second, window, cx);
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
        picker.focus_handle(cx).focus(window, cx);
        cx.global_mut::<SettingsState>().store.workspaces.select(1);
    });
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        let first = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
        assert_eq!(sidebar.selection, Some(Selection::Session(first)));
        assert!(sidebar.dashboard_open);
        assert!(sidebar
            .dashboard
            .as_ref()
            .unwrap()
            .view
            .focus_handle(cx)
            .is_focused(window));
        assert!(!sidebar.terminal_views[&first]
            .focus_handle(cx)
            .is_focused(window));
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.select(Selection::Project(0), cx));
        let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
        picker.focus_handle(cx).focus(window, cx);
        cx.global_mut::<SettingsState>().store.workspaces.select(2);
    });
    draw(cx);
    cx.update(|window, cx| {
        let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
        picker.focus_handle(cx).focus(window, cx);
        cx.global_mut::<SettingsState>().store.workspaces.select(1);
    });
    draw(cx);
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
        Some(Selection::Project(0))
    );
}

#[gpui::test]
fn edit_and_manage_scripts_return_to_selected_session_after_close_and_save(
    cx: &mut gpui::TestAppContext,
) {
    let repo = ChangedGitRepo::new();
    let path = repo
        .0
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "return target".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    for provider in [Agent::Codex, Agent::Claude] {
        let id = cx.update(|_, cx| {
            sidebar
                .read(cx)
                .runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, cx| {
                    let id = registry.insert_meta("return target".into(), path.clone(), provider);
                    cx.notify();
                    id
                })
        });
        for scripts in [false, true] {
            for save in [false, true] {
                let panel = cx.update(|window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.mode = ViewMode::List;
                        sidebar.selection = Some(Selection::Session(id));
                        sidebar.act(
                            if scripts {
                                Action::EditProjectScripts(path.clone(), false)
                            } else {
                                Action::EditProject(path.clone())
                            },
                            window,
                            cx,
                        );
                        sidebar.project_panel.clone().expect("project editor")
                    })
                });
                draw(cx);
                if save {
                    panel.update(cx, projects::ProjectPanel::save);
                } else {
                    cx.simulate_keystrokes("escape");
                }
                draw(cx);
                cx.update(|_, cx| {
                    let sidebar = sidebar.read(cx);
                    assert!(
                        sidebar.project_panel.is_none(),
                        "editor closes: provider={provider:?}, scripts={scripts}, save={save}"
                    );
                    assert_eq!(sidebar.mode, ViewMode::List);
                    assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                });
            }
        }
    }
}

#[gpui::test]
fn grid_close_is_attached_to_canvas_and_cancels_to_source(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        let project = grove_core::storage::Project {
            name: "demo".into(),
            path: "/grove-grid-modal-test".into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        };
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![project],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("demo".into(), "/grove-grid-modal-test".into(), Agent::Codex)
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.mode = ViewMode::Grid;
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            if let Some(selection) = sidebar.selection.clone() {
                sidebar.act(Action::Select(selection), window, cx);
            }
        });
    });
    draw(cx);
    let header = cx.debug_bounds("terminal-header-1").unwrap();
    let close = cx.debug_bounds("canvas-close-1").unwrap().center();
    cx.simulate_mouse_down(close, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(close, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert!(
        sidebar.read_with(cx, |sidebar, _| sidebar.canvas_close_anchor
            == Some(SessionId::from_raw(1)))
    );
    let popup = cx.debug_bounds("canvas-confirmation").unwrap();
    assert!((f32::from(popup.left() - header.left())).abs() <= 1.0);
    assert!((f32::from(popup.top() - header.bottom())).abs() <= 1.0);
    let source = sidebar
        .read_with(cx, |sidebar, _| sidebar.confirmation_return_focus.clone())
        .unwrap();
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
    cx.update(|window, _| assert!(source.is_focused(window)));
    assert_eq!(
        sidebar.read_with(cx, |sidebar, cx| sidebar
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .len()),
        1
    );
}

#[test]
fn removing_an_earlier_project_keeps_repository_selection() {
    let mapping = HashMap::from([(1, 0), (2, 1)]);
    let mut selection = Some(Selection::Worktree(2, "/third/feature".into()));
    remap_selection(&mut selection, &mapping);
    assert_eq!(
        selection,
        Some(Selection::Worktree(1, "/third/feature".into()))
    );
    let mut removed = Some(Selection::Project(0));
    remap_selection(&mut removed, &mapping);
    assert_eq!(removed, None);
}

#[test]
fn session_identity_survives_project_reindexing() {
    let mut selection = Some(Selection::Session(SessionId::from_raw(7)));
    remap_selection(&mut selection, &HashMap::new());
    assert_eq!(selection, Some(Selection::Session(SessionId::from_raw(7))));
}
