//! Native sidebar capture fixtures for the macOS Metal renderer.

use super::*;

/// Real native text shaping and Metal rendering without opening or installing the app.
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
fn update_header_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    for rem in [16.0, 20.0] {
        for (label, width, collapsed) in [
            ("narrow", 220.0, false),
            ("default", 260.0, false),
            ("wide", 360.0, false),
            ("collapsed", 260.0, true),
        ] {
            cx.update(|cx| {
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                let mut store = grove_core::storage::Store {
                    sidebar_width: Some(width),
                    sidebar_collapsed: collapsed,
                    ..Default::default()
                };
                let active = store.workspaces.active;
                store
                    .workspaces
                    .rename(active, "A very long workspace name for the update action")
                    .unwrap();
                cx.set_global(SettingsState::new(store));
            });
            let window = cx.open_window(gpui::size(gpui::px(900.0), gpui::px(480.0)), |window, cx| {
                    let runtime = cx.new(Runtime::new);
                    runtime.read(cx).upgrade.clone().update(cx, |upgrade, cx| {
                        upgrade.offer_for_test(grove_core::upgrade::InstallMethod::Dmg, grove_core::upgrade::Release {
                            version: semver::Version::new(99, 0, 0), tag: "v99.0.0".into(),
                            html_url: "https://github.com/gitfudge0/grove/releases/tag/v99.0.0".into(), body: String::new(),
                            dmg_url: Some("https://github.com/gitfudge0/grove/releases/download/v99.0.0/Grove.dmg".into()),
                            dmg_sha256_url: None, target_commitish: "main".into(),
                        }, cx);
                    });
                    let picker = cx.new(|cx| crate::views::workspace_manager::WorkspaceManager::new(window, cx));
                    let sidebar = cx.new(|cx| {
                        let mut sidebar = Sidebar::new(runtime, window, cx);
                        sidebar.set_workspace_selector(picker);
                        sidebar
                    });
                    let fixture = cx.new(|cx| {
                        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
                        WorkspaceHeaderFixture::new(sidebar, cx)
                    });
                    cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false).bg(c::BG()))
                }).unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
                assert_eq!(window.rem_size(), gpui::px(rem));
            })
            .unwrap();
            let path = format!("/tmp/grove-update-header-{label}-rem{rem:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Update header native fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// Native renderer fixtures exercise Codex and Claude card identities without live providers.
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
fn session_list_native_snapshots() {
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
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    for light in [false, true] {
        for rem in [16.0, 20.0] {
            for width in [220.0, 260.0, 340.0] {
                cx.update(|cx| {
                    cx.set_global(SettingsState::new(grove_core::storage::Store {
                        sidebar_width: Some(width),
                        projects: ["grove", "a-very-long-project-name"]
                            .into_iter()
                            .map(|name| grove_core::storage::Project {
                                name: name.into(),
                                path: format!("/tmp/{name}"),
                                scripts: grove_core::storage::ProjectScripts::default(),
                                archived: false,
                                worktree_dir: None,
                            })
                            .collect(),
                        ..Default::default()
                    }));
                    grove_core::theme::set(
                        grove_core::theme::by_name(if light {
                            "grove-light"
                        } else {
                            "grove-dark"
                        })
                        .unwrap(),
                    );
                    c::sync_component_theme(cx);
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                });
                let mut fixture_sidebar = None;
                let window = cx.open_window(gpui::size(gpui::px(1000.0), gpui::px(620.0)), |window, cx| {
                        let runtime = cx.new(Runtime::new);
                        let registry = runtime.read(cx).registry.clone();
                        runtime.read(cx).tree.clone().update(cx, |tree, _| {
                            for (idx, name) in ["grove", "a-very-long-project-name"].into_iter().enumerate() {
                                tree.set_active_worktrees(idx, vec![grove_core::git::Worktree {
                                    path: format!("/tmp/{name}"), branch: "main".into(), mtime: None, is_main: true,
                                }]);
                            }
                        });
                        let mut ids = Vec::new();
                        for (agent, project, title, state) in [
                            (Agent::Codex, "grove", "T3 Code Features to Add", ActivityState::Done),
                            (Agent::Claude, "grove", "Fix sidebar resize", ActivityState::Working),
                            (Agent::Codex, "a-very-long-project-name", "Repair very long session navigation titles without hiding meaningful actions", ActivityState::WaitingForInput),
                        ] {
                            let id = registry.update(cx, |registry, _| {
                                let id = registry.insert_meta(project.into(), format!("/tmp/{project}"), agent);
                                registry.set_restored_title(id, Some(title.into()));
                                id
                            });
                            runtime.read(cx).activity.clone().update(cx, |activity, _| activity.set_state_for_test(id, state));
                            ids.push(id);
                        }
                        let saved: grove_core::codex::SavedSession = serde_json::from_value(serde_json::json!({
                            "key":"session-card-fixture", "project":"grove", "cwd":"/tmp/grove", "label":"codex 1",
                            "title":"T3 Code Features to Add", "roots":[], "thread_id":null,
                            "items":[{"type":"plan", "plan":[{"step":"Done", "status":"completed"},{"step":"Done too", "status":"completed"}]}]
                        })).unwrap();
                        let (handle, _actions) = grove_core::codex::Handle::channel();
                        let (_updates, receive) = std::sync::mpsc::channel();
                        let session = cx.new(|cx| {
                            let mut state = grove_core::codex::Snapshot::new(saved);
                            state.status = grove_core::codex::Status::Done;
                            crate::entities::codex_session::CodexSession::from_connection(state,
                                grove_core::codex::Connection { handle: handle.clone(), updates: receive.into() }, cx)
                        });
                        registry.update(cx, |registry, _| registry.attach_codex(ids[0], session, handle));
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime, window, cx);
                            sidebar.mode = ViewMode::List;
                            sidebar.selection = Some(Selection::Session(ids[0]));
                            sidebar.snapshot = TreeSnapshot { projects: vec![crate::entities::workspace_state::SnapshotProject {
                                idx: 0, name: "grove".into(), sessions: ids.clone(),
                                worktrees: vec![crate::entities::workspace_state::SnapshotWorktree {
                                    path: "/tmp/grove".into(), name: "grove".into(), branch: "main".into(), sessions: ids.clone(), ..Default::default()
                                }], ..Default::default()
                            }], total_projects: 1 };
                            sidebar
                        });
                        fixture_sidebar = Some(sidebar.clone());
                        cx.new(|cx| gpui_component::Root::new(sidebar, window, cx).bordered(false).bg(c::BG()))
                    }).unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    // Seed after background refresh so the captured native cards
                    // exercise both short/long branch adjacency and semantic Git counts.
                    fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                        let projects = [
                            (
                                0,
                                "grove",
                                "main",
                                vec![SessionId::from_raw(1), SessionId::from_raw(2)],
                            ),
                            (
                                1,
                                "a-very-long-project-name",
                                "feature/session-cards-with-a-deliberately-long-branch-name",
                                vec![SessionId::from_raw(3)],
                            ),
                        ];
                        sidebar.runtime.read(cx).tree.clone().update(cx, |tree, _| {
                            tree.apply_git_poll(
                                projects
                                    .iter()
                                    .map(|(_, name, _, _)| {
                                        (
                                            format!("/tmp/{name}"),
                                            grove_core::git::WorktreeGitState {
                                                dirty: true,
                                                added: 23,
                                                removed: 3,
                                                ..Default::default()
                                            },
                                        )
                                    })
                                    .collect(),
                                &[],
                            );
                        });
                        sidebar.snapshot = TreeSnapshot {
                            projects: projects
                                .into_iter()
                                .map(|(idx, name, branch, sessions)| {
                                    crate::entities::workspace_state::SnapshotProject {
                                        idx,
                                        name: name.into(),
                                        sessions: sessions.clone(),
                                        worktrees: vec![
                                            crate::entities::workspace_state::SnapshotWorktree {
                                                path: format!("/tmp/{name}"),
                                                name: name.into(),
                                                branch: branch.into(),
                                                sessions,
                                                ..Default::default()
                                            },
                                        ],
                                        ..Default::default()
                                    }
                                })
                                .collect(),
                            total_projects: 2,
                        };
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                })
                .unwrap();
                cx.update(|cx| {
                    let sidebar = fixture_sidebar.as_ref().unwrap().read(cx);
                    for id in [1, 2, 3] {
                        let bounds = sidebar.session_close_bounds[&SessionId::from_raw(id)].get();
                        assert!(
                            bounds.size.width > gpui::px(0.0) && bounds.size.height > gpui::px(0.0),
                            "session {id} must actually render before capture"
                        );
                    }
                });
                let path = format!(
                    "/tmp/grove-session-list-{}-{width:.0}-rem{rem:.0}.png",
                    if light { "light" } else { "dark" }
                );
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native session-list fixture: {path}");
                for focused in [false, true] {
                    cx.update_window(window.into(), |_, window, cx| {
                        fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                            sidebar.mode = ViewMode::Project;
                            if focused {
                                sidebar.session_close_focus[&SessionId::from_raw(1)]
                                    .focus(window, cx);
                            }
                            cx.notify();
                        });
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                    let path = format!(
                        "/tmp/grove-session-tree-{}-{width:.0}-rem{rem:.0}-focus{focused}.png",
                        if light { "light" } else { "dark" }
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native session-tree fixture: {path}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}

#[test]
#[ignore = "requires macOS Metal renderer; writes single-sidebar fixture screenshots to /tmp"]
fn single_sidebar_native_snapshots() {
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
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    for (name, light, width, rem, collapsed, projects, provider) in [
        (
            "light-activity-codex",
            true,
            1100.,
            16.,
            false,
            false,
            grove_core::codex::Provider::Codex,
        ),
        (
            "light-projects-claude",
            true,
            1100.,
            16.,
            false,
            true,
            grove_core::codex::Provider::Claude,
        ),
        (
            "light-collapsed-narrow",
            true,
            760.,
            16.,
            true,
            false,
            grove_core::codex::Provider::Codex,
        ),
        (
            "dark-activity-claude",
            false,
            1100.,
            16.,
            false,
            false,
            grove_core::codex::Provider::Claude,
        ),
        (
            "dark-projects-narrow-zoom",
            false,
            760.,
            20.,
            false,
            true,
            grove_core::codex::Provider::Claude,
        ),
        (
            "dark-collapsed-codex",
            false,
            1100.,
            20.,
            true,
            true,
            grove_core::codex::Provider::Codex,
        ),
        (
            "light-worktrees-codex",
            true,
            1100.,
            16.,
            false,
            false,
            grove_core::codex::Provider::Codex,
        ),
        (
            "dark-worktrees-narrow-claude",
            false,
            760.,
            20.,
            false,
            false,
            grove_core::codex::Provider::Claude,
        ),
    ] {
        let path = "/grove-single-sidebar-fixture";
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                sidebar_collapsed: collapsed,
                sidebar_group_by_project: projects,
                sidebar_group_by_worktree: name.contains("worktrees"),
                projects: vec![grove_core::storage::Project {
                    name: "Layout fixture".into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            grove_core::theme::set(
                grove_core::theme::by_name(if light { "grove-light" } else { "grove-dark" })
                    .unwrap(),
            );
            c::sync_component_theme(cx);
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
        });
        let window = cx
            .open_window(gpui::size(gpui::px(width), gpui::px(720.)), |window, cx| {
                cx.new(|cx| {
                    let runtime = cx.new(Runtime::new);
                    runtime.read(cx).tree.clone().update(cx, |tree, _| {
                        tree.set_active_worktrees(
                            0,
                            vec![
                                grove_core::git::Worktree {
                                    path: path.into(),
                                    branch: "feature/navigation-with-a-long-worktree-name".into(),
                                    mtime: None,
                                    is_main: true,
                                },
                                grove_core::git::Worktree {
                                    path: format!("{path}/review-checkout"),
                                    branch: "feature/navigation-with-a-long-worktree-name".into(),
                                    mtime: None,
                                    is_main: false,
                                },
                            ],
                        );
                    });
                    let mut sidebar = Sidebar::new(runtime, window, cx);
                    attach_removal_layout_session(
                        &mut sidebar,
                        path,
                        grove_core::codex::Provider::Codex,
                        window,
                        cx,
                    );
                    attach_removal_layout_session(
                        &mut sidebar,
                        &format!("{path}/review-checkout"),
                        grove_core::codex::Provider::Claude,
                        window,
                        cx,
                    );
                    let id = sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .all()
                        .iter()
                        .find(|m| m.agent == provider.agent())
                        .unwrap()
                        .id;
                    sidebar.select(Selection::Session(id), cx);
                    sidebar.destination = global_navigation::Destination::Sessions;
                    sidebar.mode = ViewMode::List;
                    sidebar
                })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        let output = format!("/tmp/grove-single-sidebar-{name}.png");
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&output)
            .unwrap();
        eprintln!("Single sidebar native fixture: {output}");
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

/// Real macOS rendering with channel-backed Codex/Claude fixtures; no live provider calls.
#[test]
#[ignore = "requires macOS Metal renderer; writes worktree removal screenshots to /tmp"]
fn worktree_removal_native_snapshots() {
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    let path = "/grove-removal-layout-fixture";
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        crate::theme::sync_component_theme(cx);
        cx.set_reduce_motion(true);
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
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        for width in [1600., 768., 600.] {
            let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
            let capture = owner.clone();
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
                    let runtime = cx.new(Runtime::new);
                    runtime.read(cx).tree.clone().update(cx, |tree, _| {
                        tree.set_active_worktrees(
                            0,
                            vec![grove_core::git::Worktree {
                                path: path.into(),
                                branch: "feature".into(),
                                mtime: None,
                                is_main: false,
                            }],
                        );
                    });
                    let sidebar = cx.new(|cx| {
                        let mut sidebar = Sidebar::new(runtime, window, cx);
                        attach_removal_layout_session(&mut sidebar, path, provider, window, cx);
                        sidebar
                    });
                    *capture.borrow_mut() = Some(sidebar.clone());
                    cx.new(|cx| {
                        gpui_component::Root::new(sidebar, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                })
                .unwrap();
            for open in [false, true] {
                cx.update_window(window.into(), |_, window, cx| {
                    owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                        if open {
                            sidebar.act(Action::RemoveWorktree(0, path.into()), window, cx);
                        }
                    });
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                let output = format!(
                    "/tmp/grove-worktree-removal-{provider:?}-{width}-{}.png",
                    if open { "open" } else { "rest" }
                );
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&output)
                    .unwrap();
                eprintln!("Worktree removal native fixture: {output}");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
