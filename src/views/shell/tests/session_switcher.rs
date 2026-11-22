//! Session and workspace switching fixtures.

use super::*;

#[gpui::test]
fn switch_session_shows_scoped_picker_and_escape_restores_focus(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let prior = cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        focus
    });
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::keymap::SwitchSession), cx));
    draw(cx);
    assert!(cx.debug_bounds("session-switcher").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(prior.is_focused(window));
        assert!(!shell.read(cx).switcher_open);
    });
}

#[gpui::test]
fn switch_session_uses_visible_order_and_selects_existing_process(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    let (first, second) = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        registry.update(cx, |registry, cx| {
            let first = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                grove_core::agent::Agent::Terminal,
            );
            let second = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                grove_core::agent::Agent::Terminal,
            );
            cx.notify();
            (first, second)
        })
    });
    draw(cx);
    let targets = cx.update(|_, cx| shell.read(cx).sidebar.read(cx).visible_session_targets(cx));
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().any(|(id, _)| *id == first));
    assert!(targets.iter().any(|(id, _)| *id == second));
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::SwitchSession), cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("session-switcher-row-1").is_some());
    cx.simulate_keystrokes("down enter");
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            shell.read(cx).sidebar.read(cx).selected_session(),
            Some(targets[1].0)
        );
        assert!(!shell.read(cx).switcher_open);
        assert_eq!(shell.read(cx).runtime.read(cx).registry.read(cx).len(), 2);
    });
}

#[gpui::test]
fn switcher_groups_workspaces_and_current_workspace_sessions(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|_, cx| {
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces
            .create("Other")
            .unwrap();
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces
            .select(1);
    });
    let session = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                Agent::Terminal,
            );
            cx.notify();
            id
        })
    });
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        let rows = shell.switcher_rows(cx);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].target, SwitchTarget::Workspace(1));
        assert!(rows[0].current);
        assert_eq!(rows[1].target, SwitchTarget::Workspace(2));
        assert!(!rows[1].current);
        assert_eq!(rows[2].target, SwitchTarget::Session(session));
        assert_eq!(switcher_default_index(&rows), 2);
    });
}

#[gpui::test]
fn switcher_search_matches_names_and_session_context_case_insensitively(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|_, cx| {
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces
            .create("Other")
            .unwrap();
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces
            .select(1);
    });
    cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        registry.update(cx, |registry, cx| {
            registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                Agent::Terminal,
            );
            cx.notify();
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| {
            shell.switcher_query = "OTHER".into();
            let rows = shell.switcher_rows(cx);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].target, SwitchTarget::Workspace(2));
            assert_eq!(switcher_default_index(&rows), 0);
            shell.switcher_query = "navigation-test".into();
            let rows = shell.switcher_rows(cx);
            assert_eq!(rows.len(), 1);
            assert!(matches!(rows[0].target, SwitchTarget::Session(_)));
            shell.switcher_query = "unmatched".into();
            assert!(shell.switcher_rows(cx).is_empty());
        });
    });
}

#[gpui::test]
fn switcher_search_enter_activates_workspace(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|_, cx| {
        let workspaces = &mut cx
            .global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces;
        workspaces.create("Other").unwrap();
        workspaces.select(1);
    });
    draw(cx);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::SwitchSession), cx);
    });
    draw(cx);
    cx.simulate_input("Other");
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.switcher_query, "Other");
        assert_eq!(shell.switcher_rows(cx).len(), 1);
        assert_eq!(
            shell.switcher_rows(cx)[0].target,
            SwitchTarget::Workspace(2)
        );
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .workspaces
                .active,
            2,
            "error={:?}",
            shell.read(cx).switcher_error
        );
        assert!(!shell.read(cx).switcher_open);
    });
}

#[gpui::test]
fn grid_workspace_switch_lands_on_dashboard(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    let destination_path = "/grove-shell-grid-destination".to_string();
    cx.update(|_, cx| {
        let store = &mut cx.global_mut::<crate::settings::SettingsState>().store;
        store.projects.push(grove_core::storage::Project {
            name: "destination".into(),
            path: destination_path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.workspaces.create("Other").unwrap();
        store.assign_project_to_active_workspace(&destination_path);
        store.workspaces.select(1);
    });
    let (prior, first, second) = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        let prior_session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(
                "\0",
                "/grove-shell-navigation-test",
                cx,
            )
        });
        let first_session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(
                "\0",
                &destination_path,
                cx,
            )
        });
        let second_session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(
                "\0",
                &destination_path,
                cx,
            )
        });
        registry.update(cx, |registry, cx| {
            let prior = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                Agent::Terminal,
            );
            registry.attach(prior, prior_session, None);
            let first = registry.insert_meta(
                "destination".into(),
                destination_path.clone(),
                Agent::Terminal,
            );
            registry.attach(first, first_session, None);
            let second = registry.insert_meta(
                "destination".into(),
                destination_path.clone(),
                Agent::Terminal,
            );
            registry.attach(second, second_session, None);
            cx.notify();
            (prior, first, second)
        })
    });
    cx.update(|window, cx| {
        shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
            sidebar.select_session_id(prior, window, cx);
        });
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ToggleGrid), cx);
    });
    draw(cx);
    let prior_focus = cx.update(|_, cx| {
        shell
            .read(cx)
            .sidebar
            .read(cx)
            .canvas_terminal_focus(prior, false, cx)
            .expect("prior tile")
    });
    cx.update(|window, cx| prior_focus.focus(window, cx));
    cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
    draw(cx);
    cx.simulate_input("Other");
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        let sidebar = sidebar.read(cx);
        assert!(sidebar.is_grid());
        assert_eq!(
            sidebar.active_canvas_sessions(cx),
            vec![(first, false), (second, false)]
        );
        assert!(!prior_focus.is_focused(window));
    });
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("codex-composer").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
    draw(cx);
    cx.simulate_input("Grove");
    draw(cx);
    cx.simulate_keystrokes("up enter");
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .workspaces
                .active,
            1
        );
    });
    cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
    draw(cx);
    cx.simulate_input("Other");
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("codex-composer").is_some());
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .workspaces
            .select(1);
    });
    draw(cx);
    cx.update(|window, _| {
        assert!(!prior_focus.is_focused(window));
    });
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("codex-composer").is_some());
}

#[gpui::test]
fn empty_switcher_keeps_workspace_and_new_session_action_visible(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::SwitchSession), cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("session-switcher-row-0").is_some());
    cx.update(|_, cx| {
        assert!(shell
            .read(cx)
            .sidebar
            .read(cx)
            .visible_session_targets(cx)
            .is_empty());
    });
    assert!(cx.debug_bounds("session-switcher-new-session").is_some());
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.switcher_rows(cx).len(), 1);
        assert_eq!(shell.switcher_index, 0);
    });
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| {
        assert!(shell.read(cx).switcher_new_session_focus.is_focused(window));
    });
    cx.simulate_keystrokes("shift-tab");
    cx.update(|window, cx| {
        let input = shell.read(cx).switcher_input.clone();
        assert!(input.read(cx).focus_handle(cx).is_focused(window));
    });
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(!shell.switcher_open);
        assert!(!shell.launcher.read(cx).is_open());
    });
}

#[gpui::test]
fn switcher_panel_fits_small_window(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::SwitchSession), cx);
    });
    draw(cx);
    let panel = cx.debug_bounds("session-switcher").expect("switcher panel");
    assert!(f32::from(panel.left()) >= 0.0);
    assert!(f32::from(panel.right()) <= 320.0);
    assert!(f32::from(panel.bottom()) <= 200.0);
    assert!(cx.debug_bounds("session-switcher-list").is_some());
}
