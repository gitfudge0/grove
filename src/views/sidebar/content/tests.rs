use super::*;
#[test]
fn canvas_status_uses_real_lifecycle_before_activity() {
    assert_eq!(
        canvas_state(
            Agent::Codex,
            ActivityState::Working,
            true,
            false,
            false,
            false
        ),
        CanvasState::Failed
    );
    assert_eq!(
        canvas_state(
            Agent::Codex,
            ActivityState::Exited,
            false,
            true,
            false,
            true
        ),
        CanvasState::Starting
    );
    assert_eq!(
        canvas_state(
            Agent::Codex,
            ActivityState::Working,
            false,
            false,
            true,
            false
        ),
        CanvasState::Starting
    );
    assert_eq!(
        canvas_state(
            Agent::Terminal,
            ActivityState::Idle,
            false,
            false,
            false,
            false
        ),
        CanvasState::Running
    );
    assert_eq!(
        canvas_state(
            Agent::Terminal,
            ActivityState::Idle,
            false,
            false,
            false,
            true
        ),
        CanvasState::Exited
    );
    assert_eq!(
        canvas_state(
            Agent::Claude,
            ActivityState::WaitingForInput,
            false,
            false,
            false,
            false
        ),
        CanvasState::NeedsYou
    );
    assert!(CanvasState::NeedsYou
        .message(None)
        .unwrap()
        .contains("Review the terminal"));
    assert!(CanvasState::Failed
        .message(Some("executable was not found"))
        .unwrap()
        .contains("executable was not found"));
    assert!(!CanvasState::Exited.message(None).unwrap().contains("code"));
}
#[gpui::test]
fn grid_header_drag_swaps_tiles_and_cancels_without_affecting_controls(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-drag-fixture".into(),
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
        let runtime = cx.new(crate::runtime::Runtime::new);
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            for _ in 0..3 {
                registry.insert_meta("demo".into(), "/grove-drag-fixture".into(), Agent::Codex);
            }
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.select(
            Selection::Session(crate::entities::session_registry::SessionId::from_raw(1)),
            cx,
        );
        sidebar.mode = ViewMode::Grid;
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    let original = sidebar.read_with(cx, Sidebar::active_canvas_sessions);
    let header = cx.debug_bounds("terminal-header-1").unwrap().center();
    let destination = cx.debug_bounds("terminal-pane-2").unwrap().center();
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        original
    );
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    let mut swapped = original.clone();
    swapped.swap(0, 1);
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
    assert!(sidebar.read_with(cx, |sidebar, _| matches!(sidebar.selection, Some(Selection::Session(id)) if id == original[0].0)));
    assert!(
        cx.debug_bounds("terminal-header-1").unwrap().left()
            > cx.debug_bounds("terminal-header-2").unwrap().left()
    );

    // Body selection and the close control never initiate a reorder.
    for start in [
        cx.debug_bounds("terminal-pane-1").unwrap().center(),
        cx.debug_bounds("canvas-close-1").unwrap().center(),
    ] {
        let destination = cx.debug_bounds("terminal-pane-3").unwrap().center();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
        cx.simulate_mouse_move(
            destination,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            destination,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, Sidebar::active_canvas_sessions),
            swapped
        );
    }

    // A release without movement, an outside drop, and a lost button all cancel.
    let header = cx.debug_bounds("terminal-header-1").unwrap().center();
    let destination = cx.debug_bounds("terminal-pane-3").unwrap().center();
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
    let outside = gpui::point(gpui::px(-10.0), gpui::px(-10.0));
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(destination, None, gpui::Modifiers::default());
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );

    // Leaving grid clears the pending drag before returning to the same tiles.
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    sidebar.update(cx, |sidebar, cx| {
        sidebar.mode = ViewMode::List;
        cx.notify();
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    sidebar.update(cx, |sidebar, cx| {
        sidebar.mode = ViewMode::Grid;
        cx.notify();
    });
    draw(cx);
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
    // Separator resizing remains a distinct gesture and preserves order.
    let divider = cx.debug_bounds("grid-divider-columns-0").unwrap().center();
    let width = cx.debug_bounds("grid-tile-0").unwrap().size.width;
    let destination = gpui::point(divider.x + gpui::px(30.0), divider.y);
    cx.simulate_mouse_down(divider, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(cx.debug_bounds("grid-tile-0").unwrap().size.width > width);
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
    cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(800.0)));
    draw(cx);
    let header = cx.debug_bounds("terminal-header-2").unwrap().center();
    let destination = cx.debug_bounds("terminal-pane-3").unwrap().center();
    assert!(destination.y > header.y);
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    swapped.swap(0, 2);
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
    let header = cx.debug_bounds("terminal-header-2").unwrap().center();
    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );

    cx.simulate_mouse_down(header, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.update(|_, cx| {
        SettingsState::update(cx, |store| {
            store.workspaces.create("Other").unwrap();
        });
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    assert!(sidebar.read_with(cx, |sidebar, cx| {
        sidebar.active_canvas_sessions(cx).is_empty()
    }));
    cx.update(|_, cx| SettingsState::update(cx, |store| store.workspaces.select(1)));
    draw(cx);
    cx.simulate_mouse_up(
        destination,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(
        sidebar.read_with(cx, Sidebar::active_canvas_sessions),
        swapped
    );
}
#[gpui::test]
fn native_codex_keeps_shared_header_and_grid_drag_controls(cx: &mut gpui::TestAppContext) {
    native_header_title_edit_fixture(cx, grove_core::codex::Provider::Codex);
}

#[gpui::test]
fn native_claude_keeps_shared_header_and_grid_drag_controls(cx: &mut gpui::TestAppContext) {
    native_header_title_edit_fixture(cx, grove_core::codex::Provider::Claude);
}

fn native_header_title_edit_fixture(
    cx: &mut gpui::TestAppContext,
    provider: grove_core::codex::Provider,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-native-chrome-fixture".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (handle, actions) = grove_core::codex::Handle::channel();
    let (_updates, receive) = std::sync::mpsc::channel();
    let (sidebar, cx) = cx.add_window_view(move |window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "demo".into(),
                "/grove-native-chrome-feature".into(),
                provider.agent(),
            )
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
            key: "native-chrome".into(),
            project: "demo".into(),
            cwd: "/grove-native-chrome-fixture".into(),
            label: "Codex".into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("fixture-thread".into()),
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let mut state = grove_core::codex::Snapshot::new(saved);
        state.status = grove_core::codex::Status::Idle;
        let connection = grove_core::codex::Connection {
            handle: handle.clone(),
            updates: receive.into(),
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(state, connection, cx)
        });
        registry.update(cx, |registry, _| registry.attach_codex(id, session, handle));
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.select(Selection::Session(id), cx);
        sidebar
    });
    draw(cx);
    assert_spaced_session_breadcrumb(cx);
    for selector in [
        "terminal-header-1",
        "terminal-header-left-1",
        "terminal-header-right-1",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "missing shared native session control: {selector}"
        );
    }
    // Header editing uses the real native input but a captured provider action channel.
    let id = crate::entities::session_registry::SessionId::from_raw(1);
    let edit = cx.debug_bounds("edit-session-title-1").unwrap();
    let title_before = cx.debug_bounds("session-header-title-1").unwrap();
    let status = cx.debug_bounds("session-header-status-1").unwrap();
    assert!((f32::from(status.left() - title_before.right()) - SPACE_SM).abs() < 1.);
    assert!(status.right() <= edit.left());
    cx.simulate_mouse_move(edit.center(), None, gpui::Modifiers::default());
    draw(cx);
    assert_eq!(
        title_before,
        cx.debug_bounds("session-header-title-1").unwrap()
    );
    cx.simulate_click(edit.center(), gpui::Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input("A new title with spaces");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(action, grove_core::codex::Action::SetTitle(title) if title == "A new title with spaces")));
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.title_edit.is_none()));
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.simulate_input("discard");
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.title_edit.is_none()));
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, grove_core::codex::Action::SetTitle(_))));
    // Leaving and returning retains the originating session's independent editor.
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Selection::Project(0), cx);
        });
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar
        .title_edit
        .as_ref()
        .is_some_and(|edit| edit.session == id)));
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.select(Selection::Session(id), cx));
    });
    draw(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.finish_title_edit(id, false, window, cx);
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.mode = ViewMode::Grid;
            cx.notify();
        });
    });
    draw(cx);
    assert_spaced_session_breadcrumb(cx);
    assert!(cx.debug_bounds("canvas-close-1").is_some());
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.mode == ViewMode::Grid));
    let header = cx.debug_bounds("terminal-header-1").unwrap();
    cx.simulate_mouse_down(
        header.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_some()));
    cx.simulate_mouse_up(
        header.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    let id = crate::entities::session_registry::SessionId::from_raw(1);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(Action::Retry(id), window, cx);
            assert!(sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .codex(id)
                .is_some());
        });
    });
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, grove_core::codex::Action::Reconnect)));
    let project = cx.debug_bounds("session-header-project-1").unwrap();
    cx.simulate_click(project.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.dashboard_open));
    assert_eq!(
        sidebar.read_with(cx, super::super::Sidebar::dashboard_destination_for_test),
        Some((0, "/grove-native-chrome-fixture".into()))
    );
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.select(Selection::Session(id), cx));
    });
    draw(cx);
    let worktree = cx.debug_bounds("session-header-worktree-1").unwrap();
    cx.simulate_click(worktree.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.dashboard_open));
    assert_eq!(
        sidebar.read_with(cx, super::super::Sidebar::dashboard_destination_for_test),
        Some((0, "/grove-native-chrome-feature".into()))
    );
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Selection::Session(id), cx);
            sidebar.canvas_breadcrumb_focus[&(id, false)].focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.dashboard_open));
    assert_eq!(
        sidebar.read_with(cx, super::super::Sidebar::dashboard_destination_for_test),
        Some((0, "/grove-native-chrome-fixture".into()))
    );
}

#[gpui::test]
fn grid_header_compacts_tasks_and_discloses_them_without_grid_drag(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-task-glance-fixture".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let plan = |statuses: [&str; 2]| {
        serde_json::json!({"type": "plan", "plan": [
            {"id": "1", "step": "Map statuses", "status": statuses[0]},
            {"id": "2", "step": "Render task strip", "status": statuses[1]},
        ]})
    };
    let (handle, _actions) = grove_core::codex::Handle::channel();
    let (_updates, receive) = std::sync::mpsc::channel();
    let first = plan(["completed", "in_progress"]);
    let (sidebar, cx) = cx.add_window_view(move |window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "demo".into(),
                "/grove-task-glance-fixture".into(),
                Agent::Codex,
            )
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
            provider: grove_core::codex::Provider::Codex,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "task-glance".into(),
            project: "demo".into(),
            cwd: "/grove-task-glance-fixture".into(),
            label: "Codex".into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("fixture-thread".into()),
            items: vec![first].into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let mut state = grove_core::codex::Snapshot::new(saved);
        state.status = grove_core::codex::Status::Working;
        let connection = grove_core::codex::Connection {
            handle: handle.clone(),
            updates: receive.into(),
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(state, connection, cx)
        });
        registry.update(cx, |registry, _| registry.attach_codex(id, session, handle));
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.select(Selection::Session(id), cx);
        sidebar
    });
    let id = crate::entities::session_registry::SessionId::from_raw(1);
    draw(cx);
    // Single-session chrome leaves task progress to the transcript dock.
    assert!(cx.debug_bounds("terminal-header-1").is_some());
    assert!(cx.debug_bounds("header-tasks-1").is_none());
    assert!(cx.debug_bounds("grid-header-task-now-1").is_none());
    assert!(cx.debug_bounds("grid-header-tasks-1").is_none());
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.task_glance(id).cloned()),
        Some(super::super::task_glance::TaskGlance {
            done: 1,
            total: 2,
            current: Some(grove_core::tasks::TaskStatus::InProgress),
            now: Some("Render task strip".into()),
        })
    );
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.mode = ViewMode::Grid;
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("header-tasks-1").is_some());
    assert!(cx.debug_bounds("grid-header-task-now-1").is_none());
    let progress = cx.debug_bounds("header-tasks-1").unwrap();
    let close = cx.debug_bounds("canvas-close-1").unwrap();
    let header = cx.debug_bounds("terminal-header-1").unwrap();
    assert!(progress.right() <= close.left());
    assert!(close.right() <= header.right());
    cx.simulate_mouse_down(
        progress.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.grid_session_drag.is_none()));
    cx.simulate_mouse_up(
        progress.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(cx.debug_bounds("header-task-disclosure-1").is_some());
    assert!(cx.debug_bounds("header-task-current-1").is_some());
    assert!(cx.debug_bounds("header-task-row-1-0").is_some());
    assert!(cx.debug_bounds("header-task-row-1-1").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("header-task-disclosure-1").is_none());
    // Escape restores the trigger: Enter must open it again without moving focus.
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(cx.debug_bounds("header-task-disclosure-1").is_some());
    cx.simulate_resize(gpui::size(gpui::px(440.), gpui::px(240.)));
    draw(cx);
    let disclosure = cx.debug_bounds("header-task-disclosure-1").unwrap();
    assert!(disclosure.left() >= gpui::px(0.) && disclosure.top() >= gpui::px(0.));
    assert!(disclosure.right() <= gpui::px(440.) && disclosure.bottom() <= gpui::px(240.));
    cx.simulate_keystrokes("escape");
    draw(cx);
    // A transcript update reaches the header through the session observer.
    cx.update(|_, cx| {
        let session = sidebar
            .read(cx)
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .cloned()
            .unwrap();
        session.update(cx, |session, cx| {
            session.state.saved.items = vec![plan(["completed", "completed"])].into();
            cx.notify();
        });
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| {
        sidebar
            .task_glance(id)
            .is_some_and(super::super::task_glance::TaskGlance::all_done)
    }));
    assert!(cx.debug_bounds("header-tasks-1").is_some());
    assert!(cx.debug_bounds("grid-header-task-now-1").is_none());
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(cx.debug_bounds("header-task-disclosure-1").is_some());
    // Removing a list also dismisses its open disclosure.
    cx.update(|_, cx| {
        let session = sidebar
            .read(cx)
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .cloned()
            .unwrap();
        session.update(cx, |session, cx| {
            session.state.saved.items.clear();
            cx.notify();
        });
    });
    draw(cx);
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.task_glance(id).is_none()));
    assert!(cx.debug_bounds("header-tasks-1").is_none());
    assert!(cx.debug_bounds("header-task-disclosure-1").is_none());
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.header_tasks.open.is_none()));
}

#[gpui::test]
fn metadata_without_attached_terminal_renders_loading_header(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-canvas-fixture".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta("demo".into(), "/grove-canvas-fixture".into(), Agent::Codex)
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.sync(window, cx);
        sidebar.select(Selection::Session(id), cx);
        sidebar
    });
    draw(cx);
    assert!(cx.debug_bounds("terminal-header-1").is_some());
    cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(200.)));
    draw(cx);
    let header = cx.debug_bounds("terminal-header-1").unwrap();
    // The non-grid header leaves session closing to the sidebar's Done
    // action. Only grid tiles carry their own close control.
    assert!(cx.debug_bounds("canvas-close-1").is_none());
    assert_eq!(f32::from(header.size.height), APPBAR_H);
    let worktree = cx.debug_bounds("session-header-worktree-1").unwrap();
    let title = cx.debug_bounds("session-header-title-1").unwrap();
    assert!(worktree.right() < title.left());
    assert!(title.right() <= header.right());
    for zone in ["terminal-header-left-1", "terminal-header-right-1"] {
        let bounds = cx.debug_bounds(zone).unwrap();
        assert!(f32::from(bounds.center().y - header.center().y).abs() < 1.);
    }

    cx.update(|window, cx| {
        let missing = sidebar.update(cx, |sidebar, cx| {
            sidebar
                .terminal_content(
                    crate::entities::session_registry::SessionId::from_raw(u64::MAX),
                    false,
                    window,
                    cx,
                )
                .is_none()
        });
        assert!(missing);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.mode = ViewMode::Grid;
            cx.notify();
        });
    });
    draw(cx);
    let grid_header = cx.debug_bounds("terminal-header-1").unwrap();
    assert_eq!(f32::from(grid_header.size.height), SESSION_HEADER_H);
    let close = cx.debug_bounds("canvas-close-1").unwrap();
    assert!(f32::from(close.center().y - grid_header.center().y).abs() < 1.);
    assert!(close.left() >= grid_header.left() && close.right() <= grid_header.right());

    cx.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(800.)));
    draw(cx);
    let title = cx.debug_bounds("session-header-title-1").unwrap();
    let worktree = cx.debug_bounds("session-header-worktree-1").unwrap();
    let close = cx.debug_bounds("canvas-close-1").unwrap();
    let displayed_worktree = sidebar.read_with(cx, |sidebar, _| {
        sidebar
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|worktree| worktree.path == "/grove-canvas-fixture")
            .map_or_else(
                || "/grove-canvas-fixture".to_owned(),
                |worktree| worktree.name.clone(),
            )
    });
    let worktree_width = cx.update(|window, _| {
        let text: gpui::SharedString = displayed_worktree.into();
        let run = window.text_style().to_run(text.len());
        window
            .text_system()
            .shape_line(text, gpui::px(TEXT_SMALL), &[run], None)
            .width()
    });
    assert!(
        worktree.size.width >= worktree_width,
        "Worktree must fit without truncation in a wide header: {worktree:?}, text width {worktree_width:?}"
    );
    assert!(worktree.right() < title.left());
    assert!(title.right() < close.left());
}
#[gpui::test]
fn start_panel_scrolls_overflow_and_centers_on_desktop(cx: &mut gpui::TestAppContext) {
    let repo = super::super::tests::ChangedGitRepo::new();
    let path = repo.path();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "grove".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            sidebar_collapsed: true,
            default_landing_page: grove_core::storage::LandingPage::Projects,
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
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
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sync(window, cx);
            sidebar.available = [true; 4];
            sidebar.mode = ViewMode::Project;
            sidebar.select(Selection::Worktree(0, path.clone()), cx);
            sidebar.worktree_readiness.insert(
                path.clone(),
                (std::time::Instant::now(), Some(WorktreeReadiness::Ready)),
            );
            cx.notify();
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    draw(cx);
    let desktop_scroll = cx.debug_bounds("worktree-start-scroll").unwrap();
    let desktop_card = cx.debug_bounds("project-start-card").unwrap();
    eprintln!("desktop scroll={desktop_scroll:?} card={desktop_card:?}");
    assert!((f32::from(desktop_card.center().y - desktop_scroll.center().y)).abs() < 1.);
    let actions = cx.debug_bounds("project-start-actions").unwrap();
    for selector in [
        "start-terminal",
        "start-codex",
        "start-claude",
        "start-opencode",
    ] {
        let row = cx.debug_bounds(selector).unwrap();
        assert!(
            row.top() >= actions.top() && row.bottom() <= actions.bottom(),
            "{selector} must fit inside its clipping container: row={row:?}, actions={actions:?}"
        );
    }
    cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(540.)));
    draw(cx);
    let scroll = cx.debug_bounds("worktree-start-scroll").unwrap();
    let wrapper = cx.debug_bounds("worktree-start-scroll-content").unwrap();
    let card = cx.debug_bounds("project-start-card").unwrap();
    let before = cx.debug_bounds("project-new-worktree").unwrap();
    eprintln!("narrow scroll={scroll:?} wrapper={wrapper:?} card={card:?} CTA={before:?}");
    assert!(
        before.bottom() > scroll.bottom(),
        "fixture must overflow viewport"
    );
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(cx);
    let after = cx.debug_bounds("project-new-worktree").unwrap();
    eprintln!("after scroll CTA={after:?}");
    assert!(after.top() < before.top(), "scroll must move CTA upward");
    // More than one wheel step can be needed for long paths and short windows.
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1000.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(cx);
    let revealed = cx.debug_bounds("project-new-worktree").unwrap();
    eprintln!("revealed CTA={revealed:?}");
    assert!(
        revealed.bottom() <= scroll.bottom(),
        "scroll must reveal CTA"
    );
    cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(200.)));
    draw(cx);
    let short_scroll = cx.debug_bounds("worktree-start-scroll").unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: short_scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1000.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(cx);
    let short_cta = cx.debug_bounds("project-new-worktree").unwrap();
    eprintln!("short viewport={short_scroll:?} CTA={short_cta:?}");
    assert!(short_cta.top() >= short_scroll.top() && short_cta.bottom() <= short_scroll.bottom());
}
#[test]
fn session_header_uses_readable_label_for_uuid_only_titles() {
    assert_eq!(
        display_task_title(
            Some(" 3e9f01f9-54cd-4f09-8617-137231faccc9 ".into()),
            "Claude 1"
        ),
        "Claude 1"
    );
    assert_eq!(
        display_task_title(
            Some("Fix 3e9f01f9-54cd-4f09-8617-137231faccc9".into()),
            "Claude 1"
        ),
        "Fix 3e9f01f9-54cd-4f09-8617-137231faccc9"
    );
    assert_eq!(
        display_task_title(Some(" ".into()), "Terminal 1"),
        "Terminal 1"
    );
}

#[test]
fn missing_values_report_the_corresponding_field() {
    let errors = validate_worktree_fields("", "", "");
    assert_eq!(errors[0].as_deref(), Some("Enter a worktree name."));
    assert_eq!(errors[1].as_deref(), Some("Enter a branch name."));
    assert_eq!(
        errors[2].as_deref(),
        Some("Enter a base branch or revision.")
    );
    assert!(
        validate_worktree_fields("billing retry", "feat/billing-retry", "main")
            .iter()
            .all(Option::is_none)
    );
}
#[test]
fn invalid_branch_does_not_enable_submit() {
    for branch in [
        "feat:billing",
        "feat//billing",
        ".hidden",
        "feat/billing.lock",
        "feat/bill ing",
        "feat@{billing",
        "-",
    ] {
        assert!(
            validate_worktree_fields("billing", branch, "main")[1].is_some(),
            "{branch}"
        );
    }
}
fn assert_spaced_session_breadcrumb(cx: &mut gpui::VisualTestContext) {
    let selectors = [
        "session-header-project-1",
        "session-header-project-separator-1",
        "session-header-worktree-1",
        "session-header-title-separator-1",
        "session-header-provider-1",
        "session-header-title-1",
    ];
    for adjacent in selectors.windows(2) {
        let left = cx.debug_bounds(adjacent[0]).unwrap();
        let right = cx.debug_bounds(adjacent[1]).unwrap();
        assert!(
            (f32::from(right.left() - left.right()) - SPACE_MD).abs() <= 1.,
            "breadcrumb segments need the shared gap: {} and {}",
            adjacent[0],
            adjacent[1],
        );
    }
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
#[gpui::test]
fn worktree_form_fields_stay_within_narrow_and_desktop_canvas(cx: &mut gpui::TestAppContext) {
    let repo = super::super::tests::ChangedGitRepo::new();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "layout".into(),
                path: repo.path(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        let setup = cx.new(|cx| {
            super::super::worktree_form::WorktreeSetup::new(repo.path(), service, true, window, cx)
        });
        gpui_component::Root::new(setup, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while cx.debug_bounds("worktree-name-field").is_none() {
        assert!(std::time::Instant::now() < deadline, "worktree form did not open");
        draw(cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for (width, height) in [(1280., 800.), (320., 200.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        draw(cx);
        for field in [
            "worktree-name-field",
            "worktree-branch-field",
            "worktree-base-field",
        ] {
            let bounds = cx.debug_bounds(field).unwrap();
            assert!(f32::from(bounds.size.width) > 100., "{field}: {bounds:?}");
            assert!(f32::from(bounds.right()) <= width, "{field}: {bounds:?}");
            assert!(f32::from(bounds.left()) >= 0., "{field}: {bounds:?}");
        }
    }
}
