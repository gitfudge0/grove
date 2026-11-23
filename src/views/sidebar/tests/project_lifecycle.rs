use super::*;

#[gpui::test]
fn session_view_plus_requests_session_launcher(cx: &mut gpui::TestAppContext) {
    assert_empty_cross_project_launch(cx, false, true, true);
}

#[gpui::test]
fn sessions_list_polls_real_git_changes_and_swaps_worktree_diff_for_launchers(
    cx: &mut gpui::TestAppContext,
) {
    let repo = ChangedGitRepo::new();
    let path = repo.path();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            default_landing_page: grove_core::storage::LandingPage::Sessions,
            projects: vec![grove_core::storage::Project {
                name: "poll demo".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = add_navigation_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("poll demo".into(), path.clone(), Agent::Codex);
        });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: path.clone(),
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.mode = ViewMode::List;
        sidebar
    });
    draw(cx);
    draw(cx);
    let git = sidebar.read_with(cx, |sidebar, cx| {
        sidebar.runtime.read(cx).tree.read(cx).git_states()
    });
    assert_eq!(
        git.get(&path)
            .map(|state| (state.dirty, state.added, state.removed)),
        Some((true, 1, 0))
    );
    assert!(cx.debug_bounds("diff-chip-open-1").is_none());
    assert!(cx.debug_bounds("session-list-diff-label-1").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
    });
    draw(cx);
    let selector: &'static str = Box::leak(format!("worktree-diff-{path}").into_boxed_str());
    assert!(cx.debug_bounds("diff-chip-open-1").is_none());
    let diff = cx.debug_bounds(selector).unwrap();
    let title = cx.debug_bounds("session-tree-title-1").unwrap();
    let outside = title.center();
    assert!(outside.x < diff.left());
    cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
    draw(cx);
    cx.simulate_click(outside, gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_none()));
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
        Some(Selection::Session(SessionId::from_raw(1)))
    );
    // At rest the stats sit right-aligned on the trailing metadata column.
    let actions_selector: &'static str =
        Box::leak(format!("worktree-actions-{path}").into_boxed_str());
    assert_eq!(
        diff.right(),
        cx.debug_bounds(actions_selector).unwrap().right()
    );
    assert!(cx.debug_bounds("session-tree-diff-1").is_none());
    let row_selector: &'static str = Box::leak(format!("worktree-{path}").into_boxed_str());
    let row = cx.debug_bounds(row_selector).unwrap();
    let session = cx.debug_bounds("session-1").unwrap();
    let agent = cx.debug_bounds("session-agent-1").unwrap();
    assert_eq!(
        agent.left() - session.left(),
        gpui::px(SESSION_LEADING_INSET)
    );
    // Hovering a session replaces its trailing status with Close.
    let close = cx.debug_bounds("session-tree-close-1").unwrap();
    assert!(title.right() <= close.left());
    assert!(close.right() <= session.right());
    let worktree_title: &'static str = Box::leak(format!("worktree-title-{path}").into_boxed_str());
    assert_eq!(
        cx.debug_bounds(worktree_title).unwrap().left() - row.left(),
        gpui::px(WORKTREE_LEADING_INSET + HIERARCHY_ICON_SLOT + HIERARCHY_GAP)
    );
    // Pointer hover replaces metadata with launch controls in the same trailing slot.
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds(selector).is_none());
    let first_launcher: &'static str = Box::leak(format!("launch-{path}-0").into_boxed_str());
    let launcher = cx.debug_bounds(first_launcher).unwrap();
    assert!(row.contains(&launcher.center()));
    cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds(selector).is_some());
    // Row keyboard focus also shows launch controls, while the Git action stays in traversal.
    let (row_focus, diff_focus) = sidebar.read_with(cx, |sidebar, _| {
        (
            sidebar.worktree_focus[&path].clone(),
            sidebar.worktree_diff_focus[&path].clone(),
        )
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, _| sidebar.available = [true; 4]);
        row_focus.focus(window, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds(selector).is_none());
    cx.update(Window::focus_next);
    draw(cx);
    cx.update(|window, _| assert!(diff_focus.is_focused(window)));
    assert!(cx.debug_bounds(selector).is_some());
    cx.update(Window::focus_next);
    draw(cx);
    assert!(
        cx.debug_bounds(selector).is_none(),
        "Tab from changes reveals the next launcher"
    );
    cx.update(Window::focus_prev);
    draw(cx);
    cx.update(|window, _| assert!(diff_focus.is_focused(window)));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_some()));
    // Clean and unavailable checkouts produce no visual metadata placeholder.
    for state in [Some(grove_core::git::WorktreeGitState::default()), None] {
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::CloseDiff, window, cx);
                sidebar.focus.focus(window, cx);
                let stale = if state.is_none() {
                    vec![path.clone()]
                } else {
                    Vec::new()
                };
                let states = state
                    .map(|state| HashMap::from([(path.clone(), state)]))
                    .unwrap_or_default();
                sidebar
                    .runtime
                    .read(cx)
                    .tree
                    .clone()
                    .update(cx, |tree, _| tree.apply_git_poll(states, &stale));
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds(selector).is_none());
        assert!(cx.debug_bounds("session-tree-diff-1").is_none());
    }
}

#[gpui::test]
fn worktree_launch_controls_route_opencode_and_ignore_missing_backends(
    cx: &mut gpui::TestAppContext,
) {
    let project_path = "/grove-worktree-launch-ui-test";
    let worktree_path = format!("{project_path}/missing-feature");
    let launch_ids = [
        "launch-/grove-worktree-launch-ui-test/missing-feature-0",
        "launch-/grove-worktree-launch-ui-test/missing-feature-1",
        "launch-/grove-worktree-launch-ui-test/missing-feature-2",
        "launch-/grove-worktree-launch-ui-test/missing-feature-3",
    ];
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: project_path.into(),
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
    let (sidebar, cx) = add_project_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
        sidebar.available = [false, false, false, true];
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![
                    grove_core::git::Worktree {
                        path: project_path.into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    },
                    grove_core::git::Worktree {
                        path: worktree_path.clone(),
                        branch: "feature".into(),
                        mtime: None,
                        is_main: false,
                    },
                ],
            );
        });
        sidebar
    });
    draw(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Selection::Project(0), cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("project-start-card").is_some());
    assert!(cx.debug_bounds("start-terminal").is_some());
    assert!(cx.debug_bounds("project-initialize-git").is_some());
    assert!(cx.debug_bounds("project-new-worktree").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::NewWorktree(project_path.into()), window, cx);
            assert!(sidebar.pending_new_worktree.is_none());
            assert!(sidebar
                .content_error
                .as_deref()
                .unwrap()
                .contains("Initialize Git"));
        });
    });
    for id in launch_ids {
        assert!(cx.debug_bounds(id).is_some());
    }
    assert_eq!(
        f32::from(
            cx.debug_bounds("worktree-actions-/grove-worktree-launch-ui-test/missing-feature")
                .unwrap()
                .size
                .width
        ),
        HIERARCHY_TRAILING_W
    );
    let opencode = cx.debug_bounds(launch_ids[2]).unwrap().center();
    cx.simulate_mouse_down(
        opencode,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        opencode,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
            sidebar.pending_new_worktree = Some(0);
            sidebar.act(
                Action::Launch(0, worktree_path.clone(), Agent::OpenCode),
                window,
                cx,
            );
            assert_eq!(sidebar.pending_new_worktree, Some(0));
            assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
            assert!(sidebar.content_error.as_deref().unwrap().contains("PATH"));
            let toast = sidebar.runtime.read(cx).toast.read(cx).current().unwrap();
            assert_eq!(toast.kind, crate::entities::toast::ToastKind::Error);
            assert_eq!(Some(&toast.message), sidebar.content_error.as_ref());
            sidebar.pending_new_worktree = None;
            sidebar.available[2] = true;
            cx.notify();
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert_eq!(
                sidebar.launch_target(0, worktree_path.clone(), Agent::OpenCode, cx),
                Some(("demo".into(), worktree_path.clone(), Agent::OpenCode))
            );
            assert_eq!(
                sidebar.launch_target(0, project_path.into(), Agent::Terminal, cx),
                Some(("demo".into(), project_path.into(), Agent::Terminal))
            );
            assert_eq!(
                sidebar.launch_target(0, "/stale/worktree".into(), Agent::OpenCode, cx),
                None
            );
            let id = sidebar
                .runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, _| {
                    registry.insert_meta("demo".into(), worktree_path.clone(), Agent::Codex)
                });
            sidebar.select(Selection::Session(id), cx);
            sidebar.selection = None;
        });
        let duplicate = cx.global::<SettingsState>().store.projects[0].clone();
        cx.global_mut::<SettingsState>()
            .store
            .projects
            .push(duplicate);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(
                Action::Launch(0, worktree_path.clone(), Agent::OpenCode),
                window,
                cx,
            );
            assert_eq!(sidebar.selection, None);
            assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 1);
            assert_eq!(
                sidebar.runtime.read(cx).state.read(cx).active_session(),
                Some(sidebar.runtime.read(cx).registry.read(cx).all()[0].id)
            );
        });
    });
}

#[test]
fn display_titles_remove_only_transient_braille_prefixes() {
    for title in [
        " ⠋ Respond to a greeting ",
        "⠹ · Respond to a greeting",
        "⠋— Respond to a greeting",
        "✳ Respond to a greeting",
        "✳ · Respond to a greeting",
        "✳— Respond to a greeting",
    ] {
        assert_eq!(
            display_task_title(Some(title.into()), "Codex 1"),
            "Respond to a greeting"
        );
    }
    for title in [
        "",
        " ",
        "⠋ ",
        " 3e9f01f9-54cd-4f09-8617-137231faccc9 ",
        "⠋ 3e9f01f9-54cd-4f09-8617-137231faccc9",
    ] {
        assert_eq!(display_task_title(Some(title.into()), "Codex 1"), "Codex 1");
    }
    for title in [
        "! Important",
        "- Keep this",
        "… Thinking",
        "⠋braille",
        "✳literal",
        "A ✳ literal star",
        "* Keep literal star",
        "✳️Keep literal variation selector",
        "⠋ ⠹ Keep second glyph",
    ] {
        let expected = if title == "⠋ ⠹ Keep second glyph" {
            "⠹ Keep second glyph"
        } else {
            title
        };
        assert_eq!(display_task_title(Some(title.into()), "Codex 1"), expected);
    }
    assert_eq!(
        display_task_title(None, "  Original label  "),
        "  Original label  "
    );
}

#[gpui::test]
fn worktree_removal_rejects_main_and_keeps_failure_until_dismissed(cx: &mut gpui::TestAppContext) {
    let project_path = "/grove-worktree-removal-ui-test";
    let worktree_path = "/grove-worktree-removal-ui-test-feature";
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: project_path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    assert!(cx
        .debug_bounds("delete-worktree-/grove-worktree-removal-ui-test")
        .is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.snapshot.projects = vec![crate::entities::workspace_state::SnapshotProject {
                idx: 0,
                name: "demo".into(),
                is_git: true,
                has_run: false,
                worktrees: vec![
                    crate::entities::workspace_state::SnapshotWorktree {
                        path: project_path.into(),
                        name: "demo".into(),
                        is_main: true,
                        ..Default::default()
                    },
                    crate::entities::workspace_state::SnapshotWorktree {
                        path: worktree_path.into(),
                        name: "feature".into(),
                        is_main: false,
                        ..Default::default()
                    },
                ],
                sessions: Vec::new(),
            }];
            sidebar.act(Action::RemoveWorktree(0, project_path.into()), window, cx);
            assert!(sidebar.pending_worktree_removal.is_none());
            sidebar
                .worktree_delete_focus
                .insert(worktree_path.into(), cx.focus_handle());
            sidebar.act(Action::RemoveWorktree(0, worktree_path.into()), window, cx);
            assert!(sidebar.confirmation_open());
            assert!(!sidebar.pending_worktree_removal.as_ref().unwrap().started);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("worktree-removal-decision").is_some());
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| assert!(sidebar.read(cx).confirm_focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_worktree_removal.is_none()));
    cx.update(|window, cx| {
        assert!(sidebar.read(cx).worktree_delete_focus[worktree_path].is_focused(window));
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.snapshot.projects[0].worktrees.push(
                crate::entities::workspace_state::SnapshotWorktree {
                    path: worktree_path.into(),
                    name: "feature".into(),
                    is_main: false,
                    ..Default::default()
                },
            );
            sidebar.act(Action::RemoveWorktree(0, worktree_path.into()), window, cx);
            sidebar.act(Action::ConfirmWorktreeRemoval, window, cx);
            let error = sidebar
                .pending_worktree_removal
                .as_ref()
                .unwrap()
                .error
                .clone();
            assert!(error.is_some());
            sidebar.act(Action::ConfirmWorktreeRemoval, window, cx);
            assert_eq!(
                sidebar.pending_worktree_removal.as_ref().unwrap().error,
                error
            );
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("worktree-removal-error").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_worktree_removal.is_none()));
}

#[gpui::test]
fn worktree_removal_preserves_conversation_geometry(cx: &mut gpui::TestAppContext) {
    let path = "/grove-removal-layout-fixture";
    cx.update(|cx| {
        gpui_component::init(cx);
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
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
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
        Sidebar::new(runtime, window, cx)
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                attach_removal_layout_session(sidebar, path, provider, window, cx);
            });
        });
        for width in [1600., 768., 600.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.checkout_panel.hidden = true;
                    sidebar.toggle_session_panel(window, cx);
                });
            });
            draw(cx);
            let ids = [
                "sidebar-canvas",
                "checkout-panel",
                "codex-messages",
                "codex-composer",
            ];
            let before = ids.map(|id| cx.debug_bounds(id).unwrap());
            for cancel in [true, false] {
                cx.update(|window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.act(Action::RemoveWorktree(0, path.into()), window, cx);
                    });
                });
                draw(cx);
                assert!(cx.debug_bounds("worktree-removal-decision").is_some());
                for (id, expected) in ids.into_iter().zip(before) {
                    assert_eq!(
                        cx.debug_bounds(id),
                        Some(expected),
                        "{provider:?} {id} shifted at {width}px"
                    );
                }
                if cancel {
                    let button = cx.debug_bounds("cancel-worktree-removal").unwrap();
                    cx.simulate_click(button.center(), gpui::Modifiers::default());
                } else {
                    cx.simulate_keystrokes("escape");
                }
                draw(cx);
                assert!(
                    sidebar.read_with(cx, |sidebar, _| sidebar.pending_worktree_removal.is_none())
                );
                cx.update(|window, cx| {
                    assert!(sidebar.read(cx).worktree_delete_focus[path].is_focused(window));
                });
                for (id, expected) in ids.into_iter().zip(before) {
                    assert_eq!(
                        cx.debug_bounds(id),
                        Some(expected),
                        "{id} changed after dismissal"
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn worktree_removal_decision_fits_desktop_and_narrow_canvas(cx: &mut gpui::TestAppContext) {
    let project_path = "/grove-worktree-removal-geometry";
    let worktree_path = "/grove-worktree-removal-geometry/feature-with-a-long-folder-name";
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: project_path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.pending_worktree_removal = Some(PendingWorktreeRemoval {
            project_path: project_path.into(),
            path: worktree_path.into(),
            name: "feature-with-a-long-folder-name".into(),
            started: false,
            error: None,
        });
        sidebar
    });
    for width in [1280.0, 768.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(640.0)));
        draw(cx);
        let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
        assert!(dialog.left() >= gpui::px(0.0) && dialog.right() <= gpui::px(width));
        assert!(dialog.top() >= gpui::px(0.0) && dialog.bottom() <= gpui::px(640.0));
        for id in [
            "worktree-removal-path",
            "worktree-removal-path-value",
            "cancel-worktree-removal",
            "confirm-worktree-removal",
        ] {
            let item = cx.debug_bounds(id).unwrap();
            assert!(
                item.left() >= dialog.left() && item.right() <= dialog.right(),
                "{id} clips horizontally at {width}px"
            );
            assert!(
                item.top() >= dialog.top() && item.bottom() <= dialog.bottom(),
                "{id} clips vertically at {width}px"
            );
        }
    }
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let removal = sidebar.pending_worktree_removal.as_mut().unwrap();
            removal.started = true;
            cx.notify();
        });
    });
    draw(cx);
    let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
    let status = cx.debug_bounds("worktree-removal-status").unwrap();
    assert!(status.left() >= dialog.left() && status.right() <= dialog.right());
    assert!(status.bottom() <= dialog.bottom());
    cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.pending_worktree_removal.as_mut().unwrap().error = Some(
                    "Git could not remove the worktree because the folder still contains local changes. Review the worktree and try again.".into(),
                );
                cx.notify();
            });
        });
    draw(cx);
    let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
    let error = cx.debug_bounds("worktree-removal-error").unwrap();
    let dismiss = cx.debug_bounds("dismiss-worktree-removal").unwrap();
    assert!(dialog.left() >= gpui::px(0.0) && dialog.right() <= gpui::px(768.0));
    assert!(dialog.top() >= gpui::px(0.0) && dialog.bottom() <= gpui::px(640.0));
    for item in [error, dismiss] {
        assert!(item.left() >= dialog.left() && item.right() <= dialog.right());
        assert!(item.top() >= dialog.top() && item.bottom() <= dialog.bottom());
    }
}
