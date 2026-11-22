use super::*;

#[gpui::test]
fn saved_project_without_sessions_opens_contextual_checkouts(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "saved".into(),
                path: "/grove-saved-project-start-test".into(),
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
            sidebar.open_destination(global_navigation::Destination::Projects, window, cx);
            sidebar.act(Action::Select(Selection::Project(0)), window, cx);
        });
    });
    draw(cx);
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
        Some(Selection::Project(0))
    );
    assert!(cx.debug_bounds("checkout-management").is_some());
    assert!(cx.debug_bounds("projects-destination").is_none());
}

#[gpui::test]
fn diff_opens_for_selected_session_repo_and_restores_focus(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("alpha".into(), "/repo/alpha/".into(), Agent::Codex);
            registry.insert_meta("beta".into(), "/repo/beta/".into(), Agent::Claude);
        });
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            // Initial landing focuses the Dashboard composer. Establish the
            // actual invoking source after that first render has settled.
            sidebar.focus.focus(window, cx);
            let source = window.focused(cx).expect("focused diff trigger source");
            assert_eq!(source, sidebar.focus);
            sidebar.act(Action::OpenDiff(SessionId::from_raw(2)), window, cx);
            assert_eq!(sidebar.diff_return_focus.as_ref(), Some(&source));
            assert_eq!(
                sidebar.diff_viewer.as_ref().unwrap().read(cx).wt_path,
                "/repo/beta"
            );
            assert!(sidebar.diff_focus.is_focused(window));
            sidebar.act(Action::CloseDiff, window, cx);
            assert!(sidebar.diff_viewer.is_none());
            assert_eq!(window.focused(cx), Some(source));
            assert!(sidebar.focus.is_focused(window));
        });
    });
}

#[gpui::test]
fn session_navigation_closes_settings_and_focuses_same_or_different_session(
    cx: &mut gpui::TestAppContext,
) {
    let path = "/grove-settings-navigation-fixture";
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
        for (agent, provider) in [
            (Agent::Codex, grove_core::codex::Provider::Codex),
            (Agent::Claude, grove_core::codex::Provider::Claude),
        ] {
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta("demo".into(), path.into(), agent)
            });
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
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                context_details: None,
                key: format!("settings-navigation-{}", id.raw()),
                project: "demo".into(),
                cwd: path.into(),
                label: "Fixture".into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    grove_core::codex::Snapshot::new(saved),
                    grove_core::codex::Connection {
                        handle: handle.clone(),
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            registry.update(cx, |registry, _| registry.attach_codex(id, session, handle));
        }
        let terminal = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script("\0", path, cx)
        });
        registry.update(cx, |registry, _| {
            let id = registry.insert_meta("demo".into(), path.into(), Agent::Terminal);
            registry.attach(id, terminal, None);
        });
        let settings =
            cx.new(|cx| crate::views::settings_panel::SettingsPanel::new(runtime.clone(), cx));
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.set_settings_panel(settings, cx);
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    for collapsed in [false, true] {
        cx.update(|_, cx| {
            cx.global_mut::<SettingsState>().store.sidebar_collapsed = collapsed;
        });
        draw(cx);
        // Repeated targets exercise reopening settings on the already-selected session.
        for raw in [1, 1, 2, 2, 3, 3, 1] {
            let id = SessionId::from_raw(raw);
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar
                        .settings_panel
                        .clone()
                        .unwrap()
                        .update(cx, |panel, cx| {
                            if raw == 3 {
                                panel.open_shortcuts(window, cx);
                            } else {
                                panel.open(window, cx);
                            }
                        });
                });
            });
            draw(cx);
            assert!(
                cx.debug_bounds("sidebar-rail").is_none(),
                "Settings hides context navigation"
            );
            assert_eq!(cx.debug_bounds("global-nav-history").is_some(), !collapsed);
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.select_session_id(id, window, cx);
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                let sidebar = sidebar.read(cx);
                assert!(!sidebar.settings_panel.as_ref().unwrap().read(cx).is_open());
                assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                let focus = if let Some(view) = sidebar.codex_views.get(&id) {
                    view.focus_handle(cx)
                } else {
                    sidebar.terminal_views[&id].focus_handle(cx)
                };
                assert!(focus.contains_focused(window, cx));
            });
        }
    }
}

#[gpui::test]
fn branch_history_preserves_codex_and_claude_drafts_and_canvas(cx: &mut gpui::TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let path = fs_err::canonicalize(fixture.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Author",
            "-c",
            "user.email=a@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "Preserve draft while browsing commits",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    for number in 1..7 {
        fs_err::write(
            fixture.path().join("model.txt"),
            format!(
                "Revenue forecast\nOperating model revision {number}\n{}",
                "Validated period\n".repeat(number)
            ),
        )
        .unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["add", "model.txt"])
            .output()
            .unwrap()
            .status
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args([
                "-c",
                "user.name=A deliberately long author name for narrow history rows",
                "-c",
                "user.email=author@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m"
            ])
            .arg(format!("Review operating model revision {number}"))
            .output()
            .unwrap()
            .status
            .success());
    }
    let snapshot = grove_core::history::load(&path, 100).unwrap();
    let latest_details = grove_core::history::details(&path, &snapshot.commits[0].id).unwrap();
    let first_file = latest_details
        .files
        .first()
        .expect("latest commit changes model.txt");
    assert_eq!(first_file.path, "model.txt");
    let latest_patch =
        grove_core::history::file_patch(&path, &snapshot.commits[0].id, first_file).unwrap();
    let recent = grove_core::history::load(&path, 5).unwrap();
    assert_eq!(recent.commits.len(), 5);
    assert!(recent.has_more);
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Layout fixture".into(),
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
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
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
        Sidebar::new(runtime, window, cx)
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                attach_removal_layout_session(sidebar, &path, provider, window, cx);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(900.)));
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_branch_peek(true, window, cx);
                let cache = sidebar.branch_peek_cache.as_ref().unwrap();
                assert_eq!(
                    cache.data.read(cx).limit,
                    5,
                    "recent preview loads only five commits"
                );
                cache.data.update(cx, |data, cx| {
                    data.snapshot = Some(recent.clone().into());
                    data.loading = false;
                    cx.notify();
                });
                assert!(sidebar.render_branch_peek(window, cx).is_some());
                let (target, identity) = sidebar.branch_entry.clone().unwrap();
                let owner = sidebar.selection.clone();
                sidebar.act(
                    Action::OpenBranchHistory(target.clone(), None, identity),
                    window,
                    cx,
                );
                assert!(
                    sidebar.history.is_none(),
                    "stale session action cannot open history"
                );
                sidebar.act(
                    Action::OpenBranchHistory(target, owner, identity),
                    window,
                    cx,
                );
                assert!(sidebar.history.is_some());
                assert!(sidebar.branch_peek.is_none());
                sidebar.act(Action::CloseHistory, window, cx);
                assert!(sidebar.branch_entry_focus.is_focused(window));
                // Reopening a valid same-HEAD preview reuses the bounded data entity.
                let cached = sidebar.branch_peek_cache.as_ref().unwrap().data.entity_id();
                sidebar.open_branch_peek(true, window, cx);
                assert_eq!(
                    sidebar.branch_peek_cache.as_ref().unwrap().data.entity_id(),
                    cached
                );
                sidebar.dismiss_branch_peek(window, cx);
                assert!(sidebar.branch_entry_focus.is_focused(window));
            });
        });
        draw(cx);
        let branch_row = cx.debug_bounds("checkout-branch-history").unwrap();
        let composer_focus = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("session")
                };
                let focus = sidebar.codex_views[&id].focus_handle(cx);
                focus.focus(window, cx);
                focus
            })
        });
        let outside = gpui::point(gpui::px(1.), gpui::px(1.));
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.simulate_mouse_move(branch_row.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_none()),
            "hover intent defers opening"
        );
        cx.executor().advance_clock(Duration::from_millis(349));
        draw(cx);
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_none()),
            "recent history waits the full 350ms"
        );
        cx.executor().advance_clock(Duration::from_millis(1));
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_some()));
        cx.update(|window, _| {
            assert!(
                composer_focus.is_focused(window),
                "hover preserves composer focus"
            );
        });
        let hover_peek = cx.debug_bounds("branch-history-peek").unwrap();
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            Some(hover_peek),
            "native HTML mask matches recent history popup"
        );
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(40));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_some()),
            "pointer crossing grace retains popup"
        );
        cx.simulate_mouse_move(hover_peek.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_some()),
            "entering popup cancels dismissal"
        );
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(81));
        draw(cx);
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.branch_peek.is_none()),
            "unpinned preview closes after grace"
        );
        for width in [1280., 600.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
            draw(cx);
            let (selection, view, focus) = cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("expected session")
                    };
                    let view = sidebar.codex_views[&id].clone();
                    view.focus_handle(cx).focus(window, cx);
                    let focus = window.focused(cx);
                    let selection = sidebar.selection.clone();
                    sidebar.act(Action::OpenHistory(path.clone()), window, cx);
                    sidebar.history.as_ref().unwrap().update(cx, |history, cx| {
                        history.snapshot = Some(snapshot.clone().into());
                        history.loading = false;
                        cx.notify();
                    });
                    assert!(
                        sidebar.html_overlay_open(),
                        "{provider:?} embedded HTML must be blocked"
                    );
                    (selection, view, focus)
                })
            });
            draw(cx);
            let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
            assert_eq!(cx.debug_bounds("branch-history-viewer"), Some(canvas));
            assert!(cx.debug_bounds("history-commit-row").is_some());
            let title = cx.debug_bounds("history-header-title").unwrap();
            let branch = cx.debug_bounds("history-header-branch").unwrap();
            assert!(title.right() <= branch.left());
            assert!((f32::from(title.center().y - branch.center().y)).abs() < 2.0);
            let row = cx.debug_bounds("history-commit-row").unwrap();
            let graph = cx.debug_bounds("history-row-graph").unwrap();
            let sha = cx.debug_bounds("history-identity-sha").unwrap();
            let avatar = cx.debug_bounds("history-identity-avatar").unwrap();
            let subject = cx.debug_bounds("history-identity-subject").unwrap();
            let date = cx.debug_bounds("history-identity-date").unwrap();
            assert!(
                graph.right() <= sha.left()
                    && sha.right() <= avatar.left()
                    && avatar.right() <= subject.left(),
                "graph/SHA/initials/subject retain order at {width}"
            );
            for bounds in [sha, avatar, subject, date] {
                assert!(
                    bounds.left() >= row.left() && bounds.right() <= row.right(),
                    "fixed identity slots remain contained at {width}"
                );
                assert!(
                    (f32::from(bounds.center().y - row.center().y)).abs() <= 1.0,
                    "identity slots share a centered baseline at {width}"
                );
            }
            assert!(
                sha.size.width > gpui::px(0.),
                "short SHA remains visible at {width}"
            );
            assert!(
                cx.debug_bounds("history-identity-author").is_none(),
                "row has no supporting author line"
            );
            // Main cards use the same dwell but retain a separate crossing grace.
            let a = snapshot.commits[0].id.clone();
            let b = snapshot.commits[1].id.clone();
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(349));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| {
                    let data = sidebar.history.as_ref().unwrap().read(cx);
                    data.selected.is_none() && !data.detail_loading
                }),
                "passing a row must not select or load details before 350ms"
            );
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), false, cx);
                    sidebar.commit_row_hover(b.clone(), true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(349));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "movingAtoB resets the dwell"
            );
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .clone()),
                Some(b.clone())
            );
            // Old hover cards dismiss at 80ms independently of the next row's 350ms dwell.
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(b.clone(), false, cx);
                    sidebar.commit_row_hover(a.clone(), true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(80));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "old card dismisses while another row still dwells"
            );
            cx.executor().advance_clock(Duration::from_millis(270));
            cx.run_until_parked();
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .clone()),
                Some(a.clone())
            );
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), false, cx);
                    sidebar.commit_row_hover(b.clone(), true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(b.clone(), false, cx);
                    sidebar.commit_card_hover(true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(300));
            cx.run_until_parked();
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .clone()),
                Some(b.clone()),
                "crossing into the white card retains it"
            );
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.commit_card_hover(false, cx));
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(79));
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, cx| sidebar
                .history
                .as_ref()
                .unwrap()
                .read(cx)
                .selected
                .is_some()));
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "leaving row and card dismisses after 80ms"
            );
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), true, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), false, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "leaving before deadline cancels the open"
            );
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), true, cx);
                    sidebar.cancel_history_hover_intent(cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "scrolling cancels pending source intent"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.commit_row_hover(a.clone(), true, cx);
                    sidebar.act(Action::SelectCommit(b.clone()), window, cx);
                });
            });
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .clone()),
                Some(b.clone()),
                "explicit selection is immediate"
            );
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .clone()),
                Some(b.clone()),
                "explicit selection stays pinned against an old timer"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::RefreshHistory, window, cx);
                    sidebar.history.as_ref().unwrap().update(cx, |data, cx| {
                        data.snapshot = Some(snapshot.clone().into());
                        data.loading = false;
                        cx.notify();
                    });
                    sidebar.commit_row_hover(a.clone(), true, cx);
                    // Direct entity refresh also invalidates an already scheduled callback.
                    sidebar
                        .history
                        .as_ref()
                        .unwrap()
                        .update(cx, BranchHistoryState::reload);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selected
                    .is_none()),
                "refresh generation rejects stale intent"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.history.as_ref().unwrap().update(cx, |data, cx| {
                        data.snapshot = Some(snapshot.clone().into());
                        data.loading = false;
                        cx.notify();
                    });
                    sidebar.commit_row_hover(a.clone(), true, cx);
                    sidebar.act(Action::CloseHistory, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.history.is_none()),
                "closing cancels pending opens"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::OpenHistory(path.clone()), window, cx);
                    sidebar.history.as_ref().unwrap().update(cx, |data, cx| {
                        data.snapshot = Some(snapshot.clone().into());
                        data.loading = false;
                        cx.notify();
                    });
                    sidebar.commit_row_hover(a.clone(), true, cx);
                    sidebar.selection = None;
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .history
                    .as_ref()
                    .is_none_or(|history| history.read(cx).selected.is_none())),
                "changed session rejects stale intent"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.selection = selection.clone();
                    view.focus_handle(cx).focus(window, cx);
                    sidebar.act(Action::OpenHistory(path.clone()), window, cx);
                    sidebar.history.as_ref().unwrap().update(cx, |data, cx| {
                        data.snapshot = Some(snapshot.clone().into());
                        data.loading = false;
                        cx.notify();
                    });
                });
            });
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(
                        Action::SelectCommit(snapshot.commits[0].id.clone()),
                        window,
                        cx,
                    );
                    sidebar.history.as_ref().unwrap().update(cx, |history, cx| {
                        history.details = Some(latest_details.clone().into());
                        history.detail_loading = false;
                        cx.notify();
                    });
                });
            });
            draw(cx);
            let preview = cx.debug_bounds("history-commit-preview").unwrap();
            assert!(
                preview.left() >= canvas.left() && preview.right() <= canvas.right(),
                "preview within canvas at {width}"
            );
            // Exercise the actual expanded route with a real text change. Prefilling
            // core results makes the layout oracle independent of executor IO timing.
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::ExpandCommit, window, cx);
                    sidebar.history.as_ref().unwrap().update(cx, |history, cx| {
                        assert_eq!(
                            history.selected_file.as_deref(),
                            Some("model.txt"),
                            "opening a commit selects its first changed file for {provider:?}"
                        );
                        history.install_patch(latest_patch.clone());
                        history.patch_loading = false;
                        cx.notify();
                    });
                });
            });
            draw(cx);
            assert!(
                cx.debug_bounds("history-commit-metadata").is_none(),
                "expanded metadata starts collapsed at {width} for {provider:?}"
            );
            let header = cx.debug_bounds("history-detail-header").unwrap();
            let details = cx.debug_bounds("history-details").unwrap();
            let files = cx.debug_bounds("history-files").unwrap();
            let diff = cx.debug_bounds("history-diff-area").unwrap();
            let file_row = cx.debug_bounds("history-file-row").unwrap();
            assert!(
                f32::from(file_row.size.height) <= 44.0,
                "file row should stay compact: {file_row:?}"
            );
            let left_label = cx.debug_bounds("history-split-left-label").unwrap();
            let right_label = cx.debug_bounds("history-split-right-label").unwrap();
            assert!(
                header.top() >= details.top() && header.bottom() <= files.top(),
                "commit header precedes the horizontal file/diff workspace at {width}"
            );
            assert!(
                files.left() >= details.left()
                    && files.right() <= diff.left()
                    && diff.right() <= details.right(),
                "files and diff remain side by side within the canvas at {width}"
            );
            assert!(
                (f32::from(files.top() - diff.top())).abs() <= 1.0
                    && (f32::from(files.size.height - diff.size.height)).abs() <= 1.0,
                "file register and diff use equal-height panes at {width}"
            );
            assert!(
                    left_label.right() <= right_label.left()
                        && (f32::from(left_label.size.width - right_label.size.width)).abs() <= 1.0,
                    "split labels reserve balanced before/after panes at {width}: left={left_label:?}, right={right_label:?}"
                );
            if width == 1280.0 {
                let divider = cx.debug_bounds("history-file-divider").unwrap().center();
                cx.simulate_mouse_down(divider, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_mouse_up(divider, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_keystrokes("right");
                draw(cx);
                let keyboard_width = cx.debug_bounds("history-files").unwrap().size.width;
                assert!(
                    keyboard_width > files.size.width,
                    "keyboard increases file register width"
                );
                let divider = cx.debug_bounds("history-file-divider").unwrap().center();
                cx.simulate_mouse_down(divider, MouseButton::Left, gpui::Modifiers::default());
                let destination = gpui::point(divider.x + gpui::px(30.0), divider.y);
                cx.simulate_mouse_move(destination, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_mouse_up(destination, MouseButton::Left, gpui::Modifiers::default());
                draw(cx);
                assert!(
                    cx.debug_bounds("history-files").unwrap().size.width > keyboard_width,
                    "drag increases file register width"
                );
            }
            assert!(
                cx.debug_bounds("history-patch").is_some(),
                "selected text file renders its patch at {width}"
            );
            assert!(
                cx.debug_bounds("history-split-spacer").is_some(),
                "unmatched additions retain an empty parent cell"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::ToggleHistoryMetadata, window, cx);
                });
            });
            draw(cx);
            assert!(
                cx.debug_bounds("history-commit-metadata").is_some(),
                "full commit metadata remains available"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::ToggleHistoryMetadata, window, cx);
                });
            });
            draw(cx);

            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(
                        Action::HistoryDiffMode(
                            crate::entities::branch_history::HistoryDiffMode::Unified,
                        ),
                        window,
                        cx,
                    );
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("history-patch").is_some());
            assert!(
                cx.debug_bounds("history-split-left-label").is_none()
                    && cx.debug_bounds("history-split-right-label").is_none(),
                "Unified replaces the paired split panes at {width}"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::HistoryGraph, window, cx);
                });
            });
            draw(cx);
            assert!(
                cx.debug_bounds("history-commit-row").is_some(),
                "Back returns to graph"
            );
            cx.simulate_keystrokes("escape");
            draw(cx);
            cx.update(|window, cx| {
                let sidebar = sidebar.read(cx);
                assert!(sidebar.history.is_none());
                assert_eq!(sidebar.selection, selection);
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("expected session")
                };
                assert_eq!(
                    sidebar.codex_views[&id], view,
                    "conversation view must stay mounted"
                );
                assert_eq!(window.focused(cx), focus, "source focus restored");
                assert_eq!(
                    sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .codex(id)
                        .unwrap()
                        .read(cx)
                        .state
                        .saved
                        .draft,
                    "Preserved draft"
                );
            });
        }
    }
}
