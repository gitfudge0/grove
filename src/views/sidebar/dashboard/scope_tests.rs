//! Scope contract checks and deferred-composer fixtures. No provider is launched.
use super::*;
use grove_core::codex::{Provider, SavedSession};

fn target(project: usize, name: &str, project_path: &str, path: &str) -> ScopeTarget {
    ScopeTarget {
        project,
        project_path: project_path.into(),
        project_name: name.into(),
        path: path.into(),
        branch: "fixture-branch".into(),
    }
}

#[test]
fn dashboard_scope_working_directory_is_independent_of_selection_order() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    fs_err::create_dir_all(&first).unwrap();
    fs_err::create_dir_all(&second).unwrap();
    let first = first.to_string_lossy().into_owned();
    let second = second.to_string_lossy().into_owned();
    let scope = vec![
        target(0, "First", &first, &first),
        target(1, "Second", &second, &second),
    ];
    let projects = vec![
        (0, "First".into(), first.clone()),
        (1, "Second".into(), second.clone()),
    ];
    validate_scope(&scope, &second, &projects).unwrap();
    let mut saved = SavedSession::empty(
        "fixture".into(),
        Provider::Codex,
        "Second".into(),
        second.clone(),
    );
    saved.roots = scope_roots(&scope);
    assert_eq!(saved.cwd, second);
    assert_eq!(
        saved
            .roots
            .iter()
            .map(|root| root.wt_path.as_str())
            .collect::<Vec<_>>(),
        [first.as_str(), second.as_str()]
    );
    assert!(validate_scope(&scope, root.path().to_str().unwrap(), &projects).is_err());
}

#[test]
fn dashboard_scope_rejects_duplicate_unavailable_and_removed_project_targets() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    fs_err::create_dir_all(&first).unwrap();
    fs_err::create_dir_all(&second).unwrap();
    let first = first.to_string_lossy().into_owned();
    let second = second.to_string_lossy().into_owned();
    let first_target = target(0, "First", &first, &first);
    let second_target = target(1, "Second", &second, &second);
    let projects = vec![
        (0, "First".into(), first.clone()),
        (1, "Second".into(), second.clone()),
    ];
    validate_scope(
        &[first_target.clone(), second_target.clone()],
        &first,
        &projects,
    )
    .unwrap();
    assert!(validate_scope(
        &[first_target.clone(), first_target.clone()],
        &first,
        &projects
    )
    .is_err());
    let mut duplicate = first_target.clone();
    duplicate.path.push('/');
    assert!(validate_scope(&[first_target.clone(), duplicate], &first, &projects).is_err());
    assert!(validate_scope(
        &[first_target.clone(), second_target.clone()],
        &first,
        &projects[..1]
    )
    .is_err());
    let mut replaced = second_target.clone();
    replaced.project_path = "/replaced-registration".into();
    assert!(validate_scope(&[first_target.clone(), replaced], &first, &projects).is_err());
    fs_err::remove_dir(&second).unwrap();
    assert!(validate_scope(&[first_target, second_target], &first, &projects).is_err());
}

#[test]
fn dashboard_scope_preserves_two_worktrees_of_the_same_project_in_saved_roots() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    let topic = root.path().join("topic");
    fs_err::create_dir_all(&main).unwrap();
    fs_err::create_dir_all(&topic).unwrap();
    let main = main.to_string_lossy().into_owned();
    let topic = topic.to_string_lossy().into_owned();
    let scope = vec![
        target(3, "Shared", &main, &main),
        target(3, "Shared", &main, &topic),
    ];
    validate_scope(&scope, &topic, &[(3, "Shared".into(), main.clone())]).unwrap();
    for provider in [Provider::Codex, Provider::Claude] {
        let mut saved =
            SavedSession::empty("fixture".into(), provider, "Shared".into(), topic.clone());
        saved.roots = scope_roots(&scope);
        let restored: SavedSession =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(restored.cwd, topic);
        assert_eq!(restored.roots.len(), 2);
        assert_eq!(restored.roots[0].project, "Shared");
        assert_eq!(restored.roots[1].project, "Shared");
        assert_eq!(restored.roots[0].wt_path, main);
        assert_eq!(restored.roots[1].wt_path, topic);
    }
}

#[gpui::test]
fn dashboard_scope_codex_and_claude_arguments_exclude_the_chosen_working_directory(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        let scope = vec![
            target(0, "First", "/first", "/first/topic"),
            target(1, "Second", "/second", "/second/main"),
            target(1, "Second", "/second", "/second/other"),
        ];
        for provider in [Provider::Codex, Provider::Claude] {
            let args = dashboard_launch_args(provider, &scope, "/second/main", cx);
            let additional = args
                .windows(2)
                .filter(|pair| pair[0] == "--add-dir")
                .map(|pair| pair[1].as_str())
                .collect::<Vec<_>>();
            assert_eq!(additional, ["/first/topic", "/second/other"]);
            assert!(!args.iter().any(|arg| arg == "/second/main"));
            provider.launch_options(&args).unwrap();
            let single = dashboard_launch_args(provider, &scope[1..2], "/second/main", cx);
            assert!(!single.iter().any(|arg| arg == "--add-dir"));
        }
    });
}

#[gpui::test]
fn dashboard_scope_changed_preserves_codex_and_claude_drafts_and_blocks_missing_roots(
    cx: &mut gpui::TestAppContext,
) {
    // Deliberately absent checkout paths prevent provider discovery from starting.
    let root = tempfile::tempdir().unwrap();
    let first = root
        .path()
        .join("missing-first")
        .to_string_lossy()
        .into_owned();
    let second = root
        .path()
        .join("missing-second")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        for (name, path) in [("First", &first), ("Second", &second)] {
            store.projects.push(grove_core::storage::Project {
                name: name.into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(path);
        }
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let scope = vec![
        target(0, "First", &first, &first),
        target(1, "Second", &second, &second),
    ];
    for provider in [Provider::Codex, Provider::Claude] {
        let session = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                let session = dashboard.session.clone();
                session.update(cx, |s, _| {
                    s.state.saved.provider = provider;
                    s.state.saved.draft = "Retain this exact task".into();
                    s.state.saved.draft_images = vec!["fixture-image".into()];
                    s.state.saved.composer.model = Some("fixture-model".into());
                    s.state.saved.composer.effort = Some("high".into());
                    s.state.saved.config_args = vec!["fixture-config".into()];
                });
                dashboard.tray.update(cx, |tray, cx| {
                    tray.project = 1;
                    tray.path = second.clone();
                    tray.scope = scope.clone();
                    cx.emit(DestinationEvent::ScopeChanged);
                });
                session
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                assert_eq!(dashboard.session, session);
                assert_eq!(dashboard.tray.read(cx).path, second);
                let saved = &session.read(cx).state.saved;
                assert_eq!(saved.cwd, second);
                assert_eq!(saved.project, "Second");
                assert_eq!(saved.roots, scope_roots(&scope));
                assert_eq!(saved.provider, provider);
                assert_eq!(saved.draft, "Retain this exact task");
                assert_eq!(saved.draft_images, ["fixture-image"]);
                assert_eq!(saved.composer.model.as_deref(), Some("fixture-model"));
                assert_eq!(saved.composer.effort.as_deref(), Some("high"));
                assert_eq!(saved.config_args, ["fixture-config"]);
                sidebar.launch_dashboard(false, window, cx);
                assert!(sidebar
                    .content_error
                    .as_deref()
                    .unwrap()
                    .contains("no longer available"));
                assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
                assert_eq!(session.read(cx).state.saved.draft, "Retain this exact task");
                assert!(session.read(cx).is_deferred());
                assert!(!session.read(cx).has_transport());
            });
        });
    }
}

#[gpui::test]
fn dashboard_scope_replacement_preserves_same_project_worktrees_and_chosen_cwd(
    cx: &mut gpui::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let main = root
        .path()
        .join("missing-main")
        .to_string_lossy()
        .into_owned();
    let topic = root
        .path()
        .join("missing-topic")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Shared".into(),
            path: main.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&main);
        cx.set_global(SettingsState::new(store));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    let scope = vec![
        target(0, "Shared", &main, &main),
        target(0, "Shared", &main, &topic),
    ];
    for provider in [Provider::Codex, Provider::Claude] {
        let (_updates, receive) = std::sync::mpsc::channel();
        let (handle, _actions) = grove_core::codex::Handle::channel();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                dashboard.tray.update(cx, |tray, _| {
                    tray.project = 0;
                    tray.path = topic.clone();
                    tray.scope = scope.clone();
                    tray.worktrees = vec![
                        (main.clone(), "main".into()),
                        (topic.clone(), "topic".into()),
                    ];
                    tray.loading = false;
                    tray.error = None;
                });
                let mut saved = dashboard.session.read(cx).state.saved.clone();
                saved.provider = provider;
                saved.project = "Shared".into();
                saved.cwd = topic.clone();
                saved.roots = scope_roots(&scope);
                saved.draft = "First independent task".into();
                saved.composer.model = Some("fixture-model".into());
                saved.config_args = dashboard_launch_args(provider, &scope, &topic, cx);
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        grove_core::codex::Snapshot::new(saved),
                        grove_core::codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
                sidebar.adopt_dashboard_launch(0, session.clone(), view, false, window, cx);
                let next = sidebar.dashboard.as_ref().unwrap();
                assert_ne!(next.session, session);
                assert_eq!(next.tray.read(cx).scope, scope);
                assert_eq!(next.tray.read(cx).path, topic);
                let saved = &next.session.read(cx).state.saved;
                assert_eq!(saved.roots, scope_roots(&scope));
                assert_eq!(saved.cwd, topic);
                assert_eq!(saved.provider, provider);
                assert_eq!(saved.composer.model.as_deref(), Some("fixture-model"));
                assert!(saved.draft.is_empty());
                assert!(!next.session.read(cx).has_transport());
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                assert_eq!(
                    registry.all().last().unwrap().context_roots,
                    scope_roots(&scope)
                );
                assert_eq!(registry.all().last().unwrap().wt_path, topic);
                assert_eq!(session.read(cx).state.saved.draft, "First independent task");
            });
        });
    }
}

#[gpui::test]
fn dashboard_scope_hides_starting_directory_only_for_multiple_worktrees(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    });
    let (tray, cx) = cx.add_window_view(|_, _| {
        DestinationTray::new(
            vec![(0, "Project".into(), "/project".into())],
            0,
            "/project".into(),
        )
    });
    for (count, width) in [(0, 900.), (1, 900.), (2, 900.), (2, 360.), (1, 360.)] {
        cx.update(|_, cx| {
            tray.update(cx, |tray, cx| {
                tray.scope = (0..count)
                    .map(|index| {
                        target(
                            0,
                            "Project",
                            "/project",
                            if index == 0 { "/project" } else { "/topic" },
                        )
                    })
                    .collect();
                cx.notify();
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(300.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-project-selector").is_some());
        assert_eq!(
            cx.debug_bounds("dashboard-worktree-selector").is_some(),
            count <= 1
        );
    }
}
