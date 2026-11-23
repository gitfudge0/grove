//! Project tree geometry, disclosure and popup navigation fixtures.

use super::*;

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
            assert!(sidebar.dashboard_open);
            assert_eq!(sidebar.destination, global_navigation::Destination::Home);
            assert!(sidebar.selection.is_none());
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
}
