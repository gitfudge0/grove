use super::*;

#[gpui::test]
fn consumed_navigation_click_cannot_select_transcript_after_release(cx: &mut gpui::TestAppContext) {
    let cx = navigation_selection_fixture(cx);
    let navigation = cx.debug_bounds("consumed-navigation").unwrap();
    let transcript = cx.debug_bounds("selection-transcript").unwrap();
    cx.simulate_click(navigation.center(), gpui::Modifiers::default());
    cx.simulate_mouse_move(
        transcript.origin + gpui::point(gpui::px(100.), gpui::px(8.)),
        None,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert_eq!(
        cx.update(gpui_component::WindowExt::selected_text),
        "",
        "released navigation click must not select transcript"
    );
}

#[test]
fn numbered_tree_order_includes_every_group_and_navigation_wraps() {
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    let ids: Vec<_> = (1..=4).map(SessionId::from_raw).collect();
    let snapshot = TreeSnapshot {
        projects: vec![
            SnapshotProject {
                idx: 0,
                worktrees: vec![
                    SnapshotWorktree {
                        path: "/a/main".into(),
                        sessions: ids[..2].to_vec(),
                        ..Default::default()
                    },
                    SnapshotWorktree {
                        path: "/a/branch".into(),
                        sessions: vec![ids[2]],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            SnapshotProject {
                idx: 1,
                worktrees: vec![SnapshotWorktree {
                    path: "/b/main".into(),
                    sessions: vec![ids[3]],
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let order = visible_tree_sessions(&snapshot);
    assert_eq!(order, ids);
    assert_eq!(step_session(&order, Some(ids[3]), true), Some(ids[0]));
    assert_eq!(step_session(&order, Some(ids[0]), false), Some(ids[3]));
    assert_eq!(step_session(&order, Some(ids[2]), true), Some(ids[3]));
    assert_eq!(step_session(&order, None, false), Some(ids[3]));
}

#[gpui::test]
fn sidebar_commands_follow_group_order_and_keep_process_identity(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-sidebar-navigation-test".into(),
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
        let registry = runtime.read(cx).registry.clone();
        let ids = registry.update(cx, |registry, _| {
            (0..3)
                .map(|_| {
                    registry.insert_meta(
                        "demo".into(),
                        "/grove-sidebar-navigation-test".into(),
                        Agent::Codex,
                    )
                })
                .collect::<Vec<_>>()
        });
        let tree = runtime.read(cx).tree.clone();
        tree.update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: "/grove-sidebar-navigation-test".into(),
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let activity = runtime.read(cx).activity.clone();
        activity.update(cx, |activity, _| {
            activity.set_state_for_test(ids[0], ActivityState::Idle);
            activity.set_state_for_test(ids[1], ActivityState::Done);
            activity.set_state_for_test(ids[2], ActivityState::WaitingForInput);
        });
        Sidebar::new(runtime, window, cx)
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            let ids: Vec<_> = sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .all()
                .iter()
                .map(|meta| meta.id)
                .collect();
            sidebar.toggle_tree_list(window, cx);
            assert_eq!(
                sidebar.visible_session_order(cx),
                vec![ids[2], ids[1], ids[0]]
            );
            sidebar.select_numbered_session(1, window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            sidebar.select_next_session(window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[1]));
            sidebar.select_waiting_session(window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            sidebar.select_numbered_session(9, window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            sidebar.toggle_grid(window, cx);
            sidebar.toggle_grid(window, cx);
            assert_eq!(sidebar.mode, ViewMode::List);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            sidebar.toggle_tree_list(window, cx);
            assert_eq!(sidebar.mode, ViewMode::Project);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            assert_eq!(
                sidebar.selected_worktree(),
                Some((0, "/grove-sidebar-navigation-test".into()))
            );
            SettingsState::update(cx, |store| store.sidebar_group_by_worktree = true);
            assert_eq!(sidebar.visible_session_order(cx), ids);
            sidebar.select(Selection::Session(ids[0]), cx);
            sidebar.select_waiting_session(window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[2]));
            sidebar.select_session_id(ids[0], window, cx);
            assert_eq!(sidebar.selected_session(), Some(ids[0]));
            assert!(sidebar.session_diff_focus.contains_key(&ids[2]));
            assert!(sidebar.session_diff_observers.contains_key(&ids[2]));
            sidebar.toggle_tree_list(window, cx);
            let registry = sidebar.runtime.read(cx).registry.clone();
            registry.update(cx, |registry, _| {
                registry.remove(ids[2]);
            });
            sidebar.sync(window, cx);
            assert!(!sidebar.session_diff_focus.contains_key(&ids[2]));
            assert!(!sidebar.session_diff_observers.contains_key(&ids[2]));
            sidebar.select_session_id(ids[2], window, cx);
            assert_ne!(sidebar.selected_session(), Some(ids[2]));
        });
    });
}

#[gpui::test]
fn nonfinal_home_close_keeps_remaining_owner_and_selection(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
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
            sidebar.add_terminal(window, cx);
            let Some(Selection::Home(first)) = sidebar.selection else {
                panic!("first shell")
            };
            sidebar.add_terminal(window, cx);
            let Some(Selection::Home(second)) = sidebar.selection else {
                panic!("second shell")
            };
            sidebar.act(Action::CloseHome(first), window, cx);
            sidebar.act(Action::ConfirmHome(first), window, cx);
            let registry = sidebar.runtime.read(cx).registry.read(cx);
            assert_eq!(registry.home_terminal_count(), 1);
            assert_eq!(registry.home_terminals()[0].id, second);
            assert_eq!(sidebar.terminal_owners.get(&second), Some(&1));
            assert_eq!(sidebar.selection, Some(Selection::Home(second)));
        });
    });
}

#[gpui::test]
fn update_header_requires_installable_release_and_hides_when_collapsed(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (fixture, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        WorkspaceHeaderFixture::new(sidebar, cx)
    });
    let sidebar = fixture.read_with(cx, |fixture, _| fixture.sidebar.clone());
    draw(cx);
    assert!(cx.debug_bounds("sidebar-update").is_none());
    let mut release = grove_core::upgrade::Release {
        version: semver::Version::new(99, 0, 0),
        tag: "v99.0.0".into(),
        html_url: "https://github.com/gitfudge0/grove/releases/tag/v99.0.0".into(),
        body: String::new(),
        dmg_url: None,
        dmg_sha256_url: None,
        target_commitish: "main".into(),
    };
    let upgrade = sidebar.read_with(cx, |sidebar, cx| sidebar.runtime.read(cx).upgrade.clone());
    upgrade.update(cx, |upgrade, cx| {
        upgrade.offer_for_test(grove_core::upgrade::InstallMethod::Dmg, release.clone(), cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("sidebar-update").is_none());
    release.dmg_url =
        Some("https://github.com/gitfudge0/grove/releases/download/v99.0.0/Grove.dmg".into());
    upgrade.update(cx, |upgrade, cx| {
        upgrade.offer_for_test(grove_core::upgrade::InstallMethod::Dmg, release, cx);
    });
    assert!(upgrade.read_with(cx, |upgrade, _| upgrade.can_update()));
    fixture.update(cx, |_, cx| cx.notify());
    draw(cx);
    let update = cx.debug_bounds("sidebar-update").unwrap();
    let header = cx.debug_bounds("global-navigation").unwrap();
    assert!(update.left() >= header.left() && update.right() <= header.right());
    assert!(cx.debug_bounds("header-workspace").is_some());
    cx.update(|_, cx| SettingsState::update(cx, |store| store.sidebar_collapsed = true));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-update").is_none());
    assert!(cx.debug_bounds("global-navigation").is_none());
    assert!(cx.debug_bounds("header-workspace").is_none());
}

#[gpui::test]
fn settings_control_emits_typed_request_only_outside_a_modal(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let requests = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.update(|window, cx| {
        let requests_for_event = requests.clone();
        cx.subscribe(&sidebar, move |_, event, _| {
            assert!(matches!(
                event,
                SidebarEvent::Settings | SidebarEvent::NewMultiProjectSession
            ));
            requests_for_event.set(requests_for_event.get() + 1);
        })
        .detach();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::OpenSettings, window, cx);
            sidebar.act(Action::NewMultiProjectSession, window, cx);
            sidebar.pending_close = Some(SessionId::from_raw(99));
            sidebar.act(Action::OpenSettings, window, cx);
            sidebar.act(Action::NewMultiProjectSession, window, cx);
        });
    });
    assert_eq!(requests.get(), 2);
    draw(cx);
    assert!(cx.debug_bounds("sidebar-settings").is_some());
}

#[test]
fn multi_root_inventory_keeps_unknown_roots_visible() {
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    let meta = SessionMeta {
        id: SessionId::from_raw(1),
        project: "alpha".into(),
        wt_path: "/alpha/main".into(),
        agent: Agent::Codex,
        context_roots: vec![
            grove_core::session_meta::ContextRoot {
                project: "alpha".into(),
                wt_path: "/alpha/main/".into(),
            },
            grove_core::session_meta::ContextRoot {
                project: "beta".into(),
                wt_path: "/beta/stale/".into(),
            },
        ],
        temp_bundle_path: None,
        label: "codex 1".into(),
        restored_title: None,
        spawned_at: Instant::now(),
        attention: None,
        tmux: false,
        tmux_name: None,
    };
    let snapshot = TreeSnapshot {
        projects: vec![SnapshotProject {
            worktrees: vec![SnapshotWorktree {
                path: "/alpha/main".into(),
                name: "alpha".into(),
                branch: "feature".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        session_root_inventory(&meta, &snapshot),
        ["alpha · alpha · feature", "beta · stale"]
    );
}

#[test]
fn effective_width_clamps_desktop_and_temporarily_caps_narrow_windows() {
    assert_eq!(effective_rail_width(260.0, 1280.0), 260.0);
    assert_eq!(effective_rail_width(10.0, 1280.0), 220.0);
    assert_eq!(effective_rail_width(900.0, 1280.0), 640.0);
    assert_eq!(effective_rail_width(900.0, 800.0), 400.0);
    assert_eq!(effective_rail_width(260.0, 500.0), 200.0);
    assert_eq!(effective_rail_width(f32::NAN, 1280.0), SIDEBAR_W);
    assert_eq!(effective_rail_width(f32::INFINITY, 1280.0), SIDEBAR_W);
    assert_eq!(effective_rail_width(SIDEBAR_W, 1280.0), SIDEBAR_W);
}

#[cfg(target_os = "macos")]
#[test]
fn heading_intrinsic_width_does_not_trigger_geist_truncation() {
    use gpui::AssetSource as _;
    let platform = gpui_platform::current_platform(true);
    let text_system = std::sync::Arc::new(gpui::TextSystem::new(platform.text_system()));
    let fonts = ["fonts/DMSans-Regular.ttf", "fonts/DMSans-Medium.ttf"]
        .map(|path| crate::assets::Assets.load(path).unwrap().unwrap());
    text_system.add_fonts(fonts.into()).unwrap();
    let window_text_system = gpui::WindowTextSystem::new(text_system.clone());
    let style = gpui::TextStyle {
        font_family: crate::fonts::UI_FAMILY.into(),
        font_weight: gpui::FontWeight::MEDIUM,
        ..Default::default()
    };
    for zoom in [0.9, 1.0, 1.1, 1.2, 1.5, 2.0] {
        for label in ["Projects", "Sessions", ""] {
            let size = gpui::px(TEXT_SMALL * zoom);
            let run = style.to_run(label.len());
            let width = crate::views::components::truncation_safe_label_width(
                label,
                size,
                &run,
                &window_text_system,
            );
            let mut wrapper = text_system.line_wrapper(run.font, size);
            assert_eq!(
                wrapper.should_truncate_line(label, width, "…", gpui::TruncateFrom::End),
                None,
                "{label} incorrectly truncates at {zoom}x with preferred width {width:?}"
            );
        }
    }
}

#[gpui::test]
fn home_terminal_rows_have_inset_padding_and_aligned_controls(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_global(crate::theme::ThemeState::new(
            false,
            "tokyonight".into(),
            "tokyonight-day".into(),
        ));
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        for index in 0..2 {
            let term = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::attach_existing(
                    "grove-test-layout-not-attached",
                    24,
                    80,
                    cx,
                )
            });
            registry.update(cx, |registry, _| {
                let id = registry.next_home_id();
                registry.push_home(
                    SessionMeta {
                        id,
                        project: String::new(),
                        wt_path: String::new(),
                        agent: Agent::Terminal,
                        context_roots: Vec::new(),
                        temp_bundle_path: None,
                        label: format!("terminal {index}"),
                        restored_title: None,
                        spawned_at: std::time::Instant::now(),
                        attention: None,
                        tmux: false,
                        tmux_name: None,
                    },
                    term,
                );
            });
        }
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.open_destination(global_navigation::Destination::Terminals, window, cx);
        sidebar
    });
    draw(cx);
    let rail = cx.debug_bounds("global-navigation").unwrap();
    let mut previous: Option<gpui::Bounds<gpui::Pixels>> = None;
    for (row_id, close_id, icon_id) in [
        ("home-1", "close-home-1", "home-icon-1"),
        ("home-2", "close-home-2", "home-icon-2"),
    ] {
        let row = cx.debug_bounds(row_id).unwrap();
        let close = cx.debug_bounds(close_id).unwrap();
        let icon = cx.debug_bounds(icon_id).unwrap();
        assert!((f32::from(row.left() - rail.left()) - SPACE_LG).abs() <= 1.0);
        assert!((f32::from(rail.right() - row.right()) - SPACE_LG).abs() <= 1.0);
        assert_eq!(icon.left(), rail.left() + gpui::px(SIDEBAR_CONTENT_INSET));
        assert_eq!(icon.size.width, gpui::px(SIDEBAR_ICON_SLOT));
        assert_eq!(row.center().y, close.center().y);
        assert!((f32::from(icon.center().y - close.center().y)).abs() <= 1.0);
        assert!(close.left() >= row.left() && close.right() <= row.right());
        assert!(close.top() >= row.top() && close.bottom() <= row.bottom());
        if let Some(previous) = previous {
            assert!((f32::from(row.top() - previous.bottom()) - SPACE_XS * 2.0).abs() <= 1.0);
        }
        previous = Some(row);
    }
}

#[gpui::test]
fn restored_title_is_visible_before_terminal_attach(cx: &mut gpui::TestAppContext) {
    let path = "/grove-restored-title-test";
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: path.into(),
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
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_reattached(
                0,
                &grove_core::tmux::DiscoveredSession {
                    name: "grove__restored_title_test".into(),
                    pane_title: Some("Continue migration".into()),
                    wt_path: path.into(),
                    project: "demo".into(),
                    label: "Codex 1".into(),
                    agent: Agent::Codex,
                    context_roots: Vec::new(),
                    temp_bundle_path: None,
                },
            );
        });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: path.into(),
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        Sidebar::new(runtime, window, cx)
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
    sidebar.read_with(cx, |sidebar, cx| {
        assert_eq!(
            sidebar.visible_session_targets(cx),
            vec![(SessionId::from_raw(1), "Continue migration · demo".into())]
        );
        assert!(sidebar.terminal_views.is_empty());
    });
    assert!(cx.debug_bounds("session-list-title-1").is_some());
    assert!(cx.debug_bounds("terminal-header-1").is_some());
}

#[gpui::test]
fn background_title_change_notifies_sidebar_without_selection(cx: &mut gpui::TestAppContext) {
    let path = "/grove-background-title-test";
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
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
        let registry = runtime.read(cx).registry.clone();
        for _ in 0..2 {
            let session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::attach_existing(
                    "grove-background-title-not-attached",
                    24,
                    80,
                    cx,
                )
            });
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta("demo".into(), path.into(), Agent::Codex);
                registry.attach(id, session, None);
                cx.notify();
            });
        }
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: path.into(),
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.select_session_id(SessionId::from_raw(1), window, cx);
        sidebar
    });
    draw(cx);
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = notifications.clone();
    let _observer = cx.update(|_, cx| cx.observe(&sidebar, move |_, _| seen.set(seen.get() + 1)));
    let background = sidebar.read_with(cx, |sidebar, cx| {
        let ids = sidebar
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .all()
            .iter()
            .map(|meta| meta.id)
            .collect::<Vec<_>>();
        assert_eq!(sidebar.selection, Some(Selection::Session(ids[0])));
        assert!(sidebar.title_observers.contains_key(&ids[1]));
        assert!(!sidebar.terminal_views.contains_key(&ids[1]));
        sidebar
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .session(ids[1])
            .unwrap()
            .clone()
    });
    let before = notifications.get();
    cx.update(|_, cx| {
        background.update(cx, |session, cx| {
            session.ingest_for_test(b"\x1b]2;Fix refresh state\x07", cx);
        });
    });
    cx.run_until_parked();
    assert!(notifications.get() > before);
    sidebar.read_with(cx, |sidebar, cx| {
        let second = sidebar.runtime.read(cx).registry.read(cx).all()[1].id;
        assert_eq!(
            sidebar.observed_titles.get(&second),
            Some(&Some("Fix refresh state".into()))
        );
        assert_eq!(
            sidebar.selection,
            Some(Selection::Session(SessionId::from_raw(1)))
        );
        assert!(!sidebar.terminal_views.contains_key(&second));
        assert!(sidebar
            .visible_session_targets(cx)
            .iter()
            .any(|(id, title)| *id == second && title.starts_with("Fix refresh state")));
    });
    let after_title_change = notifications.get();
    cx.update(|_, cx| {
        background.update(cx, |session, cx| {
            session.ingest_for_test(b"\x1b]2;Fix refresh state\x07", cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), after_title_change);
    cx.update(|_, cx| {
        cx.global_mut::<SettingsState>()
            .store
            .workspaces
            .create("Other")
            .unwrap();
    });
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        assert!(sidebar.title_observers.is_empty());
        assert!(sidebar.observed_titles.is_empty());
    });
}

#[gpui::test]
fn archived_panel_dismisses_on_sidebar_navigation(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Projects,
            projects: vec![grove_core::storage::Project {
                name: "one".into(),
                path: "/grove-sidebar-archive-test".into(),
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
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::ArchivedProjects, window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("project-panel").is_some());
    let project = cx.debug_bounds("global-nav-projects").unwrap();
    cx.simulate_click(project.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_none()));
    assert!(cx.debug_bounds("project-panel").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::ArchivedProjects, window, cx);
            sidebar.act(Action::Mode(ViewMode::List), window, cx);
            assert!(sidebar.project_panel.is_none());
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("project-panel").is_none());
    for action in [
        Action::Select(Selection::Project(0)),
        Action::Select(Selection::Worktree(0, "/grove-sidebar-archive-test".into())),
    ] {
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::ArchivedProjects, window, cx);
                assert!(sidebar.project_panel.is_some());
                sidebar.act(action.clone(), window, cx);
                assert!(sidebar.project_panel.is_none());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-panel").is_none());
    }
}

#[gpui::test]
fn grid_close_shortcut_confirms_selected_tile(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        let project = grove_core::storage::Project {
            name: "demo".into(),
            path: "/grove-grid-shortcut-test".into(),
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
            registry.insert_meta(
                "demo".into(),
                "/grove-grid-shortcut-test".into(),
                Agent::Codex,
            );
            registry.insert_meta(
                "demo".into(),
                "/grove-grid-shortcut-test".into(),
                Agent::Codex,
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.mode = ViewMode::Grid;
        sidebar.selection = Some(Selection::Session(SessionId::from_raw(2)));
        sidebar
    });
    let first = SessionId::from_raw(1);
    let selected = SessionId::from_raw(2);
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
    let header = cx.debug_bounds("terminal-header-2").unwrap();
    let source = sidebar.read_with(cx, |sidebar, _| {
        sidebar.canvas_close_focus[&(selected, false)].clone()
    });
    cx.update(|window, cx| {
        source.focus(window, cx);
        sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
    });
    draw(cx);
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    let popup = cx.debug_bounds("canvas-confirmation").unwrap();
    assert!((f32::from(popup.left() - header.left())).abs() <= 1.0);
    assert!((f32::from(popup.top() - header.bottom())).abs() <= 1.0);
    assert!(sidebar.read_with(cx, |sidebar, _| {
        sidebar.pending_close == Some(selected) && sidebar.canvas_close_anchor == Some(selected)
    }));
    cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, _| assert!(source.is_focused(window)));
    assert!(sidebar.read_with(cx, |sidebar, _| !sidebar.confirmation_open()));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.is_zen()));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
    });
    draw(cx);
    let zen_popup = cx.debug_bounds("canvas-confirmation").unwrap();
    assert!(zen_popup.left() >= gpui::px(0.0));
    assert!(zen_popup.top() >= gpui::px(0.0));
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::ConfirmClose(selected), window, cx);
        });
    });
    assert!(sidebar.read_with(cx, |sidebar, cx| {
        let registry = sidebar.runtime.read(cx).registry.read(cx);
        sidebar.dashboard_open
            && registry.meta(first).is_some()
            && registry.meta(selected).is_none()
    }));
}

#[test]
fn grid_retains_last_sidebar_mode() {
    let (mut mode, mut last) = (ViewMode::Project, ViewMode::Project);
    change_mode(&mut mode, &mut last, ViewMode::List);
    change_mode(&mut mode, &mut last, ViewMode::Grid);
    assert_eq!(last, ViewMode::List);
    let restore = last;
    change_mode(&mut mode, &mut last, restore);
    assert_eq!(mode, ViewMode::List);
}
