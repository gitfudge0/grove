//! Opt-in captures of the actual Sidebar shell with frozen native sessions.
//! Run with RUST_MIN_STACK=16777216, GPUI_X11_SCALE_FACTOR=1 and a fresh
//! GROVE_CONFIG_DIR below .grove-artifacts/description-lifecycle/native/.
use super::*;

const TITLE: &str = "Grove Describe changes full shell QA";

struct NativeShell {
    sidebar: Entity<Sidebar>,
    measured: std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}
impl Render for NativeShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
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
    std::fs::create_dir_all(&output).unwrap();
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
        settings.projects.push(grove_core::storage::Project { name: "northstar".into(), path: path.clone(), scripts: Default::default(), archived: false, worktree_dir: None });
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
            let shell = cx.new(|_| NativeShell { sidebar, measured: measured_window });
            cx.new(|cx| gpui_component::Root::new(shell, window, cx).bordered(false))
        }).unwrap();
        let (sidebar, session) = sidebar_handle.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let _keep_alive = (repository, actions, updates);
            cx.background_executor().timer(Duration::from_millis(900)).await;
            for (name, state) in [("shell-conversation-dark", "conversation"), ("shell-description-dark", "description"), ("shell-back-conversation-dark", "back"), ("shell-stale-panel-dark", "stale"), ("shell-stale-description-dark", "stale-description"), ("shell-narrow-description-dark", "narrow"), ("shell-stale-description-light", "light")] {
                cx.update_window(window.into(), |_, window, cx| {
                    if matches!(state, "description" | "stale-description") {
                        let focus = sidebar.read(cx).descriptions.open_source.as_ref().expect("actual Checkout panel must render linked action").clone();
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
                if matches!(state, "description" | "stale-description") {
                    cx.update_window(window.into(), |_, window, cx| press_key(window, cx, "enter")).unwrap();
                }
                cx.background_executor().timer(Duration::from_millis(420)).await;
                let mut metadata = std::fs::OpenOptions::new().create(true).append(true).open(output.join("shell-measurements.txt")).unwrap();
                cx.update_window(window.into(), |_, window, cx| {
                    let sidebar = sidebar.read(cx);
                    let open = sidebar.descriptions.open.is_some();
                    assert_eq!(open, matches!(state, "description" | "stale-description" | "narrow" | "light"), "native keyboard activation: {state}");
                    assert_eq!(session.read(cx).state.saved.draft, "Can you walk me through the duplicate-delivery guard?");
                    writeln!(metadata, "{name}: viewport {:?}; measured shell {:?}; reading={open}; description scroll bounds {:?}; active theme {} {:?}", window.viewport_size(), measured.get(), sidebar.descriptions.entries.values().next().unwrap().scroll.bounds(), grove_core::theme::current().name, grove_core::theme::current().kind).unwrap();
                    if let Some(tree) = window.debug_a11y_tree_json() { std::fs::write(output.join(format!("{name}-a11y.json")), tree).unwrap(); }
                }).unwrap();
                assert!(std::process::Command::new("import").args(["-window", TITLE]).arg(output.join(format!("{name}.png"))).status().unwrap().success());
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
