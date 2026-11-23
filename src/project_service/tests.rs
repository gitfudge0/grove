use super::*;
use fs_err as fs;
use std::{path::PathBuf, process::Command, time::SystemTime};

#[test]
fn worktree_requires_git_and_first_commit() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "grove-onboarding-git-{}-{:?}",
        std::process::id(),
        SystemTime::now()
    ));
    fs::create_dir_all(&root).unwrap();
    let _fixture = GitFixture(root.clone());
    let path = root.to_string_lossy();
    assert!(worktree_prerequisite(&path)
        .unwrap_err()
        .contains("Initialize Git"));
    git::init_if_needed(&path).unwrap();
    assert!(worktree_prerequisite(&path)
        .unwrap_err()
        .contains("first commit"));
    let commit = Command::new("git")
        .args([
            "-C",
            &path,
            "-c",
            "user.name=Grove Test",
            "-c",
            "user.email=grove@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "Initial commit",
        ])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );
    assert!(worktree_prerequisite(&path).is_ok());
}

struct GitFixture(PathBuf);

impl Drop for GitFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project(name: &str, path: &str) -> storage::Project {
    storage::Project {
        name: name.into(),
        path: path.into(),
        scripts: storage::ProjectScripts::default(),
        archived: false,
        worktree_dir: None,
    }
}

#[gpui::test]
fn pending_creation_survives_dismissal_and_guards_project_changes(cx: &mut gpui::TestAppContext) {
    let (release, wait) = futures::channel::oneshot::channel();
    let service = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("fixture", "/pending-creation-fixture")],
            ..Default::default()
        }));
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|_| WorkspaceState::default());
        let service = cx.new(|_| ProjectService::new(registry, state));
        let result = service.update(cx, |service, cx| {
            let work = cx.background_spawn(async move {
                wait.await
                    .unwrap_or_else(|_| Err("fixture canceled".into()))
            });
            service.track_creation("/pending-creation-fixture".into(), work, cx)
        });
        drop(result); // Closing the form must not cancel service ownership.
        service.update(cx, |service, cx| {
            assert!(service
                .ensure_not_removing("/pending-creation-fixture")
                .unwrap_err()
                .contains("creation"));
            assert!(service
                .archive_project_by_path("/pending-creation-fixture", cx)
                .unwrap_err()
                .contains("creation"));
            assert!(service
                .remove_worktree_by_path(
                    "/pending-creation-fixture",
                    "/pending-creation-fixture/child",
                    cx
                )
                .unwrap_err()
                .contains("creation"));
        });
        service
    });
    release
        .send(Err("expected fixture failure".into()))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while service.read_with(cx, |service, _| !service.creations.is_empty())
        && std::time::Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    service.read_with(cx, |service, _| {
        assert!(service.creations.is_empty());
        assert!(service
            .ensure_not_removing("/pending-creation-fixture")
            .is_ok());
    });
}

#[gpui::test]
fn workspace_move_persists_preserves_sessions_and_rolls_back(cx: &mut gpui::TestAppContext) {
    // Every write stays in a subprocess-local config directory.
    if std::env::var_os("GROVE_WORKSPACE_MOVE_TEST_CHILD").is_none() {
        let config = std::env::temp_dir().join(format!(
            "grove-workspace-move-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&config).unwrap();
        let fixture = GitFixture(config.clone());
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project_service::tests::workspace_move_persists_preserves_sessions_and_rolls_back",
                "--nocapture",
            ])
            .env("GROVE_WORKSPACE_MOVE_TEST_CHILD", "1")
            .env("GROVE_CONFIG_DIR", &config)
            .env("GROVE_TEST_CONFIG_DIR", &config)
            .output()
            .unwrap();
        drop(fixture);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let repo = storage::config_dir().unwrap().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let session =
        cx.new(|cx| TerminalSession::spawn_script("sleep 60", repo.to_str().unwrap(), cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while session.read_with(cx, |session, _| session.root_pid().is_none())
        && std::time::Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.update(|cx| {
        let config = storage::config_dir().unwrap();
        let repo = config.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let path = repo.to_str().unwrap();
        let mut store = storage::Store {
            projects: vec![project("moving", path), project("other", "/other")],
            appearance: Some(storage::AppearancePreference::Dark),
            grid_order: vec![format!("moving::{path}::session")],
            recent_launches: vec![storage::RecentLaunch {
                project: "moving".into(),
                wt_path: path.into(),
                agent: grove_core::agent::Agent::Terminal,
            }],
            ..Default::default()
        };
        store.projects[1].archived = true;
        store.projects[0].scripts.run = Some("echo retained".into());
        store.projects[0].worktree_dir = Some("original-directory".into());
        let source = store.workspaces.active;
        store.workspaces.create("Destination").unwrap();
        let destination = store.workspaces.active;
        store.workspaces.select(source);
        store.normalize_workspaces();
        storage::save(&store).unwrap();
        let original = serde_json::to_value(&store).unwrap();
        let config_path = storage::config_path().unwrap();
        let saved_before = fs::read(&config_path).unwrap();
        cx.set_global(SettingsState::new(store));
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|_| WorkspaceState::default());
        let service = cx.new(|_| ProjectService::new(registry.clone(), state.clone()));
        assert!(session.read(cx).spawn_error().is_none());
        let pid = session.read(cx).root_pid().unwrap();
        let id = registry.update(cx, |registry, _| {
            let id = registry.insert_meta(
                "moving".into(),
                path.into(),
                grove_core::agent::Agent::Terminal,
            );
            registry.attach(id, session.clone(), None);
            id
        });
        let spawned_at = registry.read(cx).meta(id).unwrap().spawned_at;
        service.update(cx, |service, cx| {
            assert!(service
                .move_project_to_workspace("/missing", destination, false, cx)
                .is_err());
            assert!(service
                .move_project_to_workspace(path, u64::MAX, false, cx)
                .is_err());
            assert!(service
                .move_project_to_workspace(path, source, false, cx)
                .is_err());
            service
                .removals
                .insert(path.into(), ProjectRemoval::default());
            assert!(service
                .move_project_to_workspace(path, destination, false, cx)
                .is_err());
            service.removals.clear();
            service.worktree_removals.insert(
                path.into(),
                WorktreeRemoval {
                    stage: WorktreeRemovalStage::RunningScript,
                    error: None,
                    project_path: path.into(),
                    canonical_path: repo.clone(),
                    reviewed: None,
                },
            );
            assert!(service
                .move_project_to_workspace(path, destination, false, cx)
                .is_err());
            service.worktree_removals.clear();
        });
        assert_eq!(fs::read(&config_path).unwrap(), saved_before);
        assert_eq!(
            serde_json::to_value(&cx.global::<SettingsState>().store).unwrap(),
            original
        );
        service.update(cx, |service, cx| {
            service
                .move_project_to_workspace(path, destination, false, cx)
                .unwrap();
        });
        let store = &cx.global::<SettingsState>().store;
        assert_eq!(store.workspaces.active, source);
        assert_eq!(store.project_workspace_id(path), destination);
        assert_eq!(store.workspaces.rows[0].projects, 1);
        assert_eq!(store.workspaces.rows[1].projects, 1);
        let mut expected = original;
        expected["project_workspaces"][path] = destination.into();
        assert_eq!(serde_json::to_value(store).unwrap(), expected);
        assert_eq!(
            serde_json::to_value(storage::load().unwrap()).unwrap(),
            expected
        );
        assert_eq!(registry.read(cx).meta(id).unwrap().spawned_at, spawned_at);
        assert_eq!(registry.read(cx).meta(id).unwrap().project, "moving");
        assert_eq!(registry.read(cx).meta(id).unwrap().wt_path, path);
        assert_eq!(registry.read(cx).session(id), Some(&session));
        assert_eq!(session.read(cx).root_pid(), Some(pid));
        assert!(session.update(cx, |session, _| session.alive()));
        assert!(repo.is_dir());

        // Opening the destination persists its selection and MRU with membership.
        service.update(cx, |service, cx| {
            service
                .move_project_to_workspace(path, source, false, cx)
                .unwrap();
            service
                .move_project_to_workspace(path, destination, true, cx)
                .unwrap();
        });
        expected["workspaces"]["active"] = destination.into();
        expected["workspaces"]["mru"] = serde_json::json!([destination, source]);
        assert_eq!(
            serde_json::to_value(&cx.global::<SettingsState>().store).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(storage::load().unwrap()).unwrap(),
            expected
        );

        // A directory at the save target makes the atomic rename fail reliably.
        fs::rename(&config_path, config.join("saved-before-failure.json")).unwrap();
        fs::create_dir(&config_path).unwrap();
        service.update(cx, |service, cx| {
            assert!(service
                .move_project_to_workspace(path, source, true, cx)
                .unwrap_err()
                .contains("Could not move project"));
        });
        let restored = &cx.global::<SettingsState>().store;
        assert_eq!(serde_json::to_value(restored).unwrap(), expected);
        assert_eq!(restored.workspaces.rows[0].projects, 1);
        assert_eq!(restored.workspaces.rows[1].projects, 1);
        assert_eq!(restored.workspaces.active, destination);
        assert!(!cx.global::<SettingsState>().is_dirty());
        assert_eq!(registry.read(cx).meta(id).unwrap().spawned_at, spawned_at);
        assert_eq!(registry.read(cx).session(id), Some(&session));
        assert_eq!(session.read(cx).root_pid(), Some(pid));
        assert!(session.update(cx, |session, _| session.alive()));
    });
}

fn git_fixture() -> Option<(GitFixture, String, String)> {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return None;
    }
    let root = std::env::temp_dir().join(format!(
        "grove-worktree-removal-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let repo = root.join("repo");
    let wt = root.join("feature");
    fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let repo_path = repo.to_str().unwrap();
    let wt_path = wt.to_str().unwrap();
    git(&["-C", repo_path, "init", "-q"]);
    fs::write(repo.join("README.md"), "fixture").unwrap();
    git(&["-C", repo_path, "add", "."]);
    git(&[
        "-C",
        repo_path,
        "-c",
        "user.name=grove-test",
        "-c",
        "user.email=grove-test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "fixture",
    ]);
    git(&["-C", repo_path, "worktree", "add", "--detach", wt_path]);
    Some((GitFixture(root), repo_path.to_string(), wt_path.to_string()))
}

#[test]
fn pending_removal_tracks_target_after_another_project_is_removed() {
    let mut projects = vec![project("first", "/first"), project("target", "/target")];
    let target = "/target";
    projects.remove(0);
    assert_eq!(project_index(&projects, target), Some(0));
    projects.remove(0);
    projects.push(project("replacement", "/replacement"));
    assert_eq!(project_index(&projects, target), None);
}
#[test]
fn names_trim_and_collide_with_archived_projects_case_insensitively() {
    let mut existing = project("Grove", "/grove");
    existing.archived = true;
    let projects = vec![existing];
    assert!(validated_name(&projects, " grove ", None).is_err());
    assert!(validated_name(&projects, "bad/name", None).is_err());
    assert!(validated_name(&projects, " ", None).is_err());
    assert_eq!(
        validated_name(&projects, " Grove ", Some("/grove")).unwrap(),
        "Grove"
    );
    assert_eq!(
        validated_name(&projects, " valid-name ", None).unwrap(),
        "valid-name"
    );
}

#[test]
fn registration_validates_directory_canonical_identity_and_workspace() {
    let directory = std::env::current_dir().unwrap();
    let path = directory.to_str().unwrap();
    let mut store = storage::Store::default();
    let workspace = store.workspaces.active;
    let registered = validated_registration(&store, " repo ", path, workspace).unwrap();
    assert_eq!(registered.name, "repo");
    assert_eq!(
        Path::new(&registered.path),
        directory.canonicalize().unwrap()
    );
    store.projects.push(registered);
    assert!(validated_registration(&store, "different", &format!("{path}/."), workspace).is_err());
    assert!(validated_registration(&store, "other", path, u64::MAX).is_err());
    assert!(
        validated_registration(&store, "other", &format!("{path}/missing"), workspace).is_err()
    );
    let file = std::env::current_exe().unwrap();
    assert!(validated_registration(&store, "file", file.to_str().unwrap(), workspace).is_err());
}

#[test]
fn rename_preserves_worktree_identity_and_rewrites_only_exact_project_keys() {
    let mut store = storage::Store {
        projects: vec![project("old", "/repo")],
        grid_order: vec!["old::/repo::session".into(), "older::/other".into()],
        recent_launches: vec![
            storage::RecentLaunch {
                project: "old".into(),
                wt_path: "/repo".into(),
                agent: grove_core::agent::Agent::Codex,
            },
            storage::RecentLaunch {
                project: "older".into(),
                wt_path: "/other".into(),
                agent: grove_core::agent::Agent::Codex,
            },
        ],
        ..Default::default()
    };
    let scripts = storage::ProjectScripts {
        setup: Some("setup".into()),
        run: Some("run".into()),
        teardown: Some("teardown".into()),
        ..Default::default()
    };
    apply_project_update(&mut store, 0, "new", scripts);
    assert_eq!(store.projects[0].name, "new");
    assert_eq!(store.projects[0].worktree_dir(), "old");
    assert_eq!(store.projects[0].scripts.run.as_deref(), Some("run"));
    assert_eq!(store.projects[0].scripts.setup.as_deref(), Some("setup"));
    assert_eq!(
        store.projects[0].scripts.teardown.as_deref(),
        Some("teardown")
    );
    assert_eq!(store.grid_order, ["new::/repo::session", "older::/other"]);
    assert_eq!(store.recent_launches[0].project, "new");
    assert_eq!(store.recent_launches[1].project, "older");
    assert_eq!(project_index(&store.projects, "/repo"), Some(0));
    apply_project_update(&mut store, 0, "third", storage::ProjectScripts::default());
    assert_eq!(store.projects[0].worktree_dir(), "old");
    assert_eq!(store.grid_order[0], "third::/repo::session");
}

#[test]
fn removal_progress_counts_failed_attempts_and_retains_errors() {
    let mut operation = ProjectRemoval {
        total: 2,
        current_target: Some("/first".into()),
        ..Default::default()
    };
    operation.complete_target(Err("/first: busy".into()));
    assert_eq!(operation.completed, 1);
    assert!(operation.current_target.is_none());
    assert_eq!(operation.errors, ["/first: busy"]);
    operation.current_target = Some("/second".into());
    operation.complete_target(Ok(()));
    assert_eq!(operation.completed, operation.total);
    assert_eq!(operation.errors, ["/first: busy"]);
    assert!(!operation.finished);
}

#[test]
fn removal_never_targets_main_checkout_even_if_flag_is_wrong() {
    let directory = std::env::current_dir().unwrap();
    let path = directory.to_str().unwrap();
    assert!(!removable_worktree(path, path, false));
    assert!(!removable_worktree(path, &format!("{path}/."), false));
    assert!(!removable_worktree(path, "/some-worktree", true));
    assert!(removable_worktree(path, "/some-worktree", false));
}

#[gpui::test]
fn removal_waits_for_cancelled_startup_before_teardown_and_git(cx: &mut gpui::TestAppContext) {
    let Some((fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let marker = fixture.0.join("teardown-started");
    let (release, gate) = futures::channel::oneshot::channel();
    let (service, terminal) = cx.update(|cx| {
        let mut configured = project("fixture", &repo);
        configured.scripts.teardown = Some(format!("printf done > '{}'", marker.display()));
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![configured],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|_| WorkspaceState::default());
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        let terminal = cx.new(|cx| TerminalSession::pending_startup_for_test(gate, cx));
        registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "fixture".into(),
                wt.clone(),
                grove_core::agent::Agent::Terminal,
            );
            registry.attach_observed(id, terminal.clone(), None, cx);
        });
        (service, terminal)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    service.update(cx, |service, cx| {
        service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
    });
    while service.read_with(cx, |service, _| {
        service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Stopping
    }) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            service.read_with(cx, |service, _| service
                .worktree_removal_status(&wt)
                .cloned())
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(terminal.read_with(cx, |terminal, _| terminal.is_starting()));
    assert!(Path::new(&wt).exists());
    assert!(
        !marker.exists(),
        "teardown must wait for startup cancellation acknowledgement"
    );
    release.send(()).unwrap();
    while service.read_with(cx, |service, _| {
        service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
    }) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(120));
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(terminal.read_with(cx, |terminal, _| terminal.has_exited()));
    assert!(marker.exists());
    assert!(!Path::new(&wt).exists());
    assert!(service.read_with(cx, |service, _| service
        .worktree_removal_status(&wt)
        .unwrap()
        .error
        .is_none()));
}

#[gpui::test]
fn worktree_removal_validates_then_sweeps_all_roots_and_blocks_launches(
    cx: &mut gpui::TestAppContext,
) {
    let Some((fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let alias = fixture.0.join("worktree-alias");
    std::os::unix::fs::symlink(&wt, &alias).unwrap();
    let alias = alias.to_str().unwrap().to_string();
    let service = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("fixture", &repo)],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        let registry = runtime.read(cx).registry.clone();
        let (primary, context, alias_primary, alias_context, main) =
            registry.update(cx, |registry, _| {
                let primary = registry.insert_meta(
                    "fixture".into(),
                    wt.clone(),
                    grove_core::agent::Agent::Terminal,
                );
                let context = registry.insert_meta_with_context(
                    "fixture".into(),
                    repo.clone(),
                    grove_core::agent::Agent::Codex,
                    vec![grove_core::session_meta::ContextRoot {
                        project: "fixture".into(),
                        wt_path: wt.clone(),
                    }],
                    None,
                );
                let alias_primary = registry.insert_meta(
                    "fixture".into(),
                    alias.clone(),
                    grove_core::agent::Agent::Terminal,
                );
                let alias_context = registry.insert_meta_with_context(
                    "fixture".into(),
                    repo.clone(),
                    grove_core::agent::Agent::Codex,
                    vec![grove_core::session_meta::ContextRoot {
                        project: "fixture".into(),
                        wt_path: alias.clone(),
                    }],
                    None,
                );
                let main = registry.insert_meta(
                    "fixture".into(),
                    repo.clone(),
                    grove_core::agent::Agent::Terminal,
                );
                (primary, context, alias_primary, alias_context, main)
            });
        service.update(cx, |service, cx| {
            assert!(service.remove_worktree_by_path(&repo, &repo, cx).is_err());
            assert!(service
                .remove_worktree_by_path(&repo, &format!("{repo}/."), cx)
                .is_err());
            assert!(service
                .remove_worktree_by_path(&repo, &format!("{wt}/."), cx)
                .is_err());
            assert!(service
                .remove_worktree_by_path(&repo, "/unlisted-worktree", cx)
                .is_err());
        });
        assert!(registry.read(cx).meta(primary).is_some());
        assert!(registry.read(cx).meta(context).is_some());
        assert!(registry.read(cx).meta(alias_primary).is_some());
        assert!(registry.read(cx).meta(alias_context).is_some());
        service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
            assert_eq!(
                service.worktree_removal_status(&wt).unwrap().stage,
                WorktreeRemovalStage::Checking,
            );
            assert!(service.remove_worktree_by_path(&repo, &wt, cx).is_err());
            assert!(service.archive_project_by_path(&repo, cx).is_err());
            assert!(service
                .register_project("late registration".into(), alias.clone(), cx)
                .is_err());
        });

        let blocked_primary = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_args(
                "fixture".into(),
                wt.clone(),
                grove_core::agent::Agent::Terminal,
                Vec::new(),
                cx,
            )
        });
        assert!(!blocked_primary);
        let blocked_context = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_context(
                "fixture".into(),
                repo.clone(),
                grove_core::agent::Agent::Codex,
                Vec::new(),
                vec![grove_core::session_meta::ContextRoot {
                    project: "fixture".into(),
                    wt_path: wt.clone(),
                }],
                None,
                cx,
            )
        });
        assert!(!blocked_context);
        (
            service,
            registry,
            [primary, context, alias_primary, alias_context],
            main,
        )
    });
    let (service, registry, removed, main) = service;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while service.read_with(cx, |service, _| {
        service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
    }) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        assert!(
            std::time::Instant::now() < deadline,
            "removal must complete after actual session cleanup"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.update(|cx| {
        for id in removed {
            assert!(registry.read(cx).meta(id).is_none());
        }
        assert!(registry.read(cx).meta(main).is_some());
        let status = service.read(cx).worktree_removal_status(&wt).unwrap();
        assert_eq!(status.stage, WorktreeRemovalStage::Finished);
        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(!git::list_worktrees_checked(&repo)
            .unwrap()
            .iter()
            .any(|w| w.path == wt));
        assert!(!Path::new(&wt).exists());
    });
}

#[gpui::test]
fn stale_worktree_removal_refreshes_inventory_without_touching_replaced_folder(
    cx: &mut gpui::TestAppContext,
) {
    let Some((_fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let (runtime, id) = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("fixture", &repo)],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "fixture".into(),
                wt.clone(),
                grove_core::agent::Agent::Terminal,
            )
        });
        (runtime, id)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !runtime.read_with(cx, |runtime, cx| {
        runtime
            .tree
            .read(cx)
            .worktrees_for_project(0, 0)
            .iter()
            .any(|entry| entry.path == wt)
    }) && std::time::Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.update(|cx| {
        assert!(runtime
            .read(cx)
            .tree
            .read(cx)
            .worktrees_for_project(0, 0)
            .iter()
            .any(|entry| entry.path == wt));
    });
    // Another Git client removes the checkout while Grove still displays its cached row.
    git::remove_worktree(&repo, &wt).unwrap();
    fs::create_dir(&wt).unwrap();
    let marker = Path::new(&wt).join("unrelated.txt");
    fs::write(&marker, "keep me").unwrap();
    cx.update(|cx| {
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let runtime = runtime.read(cx);
        let status = runtime
            .projects
            .read(cx)
            .worktree_removal_status(&wt)
            .unwrap();
        assert_eq!(status.stage, WorktreeRemovalStage::Finished);
        assert_eq!(
            status.error.as_deref(),
            Some("Worktree is no longer listed by Git.")
        );
        let inventory = runtime.tree.read(cx).worktrees_for_project(0, 0);
        assert!(!inventory.iter().any(|entry| entry.path == wt));
        assert!(inventory
            .iter()
            .any(|entry| entry.path == repo && entry.is_main));
        assert!(runtime.registry.read(cx).meta(id).is_some());
        assert_eq!(fs::read_to_string(&marker).unwrap(), "keep me");
    });
}

#[gpui::test]
fn worktree_removal_refuses_another_registered_project_root(cx: &mut gpui::TestAppContext) {
    let Some((fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let alias = fixture.0.join("registered-project-alias");
    std::os::unix::fs::symlink(&wt, &alias).unwrap();
    let alias = alias.to_str().unwrap().to_string();
    cx.update(|cx| {
        let mut other = project("other project", &alias);
        other.archived = true;
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("source", &repo), other],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "other project".into(),
                wt.clone(),
                grove_core::agent::Agent::Terminal,
            )
        });
        let error = service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap_err()
        });
        assert!(error.contains("other project"), "{error}");
        assert!(service.read(cx).worktree_removal_status(&wt).is_none());
        assert!(registry.read(cx).meta(id).is_some());
        assert!(Path::new(&wt).exists());
        assert!(git::list_worktrees_checked(&repo)
            .unwrap()
            .iter()
            .any(|entry| entry.path == wt));
    });
}

#[gpui::test]
fn worktree_removal_refuses_active_project_nested_inside_target(cx: &mut gpui::TestAppContext) {
    let Some((_fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let nested = Path::new(&wt).join("subproject");
    fs::create_dir_all(&nested).unwrap();
    let nested = nested.to_str().unwrap().to_string();
    cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("source", &repo), project("nested", &nested)],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "nested".into(),
                nested.clone(),
                grove_core::agent::Agent::Terminal,
            )
        });
        let error = service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap_err()
        });
        assert!(error.contains("nested"), "{error}");
        assert!(service.read(cx).worktree_removal_status(&wt).is_none());
        assert!(registry.read(cx).meta(id).is_some());
        assert!(Path::new(&nested).exists());
        assert!(Path::new(&wt).exists());
    });
}

#[gpui::test]
fn bulk_project_removal_preserves_registered_worktree_and_continues(cx: &mut gpui::TestAppContext) {
    // Project removal persists settings, so isolate its config in a child process.
    if std::env::var_os("GROVE_BULK_REMOVAL_TEST_CHILD").is_none() {
        let config = std::env::temp_dir().join(format!(
            "grove-bulk-removal-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&config).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project_service::tests::bulk_project_removal_preserves_registered_worktree_and_continues",
                "--nocapture",
            ])
            .env("GROVE_BULK_REMOVAL_TEST_CHILD", "1")
            .env("GROVE_CONFIG_DIR", &config).env("GROVE_TEST_CONFIG_DIR", &config)
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&config);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let Some((fixture, repo, protected)) = git_fixture() else {
        return;
    };
    let safe = fixture.0.join("ordinary-worktree");
    let safe = safe.to_str().unwrap().to_string();
    let output = Command::new("git")
        .args(["-C", &repo, "worktree", "add", "--detach", &safe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let nested = Path::new(&protected).join("archived-subproject");
    fs::create_dir_all(&nested).unwrap();
    let alias = fixture.0.join("registered-project-alias");
    std::os::unix::fs::symlink(&nested, &alias).unwrap();
    let mut other = project("other project", alias.to_str().unwrap());
    other.archived = true;
    let service = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("source", &repo), other],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            service.remove_project_by_path(&repo, true, cx).unwrap();
        });
        service
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let operation = service.read(cx).removal_status(&repo).unwrap();
        assert!(operation.finished && operation.unregistered);
        assert_eq!((operation.completed, operation.total), (2, 2));
        assert_eq!(operation.errors.len(), 1, "{:?}", operation.errors);
        assert!(operation.errors[0].contains("other project"));
        assert!(operation.errors[0].contains(&protected));
        let remaining = &cx.global::<SettingsState>().store.projects;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "other project");
        assert!(remaining[0].archived);
        assert!(Path::new(&protected).exists());
        assert!(nested.exists());
        assert!(!Path::new(&safe).exists());
        let listed = git::list_worktrees_checked(&repo).unwrap();
        assert!(listed.iter().any(|entry| entry.path == protected));
        assert!(!listed.iter().any(|entry| entry.path == safe));
    });
}

#[gpui::test]
fn reviewed_bulk_project_removal_keeps_new_and_replaced_worktrees(cx: &mut gpui::TestAppContext) {
    // Project removal persists settings. All filesystem effects belong to this fixture.
    if std::env::var_os("GROVE_REVIEWED_BULK_REMOVAL_TEST_CHILD").is_none() {
        let config = tempfile::tempdir().unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project_service::tests::reviewed_bulk_project_removal_keeps_new_and_replaced_worktrees",
                "--nocapture",
            ])
            .env("GROVE_REVIEWED_BULK_REMOVAL_TEST_CHILD", "1")
            .env("GROVE_CONFIG_DIR", config.path())
            .env("GROVE_TEST_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    let Some((fixture, repo, replaced)) = git_fixture() else {
        return;
    };
    let retained_path = |name: &str| fixture.0.join(name).to_str().unwrap().to_owned();
    let unchanged = retained_path("reviewed-worktree");
    let unavailable = retained_path("unavailable-worktree");
    let unavailable_folder = retained_path("unavailable-worktree-folder");
    let moved = retained_path("moved-worktree");
    let added = retained_path("new-worktree");
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .args(["-C", &repo])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["worktree", "add", "--detach", &unchanged]);
    git(&["worktree", "add", "--detach", &unavailable]);
    fs::rename(&unavailable, &unavailable_folder).unwrap();
    let review = ProjectService::review_project_worktrees(&repo).unwrap();
    assert_eq!(review.skipped_paths, std::slice::from_ref(&unavailable));
    assert_eq!(review.verified.len(), 2);
    assert!(review.verified.iter().any(|tree| tree.path() == replaced));
    assert!(review.verified.iter().any(|tree| tree.path() == unchanged));

    git(&["worktree", "move", &replaced, &moved]);
    git(&["worktree", "add", "--detach", &replaced]);
    git(&["worktree", "add", "--detach", &added]);
    let marker = Path::new(&replaced).join("keep.txt");
    fs::write(&marker, "new checkout").unwrap();
    let service = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("source", &repo)],
            ..Default::default()
        }));
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|_| WorkspaceState::default());
        let service = cx.new(|_| ProjectService::new(registry, state));
        service.update(cx, |service, cx| {
            service
                .remove_project_with_reviewed_worktrees(&repo, review.verified, cx)
                .unwrap();
        });
        service
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let operation = service.read(cx).removal_status(&repo).unwrap();
        assert!(operation.finished && operation.unregistered);
        assert_eq!((operation.completed, operation.total), (2, 2));
        assert_eq!(operation.errors.len(), 1, "{:?}", operation.errors);
        assert!(operation.errors[0].contains(&replaced));
        assert!(operation.errors[0].contains("changed after review"));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "new checkout");
        assert!(Path::new(&unavailable_folder).exists());
        assert!(git::list_worktrees_checked(&repo)
            .unwrap()
            .iter()
            .any(|entry| entry.path == unavailable));
        assert!(Path::new(&moved).exists());
        assert!(Path::new(&added).exists());
        assert!(Path::new(&repo).exists());
        assert!(!Path::new(&unchanged).exists());
    });
}

#[gpui::test]
fn reviewed_worktree_removal_rechecks_identity_before_sessions_and_after_teardown(
    cx: &mut gpui::TestAppContext,
) {
    let Some((fixture, repo, path)) = git_fixture() else {
        return;
    };
    let review = || {
        let branch = git::list_worktrees_checked(&repo)
            .unwrap()
            .into_iter()
            .find(|checkout| checkout.path == path)
            .unwrap()
            .branch;
        ProjectService::review_worktree(&repo, &path, &branch).unwrap()
    };
    let replace = |name| {
        let moved = fixture.0.join(name);
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .args(["-C", &repo])
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["worktree", "move", &path, moved.to_str().unwrap()]);
        git(&["worktree", "add", "--detach", &path]);
        fs::write(Path::new(&path).join("keep.txt"), name).unwrap();
    };
    let original = review();
    replace("before-start");

    let (service, registry, session) = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("source", &repo)],
            ..Default::default()
        }));
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|_| WorkspaceState::default());
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        let session = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "source".into(),
                path.clone(),
                grove_core::agent::Agent::Terminal,
            )
        });
        service.update(cx, |service, cx| {
            service
                .remove_reviewed_worktree(&repo, original, cx)
                .unwrap();
        });
        (service, registry, session)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let operation = service.read(cx).worktree_removal_status(&path).unwrap();
        assert_eq!(operation.stage, WorktreeRemovalStage::Finished);
        assert!(operation
            .error
            .as_deref()
            .unwrap()
            .contains("changed after review"));
        assert!(registry.read(cx).meta(session).is_some());
        assert_eq!(
            fs::read_to_string(Path::new(&path).join("keep.txt")).unwrap(),
            "before-start"
        );
        registry.update(cx, |registry, cx| registry.remove_background(session, cx));
    });

    let reviewed = review();
    replace("during-teardown");
    cx.update(|cx| {
        service.update(cx, |service, cx| {
            // Exercise the continuation after a teardown that was already in progress.
            service.teardown_target = Some((repo.clone(), path.clone()));
            service.worktree_removals.insert(
                path.clone(),
                WorktreeRemoval {
                    stage: WorktreeRemovalStage::RunningScript,
                    error: None,
                    project_path: repo.clone(),
                    canonical_path: reviewed.canonical_path.clone(),
                    reviewed: Some(reviewed),
                },
            );
            service.skip_worktree_teardown_for(&path, cx).unwrap();
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let operation = service.read(cx).worktree_removal_status(&path).unwrap();
        assert_eq!(operation.stage, WorktreeRemovalStage::Finished);
        assert!(operation
            .error
            .as_deref()
            .unwrap()
            .contains("changed after review"));
        assert_eq!(
            fs::read_to_string(Path::new(&path).join("keep.txt")).unwrap(),
            "during-teardown"
        );
    });
}

#[gpui::test]
fn worktree_teardown_spawn_failure_can_be_skipped_once(cx: &mut gpui::TestAppContext) {
    let Some((fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let alias = fixture.0.join("late-session-alias");
    std::os::unix::fs::symlink(&wt, &alias).unwrap();
    let alias = alias.to_str().unwrap().to_string();
    let service = cx.update(|cx| {
        let mut configured = project("fixture", &repo);
        configured.scripts.teardown = Some("\0".into());
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![configured],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        let registry = runtime.read(cx).registry.clone();
        service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
            assert_eq!(
                service.worktree_removal_status(&wt).unwrap().stage,
                WorktreeRemovalStage::Checking,
            );
        });
        (service, registry)
    });
    let (service, registry) = service;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while service.read_with(cx, |service, _| {
        service
            .worktree_removal_status(&wt)
            .is_some_and(|state| state.error.is_none())
    }) && std::time::Instant::now() < deadline
    {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(120));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    service.update(cx, |service, cx| {
        assert!(service
            .worktree_removal_status(&wt)
            .unwrap()
            .error
            .is_some());
        let (late_primary, late_context) = registry.update(cx, |registry, _| {
            let primary = registry.insert_meta(
                "fixture".into(),
                alias.clone(),
                grove_core::agent::Agent::Terminal,
            );
            let context = registry.insert_meta_with_context(
                "fixture".into(),
                repo.clone(),
                grove_core::agent::Agent::Codex,
                vec![grove_core::session_meta::ContextRoot {
                    project: "fixture".into(),
                    wt_path: alias.clone(),
                }],
                None,
            );
            (primary, context)
        });
        service.skip_worktree_teardown(cx);
        service.skip_worktree_teardown(cx);
        assert!(registry.read(cx).meta(late_primary).is_none());
        assert!(registry.read(cx).meta(late_context).is_none());
        assert_eq!(
            service.worktree_removal_status(&wt).unwrap().stage,
            WorktreeRemovalStage::Stopping,
        );
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while service.read_with(cx, |service, _| {
        service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
    }) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        assert!(
            std::time::Instant::now() < deadline,
            "removal must complete after actual session cleanup"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.update(|cx| {
        let status = service.read(cx).worktree_removal_status(&wt).unwrap();
        assert_eq!(status.stage, WorktreeRemovalStage::Finished);
        assert!(status.error.is_none());
        assert!(!Path::new(&wt).exists());
    });
}

#[gpui::test]
fn failed_worktree_removal_keeps_inspectable_error(cx: &mut gpui::TestAppContext) {
    let Some((_fixture, repo, wt)) = git_fixture() else {
        return;
    };
    let locked = Command::new("git")
        .args(["-C", &repo, "worktree", "lock", &wt])
        .status()
        .unwrap();
    assert!(locked.success());
    let service = cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![project("fixture", &repo)],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
        });
        service
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let status = service.read(cx).worktree_removal_status(&wt).unwrap();
        assert_eq!(status.stage, WorktreeRemovalStage::Finished);
        assert!(status
            .error
            .as_ref()
            .is_some_and(|error| error.contains("lock")));
        assert!(Path::new(&wt).exists());
    });
}
#[gpui::test]
fn removal_blocks_runtime_launch_and_sweeps_late_metadata(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(SettingsState::new(storage::Store {
            projects: vec![
                project("removing", "/grove-removal-fixture"),
                project("other", "/other"),
            ],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, _| {
            service
                .removals
                .insert("/grove-removal-fixture".into(), ProjectRemoval::default());
            assert!(service.is_removing("/grove-removal-fixture"));
            assert!(service
                .ensure_not_removing("/grove-removal-fixture")
                .is_err());
        });
        let launched = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_args(
                "removing".into(),
                "/grove-removal-fixture".into(),
                grove_core::agent::Agent::Terminal,
                Vec::new(),
                cx,
            )
        });
        assert!(!launched);
        let stale_name_launch = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_args(
                "old-name".into(),
                "/grove-removal-fixture".into(),
                grove_core::agent::Agent::Terminal,
                Vec::new(),
                cx,
            )
        });
        assert!(!stale_name_launch);
        let registry = runtime.read(cx).registry.clone();
        assert!(registry.read(cx).is_empty());
        registry.update(cx, |registry, _| {
            registry.insert_meta(
                "removing".into(),
                "/grove-removal-fixture".into(),
                grove_core::agent::Agent::Terminal,
            );
            registry.insert_meta(
                "other".into(),
                "/other".into(),
                grove_core::agent::Agent::Terminal,
            );
        });
        service.update(cx, |service, cx| {
            service
                .sweep_removal_sessions("/grove-removal-fixture", cx)
                .unwrap();
            service
                .removals
                .get_mut("/grove-removal-fixture")
                .unwrap()
                .finished = true;
            assert!(!service.is_removing("/grove-removal-fixture"));
        });
        assert!(registry.read(cx).by_project("removing").is_empty());
        assert_eq!(registry.read(cx).by_project("other").len(), 1);
    });
}
#[gpui::test]
fn duplicate_names_and_context_roots_use_path_ownership(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let projects = vec![project("same", "/fixture/a"), project("same", "/fixture/b")];
        cx.set_global(SettingsState::new(storage::Store {
            projects: projects.clone(),
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        let roots = vec![grove_core::session_meta::ContextRoot {
            project: "same".into(),
            wt_path: "/fixture/a".into(),
        }];
        let (a, b, multi) = registry.update(cx, |r, _| {
            let a = r.insert_meta(
                "same".into(),
                "/fixture/a".into(),
                grove_core::agent::Agent::Terminal,
            );
            let b = r.insert_meta(
                "same".into(),
                "/fixture/b".into(),
                grove_core::agent::Agent::Terminal,
            );
            let multi = r.insert_meta_with_context(
                "same".into(),
                "/fixture/b".into(),
                grove_core::agent::Agent::Codex,
                roots.clone(),
                None,
            );
            r.rename_project_by_path(&projects, "/fixture/a", "renamed")
                .unwrap();
            (a, b, multi)
        });
        assert_eq!(registry.read(cx).meta(a).unwrap().project, "renamed");
        assert_eq!(registry.read(cx).meta(b).unwrap().project, "same");
        assert_eq!(registry.read(cx).meta(multi).unwrap().project, "same");
        assert_eq!(
            registry.read(cx).meta(multi).unwrap().context_roots[0].project,
            "renamed"
        );
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            assert_eq!(
                service.archive_blockers("/fixture/a", cx).unwrap(),
                vec![a, multi]
            );
            service
                .removals
                .insert("/fixture/a".into(), ProjectRemoval::default());
        });
        let launched = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_context(
                "same".into(),
                "/fixture/b".into(),
                grove_core::agent::Agent::Codex,
                Vec::new(),
                roots,
                None,
                cx,
            )
        });
        assert!(!launched);
        service.update(cx, |service, cx| {
            service.sweep_removal_sessions("/fixture/a", cx).unwrap();
        });
        assert!(registry.read(cx).meta(a).is_none());
        assert!(registry.read(cx).meta(multi).is_none());
        assert!(registry.read(cx).meta(b).is_some());
    });
}
#[gpui::test]
fn unknown_roots_block_archive_removal_rename_and_launch(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let projects = vec![project("known", "/fixture/known")];
        cx.set_global(SettingsState::new(storage::Store {
            projects: projects.clone(),
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(crate::runtime::Runtime::new);
        let registry = runtime.read(cx).registry.clone();
        registry.update(cx, |registry, _| {
            registry.insert_meta(
                "known".into(),
                "/external/unknown".into(),
                grove_core::agent::Agent::Terminal,
            );
            assert!(registry
                .rename_project_by_path(&projects, "/fixture/known", "new")
                .is_err());
        });
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            assert!(service
                .archive_project_by_path("/fixture/known", cx)
                .is_err());
            assert!(service
                .remove_project_by_path("/fixture/known", false, cx)
                .is_err());
            assert!(service
                .update_project(
                    "/fixture/known",
                    "new".into(),
                    storage::ProjectScripts::default(),
                    cx
                )
                .is_err());
        });
        let launched = runtime.update(cx, |runtime, cx| {
            runtime.spawn_session_in_with_args(
                "known".into(),
                "/external/unknown".into(),
                grove_core::agent::Agent::Terminal,
                Vec::new(),
                cx,
            )
        });
        assert!(!launched);
        assert_eq!(cx.global::<SettingsState>().store.projects[0].name, "known");
        assert_eq!(registry.read(cx).all().len(), 1);
    });
}
#[test]
fn duplicate_name_saved_keys_only_rename_the_owned_path() {
    let mut store = storage::Store {
        projects: vec![project("same", "/fixture/a"), project("same", "/fixture/b")],
        grid_order: vec!["same::/fixture/a".into(), "same::/fixture/b".into()],
        recent_launches: vec![
            storage::RecentLaunch {
                project: "same".into(),
                wt_path: "/fixture/a".into(),
                agent: grove_core::agent::Agent::Codex,
            },
            storage::RecentLaunch {
                project: "same".into(),
                wt_path: "/fixture/b".into(),
                agent: grove_core::agent::Agent::Codex,
            },
        ],
        ..Default::default()
    };
    apply_project_update(&mut store, 0, "new", storage::ProjectScripts::default());
    assert_eq!(store.grid_order, ["new::/fixture/a", "same::/fixture/b"]);
    assert_eq!(store.recent_launches[0].project, "new");
    assert_eq!(store.recent_launches[1].project, "same");
}
