mod activity;
mod checkout;
#[cfg(target_os = "macos")]
mod native_snapshots;
mod project_lifecycle;
mod session_navigation;
mod sidebar_interaction;

use super::{state_sync::SESSION_AGE_REFRESH, *};
use std::time::{Duration, Instant};

struct WorkspaceHeaderFixture {
    sidebar: Entity<Sidebar>,
    _observers: Vec<gpui::Subscription>,
}
impl WorkspaceHeaderFixture {
    fn new(sidebar: Entity<Sidebar>, cx: &mut Context<Self>) -> Self {
        let observers = vec![
            cx.observe(&sidebar, |_, _, cx| cx.notify()),
            cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
        ];
        Self {
            sidebar,
            _observers: observers,
        }
    }
}
impl Render for WorkspaceHeaderFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Shell owns this shared header above the sidebar. Mount its actual
        // control here because this component fixture does not mount Shell.
        let header = self.sidebar.update(cx, |sidebar, cx| {
            let width = sidebar.rail_width(window, cx);
            (width > 0.).then(|| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(rpx(width))
                    .h(rpx(APPBAR_TOTAL_H))
                    .px(rpx(SIDEBAR_CONTENT_INSET))
                    .child(sidebar.header_workspace_control(cx))
            })
        });
        div()
            .size_full()
            .relative()
            .child(self.sidebar.clone())
            .children(header)
    }
}

/// Mount retained tree/list recipes explicitly; the app's single sidebar no
/// longer owns these larger project and session-management compositions.
struct NavigationComponentFixture {
    sidebar: Entity<Sidebar>,
    project_recipe: bool,
}
impl Render for NavigationComponentFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.sidebar.clone();
        let project_recipe = self.project_recipe;
        self.sidebar.update(cx, |sidebar, cx| {
            super::super::inline_html::exclude_native_html(None, cx);
            sidebar.sync(window, cx);
            sidebar.sync_age_timer(cx);
            let actual_mode = sidebar.mode;
            if project_recipe {
                sidebar.mode = ViewMode::Project;
            }
            let list = sidebar.mode == ViewMode::List;
            let body = if list {
                sidebar.list(window, cx)
            } else {
                sidebar.project_tree(None, window, cx)
            };
            let hide_component = (sidebar.project_panel.is_some()
                || sidebar.pending_new_worktree.is_some())
                && Sidebar::logical_window_width(window) < 640.0;
            let component = div()
                .id("sidebar-rail")
                .debug_selector(|| "sidebar-rail".into())
                .w(rpx(sidebar.context_width(window, cx)))
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .bg(c::BG_RAIL())
                .when(list, |rail| {
                    rail.child(
                        div()
                            .px(rpx(SIDEBAR_CONTENT_INSET))
                            .child(sidebar.navigation_heading(window, cx)),
                    )
                })
                .child(
                    div()
                        .id("component-navigation-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(rpx(SPACE_LG))
                        .child(body),
                );
            // Invoke the retained canvas owner without also mounting the new
            // Projects destination's copy of the tree under test.
            sidebar.mode = actual_mode;
            let destination = sidebar.destination;
            let dashboard = sidebar.dashboard_open;
            sidebar.destination = global_navigation::Destination::Sessions;
            sidebar.dashboard_open = false;
            let content = if sidebar.selection.is_none()
                && sidebar.project_panel.is_none()
                && sidebar.project_setup.is_none()
                && sidebar.pending_new_worktree.is_none()
            {
                div().into_any_element()
            } else {
                sidebar.render_content(window, cx)
            };
            sidebar.destination = destination;
            sidebar.dashboard_open = dashboard;
            div()
                .size_full()
                .flex()
                .track_focus(&sidebar.focus)
                .when(!hide_component, |root| root.child(component))
                .child(
                    div()
                        .id("sidebar-canvas")
                        .debug_selector(|| "sidebar-canvas".into())
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(content),
                )
                .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                    owner.update(cx, |sidebar, cx| {
                        if sidebar.menu_opened_by_hover
                            && !sidebar.menu_trigger_bounds.get().contains(&event.position)
                            && !sidebar.menu_popup_bounds.get().contains(&event.position)
                        {
                            sidebar.close_menu(window, cx);
                        }
                    });
                })
                .capture_key_down(
                    cx.listener(|sidebar, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape"
                            && !sidebar.project_decision
                            && (sidebar.pending_new_worktree.is_some()
                                || sidebar.menu.is_some()
                                || sidebar.confirmation_open())
                        {
                            sidebar.act(Action::Cancel, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_key_down(super::super::shell::traverse_unhandled_tab)
        })
    }
}

fn add_component_with_mode(
    cx: &mut gpui::TestAppContext,
    project_recipe: bool,
    build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (fixture, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| build(window, cx));
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        NavigationComponentFixture {
            sidebar,
            project_recipe,
        }
    });
    let sidebar = cx.update(|_, cx| fixture.read(cx).sidebar.clone());
    (sidebar, cx)
}

fn add_navigation_component(
    cx: &mut gpui::TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
    add_component_with_mode(cx, false, build)
}

fn add_project_component(
    cx: &mut gpui::TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
    add_component_with_mode(cx, true, |window, cx| {
        let mut sidebar = build(window, cx);
        sidebar.sync(window, cx);
        sidebar.mode = ViewMode::Project;
        sidebar.last_mode = ViewMode::Project;
        sidebar
    })
}

struct NavigationSelectionFixture {
    sidebar: Entity<Sidebar>,
    text: Entity<gpui_component::text::TextViewState>,
}

impl Render for NavigationSelectionFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let control = self.sidebar.update(cx, |sidebar, cx| {
            sidebar
                .control(
                    "selection-session",
                    "Session",
                    Action::Select(Selection::Session(SessionId::from_raw(1))),
                    cx,
                )
                .debug_selector(|| "selection-session".into())
                .w(gpui::px(100.))
                .h(gpui::px(40.))
        });
        div()
            .size_full()
            .child(control)
            .child(
                div()
                    .id("consumed-navigation")
                    .debug_selector(|| "consumed-navigation".into())
                    .h(gpui::px(40.))
                    .on_click(|_, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .debug_selector(|| "selection-transcript".into())
                    .h(gpui::px(80.))
                    .child(gpui_component::text::TextView::new(&self.text).selectable(true)),
            )
    }
}

fn navigation_selection_fixture(cx: &mut gpui::TestAppContext) -> &mut gpui::VisualTestContext {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        let text = cx.new(|cx| {
            gpui_component::text::TextViewState::markdown("Transcript must remain unselected", cx)
        });
        let fixture = cx.new(|_| NavigationSelectionFixture { sidebar, text });
        gpui_component::Root::new(fixture, window, cx)
    });
    draw(cx);
    cx
}

fn assert_empty_cross_project_launch(
    cx: &mut gpui::TestAppContext,
    collapsed: bool,
    projects: bool,
    list: bool,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store {
            default_landing_page: if list {
                grove_core::storage::LandingPage::Sessions
            } else {
                grove_core::storage::LandingPage::Projects
            },
            ..Default::default()
        };
        if projects {
            for name in ["alpha", "beta"] {
                let path = format!("/grove-empty-multi-{name}");
                store.projects.push(grove_core::storage::Project {
                    name: name.into(),
                    path: path.clone(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                });
                store.assign_project_to_active_workspace(&path);
            }
        }
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = add_navigation_component(cx, |window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let requests = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.update(|window, cx| {
        let requests = requests.clone();
        cx.subscribe(&sidebar, move |_, event, _| {
            if matches!(event, SidebarEvent::NewMultiProjectSession) {
                requests.set(requests.get() + 1);
            }
        })
        .detach();
        sidebar.update(cx, |sidebar, cx| {
            if list {
                sidebar.mode = ViewMode::List;
            }
            assert!(sidebar.multi_project_sessions(cx).is_empty());
            assert_eq!(sidebar.snapshot.projects.is_empty(), !projects);
            if collapsed {
                sidebar.toggle_sidebar(window, cx);
            }
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    assert!(cx.debug_bounds("multi-project-sessions").is_none());
    assert!(cx.debug_bounds("multi-project-anchor").is_none());
    if list && !collapsed {
        let launch = cx
            .debug_bounds("projects-add")
            .expect("session list launcher");
        cx.simulate_click(launch.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(requests.get(), 1);
    } else {
        assert_eq!(requests.get(), 0);
    }
}

fn assert_workspace_switches_retain_view_mode(
    cx: &mut gpui::TestAppContext,
    sidebar_mode: ViewMode,
) {
    let paths = [
        "/grove-view-workspace-a",
        "/grove-view-workspace-b",
        "/grove-view-workspace-c",
    ];
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
            sidebar_width: Some(310.0),
            ..Default::default()
        };
        store.assign_project_to_active_workspace(paths[0]);
        store.workspaces.create("B").unwrap();
        store.assign_project_to_active_workspace(paths[1]);
        store.workspaces.create("C").unwrap();
        store.assign_project_to_active_workspace(paths[2]);
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            sidebar.act(Action::Mode(sidebar_mode), window, cx);
            sidebar.selection = Some(Selection::Project(0));
            sidebar
                .scroll
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(-40.0)));
        });
        // A -> B retains the explicitly selected project/list organization.
        cx.global_mut::<SettingsState>().store.workspaces.select(2);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert!(sidebar.dashboard_open);
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            assert_eq!(sidebar.mode, sidebar_mode);
            assert_eq!(sidebar.last_mode, sidebar_mode);
            assert_eq!(sidebar.selection, Some(Selection::Project(1)));
            assert_eq!(sidebar.scroll.offset().y, gpui::px(0.0));
            sidebar
                .scroll
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(-80.0)));
            sidebar.toggle_grid(window, cx);
        });
        // B -> C retains grid, including its global return organization.
        cx.global_mut::<SettingsState>().store.workspaces.select(3);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert!(sidebar.dashboard_open);
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            assert_eq!(sidebar.mode, ViewMode::Grid);
            assert_eq!(sidebar.last_mode, sidebar_mode);
            sidebar.toggle_sidebar(window, cx);
            assert_eq!(sidebar.mode, ViewMode::Grid);
            assert!(sidebar.is_collapsed(cx));
            assert_eq!(
                cx.global::<SettingsState>().store.sidebar_width,
                Some(310.0)
            );
        });
        // C -> A restores A's navigation without restoring its former mode.
        cx.global_mut::<SettingsState>().store.workspaces.select(1);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert!(sidebar.dashboard_open);
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            assert_eq!(sidebar.mode, ViewMode::Grid);
            // Grid selects a canvas session; this fixture has no sessions.
            assert_eq!(sidebar.selection, None);
            assert_eq!(sidebar.scroll.offset().y, gpui::px(-40.0));
            assert!(sidebar.is_collapsed(cx));
            sidebar.toggle_grid(window, cx);
            assert_eq!(sidebar.mode, sidebar_mode);
            sidebar.select(Selection::Project(0), cx);
            sidebar.sync(window, cx);
            assert!(!sidebar.dashboard_open);
        });
        // Revisiting C must honor the latest explicit exit from grid.
        cx.global_mut::<SettingsState>().store.workspaces.select(3);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert!(sidebar.dashboard_open);
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            assert_eq!(sidebar.mode, ViewMode::Project);
            assert_eq!(sidebar.last_mode, ViewMode::Project);
            assert_eq!(sidebar.selection, Some(Selection::Project(2)));
            assert!(sidebar.is_collapsed(cx));
        });
        cx.global_mut::<SettingsState>().store.workspaces.select(2);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            assert!(sidebar.dashboard_open);
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            assert_eq!(sidebar.mode, ViewMode::Project);
            assert_eq!(sidebar.selection, Some(Selection::Project(1)));
            assert_eq!(sidebar.scroll.offset().y, gpui::px(-80.0));
        });
    });
}

pub(super) struct ChangedGitRepo(std::path::PathBuf);

impl ChangedGitRepo {
    pub(super) fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "grove-sidebar-git-test-{}-{unique}",
            std::process::id()
        ));
        fs_err::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "-q"]);
        fs_err::write(repo.0.join("tracked.txt"), "first\n").unwrap();
        repo.git(&["add", "tracked.txt"]);
        repo.git(&[
            "-c",
            "user.name=Grove Test",
            "-c",
            "user.email=grove@example.invalid",
            "commit",
            "--no-gpg-sign",
            "-q",
            "-m",
            "baseline",
        ]);
        fs_err::write(repo.0.join("tracked.txt"), "first\nsecond\n").unwrap();
        repo
    }

    pub(super) fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn git(&self, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

impl Drop for ChangedGitRepo {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.0);
    }
}

fn assert_session_list_cards(cx: &mut gpui::TestAppContext, list_only: bool) {
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    let repo = ChangedGitRepo::new();
    let path = repo.path();

    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
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
        let registry = runtime.read(cx).registry.clone();
        let ids = [Agent::Codex, Agent::Claude]
            .into_iter()
            .map(|agent| {
                registry.update(cx, |registry, _| {
                    if list_only && agent == Agent::Claude {
                        registry.insert_meta_with_context(
                            "demo".into(),
                            path.clone(),
                            agent,
                            vec![grove_core::session_meta::ContextRoot {
                                project: "other".into(),
                                wt_path: "/other".into(),
                            }],
                            None,
                        )
                    } else {
                        registry.insert_meta("demo".into(), path.clone(), agent)
                    }
                })
            })
            .collect::<Vec<_>>();
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: path.clone(),
                    branch: "feature/session-cards".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
            tree.apply_git_poll(
                HashMap::from([(
                    path.clone(),
                    grove_core::git::WorktreeGitState {
                        dirty: true,
                        added: 3,
                        removed: 2,
                        ..Default::default()
                    },
                )]),
                &[],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.mode = ViewMode::List;
        sidebar.snapshot = TreeSnapshot {
            projects: vec![SnapshotProject {
                idx: 0,
                name: "demo".into(),
                sessions: ids.clone(),
                worktrees: vec![SnapshotWorktree {
                    path: path.clone(),
                    name: "demo".into(),
                    branch: "feature/session-cards".into(),
                    sessions: ids.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            total_projects: 1,
        };
        sidebar.select(Selection::Session(ids[1]), cx);
        sidebar
    });
    for width in [1280.0, 320.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0)));
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        for (card_id, close_id) in [
            ("session-1", "close-session-1"),
            ("session-2", "close-session-2"),
        ] {
            let card = cx.debug_bounds(card_id).unwrap();
            let close = cx.debug_bounds(close_id).unwrap();
            assert!(card.left() >= rail.left() + gpui::px(SPACE_LG - 1.0));
            assert!(card.right() <= rail.right() - gpui::px(SPACE_LG - 1.0));
            assert_eq!(
                cx.debug_bounds("sidebar-heading-label").unwrap().left(),
                card.left()
            );
            assert_eq!(
                cx.debug_bounds("session-list-section-heading")
                    .unwrap()
                    .left(),
                card.left()
            );
            assert_eq!(
                cx.debug_bounds(if card_id == "session-1" {
                    "session-list-title-1"
                } else {
                    "session-list-title-2"
                })
                .unwrap()
                .left(),
                card.left() + gpui::px(SPACE_LG)
            );
            assert!(card.size.height >= gpui::px(SESSION_LIST_CARD_H));
            assert!(close.left() >= card.left() && close.right() <= card.right());
            assert!(close.top() >= card.top() && close.bottom() <= card.bottom());
            assert!(cx.debug_bounds("diff-chip-open-1").is_none());
        }
        for (title_id, project_id, worktree_id, status_id, done_id) in [
            (
                "session-list-title-1",
                "session-list-context-1",
                "session-list-worktree-1",
                "session-list-status-1",
                "close-session-1",
            ),
            (
                "session-list-title-2",
                "session-list-context-2",
                "session-list-worktree-2",
                "session-list-status-2",
                "close-session-2",
            ),
        ] {
            let title = cx.debug_bounds(title_id).unwrap();
            let project = cx.debug_bounds(project_id).unwrap();
            let worktree = cx.debug_bounds(worktree_id).unwrap();
            let status = cx.debug_bounds(status_id).unwrap();
            let done = cx.debug_bounds(done_id).unwrap();
            assert!(project.bottom() <= title.top());
            assert!(worktree.bottom() <= title.top());
            assert!(worktree.left() >= project.right());
            let provider = cx
                .debug_bounds(if project_id == "session-list-context-1" {
                    "session-list-agent-1"
                } else {
                    "session-list-agent-2"
                })
                .unwrap();
            assert!(provider.left() >= worktree.right());
            assert_eq!(provider.right(), title.right());
            assert!(provider.bottom() <= title.top());
            assert!(status.top() >= title.bottom());
            assert!(status.right() <= done.left());
            assert!(done.right() <= title.right());
            assert!(cx.debug_bounds("session-number-1").is_none());
            cx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
            draw(cx);
            assert_eq!(cx.debug_bounds(title_id).unwrap(), title);
            assert_eq!(cx.debug_bounds(status_id).unwrap(), status);
        }
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(2)))
        );
    }
    if list_only {
        sidebar.read_with(cx, |sidebar, cx| {
            assert_eq!(
                sidebar.visible_session_order(cx),
                vec![SessionId::from_raw(2), SessionId::from_raw(1)]
            );
            let grouped: Vec<_> = sidebar
                .list_session_groups(cx)
                .into_iter()
                .flat_map(|(_, ids)| ids)
                .collect();
            assert_eq!(
                grouped,
                vec![SessionId::from_raw(2), SessionId::from_raw(1)]
            );
            assert_eq!(sidebar.multi_project_sessions(cx).len(), 1);
        });
    }
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.age_timer.is_some()));
    let before = sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks);
    cx.executor().advance_clock(SESSION_AGE_REFRESH);
    draw(cx);
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks),
        before + 1
    );
    if list_only {
        for (id, selector) in [(1, "close-session-1"), (2, "close-session-2")] {
            let done = cx.debug_bounds(selector).unwrap();
            cx.simulate_mouse_move(done.center(), None, gpui::Modifiers::default());
            draw(cx);
            cx.simulate_click(done.center(), gpui::Modifiers::default());
            draw(cx);
            sidebar.read_with(cx, |sidebar, cx| {
                assert!(sidebar.pending_close.is_none());
                assert!(sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(SessionId::from_raw(id))
                    .is_none());
                if id == 1 {
                    assert_eq!(
                        sidebar.selection,
                        Some(Selection::Session(SessionId::from_raw(2)))
                    );
                    assert!(sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .meta(SessionId::from_raw(2))
                        .is_some());
                }
            });
        }
        return;
    }
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.age_timer.is_none()));
    let project = cx.debug_bounds("project-title-0").unwrap();
    let worktree_selector: &'static str =
        Box::leak(format!("worktree-title-{path}").into_boxed_str());
    let worktree = cx.debug_bounds(worktree_selector).unwrap();
    let first = cx.debug_bounds("session-agent-1").unwrap();
    let second = cx.debug_bounds("session-agent-2").unwrap();
    assert_eq!(worktree.left() - project.left(), gpui::px(SPACE_2XL));
    let status = cx.debug_bounds("session-tree-status-1").unwrap();
    assert!(cx.debug_bounds("session-tree-title-1").unwrap().right() <= status.left());
    assert!(first.right() <= cx.debug_bounds("session-tree-title-1").unwrap().left());
    assert_eq!(first.left(), second.left());
    // Expanded session items omit checkout metadata; only the worktree owns changes.
    assert!(cx.debug_bounds("diff-chip-open-1").is_none());
    assert!(cx.debug_bounds("diff-chip-open-2").is_none());
    assert!(cx.debug_bounds("session-tree-diff-1").is_none());
    assert!(cx.debug_bounds("session-tree-diff-2").is_none());
    let worktree_diff: &'static str = Box::leak(format!("worktree-diff-{path}").into_boxed_str());
    assert!(cx.debug_bounds(worktree_diff).is_some());
    assert!(cx.debug_bounds("session-number-1").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
    });
    draw(cx);
    // Metadata clicks bubble to the session card for both provider identities.
    for (id, provider) in [(1, "Codex"), (2, "Claude")] {
        let counts = cx
            .debug_bounds(if id == 1 {
                "session-list-worktree-1"
            } else {
                "session-list-worktree-2"
            })
            .unwrap();
        cx.simulate_click(counts.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(id))),
            "{provider} worktree metadata selects its owning session"
        );
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_none()),
            "{provider} metadata must not open a diff"
        );
    }
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[cfg(unix)]
#[gpui::test]
fn codex_launch_action_creates_ready_native_chat_and_keeps_composer_focus(
    cx: &mut gpui::TestAppContext,
) {
    if std::env::var_os("GROVE_NATIVE_LAUNCH_TEST_CHILD").is_none() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "grove-native-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(root.join("bin")).unwrap();
        fs_err::create_dir_all(root.join("config")).unwrap();
        let provider = root.join("bin/codex");
        fs_err::write(
                &provider,
                r#"#!/usr/bin/python3
import json,sys
from pathlib import Path
Path(__file__).resolve().parent.parent.joinpath("provider-invocation").write_text(str(Path(__file__).resolve()))
for line in sys.stdin:
    request=json.loads(line)
    method=request.get("method")
    ident=request.get("id")
    result=None
    if method=="initialize": result={"userAgent":"fixture"}
    elif method=="config/read": result={"config":{}}
    elif method in ("thread/start","thread/resume"):
        result={"thread":{"id":"fixture-chat","turns":[]}}
    if result is not None:
        print(json.dumps({"id":ident,"result":result}),flush=True)
"#,
            )
            .unwrap();
        fs_err::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shell = root.join("bin/fixture-shell");
        fs_err::write(
                &shell,
                "#!/bin/sh\nif [ \"$1\" = \"-lic\" ]; then\n  shift 3\n  exec \"$@\"\nfi\nexec /bin/sh \"$@\"\n",
            )
            .unwrap();
        fs_err::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(std::iter::once(root.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "views::sidebar::tests::codex_launch_action_creates_ready_native_chat_and_keeps_composer_focus", "--nocapture"])
                .env("GROVE_NATIVE_LAUNCH_TEST_CHILD", &root)
                .env("GROVE_CONFIG_DIR", root.join("config"))
                .env("GROVE_TEST_CONFIG_DIR", root.join("config"))
                .env("SHELL", shell)
                .env("PATH", path)
                .output().unwrap();
        let _ = fs_err::remove_dir_all(&root);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let path = std::env::var("GROVE_NATIVE_LAUNCH_TEST_CHILD").unwrap();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Native launch".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            tmux_enabled: Some(true),
            native_agents_enabled: true,
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
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::Launch(0, path.clone(), Agent::Codex), window, cx);
            assert!(sidebar.content_error.is_none());
            assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 1);
        });
    });
    // Starting a real fixture subprocess can take longer on a busy CI runner.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        cx.executor().advance_clock(Duration::from_millis(50));
        draw(cx);
        let (status, error) = sidebar.read_with(cx, |sidebar, cx| {
            let registry = sidebar.runtime.read(cx).registry.read(cx);
            let id = registry.all()[0].id;
            assert!(
                registry.session(id).is_none(),
                "Codex must not attach a PTY"
            );
            let state = &registry.codex(id).unwrap().read(cx).state;
            (state.status, state.error.clone())
        });
        if status == grove_core::codex::Status::Idle {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Native launch did not become ready: status={status:?}, error={error:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs_err::read_to_string(std::path::Path::new(&path).join("provider-invocation")).unwrap(),
        std::path::Path::new(&path)
            .join("bin/codex")
            .canonicalize()
            .unwrap()
            .to_string_lossy(),
        "native launch must execute the isolated provider fixture"
    );
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let id = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
            let view = sidebar
                .codex_views
                .get(&id)
                .expect("native conversation rendered")
                .clone();
            assert!(
                view.focus_handle(cx).contains_focused(window, cx),
                "new native chat must focus its composer"
            );
            sidebar.focus_active_session_after_palette(window, cx);
            assert!(view.focus_handle(cx).contains_focused(window, cx));
            assert!(sidebar.pending_canvas_focus.is_none());
            assert!(sidebar.pending_grid_workspace_focus.is_none());
            sidebar.focus.focus(window, cx);
            sidebar.navigate_to_session(id, window, cx);
            assert!(view.focus_handle(cx).contains_focused(window, cx));
            sidebar.focus.focus(window, cx);
            sidebar.act(Action::Select(Selection::Session(id)), window, cx);
            assert!(view.focus_handle(cx).contains_focused(window, cx));
            sidebar.mode = ViewMode::Grid;
            sidebar.focus.focus(window, cx);
            sidebar.act(Action::Select(Selection::Session(id)), window, cx);
            assert!(view.focus_handle(cx).contains_focused(window, cx));
            sidebar.focus_active_session_after_palette(window, cx);
            assert!(view.focus_handle(cx).contains_focused(window, cx));
            cx.global_mut::<SettingsState>().store.native_agents_enabled = false;
            sidebar.act(Action::Launch(0, path.clone(), Agent::Codex), window, cx);
            assert!(sidebar.content_error.is_none());
            let fallback = {
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                assert_eq!(registry.len(), 2);
                assert!(
                    registry.codex(id).is_some(),
                    "existing native chat stays open"
                );
                let fallback = registry.all().iter().find(|meta| meta.id != id).unwrap().id;
                assert!(registry.codex(fallback).is_none());
                assert!(
                    registry.session(fallback).is_some(),
                    "disabled native agents use a PTY"
                );
                fallback
            };
            sidebar
                .runtime
                .update(cx, |runtime, cx| runtime.kill_session(fallback, cx));
            let stopped = sidebar
                .runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, _| registry.stop_codex_sessions());
            for receiver in stopped {
                receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    });
}

fn attach_removal_layout_session(
    sidebar: &mut Sidebar,
    path: &str,
    provider: grove_core::codex::Provider,
    window: &mut Window,
    cx: &mut Context<Sidebar>,
) {
    let registry = sidebar.runtime.read(cx).registry.clone();
    let id = registry.update(cx, |registry, _| {
        registry.insert_meta("Layout fixture".into(), path.into(), provider.agent())
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
        provider,
        composer: grove_core::codex::ComposerSettings::default(),
        context: None,
        context_details: None,
        key: "layout-fixture".into(),
        project: "Layout fixture".into(),
        cwd: path.into(),
        label: provider.label().into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: None,
        items: vec![
            serde_json::json!({"type":"userMessage", "text":"Keep this conversation in place"}),
        ]
        .into(),
        work_periods: Vec::new(),
        draft: "Preserved draft".into(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        activity_reporting: false,
    };
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
    sidebar.select(Selection::Session(id), cx);
    sidebar
        .worktree_delete_focus
        .insert(path.into(), cx.focus_handle());
    sidebar.sync(window, cx);
    cx.notify();
}

#[gpui::test]
fn project_move_preserves_session_and_source_navigation(cx: &mut gpui::TestAppContext) {
    if std::env::var_os("GROVE_MOVE_SIDEBAR_TEST_CHILD").is_none() {
        let root = std::env::temp_dir().join(format!(
            "grove-move-sidebar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "views::sidebar::tests::project_move_preserves_session_and_source_navigation",
                "--nocapture",
            ])
            .env("GROVE_MOVE_SIDEBAR_TEST_CHILD", "1")
            .env("GROVE_CONFIG_DIR", &root)
            .env("GROVE_TEST_CONFIG_DIR", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let repo = ChangedGitRepo::new();
    let path = repo
        .0
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Moving".into(),
            path: path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&path);
        store.workspaces.create("Destination").unwrap();
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    draw(cx);
    let id = cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let registry = sidebar.runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, cx| {
                let id = registry.insert_meta("Moving".into(), path.clone(), Agent::Codex);
                cx.notify();
                id
            });
            sidebar.sync(window, cx);
            sidebar.act(Action::Select(Selection::Session(id)), window, cx);
            sidebar.act(Action::Menu(0), window, cx);
            sidebar.act(Action::MoveProject(path.clone()), window, cx);
            id
        })
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar.project_panel.is_none());
            assert_eq!(sidebar.selection, Some(Selection::Session(id)));
            assert!(sidebar.project_menu_focus[&0].is_focused(window));
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                1
            );
            sidebar.act(Action::Menu(0), window, cx);
            sidebar.act(Action::MoveProject(path.clone()), window, cx);
        });
    });
    draw(cx);
    // Pointer chooses the destination; keyboard traverses to the explicit primary action.
    let destination = cx.debug_bounds("project-move-destination-2").unwrap();
    cx.simulate_click(destination.center(), gpui::Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes("tab tab tab tab enter");
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar.project_panel.is_none());
            assert_eq!(sidebar.selection, None);
            assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                2
            );
            assert_eq!(
                sidebar
                    .runtime
                    .read(cx)
                    .toast
                    .read(cx)
                    .current()
                    .unwrap()
                    .message,
                "Moved Moving to Destination."
            );
            assert!(sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(id)
                .is_some());
            assert!(!sidebar
                .snapshot
                .projects
                .iter()
                .any(|project| project.sessions.contains(&id)));
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.sync(window, cx);
            assert!(sidebar
                .snapshot
                .projects
                .iter()
                .any(|project| project.sessions.contains(&id)));
        });
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::MoveProject(path.clone()), window, cx);
        });
    });
    draw(cx);
    let destination = cx.debug_bounds("project-move-destination-1").unwrap();
    cx.simulate_click(destination.center(), gpui::Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes("tab tab space tab tab enter");
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            assert!(sidebar.project_panel.is_none());
            assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                1
            );
            assert_eq!(sidebar.selection, Some(Selection::Project(0)));
            assert!(sidebar.project_menu_focus[&0].is_focused(window));
            assert!(sidebar
                .snapshot
                .projects
                .iter()
                .any(|project| project.sessions.contains(&id)));
            assert!(sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(id)
                .is_some());
        });
    });
}
