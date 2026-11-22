use super::*;

#[gpui::test]
#[cfg(not(windows))]
fn history_reopen_rejects_primary_and_context_removal_before_preparing_recovery(
    cx: &mut gpui::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().join("primary");
    let context = root.path().join("context");
    fs_err::create_dir_all(&primary).unwrap();
    fs_err::create_dir_all(&context).unwrap();
    let primary = primary.to_str().unwrap().to_owned();
    let context = context.to_str().unwrap().to_owned();
    for (project, path, worktree) in [
        (&primary, &primary, false),
        (&primary, &primary, true),
        (&context, &context, false),
        (&context, &context, true),
    ] {
        cx.update(|cx|{
            cx.set_global(SettingsState::new(grove_core::storage::Store{native_agents_enabled:true,projects:[("Primary",&primary),("Context",&context)].into_iter().map(|(name,path)|grove_core::storage::Project{name:name.into(),path:path.clone(),scripts:grove_core::storage::ProjectScripts::default(),archived:false,worktree_dir:None}).collect(),..Default::default()}));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime=cx.new(Runtime::new);
            runtime.read(cx).projects.clone().update(cx,|service,_|service.mark_history_removal_for_test(project,worktree.then_some(path.as_str())));
            let saved:grove_core::codex::SavedSession=serde_json::from_value(serde_json::json!({"key":"invalid key prevents any writes if the guard regresses","provider":"codex","project":"Primary","cwd":primary,"label":"Fixture","roots":[{"project":"Context","wt_path":context}],"thread_id":null,"items":[]})).unwrap();
            let error=runtime.update(cx,|runtime,cx|runtime.reopen_session_history(saved,cx)).unwrap_err();
            assert!(error.contains("being removed"),"{error}");
            assert!(runtime.read(cx).registry.read(cx).all().is_empty());
        });
    }
}

#[test]
fn native_codex_requires_opt_in_and_respects_tmux_override() {
    let mut store = grove_core::storage::Store::default();
    assert!(!native_codex_enabled(&store, None));
    assert!(!native_codex_enabled(&store, Some(false)));
    store.native_agents_enabled = true;
    assert_eq!(native_codex_enabled(&store, None), !cfg!(windows));
    assert_eq!(native_codex_enabled(&store, Some(false)), !cfg!(windows));
    assert!(!native_codex_enabled(&store, Some(true)));
    store.native_agents_enabled = false;
    assert!(!native_codex_enabled(&store, Some(false)));
}

#[test]
fn managed_launch_requires_explicit_tmux_preference() {
    assert!(!saved_tmux_preference_enabled(None));
    assert!(!saved_tmux_preference_enabled(Some(false)));
    assert!(saved_tmux_preference_enabled(Some(true)));
}

#[test]
fn backend_choice_only_on_first_managed_launch_with_tmux() {
    assert!(should_choose_backend(None, || true));
    assert!(!should_choose_backend(None, || false));
    assert!(!should_choose_backend(Some(false), || panic!(
        "must not probe tmux"
    )));
    assert!(!should_choose_backend(Some(true), || panic!(
        "must not probe tmux"
    )));
}

#[test]
fn root_binary_tests_cannot_use_production_tmux() {
    assert!(!managed_session_uses_tmux(Some(true)));
    assert!(!tmux_discovery_enabled());
}

use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct GitFixture(std::path::PathBuf);

impl GitFixture {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for GitFixture {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.0);
    }
}

fn script_project(path: &str, script: &str) -> grove_core::storage::Project {
    grove_core::storage::Project {
        name: "script fixture".into(),
        path: path.into(),
        scripts: grove_core::storage::ProjectScripts {
            run: Some(script.into()),
            ..Default::default()
        },
        archived: false,
        worktree_dir: None,
    }
}

fn init_script_runtime(cx: &mut App, project: grove_core::storage::Project) -> Entity<Runtime> {
    cx.set_global(SettingsState::new(grove_core::storage::Store {
        projects: vec![project],
        ..Default::default()
    }));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.new(Runtime::new)
}

fn git_fixture() -> GitFixture {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
    let dir = GitFixture(std::env::temp_dir().join(format!(
        "grove-script-test-{}-{unique}-{sequence}",
        std::process::id()
    )));
    fs_err::create_dir(&dir.0).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(dir.path())
        .status()
        .unwrap()
        .success());
    dir
}

fn pending_fixture(path: &str) -> PendingManagedLaunch {
    PendingManagedLaunch {
        name: "script fixture".into(),
        cwd: path.into(),
        agent: Agent::Terminal,
        args: Vec::new(),
        context_roots: Vec::new(),
        temp_bundle_path: None,
        record_recent: false,
    }
}

#[gpui::test]
fn rewind_reservation_blocks_new_sessions_before_provider_dispatch(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let root = repo.path().canonicalize().unwrap();
    let path = root.to_string_lossy().into_owned();
    let reservation = grove_core::checkpoint::acquire_restore(&[root]).unwrap();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(&path, "true"));
        runtime.update(cx, |runtime, cx| {
            assert!(!runtime.spawn_session_in("script fixture".into(), path, Agent::Terminal, cx));
            assert!(runtime.registry.read(cx).is_empty());
        });
    });
    drop(reservation);
    assert!(!grove_core::checkpoint::restore_blocked(&[repo
        .path()
        .into()]));
}

#[gpui::test]
fn deferred_launch_does_not_spawn_and_cancel_discards_it(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let path = repo
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(&path, "true"));
        runtime.update(cx, |runtime, cx| {
            runtime.queue_backend_choice(pending_fixture(&path), cx);
        });
        assert!(runtime.read(cx).has_pending_managed_launch());
        assert!(runtime.read(cx).registry.read(cx).is_empty());
        runtime.update(cx, |runtime, _| runtime.cancel_pending_managed_launch());
        assert!(!runtime.read(cx).has_pending_managed_launch());
        assert!(!runtime.update(cx, Runtime::resume_pending_managed_launch));
        assert!(runtime.read(cx).registry.read(cx).is_empty());
    });
}

#[gpui::test]
fn deferred_launch_resumes_once_with_original_target(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let path = repo
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(&path, "true"));
        runtime.update(cx, |runtime, cx| {
            runtime.queue_backend_choice(pending_fixture(&path), cx);
        });
        assert!(runtime.read(cx).registry.read(cx).is_empty());
        cx.global_mut::<SettingsState>().store.tmux_enabled = Some(false);
        assert!(runtime.update(cx, Runtime::resume_pending_managed_launch));
        assert!(!runtime.read(cx).has_pending_managed_launch());
        assert!(!runtime.update(cx, Runtime::resume_pending_managed_launch));
        let registry = runtime.read(cx).registry.read(cx);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.all()[0].wt_path, path);
        assert_eq!(registry.all()[0].agent, Agent::Terminal);
    });
}

#[gpui::test]
fn worktree_shell_pty_runs_in_linked_checkout_without_agent_session(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let root = repo.path().canonicalize().unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture"
        ])
        .status()
        .unwrap()
        .success());
    let linked = root.join("linked");
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "add", "-qb", "shell-fixture"])
        .arg(&linked)
        .status()
        .unwrap()
        .success());
    let path = linked.to_str().unwrap();
    let output = root.join("shell-cwd.txt");
    let runtime = cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(root.to_str().unwrap(), "true"));
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_wt_shell(path, cx)));
        let registry = runtime.read(cx).registry.read(cx);
        assert!(registry.all().is_empty());
        assert!(registry.home_terminals().is_empty());
        let terminal = registry.wt_shell(path, 0).unwrap().clone();
        assert_eq!(terminal.read(cx).initial_cwd(), Some(path));
        assert_eq!(
            terminal.read(cx).backend(),
            &crate::entities::terminal_session::Backend::Native
        );
        (runtime, terminal)
    });
    let (runtime, terminal) = runtime;
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while terminal.read_with(cx, |terminal, _| terminal.root_pid().is_none())
        && Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    terminal
        .update(cx, |terminal, _| {
            terminal
                .send_checked(format!("printf '%s' \"$PWD\" > '{}'\n", output.display()).as_bytes())
        })
        .unwrap();
    for _ in 0..200 {
        if fs_err::read_to_string(&output).is_ok_and(|value| value == path) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        cx.run_until_parked();
    }
    assert_eq!(fs_err::read_to_string(&output).unwrap(), path);
    cx.update(|cx| {
        let service = runtime.read(cx).projects.clone();
        service.update(cx, |service, cx| {
            service
                .kill_sessions_for_project_path(root.to_str().unwrap(), cx)
                .unwrap();
        });
        assert!(runtime
            .read(cx)
            .registry
            .read(cx)
            .wt_shells(path)
            .is_empty());
        assert!(runtime.read(cx).registry.read(cx).all().is_empty());
    });
}

#[gpui::test]
fn worktree_shell_rejects_stale_archived_and_removing_checkouts(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let root = repo.path().canonicalize().unwrap();
    let path = root.to_str().unwrap();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, "true"));
        let missing = root.join("removed");
        assert!(!runtime.update(cx, |runtime, cx| runtime
            .spawn_wt_shell(missing.to_str().unwrap(), cx)));
        cx.global_mut::<SettingsState>().store.projects[0].archived = true;
        assert!(!runtime.update(cx, |runtime, cx| runtime.spawn_wt_shell(path, cx)));
        cx.global_mut::<SettingsState>().store.projects[0].archived = false;
        runtime.read(cx).projects.clone().update(cx, |service, _| {
            service.mark_history_removal_for_test(path, Some(path));
        });
        assert!(!runtime.update(cx, |runtime, cx| runtime.spawn_wt_shell(path, cx)));
        assert!(runtime
            .read(cx)
            .registry
            .read(cx)
            .worktree_shells()
            .next()
            .is_none());
    });
}

#[gpui::test]
fn projectless_directory_runs_scripts_and_terminal(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap();
    cx.update(|cx| {
        let project = script_project(path, "echo directory-script");
        let scripts = project.scripts.clone();
        let runtime = init_script_runtime(cx, project);
        cx.global_mut::<SettingsState>().store.projects.clear();
        cx.global_mut::<SettingsState>()
            .store
            .directory_scripts
            .insert(path.into(), scripts.clone());
        runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta(String::new(), path.into(), Agent::Codex);
        });
        let id = scripts.effective_entries()[0].id.clone();
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_wt_shell(path, cx)));
        assert!(runtime.update(cx, |runtime, cx| runtime
            .spawn_project_script(path, path, &id, cx)));
        assert_eq!(runtime.read(cx).script_runs.len(), 1);
        assert!(cx.global::<SettingsState>().store.projects.is_empty());
        let run = runtime.read(cx).script_runs[0].clone();
        run.terminal.update(cx, TerminalSession::stop_script);
    });
}

#[gpui::test]
fn blank_run_script_does_not_spawn_or_register(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let canonical = repo.path().canonicalize().unwrap();
    let path = canonical.to_str().unwrap();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, "  \n "));
        assert!(!runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
        let registry = runtime.read(cx).registry.read(cx);
        assert!(registry.wt_shells(path).is_empty());
        assert_eq!(registry.active_wt_shell_idx(path), None);
        assert!(registry.is_empty());
    });
}

#[gpui::test]
fn run_script_uses_worktree_cwd_without_selecting_session(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let canonical = repo.path().canonicalize().unwrap();
    let path = canonical.to_str().unwrap();
    let output = repo.path().join("script-cwd.txt");
    let script = format!("pwd > '{}'", output.display());
    let runtime = cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, &script));
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
        let state = runtime.read(cx);
        assert!(state.registry.read(cx).is_empty());
        assert_eq!(state.script_runs.len(), 1);
        let run = &state.script_runs[0];
        assert_eq!(run.project_path, path);
        assert_eq!(run.checkout_path, path);
        assert_eq!(run.command, script);
        assert!(state.state.read(cx).active_session().is_none());
        assert_eq!(
            run.terminal.read(cx).backend(),
            &crate::entities::terminal_session::Backend::Native
        );
        runtime
    });
    let mut actual = None;
    // A login shell can start slowly on a busy CI runner. Wait for its
    // complete pwd line rather than the file's creation or a fixed poll count.
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if let Ok(contents) = fs_err::read_to_string(&output) {
            if contents.ends_with('\n') {
                actual = Some(contents);
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        actual.as_deref().map(str::trim),
        Some(path),
        "login shell did not write the expected cwd within 10 seconds"
    );
    drop(runtime);
}

#[gpui::test]
fn delayed_tmux_restore_revalidates_renamed_archived_removed_and_busy_projects(
    cx: &mut gpui::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(&path, ""));
        let mut session = grove_core::tmux::DiscoveredSession {
            name: "restore-fixture".into(),
            pane_title: None,
            wt_path: path.clone(),
            project: "old-name".into(),
            label: "1".into(),
            agent: Agent::Terminal,
            context_roots: Vec::new(),
            temp_bundle_path: None,
        };
        cx.global_mut::<SettingsState>().store.projects[0].name = "Renamed".into();
        assert!(runtime.read(cx).revalidate_tmux_restore(&mut session, cx));
        assert_eq!(session.project, "Renamed");
        cx.global_mut::<SettingsState>().store.projects[0].archived = true;
        assert!(!runtime.read(cx).revalidate_tmux_restore(&mut session, cx));
        cx.global_mut::<SettingsState>().store.projects[0].archived = false;
        runtime.read(cx).projects.clone().update(cx, |service, _| {
            service.mark_history_removal_for_test(&path, None);
        });
        assert!(!runtime.read(cx).revalidate_tmux_restore(&mut session, cx));
        cx.global_mut::<SettingsState>().store.projects.clear();
        assert!(!runtime.read(cx).revalidate_tmux_restore(&mut session, cx));
    });
}

#[test]
fn fresh_launch_policy_preserves_git_inventory_for_scripts_and_multi_roots() {
    let _settings = SettingsState::new(grove_core::storage::Store::default());
    let directory = tempfile::tempdir().unwrap();
    let projects = vec![script_project(directory.path().to_str().unwrap(), "")];
    let paths = vec![directory.path().to_path_buf()];
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::RegisteredFolder,
        &projects
            .iter()
            .map(|project| project.path.clone())
            .collect::<Vec<_>>()
    )
    .is_ok());
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &projects
            .iter()
            .map(|project| project.path.clone())
            .collect::<Vec<_>>()
    )
    .is_err());
    let repository = git_fixture();
    let projects = vec![script_project(repository.path().to_str().unwrap(), "")];
    let paths = vec![repository.path().to_path_buf()];
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &projects
            .iter()
            .map(|project| project.path.clone())
            .collect::<Vec<_>>()
    )
    .is_ok());
    fs_err::rename(
        repository.path().join(".git"),
        repository.path().join("retained-git-metadata"),
    )
    .unwrap();
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::RegisteredFolder,
        &projects
            .iter()
            .map(|project| project.path.clone())
            .collect::<Vec<_>>()
    )
    .is_ok());
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &projects
            .iter()
            .map(|project| project.path.clone())
            .collect::<Vec<_>>()
    )
    .is_err());
}

#[test]
fn fresh_launch_validation_rejects_checkout_reassigned_to_another_project() {
    let _settings = SettingsState::new(grove_core::storage::Store::default());
    let first = git_fixture();
    let second = git_fixture();
    let destination = tempfile::tempdir().unwrap();
    let checkout = destination.path().join("checkout");
    let git = |repository: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for repository in [first.path(), second.path()] {
        git(
            repository,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
    }
    let target = checkout.to_str().unwrap();
    git(first.path(), &["worktree", "add", "--detach", target]);
    let projects = vec![
        script_project(first.path().to_str().unwrap(), ""),
        script_project(second.path().to_str().unwrap(), ""),
    ];
    let paths = vec![checkout.clone()];
    let expected = vec![projects[0].path.clone()];
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &expected
    )
    .is_ok());
    git(first.path(), &["worktree", "remove", target]);
    git(second.path(), &["worktree", "add", "--detach", target]);
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &expected
    )
    .is_err());
    assert!(validate_checkout_paths(
        &projects,
        &paths,
        CheckoutValidation::GitInventory,
        &[projects[1].path.clone()]
    )
    .is_ok());
}

#[gpui::test]
fn failed_run_script_spawn_shows_error_without_bogus_shell(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let canonical = repo.path().canonicalize().unwrap();
    let path = canonical.to_str().unwrap();
    let runtime = cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, "\0"));
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
        runtime
    });
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while runtime.read_with(cx, |runtime, _| !runtime.script_runs.is_empty())
        && Instant::now() < deadline
    {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.update(|cx| {
        let state = runtime.read(cx);
        assert!(
            state.script_runs.is_empty(),
            "failed asynchronous launches must remove their pending run"
        );
        let registry = state.registry.read(cx);
        assert!(registry.wt_shells(path).is_empty());
        assert_eq!(registry.active_wt_shell_idx(path), None);
        assert!(registry.is_empty());
        assert!(state
            .toast
            .read(cx)
            .current()
            .unwrap()
            .message
            .starts_with("terminal failed:"));
    });
}

#[gpui::test]
fn repeated_run_scripts_have_distinct_inline_ids_and_removal_guard(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let canonical = repo.path().canonicalize().unwrap();
    let path = canonical.to_str().unwrap();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, "sleep 30"));
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
        assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
        let state = runtime.read(cx);
        assert!(state.registry.read(cx).is_empty());
        assert_eq!(state.script_runs.len(), 2);
        let first = state.script_runs[0].clone();
        let second = state.script_runs[1].clone();
        assert_ne!(first.id, second.id);
        assert_eq!(first.script_id, second.script_id);
        let service = state.projects.clone();
        assert!(service
            .update(cx, |service, cx| service.archive_blockers(path, cx))
            .is_err());
        first.terminal.update(cx, TerminalSession::stop_script);
        second.terminal.update(cx, TerminalSession::stop_script);
        runtime.update(cx, |runtime, _| {
            runtime.script_runs.retain(|run| run.id != first.id);
        });
        assert_eq!(runtime.read(cx).script_runs.len(), 1);
        assert_eq!(runtime.read(cx).script_runs[0].id, second.id);
    });
}

#[gpui::test]
fn script_edits_preserve_run_snapshots_and_stop_retyped_or_deleted_runs(
    cx: &mut gpui::TestAppContext,
) {
    use grove_core::storage::{ProjectAction, ProjectActionKind, ProjectScripts};
    let repo = git_fixture();
    let path = repo
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let scripts = ProjectScripts {
        entries: vec![ProjectAction {
            id: "stable-action".into(),
            name: "Original".into(),
            kind: ProjectActionKind::Shell {
                command: "sleep 30".into(),
            },
        }],
        ..Default::default()
    };
    let runtime = cx.update(|cx| {
        let mut project = script_project(&path, "");
        project.scripts = scripts.clone();
        let runtime = init_script_runtime(cx, project);
        assert!(
            runtime.update(cx, |runtime, cx| runtime.spawn_project_script(
                &path,
                &path,
                "stable-action",
                cx
            ))
        );
        runtime
    });
    let (service, original) = cx.update(|cx| {
        let state = runtime.read(cx);
        (state.projects.clone(), state.script_runs[0].clone())
    });
    cx.update(|cx| {
        assert!(service
            .update(cx, |service, cx| service.archive_project_by_path(&path, cx))
            .is_err());
        let mut renamed = scripts.clone();
        renamed.entries[0].name = "Renamed".into();
        renamed.entries[0].kind = ProjectActionKind::Shell {
            command: "sleep 31".into(),
        };
        service
            .update(cx, |service, cx| {
                service.update_project(&path, "script fixture".into(), renamed, cx)
            })
            .unwrap();
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let state = runtime.read(cx);
        assert_eq!(state.script_runs.len(), 1);
        assert_eq!(state.script_runs[0].id, original.id);
        assert_eq!(state.script_runs[0].name, "Original");
        assert_eq!(state.script_runs[0].command, "sleep 30");
        assert_eq!(state.script_runs[0].checkout_path, path);
        assert!(!original.terminal.read(cx).has_exited());
        let mut prompt = scripts.clone();
        prompt.entries[0].kind = ProjectActionKind::Prompt {
            template: "Review the changes".into(),
        };
        service
            .update(cx, |service, cx| {
                service.update_project(&path, "script fixture".into(), prompt, cx)
            })
            .unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !original
        .terminal
        .read_with(cx, |terminal, _| terminal.has_exited())
    {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(25));
        assert!(
            std::time::Instant::now() < deadline,
            "script must confirm actual process exit"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.update(|cx| {
        assert!(runtime.read(cx).script_runs.is_empty());
        assert!(original.terminal.read(cx).has_exited());
        service
            .update(cx, |service, cx| {
                service.update_project(&path, "script fixture".into(), scripts.clone(), cx)
            })
            .unwrap();
    });
    cx.run_until_parked();
    let deleted = cx.update(|cx| {
        assert!(
            runtime.update(cx, |runtime, cx| runtime.spawn_project_script(
                &path,
                &path,
                "stable-action",
                cx
            ))
        );
        let deleted = runtime.read(cx).script_runs[0].clone();
        assert_ne!(deleted.id, original.id);
        service
            .update(cx, |service, cx| {
                service.update_project(
                    &path,
                    "script fixture".into(),
                    ProjectScripts::default(),
                    cx,
                )
            })
            .unwrap();
        deleted
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !deleted
        .terminal
        .read_with(cx, |terminal, _| terminal.has_exited())
    {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(25));
        assert!(
            std::time::Instant::now() < deadline,
            "script must confirm actual process exit"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.update(|cx| {
        assert!(runtime.read(cx).script_runs.is_empty());
        assert!(deleted.terminal.read(cx).has_exited());
        assert!(service
            .update(cx, |service, cx| service.archive_blockers(&path, cx))
            .unwrap()
            .is_empty());
    });
}

#[gpui::test]
fn multi_root_launch_returns_false_for_empty_or_stale_selection(cx: &mut gpui::TestAppContext) {
    let repo = git_fixture();
    let canonical = repo.path().canonicalize().unwrap();
    let path = canonical.to_str().unwrap();
    let stale = canonical
        .join("removed-worktree")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project(path, "echo test"));
        assert!(!runtime.update(cx, |runtime, cx| {
            runtime.launch_multi_root_session(&[], Agent::Terminal, cx)
        }));
        assert!(!runtime.update(cx, |runtime, cx| {
            runtime.launch_multi_root_session(&[stale], Agent::Terminal, cx)
        }));
        assert!(runtime.read(cx).registry.read(cx).is_empty());
        assert!(cx
            .global::<SettingsState>()
            .store
            .recent_launches
            .is_empty());
    });
}

#[gpui::test]
fn empty_shell_runtime_does_not_spawn_sessions_or_rewrite_preferences(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let store = grove_core::storage::Store {
            onboarded: true,
            ui_zoom: Some(1.4),
            grid_order: vec!["saved-session-order".to_string()],
            ..Default::default()
        };
        let before = serde_json::to_value(&store).ok();
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(Runtime::new);
        assert!(runtime.read(cx).registry.read(cx).is_empty());
        assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
        assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
        runtime.update(cx, Runtime::shutdown);
        assert_eq!(
            serde_json::to_value(&cx.global::<SettingsState>().store).ok(),
            before
        );
    });
}

#[gpui::test]
fn closing_only_home_terminal_stays_empty_until_explicitly_opened(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(Runtime::new);
        let first = runtime
            .update(cx, Runtime::spawn_home_terminal)
            .expect("shell should start");
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
        assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
        assert_eq!(runtime.read(cx).state.read(cx).active_terminal(), None);
        let reopened = runtime
            .update(cx, Runtime::spawn_home_terminal)
            .expect("explicitly opening a shell should still work");
        let registry = runtime.read(cx).registry.read(cx);
        assert_ne!(first, reopened);
        assert_eq!(registry.home_terminal_count(), 1);
        assert_eq!(registry.home_terminals()[0].id, reopened);
        assert_eq!(registry.home_terminals()[0].label, "terminal 2");
        assert_eq!(runtime.read(cx).state.read(cx).active_terminal(), Some(0));
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
    });
}

#[gpui::test]
fn closing_nonfinal_home_terminal_does_not_spawn(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let runtime = cx.new(Runtime::new);
        let first = runtime
            .update(cx, Runtime::spawn_home_terminal)
            .expect("first shell should start");
        let second = runtime
            .update(cx, Runtime::spawn_home_terminal)
            .expect("second shell should start");
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(2, cx));
        assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 2);
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
        let registry = runtime.read(cx).registry.read(cx);
        assert_eq!(registry.home_terminal_count(), 1);
        assert_eq!(registry.home_terminals()[0].id, second);
        assert_ne!(registry.home_terminals()[0].id, first);
        assert_eq!(registry.home_terminals()[0].label, "terminal 2");
        runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
    });
}
#[gpui::test]
fn native_title_updates_existing_metadata_only_when_title_changes(cx: &mut gpui::TestAppContext) {
    let (handle, _actions) = grove_core::codex::Handle::channel();
    let (_updates, receive) = std::sync::mpsc::channel();
    let (runtime, session, id) = cx.update(|cx| {
        let runtime = init_script_runtime(cx, script_project("/native-title-fixture", "true"));
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(
                "script fixture".into(),
                "/native-title-fixture".into(),
                Agent::Codex,
            )
        });
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Explicit,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: HashMap::default(),
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
            key: "native-title-test".into(),
            project: "script fixture".into(),
            cwd: "/native-title-fixture".into(),
            label: "Codex".into(),
            title: Some("Recovered conversation".into()),
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("thread-title".into()),
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let connection = grove_core::codex::Connection {
            handle: handle.clone(),
            updates: receive.into(),
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                grove_core::codex::Snapshot::new(saved),
                connection,
                cx,
            )
        });
        registry.update(cx, |registry, _| {
            registry.attach_codex(id, session.clone(), handle);
        });
        runtime.update(cx, |runtime, cx| {
            runtime.observe_native_title(id, &session, cx);
        });
        (runtime, session, id)
    });
    cx.run_until_parked();
    let registry = runtime.read_with(cx, |runtime, _| runtime.registry.clone());
    let stable_label =
        registry.read_with(cx, |registry, _| registry.meta(id).unwrap().label.clone());
    assert_eq!(
        registry.read_with(cx, |registry, _| registry
            .meta(id)
            .unwrap()
            .restored_title
            .clone()),
        Some("Recovered conversation".into())
    );
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = notifications.clone();
    let _observer = cx.update(|cx| cx.observe(&registry, move |_, _| seen.set(seen.get() + 1)));
    session.update(cx, |session, cx| {
        session.state.saved.title = Some("Updated provider title".into());
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        registry.read_with(cx, |registry, _| registry
            .meta(id)
            .unwrap()
            .restored_title
            .clone()),
        Some("Updated provider title".into())
    );
    assert_eq!(notifications.get(), 1);
    session.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
    assert_eq!(
        registry.read_with(cx, |registry, _| registry.meta(id).unwrap().label.clone()),
        stable_label
    );
    assert_eq!(
        registry.read_with(cx, |registry, _| registry.meta(id).unwrap().id),
        id
    );
}
