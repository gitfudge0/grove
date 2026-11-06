//! Opt-in live Linux acceptance through the actual Sidebar owner and renderer.
//! A fresh empty GROVE_CONFIG_DIR below ignored artifacts is mandatory. Composer
//! Submit has a separate fixture; this starts at the real native skill event.
use super::*;

const TITLE: &str = "Grove live Describe changes acceptance";
const SOURCE_DRAFT: &str = "/describe-changes local";
const EDIT: &str = "export function shouldProcess(seen: boolean) { return !seen; }\n";

fn git(path: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("Temporary native fixture Git operation failed".into());
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}
fn ensure(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.into())
}
fn press_enter(window: &mut Window, cx: &mut App) {
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    window.dispatch_event(
        gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
        cx,
    );
}

#[test]
#[ignore = "live Codex call and native window; use empty artifact GROVE_CONFIG_DIR and 16 MiB stack"]
fn describe_changes_live_native_session_pipeline() {
    let config = std::path::PathBuf::from(
        std::env::var("GROVE_CONFIG_DIR").expect("use an empty isolated native QA config"),
    );
    assert!(
        config.is_absolute()
            && config.is_dir()
            && config
                .to_string_lossy()
                .contains(".grove-artifacts/description-lifecycle/")
            && std::fs::read_dir(&config).unwrap().next().is_none(),
        "refusing to use a nonempty or non-artifact Grove configuration"
    );
    assert_eq!(
        std::env::var_os("GROVE_TEST_CONFIG_DIR"),
        Some(config.clone().into_os_string()),
        "GROVE_TEST_CONFIG_DIR must match GROVE_CONFIG_DIR so SettingsState keeps the isolated store"
    );
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path().to_string_lossy().into_owned();
    git(repository.path(), &["init", "-b", "main"]).unwrap();
    git(
        repository.path(),
        &["config", "user.name", "Grove Native QA"],
    )
    .unwrap();
    git(
        repository.path(),
        &["config", "user.email", "qa@example.test"],
    )
    .unwrap();
    std::fs::write(
        repository.path().join("handler.ts"),
        "export function shouldProcess(seen: boolean) { return true; }\n",
    )
    .unwrap();
    git(repository.path(), &["add", "."]).unwrap();
    git(repository.path(), &["commit", "-m", "Native fixture"]).unwrap();
    std::fs::write(repository.path().join("handler.ts"), EDIT).unwrap();
    let initial_head = git(repository.path(), &["rev-parse", "HEAD"]).unwrap();
    let initial_index = git(repository.path(), &["ls-files", "--stage"]).unwrap();
    let initial_status = git(repository.path(), &["status", "--porcelain=v1"]).unwrap();
    let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
    let completed = outcome.clone();
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            crate::fonts::register(cx).unwrap();
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            grove_core::theme::set_by_name("grove-dark");
            c::sync_component_theme(cx);
            let mut settings = grove_core::storage::Store {
                native_agents_enabled: true,
                default_agent: Some(Agent::Codex),
                ..Default::default()
            };
            settings.projects.push(grove_core::storage::Project {
                name: "Native description fixture".into(),
                path: path.clone(),
                scripts: Default::default(),
                archived: false,
                worktree_dir: None,
            });
            settings.assign_project_to_active_workspace(&path);
            cx.set_global(SettingsState::new(settings));
            assert_eq!(
                grove_core::storage::config_dir().unwrap(),
                config,
                "SettingsState changed the isolated native acceptance directory"
            );
            let (handle, actions) = grove_core::codex::Handle::channel();
            let (updates, receive) = std::sync::mpsc::channel();
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(1280.), gpui::px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title(TITLE);
                        window.set_rem_size(gpui::px(16.));
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
                        let mut saved = grove_core::codex::SavedSession::empty(
                            "native-description-source".into(),
                            grove_core::codex::Provider::Codex,
                            "Native description fixture".into(),
                            path.clone(),
                        );
                        saved.composer.model = Some("gpt-6-luna".into());
                        saved.composer.effort = Some("low".into());
                        saved.draft = SOURCE_DRAFT.into();
                        saved.thread_id = Some("native-description-source-thread".into());
                        let mut snapshot = grove_core::codex::Snapshot::new(saved);
                        snapshot.status = grove_core::codex::Status::Idle;
                        let source = cx.new(|cx| {
                            crate::entities::codex_session::CodexSession::from_connection(
                                snapshot,
                                grove_core::codex::Connection {
                                    handle,
                                    updates: receive.into(),
                                },
                                cx,
                            )
                        });
                        let source_id = runtime.update(cx, |runtime, cx| {
                            runtime.adopt_dashboard_session(source.clone(), cx)
                        });
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
                            sidebar.initial_selection_pending = false;
                            sidebar.dashboard_open = false;
                            sidebar.mode = ViewMode::List;
                            sidebar.last_mode = ViewMode::List;
                            sidebar.destination =
                                super::super::global_navigation::Destination::Sessions;
                            sidebar.snapshot = runtime.update(cx, |runtime, cx| runtime.snapshot(cx));
                            sidebar.selection = Some(Selection::Session(source_id));
                            sidebar.checkout_panel.hidden = false;
                            sidebar.checkout_panel.expanded = true;
                            sidebar.checkout_panel.chosen_root = Some(path.clone());
                            sidebar
                        });
                        retained = Some((sidebar.clone(), source, source_id));
                        cx.new(|cx| gpui_component::Root::new(sidebar, window, cx).bordered(false))
                    },
                )
                .unwrap();
            let (sidebar, source, source_id) = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let _keep_alive = (repository, actions, updates);
                let result: Result<(), String> = async {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let (new_id, new_key, target) = cx.update_window(window.into(), |_, window, cx| {
                        sidebar.update(cx, |sidebar, cx| {
                            let focus = cx.focus_handle();
                            sidebar.invoke_description_skill(ScopePreference::Local, focus, cx);
                            let Some(Selection::Session(new_id)) = sidebar.selection else {
                                return Err("Skill did not select a description session".into());
                            };
                            ensure(new_id != source_id, "Skill reused the source session")?;
                            let new_session = sidebar.runtime.read(cx).registry.read(cx).codex(new_id)
                                .cloned().ok_or("New description session is missing")?;
                            let new_key = new_session.read(cx).state.saved.key.clone();
                            ensure(new_key != source.read(cx).state.saved.key, "New session reused the source key")?;
                            ensure(source.read(cx).state.saved.draft == SOURCE_DRAFT, "Source command draft was lost")?;
                            let target = sidebar.descriptions.linked.get(&new_id).cloned().ok_or("New session has no description link")?;
                            ensure(sidebar.descriptions.open.as_ref() == Some(&target), "New reader did not open")?;
                            ensure(sidebar.descriptions.entries.get(&target).is_some_and(|entry| entry.session == Some(new_id)), "Reader ownership differs from new session")?;
                            let _ = window;
                            Ok::<_, String>((new_id, new_key, target))
                        })
                    }).map_err(|error| error.to_string())??;
                    eprintln!("Native skill started a distinct Codex session and retained the source draft");
                    let deadline = Instant::now() + Duration::from_secs(120);
                    let description = loop {
                        if Instant::now() >= deadline {
                            return Err("Native description generation timed out".into());
                        }
                        let progress = cx.update_window(window.into(), |_, _, cx| {
                            let sidebar = sidebar.read(cx);
                            let entry = sidebar.descriptions.entries.get(&target).ok_or("Reader entry disappeared")?;
                            if let Some(error) = &entry.error { return Err(format!("Native description failed: {error}")); }
                            Ok(entry.result.clone().filter(|_| entry.cancel.is_none() && !entry.finalizing))
                        }).map_err(|error| error.to_string())??;
                        if let Some(description) = progress { break description; }
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                    };
                    ensure(description.files.len() == 1 && description.files[0].path == "handler.ts", "Generated file walkthrough did not cover the fixture")?;
                    let store_root = config.join("change-descriptions");
                    let store = grove_core::change_description_store::Store::new(store_root.clone());
                    let binding = store.source_for_session(&new_key)?.ok_or("Durable session binding is missing")?;
                    let canonical = store.result_path(&binding)?;
                    ensure(canonical.is_file(), "Canonical description file was not saved")?;
                    let reloaded = grove_core::change_description_store::Store::new(store_root);
                    ensure(reloaded.load(&binding)? == Some(description.as_ref().clone()), "Restarted store did not load the exact generated result")?;
                    ensure(reloaded.pending_for_session(&new_key)?.is_none(), "Pending generation remained after success")?;
                    ensure(git(std::path::Path::new(&path), &["rev-parse", "HEAD"])? == initial_head, "Generation changed HEAD")?;
                    ensure(git(std::path::Path::new(&path), &["ls-files", "--stage"])? == initial_index, "Generation changed the index")?;
                    ensure(git(std::path::Path::new(&path), &["status", "--porcelain=v1"])? == initial_status, "Generation changed working files")?;
                    cx.update_window(window.into(), |_, window, cx| {
                        sidebar.update(cx, |sidebar, cx| sidebar.close_description(window, cx));
                    }).map_err(|error| error.to_string())?;
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    cx.update_window(window.into(), |_, window, cx| {
                        let sidebar = sidebar.read(cx);
                        ensure(sidebar.descriptions.open.is_none(), "Back did not return to conversation")?;
                        ensure(sidebar.selection == Some(Selection::Session(new_id)), "Back selected a different conversation")?;
                        let focus = sidebar.descriptions.open_source.as_ref().ok_or("Right-panel Describe changes button was not rendered")?.clone();
                        focus.focus(window, cx);
                        window.refresh();
                        Ok::<_, String>(())
                    }).map_err(|error| error.to_string())??;
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    cx.update_window(window.into(), |_, window, cx| press_enter(window, cx)).map_err(|error| error.to_string())?;
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    cx.update_window(window.into(), |_, _, cx| {
                        let sidebar = sidebar.read(cx);
                        ensure(sidebar.description_is_open(cx) && sidebar.descriptions.open.as_ref() == Some(&target), "Right-panel keyboard action did not reopen the same reader")?;
                        ensure(source.read(cx).state.saved.draft == SOURCE_DRAFT, "Navigation changed the original draft")?;
                        Ok::<_, String>(())
                    }).map_err(|error| error.to_string())??;
                    // Exercise real freshness from a changed fixture, without generating again.
                    std::fs::write(std::path::Path::new(&path).join("handler.ts"), format!("{EDIT}// Changed after generation.\n")).map_err(|error| error.to_string())?;
                    cx.update_window(window.into(), |_, _, cx| sidebar.update(cx, |sidebar, cx| {
                        let entry = sidebar.descriptions.entries.get_mut(&target).unwrap();
                        entry.checked_at = None;
                        entry.checking = false;
                        sidebar.check_description(&target, cx);
                    })).map_err(|error| error.to_string())?;
                    let stale_deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        let stale = cx.update_window(window.into(), |_, _, cx| {
                            sidebar.read(cx).descriptions.entries.get(&target)
                                .is_some_and(|entry| matches!(entry.freshness, Some(Ok(false))))
                        }).map_err(|error| error.to_string())?;
                        if stale { break; }
                        ensure(Instant::now() < stale_deadline, "Native reader did not mark the changed source stale")?;
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                    }
                    ensure(reloaded.load(&binding)? == Some(description.as_ref().clone()), "Staleness overwrote the saved description")?;
                    std::fs::write(std::path::Path::new(&path).join("handler.ts"), EDIT).map_err(|error| error.to_string())?;
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    if let Some(output) = config.parent() {
                        let captured = std::process::Command::new("import").args(["-window", TITLE]).arg(output.join("live-native-session-reader.png")).status();
                        if captured.is_ok_and(|status| status.success()) { eprintln!("Captured the owned native description window"); }
                    }
                    eprintln!("PASS native lifecycle: real session -> capture -> normal Codex -> import -> canonical save/reload; Back and right-panel Enter preserved ownership; changed fixture marked stale; source draft and Git state preserved");
                    Ok(())
                }.await;
                // Close only actual sessions created by this test, never the mock source.
                let handles = cx.update_window(window.into(), |_, _, cx| {
                    let sidebar = sidebar.read(cx);
                    let registry = sidebar.runtime.read(cx).registry.read(cx);
                    registry.all().iter().filter(|meta| meta.id != source_id)
                        .filter_map(|meta| registry.codex(meta.id).map(|session| session.read(cx).handle.clone()))
                        .collect::<Vec<_>>()
                }).unwrap_or_default();
                let stopped = cx.background_spawn(async move {
                    for handle in handles {
                        handle.stop_completion(false).recv_timeout(Duration::from_secs(10))
                            .map_err(|_| "Owned native provider did not acknowledge shutdown".to_string())?;
                    }
                    Ok::<_, String>(())
                }).await;
                *completed.borrow_mut() = Some(result.and(stopped));
                cx.update(|cx| cx.quit());
            }).detach();
        });
    let result = outcome
        .borrow_mut()
        .take()
        .expect("native acceptance did not complete");
    assert!(result.is_ok(), "{}", result.unwrap_err());
}
