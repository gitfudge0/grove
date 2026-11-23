use super::*;
use grove_core::storage::{Project, ProjectScripts, RecentLaunch, Store};
use std::process::Command;

struct TempProjects(std::path::PathBuf);

impl Drop for TempProjects {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.0);
    }
}

fn project(name: &str, path: &str) -> Project {
    Project {
        name: name.into(),
        path: path.into(),
        scripts: ProjectScripts::default(),
        archived: false,
        worktree_dir: None,
    }
}

fn setup(cx: &mut App) {
    gpui_component::init(cx);
    let root = env!("CARGO_MANIFEST_DIR").to_string();
    let missing = "/grove-launcher-test-missing".to_string();
    let outside = "/grove-launcher-test-outside".to_string();
    let mut store = Store {
        projects: vec![
            project("current", &root),
            project("missing", &missing),
            project("outside", &outside),
        ],
        recent_launches: vec![
            RecentLaunch {
                project: "current".into(),
                wt_path: root.clone(),
                agent: Agent::Terminal,
            },
            RecentLaunch {
                project: "missing".into(),
                wt_path: "/grove-launcher-test-stale".into(),
                agent: Agent::Terminal,
            },
            RecentLaunch {
                project: "outside".into(),
                wt_path: outside.clone(),
                agent: Agent::Terminal,
            },
        ],
        ..Store::default()
    };
    store.workspaces.create("Other").expect("workspace");
    let other = store.workspaces.active;
    store.project_workspaces.insert(outside, other);
    store.workspaces.select(1);
    cx.set_global(SettingsState::new(store));
    cx.set_global(crate::zoom::ZoomState::new(1.0));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(crate::theme::ThemeState::new(
        false,
        "tokyonight".into(),
        "tokyonight-day".into(),
    ));
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn enter_picker(
    launcher: &mut WorktreeLauncher,
    command: PaletteRow,
    window: &mut Window,
    cx: &mut Context<WorktreeLauncher>,
) {
    let rows = launcher.rows(cx);
    let index = rows
        .iter()
        .position(|row| *row == command)
        .expect("picker command");
    launcher.selected = index;
    launcher.anchor = Some(launcher::row_identity(&rows[index]));
    launcher.activate(window, cx);
}

#[gpui::test]
fn project_picker_opens_dashboard_without_waiting_for_cold_worktree_cache(
    cx: &mut gpui::TestAppContext,
) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let temp = TempProjects(std::env::temp_dir().join(format!(
        "grove-palette-cold-cache-{}-{unique}",
        std::process::id()
    )));
    fs_err::create_dir(&temp.0).expect("temporary project parent");
    let grove = temp.0.join("grove");
    let api = temp.0.join("api");
    let feature = temp.0.join("feature-auth");
    for project in [&grove, &api] {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(project)
            .status()
            .expect("git init")
            .success());
    }
    assert!(Command::new("git")
        .args(["-C"])
        .arg(&api)
        .args([
            "-c",
            "user.name=grove-test",
            "-c",
            "user.email=grove-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Initial commit",
        ])
        .status()
        .expect("git commit")
        .success());
    assert!(Command::new("git")
        .args(["-C"])
        .arg(&api)
        .args(["worktree", "add", "-q", "-b", "feature-auth"])
        .arg(&feature)
        .status()
        .expect("git worktree add")
        .success());
    let grove = fs_err::canonicalize(grove)
        .expect("Grove project")
        .to_string_lossy()
        .into_owned();
    let api = fs_err::canonicalize(api)
        .expect("API project")
        .to_string_lossy()
        .into_owned();
    let feature = fs_err::canonicalize(feature)
        .expect("API worktree")
        .to_string_lossy()
        .into_owned();

    cx.update(|cx| {
        setup(cx);
        let store = &mut cx.global_mut::<SettingsState>().store;
        store.projects = vec![project("Grove", &grove), project("API", &api)];
        store.project_workspaces.clear();
        store.assign_project_to_active_workspace(&grove);
        store.assign_project_to_active_workspace(&api);
    });
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            let tree = view.runtime.read(cx).tree.clone();
            assert!(tree.read(cx).worktrees_for_project(1, 0).is_empty());
            view.launch_probe = Some(Vec::new());
            view.open(window, cx);
            enter_picker(view, PaletteRow::NewSession, window, cx);
            view.update_query("API".into(), cx);
            assert!(matches!(
                view.selected_row(cx),
                Some(PaletteRow::Combo { proj: 1, .. })
            ));
            view.activate(window, cx);
            assert!(!view.is_open());
            assert!(!view.loading_worktrees);
            assert_eq!(
                view.sidebar.read(cx).dashboard_destination_for_test(cx),
                Some((1, api.clone()))
            );
            assert!(view.launch_probe.as_ref().is_some_and(Vec::is_empty));
            tree.update(cx, |tree, _| tree.rebuild_wt_cache());
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            view.open(window, cx);
            enter_picker(view, PaletteRow::NewMultiProjectSession, window, cx);
            assert!(!view.loading_worktrees);
            assert!(view.rows(cx).iter().any(|row| matches!(row,
                PaletteRow::Combo { proj: 1, wt_path, .. } if wt_path == &feature
            )));
            view.update_query("API feature-auth".into(), cx);
            assert_eq!(view.rows(cx).len(), 1);
        });
    });
}

#[gpui::test]
fn new_session_project_selection_opens_dashboard_without_launching(cx: &mut gpui::TestAppContext) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let temp = TempProjects(
        std::env::temp_dir().join(format!("grove-palette-api-{}-{unique}", std::process::id())),
    );
    fs_err::create_dir(&temp.0).expect("temporary project parent");
    let api = temp.0.join("feature-auth");
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&api)
        .status()
        .expect("git init")
        .success());
    let api = fs_err::canonicalize(&api)
        .expect("API project")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        setup(cx);
        let store = &mut cx.global_mut::<SettingsState>().store;
        store.projects.push(project("API", &api));
        store.assign_project_to_active_workspace(&api);
        store.default_agent = Some(Agent::Terminal);
    });
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            view.launch_probe = Some(Vec::new());
            view.open(window, cx);
        });
    });
    draw(cx);
    cx.simulate_input("new session");
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            launcher.read(cx).selected_row(cx),
            Some(PaletteRow::NewSession)
        );
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|window, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.mode, PaletteMode::Single);
        assert!(view.input.focus_handle(cx).is_focused(window));
    });
    cx.simulate_input("API");
    draw(cx);
    let project_row = cx.debug_bounds("launcher-row-0").expect("project row");
    cx.simulate_click(project_row.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        assert!(!view.is_open());
        assert_eq!(view.launch_probe.as_deref(), Some(&[][..]));
        assert_eq!(
            view.sidebar.read(cx).dashboard_destination_for_test(cx),
            Some((3, api))
        );
        assert_eq!(view.runtime.read(cx).registry.read(cx).len(), 0);
    });
}

#[gpui::test]
fn picker_escape_clears_multi_selection_before_single_launch(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    let _prior = cx.update(|window, cx| {
        let prior = cx.focus_handle();
        prior.focus(window, cx);
        launcher.update(cx, |view, cx| {
            view.launch_probe = Some(Vec::new());
            view.open(window, cx);
            enter_picker(view, PaletteRow::NewMultiProjectSession, window, cx);
        });
        prior
    });
    draw(cx);
    cx.simulate_keystrokes("shift-space escape");
    draw(cx);
    cx.update(|window, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.mode, PaletteMode::Root);
        assert_eq!(view.selected_worktrees.count(), 0);
        assert!(view.query.is_empty());
        assert!(view.input.focus_handle(cx).is_focused(window));
    });
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            enter_picker(view, PaletteRow::NewSession, window, cx);
            assert_eq!(view.selected_worktrees.count(), 0);
            view.activate(window, cx);
            assert!(!view.is_open());
            assert_eq!(view.launch_probe.as_deref(), Some(&[][..]));
        });
    });
}

#[gpui::test]
fn single_no_match_and_multi_empty_selection_do_not_launch(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            view.open(window, cx);
            enter_picker(view, PaletteRow::NewSession, window, cx);
            view.update_query("no matching worktree".into(), cx);
            assert!(view.rows(cx).is_empty());
            view.activate(window, cx);
            assert!(view.is_open());
            assert!(view.runtime.read(cx).registry.read(cx).all().is_empty());
            view.close(window, cx);
            view.open(window, cx);
            enter_picker(view, PaletteRow::NewMultiProjectSession, window, cx);
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            view.activate(window, cx);
            assert_eq!(view.error.as_deref(), Some("Select one or more worktrees."));
            assert!(view.runtime.read(cx).registry.read(cx).all().is_empty());
        });
    });
}

#[gpui::test]
fn scoped_open_skips_project_search_and_preserves_selected_worktree(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    let path = env!("CARGO_MANIFEST_DIR").to_string();
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.open_for_worktree(0, &path, window, cx);
        });
        let launcher = launcher.read(cx);
        assert!(launcher.is_open());
        assert!(matches!(&launcher.anchor,
                Some(RowIdentity::Session { proj: 0, wt_path, .. }) if wt_path == &path));
        assert!(launcher.agent_focus);
        assert!(launcher.focus.is_focused(window));
    });
    draw(cx);
    assert!(cx.debug_bounds("launcher-agent-selector-0").is_some());
    cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.close(window, cx);
                launcher.open_for_worktree(1, "/grove-launcher-test-missing", window, cx);
                assert!(matches!(&launcher.anchor,
                    Some(RowIdentity::Session { proj: 1, wt_path, .. }) if wt_path == "/grove-launcher-test-missing"));
            });
        });
}

#[gpui::test]
fn rows_scope_filter_and_validate_recents(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|_, cx| {
        assert!(launcher
            .read(cx)
            .sidebar
            .read(cx)
            .visible_session_targets(cx)
            .is_empty());
        let rows = launcher.read(cx).rows(cx);
        assert!(matches!(
            rows.first(),
            Some(PaletteRow::Recent { proj: 0, .. })
        ));
        assert!(rows.iter().any(|row| matches!(row, PaletteRow::Settings)));
        assert!(rows.iter().any(|row| matches!(row, PaletteRow::AddProject)));
        assert!(rows
            .iter()
            .any(|row| matches!(row, PaletteRow::SwitchToSession)));
        assert!(!rows
            .iter()
            .any(|row| matches!(row, PaletteRow::Recent { proj: 1, .. })));
        assert!(!rows.iter().any(|row| matches!(
            row,
            PaletteRow::Recent { proj: 2, .. } | PaletteRow::Combo { proj: 2, .. }
        )));
        launcher.update(cx, |launcher, cx| {
            launcher.update_query("missing".into(), cx);
            let initial = launcher.rows(cx);
            let combo = initial
                .iter()
                .position(|row| matches!(row, PaletteRow::Combo { proj: 1, .. }))
                .expect("uncached project root");
            let query = match &initial[combo] {
                PaletteRow::Combo { wt_path, .. } => worktree_name(wt_path).to_string(),
                _ => unreachable!(),
            };
            launcher.selected = combo;
            launcher.anchor = Some(launcher::row_identity(&initial[combo]));
            let identity = launcher.anchor.clone();
            launcher.update_query(query, cx);
            assert_eq!(launcher.anchor, identity);
            assert_eq!(
                launcher
                    .rows(cx)
                    .get(launcher.selected)
                    .map(launcher::row_identity),
                identity
            );
            launcher.update_query("absent query".into(), cx);
            assert!(launcher.rows(cx).is_empty());
        });
    });
}

#[gpui::test]
fn recent_worktree_has_one_row_with_most_recent_agent(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        setup(cx);
        let store = &mut cx.global_mut::<SettingsState>().store;
        let path = env!("CARGO_MANIFEST_DIR").to_string();
        let newest_agent = Agent::ALL
            .into_iter()
            .find(|agent| *agent != Agent::Terminal && agent.available())
            .unwrap_or(Agent::Terminal);
        store.recent_launches = vec![
            RecentLaunch {
                project: "current".into(),
                wt_path: path.clone(),
                agent: newest_agent,
            },
            RecentLaunch {
                project: "current".into(),
                wt_path: path,
                agent: Agent::Terminal,
            },
        ];
    });
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.open(window, cx);
            let expected = cx.global::<SettingsState>().store.recent_launches[0].agent;
            let rows = launcher.rows(cx);
            assert_eq!(
                rows.iter()
                    .filter(|row| matches!(row, PaletteRow::Recent { proj: 0, .. }))
                    .count(),
                1
            );
            assert!(
                matches!(rows.first(), Some(PaletteRow::Recent { agent, .. }) if *agent == expected)
            );
            assert_eq!(Agent::ALL[launcher.agent_selected], expected);

            launcher.update_query("current".into(), cx);
            assert_eq!(
                launcher
                    .rows(cx)
                    .iter()
                    .filter(|row| matches!(row,
                        PaletteRow::Recent { proj: 0, wt_path, .. }
                            | PaletteRow::Combo { proj: 0, wt_path, .. }
                            if wt_path == env!("CARGO_MANIFEST_DIR")
                    ))
                    .count(),
                1
            );
        });
    });
}

#[gpui::test]
fn command_rows_and_setting_search_are_present(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|_, cx| {
        let rows = launcher.read(cx).rows(cx);
        for action in [
            PaletteRow::NewSession,
            PaletteRow::NewMultiProjectSession,
            PaletteRow::TerminalHome,
            PaletteRow::AddProject,
            PaletteRow::Settings,
        ] {
            assert!(rows.contains(&action), "missing {action:?}");
        }
        launcher.update(cx, |launcher, cx| {
            launcher.update_query("appearance".into(), cx);
            assert!(launcher
                .rows(cx)
                .contains(&PaletteRow::Setting(launcher::SettingRow::Appearance)));
            launcher.update_query("terminal home".into(), cx);
            assert!(launcher.rows(cx).contains(&PaletteRow::TerminalHome));
        });
    });
}

#[gpui::test]
fn tool_icons_follow_tab_focus_and_keyboard_cycles_available_tools(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
    draw(cx);
    let icon_names = [
        "launcher-agent-selector-0",
        "launcher-agent-selector-1",
        "launcher-agent-selector-2",
        "launcher-agent-selector-3",
    ];
    for name in icon_names {
        assert!(
            cx.debug_bounds(name).is_none(),
            "tool icon should be hidden while search has focus"
        );
    }
    let before = cx.update(|_, cx| launcher.read(cx).agent_selected);
    cx.simulate_keystrokes("tab right");
    draw(cx);
    let selected_row = cx.debug_bounds("launcher-row-0").expect("selected row");
    for name in icon_names {
        let bounds = cx.debug_bounds(name).expect("focused tool icon");
        assert!(
            selected_row.contains(&bounds.center()),
            "tool icon should be inline in the selected row"
        );
    }
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        let available = available_agents();
        let before_agent = Agent::ALL[before];
        let expected = available[launcher::cycle(
            launcher::agent_sel_for(&available, before_agent),
            1,
            available.len(),
        )];
        assert_eq!(Agent::ALL[view.agent_selected], expected);
        assert!(view.agent_focus);
        assert!(view.agent_touched);
    });
    let selected_agent = cx.update(|_, cx| launcher.read(cx).agent_selected);
    let selected_icon = cx
        .debug_bounds(icon_names[selected_agent])
        .expect("selected tool icon");
    cx.simulate_click(selected_icon.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        assert!(
            view.is_open(),
            "tool click should not activate the parent row"
        );
        assert_eq!(view.selected, 0);
        assert_eq!(view.agent_selected, selected_agent);
    });
    cx.simulate_keystrokes("tab");
    draw(cx);
    for name in icon_names {
        assert!(
            cx.debug_bounds(name).is_none(),
            "tool icon should hide when Tab returns focus to search"
        );
    }
    cx.update(|window, cx| {
        let view = launcher.read(cx);
        assert!(!view.agent_focus);
        assert!(view.input.focus_handle(cx).is_focused(window));
    });
}

#[gpui::test]
fn keyboard_selection_agent_step_and_cancel_restore_focus(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    let prior = cx.update(|window, cx| {
        let prior = cx.focus_handle();
        prior.focus(window, cx);
        launcher.update(cx, |launcher, cx| launcher.open(window, cx));
        assert!(launcher.read(cx).is_open());
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
        prior
    });
    draw(cx);
    cx.simulate_keystrokes("up");
    draw(cx);
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.selected, view.rows(cx).len() - 1);
        assert_eq!(
            view.anchor,
            view.rows(cx).last().map(launcher::row_identity)
        );
    });
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        assert!(launcher.read(cx).agent_focus);
        assert!(launcher.read(cx).focus.is_focused(window));
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(!launcher.read(cx).is_open());
        assert!(prior.is_focused(window));
    });
}

#[gpui::test]
fn palette_exit_retains_paint_releases_focus_and_cancels_on_reopen(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        cx.set_reduce_motion(false);
        let prior = cx.focus_handle();
        prior.focus(window, cx);
        launcher.update(cx, |view, cx| {
            view.open(window, cx);
            view.close(window, cx);
            assert!(!view.is_open());
            assert!(
                view.is_visible(cx),
                "dismissed palette remains painted for its exit"
            );
            assert!(
                prior.is_focused(window),
                "dismissal immediately restores input ownership"
            );
            view.open(window, cx);
        });
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(motion::Presence::duration() * 2);
    cx.run_until_parked();
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            assert!(view.is_open());
            assert!(
                view.is_visible(cx),
                "old dismissal cannot unmount reopened palette"
            );
            view.close(window, cx);
        });
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(motion::Presence::duration() * 2);
    cx.run_until_parked();
    cx.update(|window, cx| {
        launcher.update(cx, |view, cx| {
            assert!(!view.is_visible(cx), "exit completion unmounts palette");
            cx.set_reduce_motion(true);
            view.open(window, cx);
            view.close(window, cx);
            assert!(!view.is_visible(cx));
            assert!(
                view.close_task.is_none(),
                "reduced motion schedules no dismissal timer"
            );
        });
    });
}

#[gpui::test]
fn space_selection_survives_filter_and_agent_back(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            enter_picker(launcher, PaletteRow::NewMultiProjectSession, window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("shift-space");
    draw(cx);
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.selected_worktrees.count(), 1);
        assert!(view.query.is_empty());
    });
    cx.update(|_, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.update_query("no matching worktree".into(), cx);
            assert!(launcher.rows(cx).is_empty());
            assert_eq!(launcher.selected_worktrees.count(), 1);
        });
    });
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.selected_worktrees.count(), 1);
        assert!(view.agent_focus);
        assert!(view.focus.is_focused(window));
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|_, cx| assert_eq!(launcher.read(cx).selected_worktrees.count(), 0));
}

#[gpui::test]
fn search_accepts_spaces_without_toggling_worktrees(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
    draw(cx);
    cx.simulate_input("current");
    cx.simulate_keystrokes("space");
    cx.simulate_input("branch");
    draw(cx);
    cx.update(|_, cx| {
        let view = launcher.read(cx);
        assert_eq!(view.input.read(cx).value().as_ref(), "current branch");
        assert_eq!(view.query, "current branch");
        assert_eq!(view.selected_worktrees.count(), 0);
    });
}

#[gpui::test]
fn selected_stale_path_cannot_launch(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.open(window, cx);
            enter_picker(launcher, PaletteRow::NewMultiProjectSession, window, cx);
            launcher
                .selected_worktrees
                .toggle("/grove-launcher-test-stale");
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.activate(window, cx);
            assert_eq!(
                launcher.error.as_deref(),
                Some("A selected worktree is no longer available in this workspace.")
            );
            assert!(launcher.is_open());
            assert!(launcher
                .runtime
                .read(cx)
                .state
                .read(cx)
                .active_session()
                .is_none());
            assert!(launcher.sidebar.read(cx).selected_session().is_none());
        });
    });
}

#[gpui::test]
fn rejected_launch_keeps_palette_open_and_explains_error(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| launcher.update(cx, |launcher, cx| {
            launcher.open(window, cx);
            launcher.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
                toast.set_error("Codex executable could not be started. Refresh the shell environment in Settings.", cx);
            });
            launcher.finish_launch(false, None, window, cx);
            assert!(launcher.is_open());
            let error = launcher.error.as_deref().unwrap();
            assert!(error.contains("Refresh the shell environment"));
            assert_eq!(launcher.runtime.read(cx).toast.read(cx).current().unwrap().message, error);
            assert!(launcher.runtime.read(cx).registry.read(cx).all().is_empty());
        }));
}

#[gpui::test]
fn registered_failed_session_closes_palette_and_selects_retry_row(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        setup(cx);
        cx.global_mut::<SettingsState>().store.tmux_enabled = Some(false);
    });
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    let terminal = cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            let prior_focus = cx.focus_handle();
            prior_focus.focus(window, cx);
            launcher.open(window, cx);
            let runtime = launcher.runtime.clone();
            let before = runtime.read(cx).state.read(cx).active_session();
            let path = env!("CARGO_MANIFEST_DIR").to_string();
            let did_spawn = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_context(
                    "current".into(),
                    path,
                    Agent::Terminal,
                    vec!["\0".into()],
                    Vec::new(),
                    None,
                    cx,
                )
            });
            assert!(
                did_spawn,
                "launch is accepted before asynchronous startup resolves"
            );
            let failed_id = runtime
                .read(cx)
                .state
                .read(cx)
                .active_session()
                .expect("failed session is registered");
            let terminal = runtime
                .read(cx)
                .registry
                .read(cx)
                .session(failed_id)
                .unwrap()
                .clone();
            launcher.finish_launch(did_spawn, before, window, cx);
            assert!(!launcher.is_open());
            assert_eq!(
                launcher.sidebar.read(cx).selected_session(),
                Some(failed_id)
            );
            assert!(launcher.sidebar.focus_handle(cx).is_focused(window));
            terminal
        })
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while terminal.read_with(cx, |terminal, _| terminal.is_starting())
        && std::time::Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        terminal.read_with(cx, |terminal, _| terminal.spawn_error().is_some()),
        "failed asynchronous startup remains available for retry"
    );
}

#[gpui::test]
fn selected_roots_launch_one_session_in_selection_order(cx: &mut gpui::TestAppContext) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let temp = TempProjects(std::env::temp_dir().join(format!(
        "grove-launcher-test-{}-{unique}",
        std::process::id()
    )));
    fs_err::create_dir(&temp.0).expect("temporary projects");
    let first = temp.0.join("first");
    let second = temp.0.join("second");
    for path in [&first, &second] {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(path)
            .status()
            .expect("git init")
            .success());
    }
    let first = fs_err::canonicalize(&first)
        .expect("first project")
        .to_string_lossy()
        .into_owned();
    let second = fs_err::canonicalize(&second)
        .expect("second project")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        setup(cx);
        let store = &mut cx.global_mut::<SettingsState>().store;
        store.projects = vec![project("first", &first), project("second", &second)];
        store.recent_launches.clear();
        store.assign_project_to_active_workspace(&first);
        store.assign_project_to_active_workspace(&second);
    });
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.launch_probe = Some(Vec::new());
            launcher.open(window, cx);
            enter_picker(launcher, PaletteRow::NewMultiProjectSession, window, cx);
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            let rows = launcher.rows(cx);
            for target in [&second, &first] {
                let index = rows
                    .iter()
                    .position(
                        |row| matches!(row, PaletteRow::Combo { wt_path, .. } if wt_path == target),
                    )
                    .expect("worktree row");
                launcher.selected = index;
                launcher.anchor = Some(launcher::row_identity(&rows[index]));
                launcher.toggle_selected_row(cx);
            }
            assert_eq!(
                launcher.selected_worktrees.selected_targets(),
                vec![second.clone(), first.clone()]
            );
            launcher.agent_selected = launcher::agent_sel_for(&Agent::ALL, Agent::Terminal);
            launcher.activate(window, cx);
            assert!(!launcher.is_open());
        });
        assert_eq!(
            launcher.read(cx).launch_probe.as_deref(),
            Some(&[(vec![second.clone(), first.clone()], Agent::Terminal)][..])
        );
    });
}

#[gpui::test]
fn stale_selection_cannot_launch(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            launcher.open(window, cx);
            launcher.launch(
                RowIdentity::Session {
                    proj: 1,
                    wt_path: "/grove-launcher-test-missing".into(),
                    agent: Agent::Terminal,
                },
                Agent::Terminal,
                window,
                cx,
            );
            assert_eq!(
                launcher.error.as_deref(),
                Some("Worktree is no longer available.")
            );
            assert!(launcher.is_open());
            launcher.launch(
                RowIdentity::Session {
                    proj: 2,
                    wt_path: "/grove-launcher-test-outside".into(),
                    agent: Agent::Terminal,
                },
                Agent::Terminal,
                window,
                cx,
            );
            assert_eq!(
                launcher.error.as_deref(),
                Some("Project is no longer in this workspace.")
            );
        });
    });
}

#[gpui::test]
fn valid_recent_target_routes_once_and_invalid_retry_stays_open(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| {
        let path = env!("CARGO_MANIFEST_DIR").to_string();
        launcher.update(cx, |launcher, cx| {
            launcher.launch_probe = Some(Vec::new());
            launcher.open(window, cx);
            assert!(matches!(
                launcher.selected_row(cx),
                Some(PaletteRow::Recent { proj: 0, .. })
            ));
            launcher.activate(window, cx);
            assert!(!launcher.is_open());
        });
        assert_eq!(
            launcher.read(cx).launch_probe.as_deref(),
            Some(&[(vec![path], Agent::Terminal)][..])
        );
        launcher.update(cx, |launcher, cx| {
            launcher.open(window, cx);
            launcher.launch(
                RowIdentity::Session {
                    proj: 1,
                    wt_path: "/grove-launcher-test-missing".into(),
                    agent: Agent::Terminal,
                },
                Agent::Terminal,
                window,
                cx,
            );
            assert!(launcher.is_open());
        });
        assert_eq!(
            launcher.read(cx).launch_probe.as_ref().map(Vec::len),
            Some(1)
        );
    });
}

#[gpui::test]
fn palette_fits_minimum_and_desktop_viewports(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
    for (width, height) in [(320.0, 200.0), (1280.0, 800.0)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        draw(cx);
        let panel = cx.debug_bounds("worktree-launcher-panel").expect("panel");
        let search = cx.debug_bounds("worktree-launcher-search").expect("search");
        let list = cx
            .debug_bounds("worktree-launcher-list")
            .expect("scroll list");
        let row = cx.debug_bounds("launcher-row-0").expect("first row");
        let footer = cx.debug_bounds("worktree-launcher-footer").expect("footer");
        assert!(
            panel.left() >= gpui::px(0.0) && panel.right() <= gpui::px(width),
            "panel width at {width}: {panel:?}"
        );
        assert!(
            panel.top() >= gpui::px(0.0) && panel.bottom() <= gpui::px(height),
            "panel height at {height}: {panel:?}"
        );
        for (name, bounds) in [
            ("search", search),
            ("list", list),
            ("row", row),
            ("footer", footer),
        ] {
            assert!(
                bounds.left() >= panel.left() && bounds.right() <= panel.right(),
                "{name} clips horizontally at {width}: {bounds:?} / {panel:?}"
            );
            assert!(
                bounds.top() >= panel.top() && bounds.bottom() <= panel.bottom(),
                "{name} clips vertically at {height}: {bounds:?} / {panel:?}"
            );
        }
        assert!(search.bottom() <= list.top());
        assert!(list.bottom() <= footer.top());
        assert!(f32::from(panel.size.width) <= MODAL_W_LG);
        assert!(f32::from(panel.size.height) <= PANEL_MAX_H);
        cx.update(|window, cx| {
            assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
        });
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.agent_focus = true;
                launcher.focus.focus(window, cx);
                cx.notify();
            });
        });
        draw(cx);
        let panel = cx
            .debug_bounds("worktree-launcher-panel")
            .expect("agent panel");
        let agent = cx
            .debug_bounds("launcher-agent-selector-0")
            .expect("first agent");
        let footer = cx
            .debug_bounds("worktree-launcher-footer")
            .expect("agent footer");
        assert!(agent.left() >= panel.left() && agent.right() <= panel.right());
        assert!(agent.top() >= panel.top() && agent.bottom() <= panel.bottom());
        assert!(footer.left() >= panel.left() && footer.right() <= panel.right());
        assert!(footer.top() >= panel.top() && footer.bottom() <= panel.bottom());
        cx.update(|window, cx| {
            assert!(launcher.read(cx).focus.is_focused(window));
            launcher.update(cx, |launcher, cx| {
                launcher.agent_focus = false;
                launcher
                    .input
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            });
        });
    }
}

#[gpui::test]
fn picker_keeps_a_full_row_and_actions_visible_at_minimum_viewport(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (launcher, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        WorktreeLauncher::new(runtime, sidebar, window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
    for command in [PaletteRow::NewSession, PaletteRow::NewMultiProjectSession] {
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.close(window, cx);
                launcher.open(window, cx);
                enter_picker(launcher, command.clone(), window, cx);
            });
        });
        draw(cx);
        let panel = cx.debug_bounds("worktree-launcher-panel").expect("panel");
        let search = cx.debug_bounds("worktree-launcher-search").expect("search");
        let list = cx.debug_bounds("worktree-launcher-list").expect("list");
        let row = cx.debug_bounds("launcher-row-0").expect("first worktree");
        let footer = cx.debug_bounds("worktree-launcher-footer").expect("footer");
        assert!(panel.top() >= gpui::px(0.0) && panel.bottom() <= gpui::px(200.0));
        assert!(search.top() >= panel.top() && search.bottom() <= list.top());
        assert!(
            row.top() >= list.top() && row.bottom() <= list.bottom(),
            "{command:?}: row {row:?}, list {list:?}"
        );
        assert!(list.bottom() <= footer.top() && footer.bottom() <= panel.bottom());
        assert!(footer.left() >= panel.left() && footer.right() <= panel.right());
        if command == PaletteRow::NewMultiProjectSession {
            let action = cx
                .debug_bounds("worktree-launcher-launch-selected")
                .expect("launch action");
            let count = cx
                .debug_bounds("worktree-launcher-selected-count")
                .expect("selected count");
            assert!(action.top() >= row.bottom() && action.bottom() <= footer.top());
            assert!(count.top() >= row.bottom() && count.bottom() <= footer.top());
            assert!(count.right() <= action.left());
            assert!(action.right() <= panel.right());

            cx.update(|window, cx| {
                launcher.update(cx, |launcher, cx| launcher.activate(window, cx));
                assert_eq!(
                    launcher.read(cx).error.as_deref(),
                    Some("Select one or more worktrees.")
                );
            });
            draw(cx);
            let panel = cx
                .debug_bounds("worktree-launcher-panel")
                .expect("error panel");
            let search = cx
                .debug_bounds("worktree-launcher-search")
                .expect("error search");
            let list = cx
                .debug_bounds("worktree-launcher-list")
                .expect("error list");
            let row = cx
                .debug_bounds("launcher-row-0")
                .expect("error worktree row");
            let count = cx
                .debug_bounds("worktree-launcher-selected-count")
                .expect("error count");
            let action = cx
                .debug_bounds("worktree-launcher-launch-selected")
                .expect("error action");
            let footer = cx
                .debug_bounds("worktree-launcher-footer")
                .expect("error footer");
            let error = cx
                .debug_bounds("worktree-launcher-compact-error")
                .expect("specific error");
            assert!(panel.top() >= gpui::px(0.0) && panel.bottom() <= gpui::px(200.0));
            assert!(search.top() >= panel.top() && search.bottom() <= list.top());
            assert!(row.top() >= list.top() && row.bottom() <= list.bottom());
            assert!(count.top() >= row.bottom() && count.bottom() <= footer.top());
            assert!(action.top() >= row.bottom() && action.bottom() <= footer.top());
            assert!(footer.top() >= action.bottom() && footer.bottom() <= panel.bottom());
            assert!(error.top() >= footer.top() && error.bottom() <= footer.bottom());
            assert!(error.left() >= footer.left() && error.right() <= footer.right());
        }
    }
}
