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
fn project_title_has_descender_room_and_header_controls_align_at_zoom(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: ["grove", "alpha", "beta"]
                .into_iter()
                .map(|name| grove_core::storage::Project {
                    name: name.into(),
                    path: format!("/grove-project-line-height-{name}"),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect(),
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (_, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    for zoom in [0.6, 1.0, 1.5, 2.0] {
        cx.update(|window, _| window.set_rem_size(gpui::px(16.0 * zoom)));
        draw(cx);
        let row = cx.debug_bounds("project-0").unwrap();
        let title = cx.debug_bounds("project-title-0").unwrap();
        assert!(
            f32::from(title.size.height) >= PROJECT_TITLE_LINE_H * zoom - 1.0,
            "project title line is too short at {zoom}x"
        );
        assert!(title.top() >= row.top() && title.bottom() <= row.bottom());
        assert!((f32::from(row.size.height) - PROJECT_ROW_H * zoom).abs() <= 1.0);
        let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
        let count = cx.debug_bounds("projects-count").unwrap();
        assert!(
            (f32::from(count.left() - heading.right()) - SPACE_SM * zoom).abs() <= 1.0,
            "project count is not adjacent to its title at {zoom}x"
        );
        let project_icon = cx.debug_bounds("project-folder-toggle-0").unwrap();
        assert_eq!(heading.left(), project_icon.left());
        let project_action = cx.debug_bounds("project-menu-0").unwrap();
        let add = cx.debug_bounds("projects-add").unwrap();
        assert_eq!(project_action.right(), add.right());
        assert!(cx.debug_bounds("sidebar-terminals-heading").is_none());
        let centers = ["sidebar-view", "sidebar-grid", "projects-add"]
            .map(|id| f32::from(cx.debug_bounds(id).unwrap().center().x));
        for pair in centers.windows(2) {
            assert!(
                (pair[1] - pair[0] - (CHROME_CONTROL_H + SPACE_SM) * zoom).abs() <= 1.0,
                "header spacing differs at {zoom}x: {centers:?}"
            );
        }
        for id in ["sidebar-view", "sidebar-grid", "projects-add"] {
            assert!(
                (f32::from(cx.debug_bounds(id).unwrap().size.width) - CHROME_CONTROL_H * zoom)
                    .abs()
                    <= 1.0,
                "{id} has a different slot width at {zoom}x"
            );
        }
    }
}

#[gpui::test]
fn project_heading_fits_full_text_at_zoom(cx: &mut gpui::TestAppContext) {
    struct Fixture(Entity<Sidebar>);
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .font_family(crate::fonts::UI_FAMILY)
                .child(self.0.update(cx, |sidebar, cx| {
                    sidebar.mode = ViewMode::Project;
                    sidebar.sync(window, cx);
                    div()
                        .w(rpx(sidebar.context_width(window, cx)))
                        .child(sidebar.project_tree(None, window, cx))
                }))
        }
    }
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: (0..4)
                .map(|index| grove_core::storage::Project {
                    name: format!("project-{index}"),
                    path: format!("/grove-heading-{index}"),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect(),
            sidebar_width: Some(300.0),
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Fixture(cx.new(|cx| Sidebar::new(runtime, window, cx)))
    });
    for rail_width in [260.0, 280.0, 300.0] {
        cx.update(|_, cx| {
            cx.global_mut::<SettingsState>().store.sidebar_width = Some(rail_width);
        });
        for zoom in [0.9, 1.0, 1.1, 1.2, 1.5, 2.0] {
            cx.update(|window, _| window.set_rem_size(gpui::px(16.0 * zoom)));
            draw(cx);
            let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
            let width = cx.update(|window, _| {
                let text: SharedString = "Projects".into();
                let mut run = window.text_style().to_run(text.len());
                run.font.weight = gpui::FontWeight::MEDIUM;
                run.font.family = crate::fonts::UI_FAMILY.into();
                let font_id = window.text_system().resolve_font(&run.font);
                let glyph_width: gpui::Pixels = text
                    .chars()
                    .map(|ch| {
                        window
                            .text_system()
                            .layout_width(font_id, gpui::px(TEXT_SMALL * zoom), ch)
                    })
                    .sum();
                let shaped_width = window
                    .text_system()
                    .shape_line(text, gpui::px(TEXT_SMALL * zoom), &[run], None)
                    .width();
                shaped_width.max(glyph_width.floor())
            });
            assert!(
                heading.size.width >= width,
                "heading truncated at {zoom}x: {:?}, full text width {width:?}",
                heading.size.width
            );
        }
    }
    // A genuinely narrow viewport must still shrink the label, keeping the
    // count and controls visible instead of forcing the word to overflow.
    cx.update(|window, _| window.set_rem_size(gpui::px(16.0)));
    draw(cx);
    let full_width = cx.debug_bounds("sidebar-heading-label").unwrap().size.width;
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
    draw(cx);
    let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
    let count = cx.debug_bounds("projects-count").unwrap();
    let control = cx.debug_bounds("sidebar-view").unwrap();
    assert!(heading.size.width < full_width);
    assert!(heading.right() < count.left());
    assert!((f32::from(count.left() - heading.right()) - SPACE_SM).abs() <= 1.0);
    assert!(count.right() <= control.left());
    // At the minimum supported width, fixed metadata and actions may wrap
    // while every control keeps its complete hit target.
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(800.0)));
    draw(cx);
    let count = cx.debug_bounds("projects-count").unwrap();
    let header = cx.debug_bounds("sidebar-navigation-header").unwrap();
    for selector in ["sidebar-view", "sidebar-grid", "projects-add"] {
        let action = cx.debug_bounds(selector).unwrap();
        assert!(count.right() <= action.left() || count.bottom() <= action.top());
        assert!(action.left() >= header.left() && action.right() <= header.right());
        assert_eq!(action.size.width, gpui::px(CHROME_CONTROL_H));
    }
}

#[gpui::test]
fn project_title_toggles_without_selecting_or_preserving_descendants(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        let projects = ["one", "two"]
            .into_iter()
            .map(|name| grove_core::storage::Project {
                name: name.into(),
                path: format!("/grove-project-select-test-{name}"),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            })
            .collect();
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects,
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        registry.update(cx, |registry, _| {
            registry.insert_meta(
                "one".into(),
                "/grove-project-select-test-one".into(),
                Agent::Codex,
            );
        });
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let first = cx.debug_bounds("project-0").unwrap();
    let second = cx.debug_bounds("project-1").unwrap();
    assert!(f32::from(second.top() - first.bottom()) >= SPACE_20);
    assert!(cx.debug_bounds("tree-expand-cycle").is_none());
    assert!(cx.debug_bounds("projects-count").is_some());
    assert!(cx.debug_bounds("project-count-0").is_none());
    assert!(cx.debug_bounds("project-count-1").is_none());
    assert!(cx.debug_bounds("session-1").is_some());
    let initial_selection = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
    let title = cx.debug_bounds("project-title-0").unwrap().center();
    cx.simulate_click(title, gpui::Modifiers::default());
    draw(cx);
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
        initial_selection
    );
    assert!(cx.debug_bounds("session-1").is_none());
    assert!(cx.debug_bounds("project-1").is_some());
}

#[gpui::test]
fn project_folder_toggle_hides_only_its_descendants_and_preserves_selection(
    cx: &mut gpui::TestAppContext,
) {
    let paths = ["/grove-folder-toggle-one", "/grove-folder-toggle-two"];
    cx.update(|cx| {
        gpui_component::init(cx);
        let projects = paths
            .iter()
            .enumerate()
            .map(|(index, path)| grove_core::storage::Project {
                name: format!("project-{index}"),
                path: (*path).into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            })
            .collect();
        let mut store = grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects,
            ..Default::default()
        };
        store.workspaces.create("Other").unwrap();
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            for (index, path) in paths.iter().enumerate() {
                registry.insert_meta(format!("project-{index}"), (*path).into(), Agent::Codex);
            }
        });
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(
                Action::Select(Selection::Session(SessionId::from_raw(1))),
                window,
                cx,
            );
        });
    });
    draw(cx);
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-one")
        .is_some());
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-two")
        .is_some());
    assert!(cx.debug_bounds("session-1").is_some());
    assert!(cx.debug_bounds("session-2").is_some());
    let initial = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
    let folder = cx.debug_bounds("project-folder-toggle-0").unwrap().center();
    cx.simulate_click(folder, gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, cx| {
        assert!(sidebar.collapsed_projects.contains(paths[0]));
        assert_eq!(sidebar.selection, initial);
        assert_eq!(
            sidebar.visible_session_order(cx),
            vec![SessionId::from_raw(2), SessionId::from_raw(1)]
        );
        assert!(sidebar.menu.is_none());
    });
    assert!(cx.debug_bounds("project-0").is_some());
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-one")
        .is_none());
    assert!(cx.debug_bounds("session-1").is_none());
    assert!(cx.debug_bounds("terminal-header-1").is_some());
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-two")
        .is_some());
    assert!(cx.debug_bounds("session-2").is_some());
    let title = cx.debug_bounds("project-title-0").unwrap().center();
    cx.simulate_click(title, gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.selection, initial);
        assert!(!sidebar.collapsed_projects.contains(paths[0]));
    });
    cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(2));
    draw(cx);
    assert!(cx.debug_bounds("project-0").is_none());
    cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(1));
    draw(cx);
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-one")
        .is_some());
    cx.update(|window, cx| {
        let handle = sidebar.read(cx).project_toggle_focus[paths[0]].clone();
        handle.focus(window, cx);
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-one")
        .is_none());
    assert!(cx.debug_bounds("session-1").is_none());
    assert!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.is_none()),
        "disclosure preserves the unselected Projects landing after workspace return"
    );
    cx.simulate_keystrokes("space");
    draw(cx);
    assert!(cx
        .debug_bounds("worktree-/grove-folder-toggle-one")
        .is_some());
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, _| sidebar.selection = None);
        cx.global_mut::<SettingsState>().store.workspaces.select(2);
    });
    draw(cx);
    cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(1));
    draw(cx);
    assert!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.is_none()),
        "configured Projects landing is unselected after a workspace switch"
    );
    cx.update(|window, cx| {
        cx.global_mut::<SettingsState>().store.projects.remove(0);
        sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
    });
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.collapsed_projects.is_empty()));
}

#[gpui::test]
fn project_menu_click_preserves_selection_and_rows(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "one".into(),
                path: "/grove-project-menu-test".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let before = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
    let menu = cx.debug_bounds("project-menu-0").unwrap().center();
    cx.simulate_click(menu, gpui::Modifiers::default());
    draw(cx);
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
        before
    );
    assert!(cx.debug_bounds("project-0").is_some());
    assert!(cx.debug_bounds("project-actions-popup").is_some());
}

#[gpui::test]
fn project_hierarchy_spaces_worktree_groups_more_than_sibling_sessions(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-spacing-main".into(),
                scripts: grove_core::storage::ProjectScripts {
                    run: Some("echo fixture".into()),
                    ..Default::default()
                },
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        registry.update(cx, |registry, _| {
                registry.insert_reattached(0, &grove_core::tmux::DiscoveredSession {
                    name: "grove-spacing-fixture".into(),
                    pane_title: Some("Let's show Image 1 the diff stats too\nImage 1\nWhen running something via Claude".into()),
                    wt_path: "/grove-spacing-main".into(),
                    project: "demo".into(),
                    label: "codex 1".into(),
                    agent: Agent::Codex,
                    context_roots: Vec::new(),
                    temp_bundle_path: None,
                });
                registry.insert_meta("demo".into(), "/grove-spacing-main".into(), Agent::Claude);
                registry.insert_meta(
                    "demo".into(),
                    "/grove-spacing-feature".into(),
                    Agent::Terminal,
                );
            });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![
                    grove_core::git::Worktree {
                        path: "/grove-spacing-main".into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    },
                    grove_core::git::Worktree {
                        path: "/grove-spacing-feature".into(),
                        branch: "feature".into(),
                        mtime: None,
                        is_main: false,
                    },
                ],
            );
            tree.apply_git_poll(
                HashMap::from([(
                    "/grove-spacing-main".into(),
                    grove_core::git::WorktreeGitState {
                        dirty: true,
                        added: 42,
                        removed: 12,
                        ..Default::default()
                    },
                )]),
                &[],
            );
        });
        let (handle, _actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider: grove_core::codex::Provider::Codex,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "sidebar-progress-fixture".into(),
            project: "demo".into(),
            cwd: "/grove-spacing-main".into(),
            label: "Codex".into(),
            title: Some(
                "Let's show Image 1 the diff stats too\nImage 1\nWhen running something via Claude"
                    .into(),
            ),
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("fixture-thread".into()),
            items: vec![serde_json::json!({
                "type": "plan",
                "plan": [
                    {"step": "Implement", "status": "completed"},
                    {"step": "Verify", "status": "in_progress"}
                ]
            })]
            .into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let state = grove_core::codex::Snapshot::new(saved);
        let connection = grove_core::codex::Connection {
            handle: handle.clone(),
            updates: receive.into(),
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(state, connection, cx)
        });
        registry.update(cx, |registry, _| {
            registry.attach_codex(SessionId::from_raw(1), session, handle);
        });
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    // Finish the nonexistent-path poll before injecting the deterministic diff fixture.
    cx.update(|_, cx| {
        sidebar
            .read(cx)
            .runtime
            .read(cx)
            .tree
            .clone()
            .update(cx, |tree, _| {
                tree.apply_git_poll(
                    HashMap::from([(
                        "/grove-spacing-main".into(),
                        grove_core::git::WorktreeGitState {
                            dirty: true,
                            added: 42,
                            removed: 12,
                            ..Default::default()
                        },
                    )]),
                    &[],
                );
            });
    });
    draw(cx);
    let labels = sidebar.read_with(cx, |sidebar, _| {
        sidebar.snapshot.projects[0]
            .worktrees
            .iter()
            .map(|worktree| sidebar_worktree_name(&worktree.name, worktree.is_main).to_owned())
            .collect::<Vec<_>>()
    });
    assert_eq!(labels, ["Main checkout", "grove-spacing-feature"]);
    assert!(cx
        .debug_bounds("worktree-title-/grove-spacing-main")
        .is_some());
    assert!(cx
        .debug_bounds("worktree-title-/grove-spacing-feature")
        .is_some());
    let first = cx.debug_bounds("session-1").unwrap();
    let second = cx.debug_bounds("session-2").unwrap();
    let next_worktree = cx.debug_bounds("worktree-/grove-spacing-feature").unwrap();
    assert_eq!(second.top(), first.bottom());
    assert_eq!(
        f32::from(next_worktree.top() - second.bottom()),
        WORKTREE_GROUP_GAP
    );
    let first_worktree = cx.debug_bounds("worktree-/grove-spacing-main").unwrap();
    assert_eq!(
        f32::from(first.top() - first_worktree.bottom()),
        SESSION_FIRST_GAP
    );
    for width in [220.0, 260.0, 320.0] {
        cx.simulate_mouse_move(
            gpui::point(gpui::px(900.0), gpui::px(740.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>().store.sidebar_width = Some(width);
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        draw(cx);
        let launcher = cx.debug_bounds("session-agent-1").unwrap();
        let title = cx.debug_bounds("session-tree-title-1").unwrap();
        assert!(cx.debug_bounds("session-tree-metadata-1").is_none());
        assert!(cx.debug_bounds("diff-chip-open-1").is_none());
        let diff = cx
            .debug_bounds("worktree-diff-/grove-spacing-main")
            .unwrap();
        let worktree = cx
            .debug_bounds("worktree-title-/grove-spacing-main")
            .unwrap();
        let trailing = cx
            .debug_bounds("worktree-actions-/grove-spacing-main")
            .unwrap();
        assert!(worktree.right() <= diff.left());
        assert!(diff.left() >= trailing.left());
        assert_eq!(diff.right(), trailing.right());
        let status = cx.debug_bounds("session-tree-status-1").unwrap();
        let close = cx.debug_bounds("session-tree-close-1").unwrap();
        let project_title = cx.debug_bounds("project-title-0").unwrap();
        let worktree_title = cx
            .debug_bounds("worktree-title-/grove-spacing-main")
            .unwrap();
        assert_eq!(
            worktree_title.left() - project_title.left(),
            gpui::px(SPACE_2XL)
        );
        let row = cx.debug_bounds("session-1").unwrap();
        assert_eq!(f32::from(row.size.height), SESSION_TREE_ROW_H);
        assert!(title.right() <= status.left());
        assert!(launcher.right() <= title.left());
        let progress = cx.debug_bounds("session-tasks-1").unwrap();
        let trailing = cx.debug_bounds("session-tree-trailing-1").unwrap();
        assert!(title.right() <= progress.left());
        assert!(progress.right() <= close.left());
        assert_eq!(f32::from(row.right() - close.right()), HIERARCHY_INSET);
        assert_eq!(close.right(), trailing.right());
        assert_eq!(
            f32::from(launcher.left() - row.left()),
            SESSION_LEADING_INSET
        );
        // Row hover swaps status for Close without moving title or task progress.
        cx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(cx.debug_bounds("session-tree-title-1").unwrap(), title);
        assert_eq!(cx.debug_bounds("session-tree-close-1").unwrap(), close);
        assert!(cx.debug_bounds("session-tree-status-1").is_none());
        assert_eq!(cx.debug_bounds("session-tasks-1").unwrap(), progress);
        cx.simulate_mouse_move(progress.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(cx.debug_bounds("session-tasks-1").unwrap(), progress);
        assert_eq!(cx.debug_bounds("session-tree-close-1").unwrap(), close);
        cx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(cx.debug_bounds("session-tasks-1").unwrap(), progress);
        let empty_slot = cx.debug_bounds("session-tree-trailing-2").unwrap();
        assert_eq!(cx.debug_bounds("session-tree-close-2").unwrap(), empty_slot);
        assert_eq!(f32::from(empty_slot.size.width), CONTROL_H);
        assert!(status.top() >= row.top() && status.bottom() <= row.bottom());
        assert!(title.top() >= row.top() && title.bottom() <= row.bottom());
        assert!(close.bottom() <= row.bottom());
        cx.simulate_mouse_move(
            gpui::point(gpui::px(900.0), gpui::px(740.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.session_close_focus[&SessionId::from_raw(1)].focus(window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("session-tree-status-1").is_none());
        assert_eq!(cx.debug_bounds("session-tree-title-1").unwrap(), title);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.focus.focus(window, cx));
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("session-tree-status-1").unwrap(), status);
        // The longest launcher row includes all agents and Delete; dense controls
        // retain their shared size without covering the truncated worktree title at narrow widths.
        for path in ["/grove-spacing-main", "/grove-spacing-feature"] {
            let selector = |prefix: &str| -> &'static str {
                Box::leak(format!("{prefix}{path}").into_boxed_str())
            };
            let worktree = cx.debug_bounds(selector("worktree-")).unwrap();
            cx.simulate_mouse_move(worktree.center(), None, gpui::Modifiers::default());
            draw(cx);
            let actions = cx.debug_bounds(selector("worktree-actions-")).unwrap();
            let worktree_title = cx.debug_bounds(selector("worktree-title-")).unwrap();
            assert!(
                worktree_title.right() <= actions.left(),
                "worktree title overlaps controls at {width}px"
            );
            let mut controls: Vec<&'static str> = (0..WORKTREE_LAUNCH_AGENTS.len())
                .map(|index| -> &'static str {
                    Box::leak(format!("launch-{path}-{index}").into_boxed_str())
                })
                .collect();
            assert!(cx.debug_bounds(selector("run-script-")).is_none());
            if path.ends_with("feature") {
                controls.push(selector("delete-worktree-"));
            }
            for control in controls {
                let bounds = cx.debug_bounds(control).unwrap();
                assert_eq!(
                    bounds.size,
                    gpui::size(gpui::px(CHROME_CONTROL_H), gpui::px(CHROME_CONTROL_H))
                );
                assert!(
                    bounds.left() >= worktree.left() && bounds.right() <= worktree.right(),
                    "{control} clips at {width}px"
                );
                assert!(bounds.top() >= worktree.top() && bounds.bottom() <= worktree.bottom());
            }
        }
    }
    assert!(cx.debug_bounds("session-3").is_some());
    assert!(cx.debug_bounds("fold-/grove-spacing-feature").is_none());
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
fn project_hierarchy_trailing_actions_align_with_blank_worktree_slot(
    cx: &mut gpui::TestAppContext,
) {
    let path = "/grove-sidebar-trailing-alignment";
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "alignment".into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (_, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let centers = [
        "project-menu-glyph-0",
        "worktree-actions-/grove-sidebar-trailing-alignment",
    ]
    .map(|selector| f32::from(cx.debug_bounds(selector).unwrap().center().x));
    assert!(cx
        .debug_bounds("worktree-/grove-sidebar-trailing-alignment")
        .is_some());
    assert!(cx.debug_bounds("worktrees-count-0").is_none());
    assert!(cx
        .debug_bounds("worktree-count-/grove-sidebar-trailing-alignment")
        .is_none());
    assert!(
        centers
            .iter()
            .all(|center| (center - centers[0]).abs() <= 1.0),
        "{centers:?}"
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
fn project_menu_opens_on_hover_and_closes_after_pointer_leaves(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "one".into(),
                path: "/grove-sidebar-hover-test".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let trigger = cx.debug_bounds("project-menu-0").unwrap();
    cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu == Some(0)));
    assert!(
        sidebar.read_with(cx, |sidebar, _| !sidebar.html_overlay_open()),
        "a project menu must not hide the entire native document"
    );
    assert!(
        cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx))
            .is_some(),
        "the menu reserves its pixels and pointer region above native HTML"
    );
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, _| {
            sidebar.project_decision = true;
            assert!(
                sidebar.html_overlay_open(),
                "blocking confirmations still hide native HTML"
            );
            sidebar.project_decision = false;
        });
    });
    let popup = cx.debug_bounds("project-actions-popup").unwrap();
    cx.simulate_mouse_move(popup.center(), None, gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu == Some(0)));
    cx.simulate_mouse_move(
        gpui::point(gpui::px(1.0), gpui::px(1.0)),
        None,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
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
fn project_popup_preserves_rows_and_restores_focus_on_dismiss(cx: &mut gpui::TestAppContext) {
    let repo = ChangedGitRepo::new();
    let one_path = repo
        .0
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        let projects = ["one", "two"]
            .into_iter()
            .map(|name| grove_core::storage::Project {
                name: name.into(),
                path: if name == "one" {
                    one_path.clone()
                } else {
                    format!("/grove-sidebar-test-{name}")
                },
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            })
            .collect();
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects,
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::EditProject("/missing-project".into()), window, cx);
            sidebar.act(Action::RemoveProject("/missing-project".into()), window, cx);
            assert!(sidebar.project_panel.is_none());
            sidebar.act(Action::EditProject(one_path.clone()), window, cx);
            assert!(sidebar.project_panel.is_some());
            cx.global_mut::<SettingsState>()
                .store
                .workspaces
                .create("Other")
                .unwrap();
            sidebar.sync(window, cx);
            assert!(sidebar.project_panel.is_none());
            assert!(sidebar.project_return_focus.is_none());
            assert!(!sidebar.dashboard_open);
            assert_eq!(
                sidebar.destination,
                global_navigation::Destination::Projects
            );
            assert!(sidebar.selection.is_none());
            assert_eq!(window.focused(cx), Some(sidebar.focus.clone()));
            sidebar.act(Action::EditProject(one_path.clone()), window, cx);
            assert!(
                sidebar.project_panel.is_none(),
                "cross-workspace action is stale"
            );
            sidebar.finish_project_selection(&one_path, window, cx);
            assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
            assert_eq!(
                window.focused(cx),
                sidebar.project_menu_focus.get(&0).cloned()
            );
            assert!(sidebar.project_return_focus.is_none());
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("projects-add").is_some());
    assert!(cx.debug_bounds("projects-archive").is_none());
    let selector = |prefix: &str| -> &'static str {
        Box::leak(format!("{prefix}{one_path}").into_boxed_str())
    };
    let project_title = cx.debug_bounds("project-title-0").unwrap();
    let worktree_title = cx.debug_bounds(selector("worktree-title-")).unwrap();
    let empty = cx.debug_bounds(selector("worktree-empty-")).unwrap();
    assert_eq!(
        worktree_title.left() - project_title.left(),
        gpui::px(SPACE_2XL)
    );
    assert!(empty.left() > worktree_title.left());
    assert!(cx.debug_bounds("projects-count").is_some());
    assert!(cx.debug_bounds(selector("worktree-count-")).is_none());
    assert!(cx.debug_bounds("tree-expand-cycle").is_none());
    assert!(cx.debug_bounds(selector("fold-")).is_none());
    let worktree_actions = cx.debug_bounds(selector("worktree-actions-")).unwrap();
    assert_eq!(f32::from(worktree_actions.size.width), HIERARCHY_TRAILING_W);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let registry = sidebar.runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                registry.insert_meta("one".into(), one_path.clone(), Agent::Codex);
                cx.notify();
            });
            sidebar.act(Action::Select(Selection::Project(0)), window, cx);
        });
    });
    draw(cx);
    assert_eq!(
        cx.debug_bounds("project-title-0").unwrap().left(),
        project_title.left()
    );
    assert!(cx.debug_bounds("session-1").is_some());
    let agent = cx.debug_bounds("session-agent-1").unwrap();
    let status = cx.debug_bounds("session-tree-status-1").unwrap();
    assert_eq!(
        empty.left(),
        cx.debug_bounds("session-tree-title-1").unwrap().left()
    );
    assert!(cx.debug_bounds("session-tree-title-1").unwrap().right() <= status.left());
    assert!(agent.right() <= cx.debug_bounds("session-tree-title-1").unwrap().left());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(
                Action::Select(Selection::Worktree(0, one_path.clone())),
                window,
                cx,
            );
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("session-1").is_some());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_some()));
    assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.project_decision));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_none()));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::ArchivedProjects, window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("project-panel").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::AddProject, window, cx);
        });
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_setup.is_some()));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_setup.is_none()));
    let before = cx.debug_bounds("project-1").unwrap();
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
    });
    draw(cx);
    let popup = cx.debug_bounds("project-actions-popup").unwrap();
    let trigger = cx.debug_bounds("project-menu-0").unwrap();
    assert!(
        (f32::from(popup.left() - trigger.right())).abs() <= 1.0,
        "popup {popup:?} must attach to trigger {trigger:?}"
    );
    assert!(
        (f32::from(popup.top() - trigger.top())).abs() <= 1.0,
        "popup must align vertically to its trigger"
    );
    assert_eq!(cx.debug_bounds("project-1").unwrap(), before);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
    cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
    });
    draw(cx);
    cx.simulate_mouse_down(
        gpui::point(gpui::px(1.0), gpui::px(1.0)),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
    cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
    });
    draw(cx);
    // Remove remains last while readiness-dependent worktree actions refresh.
    cx.simulate_keystrokes("up enter");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
    for _ in 0..5 {
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            let panel = sidebar.read(cx).project_panel.as_ref().unwrap();
            assert!(panel.focus_handle(cx).contains_focused(window, cx));
        });
    }
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
    cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
    draw(cx);
    let rail = cx.debug_bounds("sidebar-rail").unwrap();
    let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
    assert!((f32::from(rail.size.width) - 128.0).abs() <= 1.0);
    let navigation_width = cx.debug_bounds("sidebar-rail").unwrap().size.width;
    assert!((canvas.size.width - (gpui::px(320.0) - navigation_width)).abs() <= gpui::px(1.0));
    assert!(canvas.left() >= rail.right());
    let project = cx.debug_bounds("project-0").unwrap();
    assert!(project.right() <= rail.right());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
    });
    draw(cx);
    let popup = cx.debug_bounds("project-actions-popup").unwrap();
    assert!(
        popup.left() >= gpui::px(0.0) && popup.right() <= gpui::px(320.0),
        "popup: {popup:?}; viewport: {:?}",
        cx.update(|window, _| window.viewport_size())
    );
    assert!(popup.top() >= gpui::px(0.0) && popup.bottom() <= gpui::px(200.0));
    cx.simulate_keystrokes("down down enter");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_new_worktree == Some(0)));
    assert!(
        cx.debug_bounds("project-0").is_none(),
        "narrow editor must receive full canvas"
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_new_worktree.is_none()));
    assert!(cx.debug_bounds("project-0").is_some());
    cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
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
