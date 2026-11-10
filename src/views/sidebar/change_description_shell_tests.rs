//! Opt-in captures of the actual Sidebar shell with frozen native sessions.
//! Run with RUST_MIN_STACK=16777216, GPUI_X11_SCALE_FACTOR=1 and a fresh
//! GROVE_CONFIG_DIR below .grove-artifacts/description-lifecycle/native/.
use super::*;

const TITLE: &str = "Grove Describe changes full shell QA";

struct NativeShell {
    sidebar: Entity<Sidebar>,
    drive_frames: std::rc::Rc<std::cell::Cell<bool>>,
    measured: std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}
impl Render for NativeShell {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Drive presentation while capturing state changes without pointer input.
        if self.drive_frames.get() {
            window.request_animation_frame();
        }
        let measured = self.measured.clone();
        div()
            .relative()
            .size_full()
            .font_family(crate::fonts::UI_FAMILY)
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured.set(bounds.size),
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(self.sidebar.clone())
    }
}
fn press_key(window: &mut Window, cx: &mut App, name: &str) {
    let keystroke = gpui::Keystroke::parse(name).unwrap();
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
#[ignore = "opens an isolated native Sidebar; use a 16 MiB test stack and fresh GROVE_CONFIG_DIR"]
fn describe_changes_full_sidebar_linux_capture() {
    let config =
        std::path::PathBuf::from(std::env::var("GROVE_CONFIG_DIR").expect("use a fresh QA config"));
    assert!(
        config.is_absolute()
            && config
                .to_string_lossy()
                .contains(".grove-artifacts/description-lifecycle/native/")
    );
    let output = config.parent().unwrap().to_path_buf();
    fs_err::create_dir_all(&output).unwrap();
    let repository = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Grove QA",
            "-c",
            "user.email=qa@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "QA fixture",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap()
            .status
            .success());
    }
    let path = repository.path().to_string_lossy().into_owned();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
        initialize(cx);
        crate::fonts::register(cx).unwrap();
        grove_core::theme::set_by_name("grove-dark");
        c::sync_component_theme(cx);
        let mut settings = grove_core::storage::Store::default();
        settings.projects.push(grove_core::storage::Project { name: "northstar".into(), path: path.clone(), scripts: grove_core::storage::ProjectScripts::default(), archived: false, worktree_dir: None });
        settings.assign_project_to_active_workspace(&path);
        cx.set_global(SettingsState::new(settings));
        let store_root = config.join("change-descriptions");
        let store = grove_core::change_description_store::Store::new(store_root.clone());
        let mut description = frozen_description();
        description.snapshot.checkout = path.clone();
        store.save(&description).unwrap();
        let session_key = "full-shell-codex-description";
        store.bind(session_key, &grove_core::change_description_store::Source::from(&description)).unwrap();
        let (handle, actions) = grove_core::codex::Handle::channel();
        let (updates, receive) = std::sync::mpsc::channel();
        let mut sidebar_handle = None;
        let measured = std::rc::Rc::new(std::cell::Cell::new(gpui::size(gpui::px(0.), gpui::px(0.))));
        let measured_window = measured.clone();
        let drive_frames = std::rc::Rc::new(std::cell::Cell::new(true));
        let window_drive_frames = drive_frames.clone();
        let window = cx.open_window(gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(gpui::point(gpui::px(40.), gpui::px(40.)), gpui::size(gpui::px(1280.), gpui::px(900.))))),
            ..Default::default()
        }, |window, cx| {
            window.set_window_title(TITLE);
            window.set_rem_size(gpui::px(16.));
            let runtime = cx.new(crate::runtime::Runtime::new);
            runtime.read(cx).tree.clone().update(cx, |tree, _| tree.set_active_worktrees(0, vec![grove_core::git::Worktree { path: path.clone(), branch: "main".into(), mtime: None, is_main: true }]));
            let mut saved = grove_core::codex::SavedSession::empty(session_key.into(), grove_core::codex::Provider::Codex, "northstar".into(), path.clone());
            saved.title = Some("Describe PR #142".into());
            saved.thread_id = Some("frozen-description-thread".into());
            saved.draft = "Can you walk me through the duplicate-delivery guard?".into();
            saved.items = vec![serde_json::json!({"id":"describe-request","type":"userMessage","content":[{"type":"text","text":"/describe-changes https://github.com/example/northstar/pull/142"}]}), serde_json::json!({"id":"describe-result","type":"agentMessage","phase":"final","text":"The description is saved. Delivery IDs are claimed before creating orders, and duplicate deliveries are covered by tests. Open Describe changes in the right panel to inspect the captured walkthrough."})].into();
            let mut snapshot = grove_core::codex::Snapshot::new(saved);
            snapshot.status = grove_core::codex::Status::Idle;
            let session = cx.new(|cx| crate::entities::codex_session::CodexSession::from_connection(snapshot, grove_core::codex::Connection { handle, updates: receive.into() }, cx));
            let id = runtime.update(cx, |runtime, cx| runtime.adopt_dashboard_session(session.clone(), cx));
            let sidebar = cx.new(|cx| {
                let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
                sidebar.initial_selection_pending = false;
                sidebar.dashboard_open = false;
                sidebar.mode = ViewMode::List;
                sidebar.last_mode = ViewMode::List;
                sidebar.destination = super::super::super::global_navigation::Destination::Sessions;
                sidebar.snapshot = runtime.update(cx, |runtime, cx| runtime.snapshot(cx));
                sidebar.selection = Some(Selection::Session(id));
                sidebar.checkout_panel.hidden = false;
                sidebar.checkout_panel.expanded = true;
                sidebar.checkout_panel.chosen_root = Some(path.clone());
                let target = key(&sidebar.selection, &path, &description.snapshot.scope);
                let mut entry = Entry { scope: Some(description.snapshot.scope.clone()), session: Some(id), session_key: Some(session_key.into()), result: Some(Arc::new(description.clone())), freshness: Some(Ok(true)), checking: true, checked_at: Some(Instant::now()), ..Default::default() };
                entry.text = Some(cx.new(|cx| TextViewState::markdown("# Prevent duplicate webhook orders\n\nBug fix · Tests\n\n## Description\n\n- Claim webhook delivery IDs before creating orders.\n- Cover duplicate deliveries in the webhook tests.", cx)));
                for file in &description.files {
                    entry.file_text.insert(file.path.clone(), cx.new(|cx| TextViewState::markdown(&format!("**{}**\n\n- {}", file.title, file.summary[0]), cx)));
                }
                sidebar.descriptions.entries.insert(target.clone(), entry);
                sidebar.descriptions.linked.insert(id, target);
                sidebar.descriptions.hydrated.insert((id, session_key.into()));
                sidebar.descriptions.store_root = Some(store_root.clone());
                sidebar
            });
            sidebar_handle = Some((sidebar.clone(), session));
            let shell = cx.new(|_| NativeShell { sidebar, drive_frames: window_drive_frames, measured: measured_window });
            cx.new(|cx| gpui_component::Root::new(shell, window, cx).bordered(false))
        }).unwrap();
        let (sidebar, session) = sidebar_handle.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let keep_alive = (repository, actions, updates);
            cx.background_executor().timer(Duration::from_millis(900)).await;
            for (name, state) in [("shell-conversation-dark", "conversation"), ("shell-menu-dark", "menu"), ("shell-description-dark", "description"), ("shell-back-conversation-dark", "back"), ("shell-stale-panel-dark", "stale"), ("shell-stale-description-dark", "stale-description"), ("shell-narrow-description-dark", "narrow"), ("shell-stale-description-light", "light")] {
                cx.update_window(window.into(), |_, window, cx| {
                    if matches!(state, "menu" | "description" | "stale-description") {
                        let focus = sidebar.read(cx).checkout_panel.change_tools.focus.get(&false).expect("actual Checkout panel must render PR menu").clone();
                        focus.focus(window, cx);
                        window.refresh();
                    }
                    if state == "back" { press_key(window, cx, "escape"); }
                    if state == "stale" { sidebar.update(cx, |sidebar, cx| { let target = sidebar.descriptions.linked.values().next().unwrap().clone(); sidebar.descriptions.entries.get_mut(&target).unwrap().freshness = Some(Ok(false)); cx.notify(); }); }
                    if state == "narrow" { window.resize(gpui::size(gpui::px(760.), gpui::px(900.))); }
                    if state == "light" {
                        assert!(grove_core::theme::set_by_name("grove-light"));
                        c::sync_component_theme(cx);
                        cx.refresh_windows();
                        sidebar.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    }
                }).unwrap();
                cx.background_executor().timer(Duration::from_millis(120)).await;
                if state == "menu" {
                    cx.update_window(window.into(), |_, window, cx| press_key(window, cx, "enter")).unwrap();
                }
                if matches!(state, "description" | "stale-description") {
                    cx.update_window(window.into(), |_, window, cx| { press_key(window, cx, "enter"); press_key(window, cx, "down"); press_key(window, cx, "enter"); }).unwrap();
                }
                cx.background_executor().timer(Duration::from_millis(420)).await;
                let mut metadata = fs_err::OpenOptions::new().create(true).append(true).open(output.join("shell-measurements.txt")).unwrap();
                cx.update_window(window.into(), |_, window, cx| {
                    let sidebar = sidebar.read(cx);
                    if state == "menu" { assert!(sidebar.checkout_panel.change_tools.menu.is_some(), "actual Checkout section menu opened"); }
                    let open = sidebar.descriptions.open.is_some();
                    assert_eq!(open, matches!(state, "description" | "stale-description" | "narrow" | "light"), "native keyboard activation: {state}");
                    assert_eq!(session.read(cx).state.saved.draft, "Can you walk me through the duplicate-delivery guard?");
                    writeln!(metadata, "{name}: viewport {:?}; measured shell {:?}; reading={open}; description scroll bounds {:?}; active theme {} {:?}", window.viewport_size(), measured.get(), sidebar.descriptions.entries.values().next().unwrap().scroll.bounds(), grove_core::theme::current().name, grove_core::theme::current().kind).unwrap();
                    if let Some(tree) = window.debug_a11y_tree_json() { fs_err::write(output.join(format!("{name}-a11y.json")), tree).unwrap(); }
                }).unwrap();
                assert!(std::process::Command::new("import").args(["-window", TITLE]).arg(output.join(format!("{name}.png"))).status().unwrap().success());
            }
            // Capture the grouped production panel with frozen provider fixtures.
            // No provider process, shell script or GitHub mutation is started.
            fs_err::write(keep_alive.0.path().join("main.rs"), "fn main() {}\n").unwrap();
            let frozen_git = grove_core::checkout_git::inspect(keep_alive.0.path()).unwrap();
            let summary = grove_core::diff::summarize(&grove_core::diff::changed_files(&path));
            for provider in [grove_core::codex::Provider::Codex, grove_core::codex::Provider::Claude] {
                for (state, theme, width, rem) in [
                    ("dirty", "grove-dark", 1280., 16.),
                    ("clean", "grove-light", 1280., 16.),
                    ("working", "grove-dark", 1280., 16.),
                    ("narrow", "grove-light", 760., 20.),
                    ("pull-error", "grove-dark", 1280., 16.),
                    ("pull-error-narrow", "grove-light", 760., 20.),
                ] {
                    cx.update_window(window.into(), |_, window, cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        window.resize(gpui::size(gpui::px(width), gpui::px(1100.)));
                        window.set_rem_size(gpui::px(rem));
                        cx.global_mut::<SettingsState>().store.projects[0].scripts.entries = vec![
                            grove_core::storage::ProjectAction { id: "dev".into(), name: "Dev server".into(), kind: grove_core::storage::ProjectActionKind::Shell { command: "npm run dev".into() } },
                            grove_core::storage::ProjectAction { id: "review".into(), name: "Review payment retry edge cases".into(), kind: grove_core::storage::ProjectActionKind::Prompt { template: "Review payment retry edge cases".into() } },
                        ];
                        session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.saved.work_periods = vec![grove_core::codex::WorkPeriod { start_item: 0, end_item: None, duration_ms: None, subagents: vec![grove_core::codex::Subagent { id: "group-qa".into(), name: "Review retry edge cases".into(), activity: "Inspecting payment handlers".into(), role: None, task: Some("Review idempotency".into()), status: if state == "working" { grove_core::codex::SubagentStatus::Working } else { grove_core::codex::SubagentStatus::Completed } }] }];
                            cx.notify();
                        });
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.descriptions.open = None;
                            sidebar.configure_checkout_group_capture(&path, &frozen_git, &summary, state);
                            cx.notify();
                        });
                        window.refresh();
                    }).unwrap();
                    cx.update(gpui::App::refresh_windows);
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    cx.update_window(window.into(), |_, window, cx| {
                        window.set_rem_size(gpui::px(rem));
                        window.refresh();
                        let sidebar = sidebar.read(cx);
                        let mut metadata = fs_err::OpenOptions::new().create(true).append(true).open(output.join("group-measurements.txt")).unwrap();
                        if matches!(state, "pull-error" | "pull-error-narrow") {
                            if let Some(tree) = window.debug_a11y_tree_json() {
                            let serialized = serde_json::to_string(&tree).unwrap();
                            fs_err::write(output.join(format!("groups-{}-{state}-a11y.json", provider.label().to_lowercase())), &serialized).unwrap();
                            assert_eq!(serialized.matches("Couldn’t pull: Branches have diverged").count(), 1, "one failure in actual Sidebar");
                            assert_eq!(serialized.matches("Resolve the branch divergence before pulling again").count(), 1, "one recovery explanation in actual Sidebar");
                            } else {
                                writeln!(metadata, "Accessibility uniqueness check unavailable: no active accessibility tree").unwrap();
                            }
                        }
                        writeln!(metadata, "{} {state}: viewport {:?}, rem {:?}, theme {}; fixture {}", provider.label(), window.viewport_size(), window.rem_size(), grove_core::theme::current().name, sidebar.checkout_group_capture_state()).unwrap();
                    }).unwrap();
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    assert!(std::process::Command::new("import").args(["-window", TITLE]).arg(output.join(format!("groups-{}-{state}.png", provider.label().to_lowercase()))).status().unwrap().success());
                }
            }
            drive_frames.set(false);
            cx.background_executor().timer(Duration::from_millis(100)).await;
            cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
