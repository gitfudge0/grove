//! Native production-reader fixtures. They use frozen source data and never
//! start a provider or perform a GitHub write.
use super::*;
use gpui::AppContext as _;
use std::io::Write as _;

const FIXTURE_PATH: &str = "/grove-description-fixture";
const FIXTURE_FILE: &str = "src/webhooks/handler.ts";

struct DescriptionReaderFixture {
    sidebar: Entity<Sidebar>,
    source: FocusHandle,
    rem: f32,
    width: f32,
    height: f32,
    measured: std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}
impl Render for DescriptionReaderFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(gpui::px(self.rem));
        let reader = self.sidebar.update(cx, |sidebar, cx| {
            if sidebar
                .descriptions
                .open
                .as_ref()
                .is_some_and(|target| target.owner == owner(&sidebar.selection))
            {
                sidebar.render_description(window, cx)
            } else {
                let controls =
                    sidebar.render_description_controls(FIXTURE_PATH, false, None, window, cx);
                div()
                    .flex()
                    .size_full()
                    .child(
                        div()
                            .debug_selector(|| "description-fixture-conversation".into())
                            .flex_1()
                            .child("Conversation"),
                    )
                    .child(div().w(rpx(280.)).child(controls))
                    .into_any_element()
            }
        });
        let sidebar = self.sidebar.clone();
        let measured = self.measured.clone();
        div()
            .debug_selector(|| "description-fixture-viewport".into())
            .relative()
            .w(gpui::px(self.width))
            .h(gpui::px(self.height))
            .max_w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .bg(c::BG())
            .text_color(c::FG())
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured.set(bounds.size),
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                action(
                    "fixture-source",
                    "Conversation source",
                    "arrow-left",
                    window,
                    cx,
                )
                .with_focus_handle(self.source.clone()),
            )
            .child(div().flex_1().min_h_0().child(reader))
            .on_key_down(
                cx.listener(move |_, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        sidebar.update(cx, |sidebar, cx| sidebar.close_description(window, cx));
                        cx.stop_propagation();
                    }
                }),
            )
    }
}
fn initialize(cx: &mut App) {
    gpui_component::init(cx);
    cx.set_reduce_motion(true);
    cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(crate::zoom::ZoomState::new(1.0));
    c::sync_component_theme(cx);
}
fn frozen_description() -> Description {
    Description {
        snapshot: domain::Snapshot {
            scope: Scope::PullRequest {
                url: "https://github.com/example/northstar/pull/142".into(),
            },
            fingerprint: "fixture-a81c3d2".into(),
            label: "PR #142 · northstar · webhook-dedup".into(),
            head: "fixture-head".into(),
            base: "fixture-base".into(),
            original_body: Some("Fix duplicate orders.\nCloses #138.".into()),
            title: "Prevent duplicate webhook orders".into(),
            checkout: FIXTURE_PATH.into(),
            files: vec![
                domain::SnapshotFile {
                    path: FIXTURE_FILE.into(),
                    added: 8,
                    removed: 1,
                    patch: "diff --git a/src/webhooks/handler.ts b/src/webhooks/handler.ts\n@@ -1,2 +1,3 @@\n export async function deliver(event) {\n+  if (!await claim(event.id)) return;\n   await createOrder(event.data);\n".into(),
                    omitted: None,
                },
                domain::SnapshotFile {
                    path: "tests/webhooks.test.ts".into(),
                    added: 12,
                    removed: 0,
                    patch: "@@ -0,0 +1 @@\n+it('ignores duplicate deliveries', async () => {});\n".into(),
                    omitted: None,
                },
            ],
        },
        title: "Prevent duplicate webhook orders".into(),
        types: vec!["Bug fix".into(), "Tests".into()],
        summary: vec![
            "Claim webhook delivery IDs before creating orders.".into(),
            "Cover duplicate deliveries in the webhook tests.".into(),
        ],
        files: vec![
            domain::FileDescription {
                path: FIXTURE_FILE.into(),
                title: "Skip webhook deliveries already processed".into(),
                summary: vec!["Acknowledge duplicates without creating a second order.".into()],
                category: "Behavior".into(),
            },
            domain::FileDescription {
                path: "tests/webhooks.test.ts".into(),
                title: "Cover repeated webhook delivery IDs".into(),
                summary: vec!["Verify repeated deliveries create only one order.".into()],
                category: "Tests".into(),
            },
        ],
        diagram: None,
    }
}
fn create_sidebar(window: &mut Window, cx: &mut App) -> (Entity<Sidebar>, FocusHandle) {
    let runtime = cx.new(crate::runtime::Runtime::new);
    let mut source = None;
    let sidebar = cx.new(|cx| {
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.initial_selection_pending = false;
        sidebar.selection = Some(Selection::Worktree(0, FIXTURE_PATH.into()));
        let description = Arc::new(frozen_description());
        let key = key(&sidebar.selection, FIXTURE_PATH, &description.snapshot.scope);
        let text = cx.new(|cx| TextViewState::markdown("# Prevent duplicate webhook orders\n\nBug fix · Tests\n\n## Description\n\n- Claim webhook delivery IDs before creating orders.\n- Cover duplicate deliveries in the webhook tests.", cx));
        let file_text = description.files.iter().map(|file| {
            let text = format!("**{}**\n\n- {}", file.title, file.summary[0]);
            (file.path.clone(), cx.new(|cx| TextViewState::markdown(&text, cx)))
        }).collect();
        let return_focus = cx.focus_handle();
        source = Some(return_focus.clone());
        sidebar.descriptions.entries.insert(key.clone(), Entry {
            scope: Some(description.snapshot.scope.clone()),
            result: Some(description),
            freshness: Some(Ok(true)),
            // Frozen snapshots never launch a subprocess, even on a slow renderer.
            checking: true,
            checked_at: Some(Instant::now()),
            text: Some(text),
            file_text,
            ..Default::default()
        });
        sidebar.descriptions.open = Some(key);
        sidebar.descriptions.return_focus = Some(return_focus);
        sidebar.descriptions.focus_pending = true;
        sidebar
    });
    (sidebar, source.unwrap())
}
fn fixture(cx: &mut gpui::TestAppContext) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
    cx.update(initialize);
    let mut sidebar = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let (view, source) = create_sidebar(window, cx);
        sidebar = Some(view.clone());
        let fixture = cx.new(|_| DescriptionReaderFixture {
            sidebar: view,
            source,
            rem: 16.,
            width: 1100.,
            height: 1000.,
            measured: Default::default(),
        });
        gpui_component::Root::new(fixture, window, cx).bordered(false)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1000.)));
    draw(cx);
    (sidebar.unwrap(), cx)
}
fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn reader_expands_files_and_opens_the_captured_diff(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    assert!(cx.debug_bounds("description-reader").is_some());
    let file = cx
        .debug_bounds("describe-file-src/webhooks/handler.ts")
        .unwrap();
    cx.simulate_click(file.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.descriptions.entries.values().next().unwrap();
        assert!(entry.expanded.contains(FIXTURE_FILE));
    });
    let diff = cx
        .debug_bounds("describe-diff-src/webhooks/handler.ts")
        .unwrap();
    cx.simulate_click(diff.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.descriptions.entries.values().next().unwrap();
        assert_eq!(entry.diff.as_ref().unwrap().0, FIXTURE_FILE);
        assert_eq!(entry.result.as_ref().unwrap().snapshot.head, "fixture-head");
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.descriptions.entries.values().next().unwrap();
        assert!(entry.diff.is_none());
        assert!(entry.expanded.contains(FIXTURE_FILE));
    });
}

#[gpui::test]
fn stale_reader_blocks_publish_and_preview_restores_focus(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .descriptions
                .entries
                .values_mut()
                .next()
                .unwrap()
                .freshness = Some(Ok(false));
            cx.notify();
        })
    });
    draw(cx);
    let publish = cx.debug_bounds("describe-publish").unwrap();
    cx.simulate_click(publish.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        assert!(sidebar
            .descriptions
            .entries
            .values()
            .next()
            .unwrap()
            .preview
            .is_none());
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .descriptions
                .entries
                .values_mut()
                .next()
                .unwrap()
                .freshness = Some(Ok(true));
            cx.notify();
        })
    });
    draw(cx);
    let publish = cx.debug_bounds("describe-publish").unwrap();
    cx.simulate_click(publish.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        let input = sidebar
            .descriptions
            .entries
            .values()
            .next()
            .unwrap()
            .preview
            .as_ref()
            .unwrap();
        assert!(input.focus_handle(cx).is_focused(window));
        assert!(input.read(cx).value().contains("Closes #138."));
    });
    let well = cx.debug_bounds("description-preview-well").unwrap();
    let input = cx.debug_bounds("description-preview-input").unwrap();
    assert!(
        input.origin.y >= well.origin.y,
        "multiline input escaped above its owning well"
    );
    assert!(
        input.origin.y + input.size.height <= well.origin.y + well.size.height,
        "multiline input escaped below its owning well"
    );
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .descriptions
                .entries
                .values()
                .next()
                .unwrap()
                .preview_scroll
                .scroll_to_bottom();
            cx.notify();
        })
    });
    draw(cx);
    let confirm = cx.debug_bounds("description-publish-confirm").unwrap();
    let well = cx.debug_bounds("description-preview-well").unwrap();
    assert!(
        confirm.origin.y >= well.origin.y + well.size.height,
        "publish action overlaps the multiline input"
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        assert!(sidebar
            .descriptions
            .entries
            .values()
            .next()
            .unwrap()
            .preview
            .is_none());
        assert!(sidebar
            .descriptions
            .focus
            .as_ref()
            .unwrap()
            .is_focused(window));
    });
    let source = sidebar.read_with(cx, |sidebar, _| {
        sidebar.descriptions.return_focus.clone().unwrap()
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = sidebar.read(cx);
        assert!(sidebar.descriptions.open.is_none());
        // The reader must return ownership to the exact source focus handle.
        assert!(sidebar.descriptions.return_focus.is_none());
        assert!(source.is_focused(window));
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated Linux native fixture window; run under Xvfb with a fresh GROVE_CONFIG_DIR"]
fn describe_changes_linux_native_capture() {
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            initialize(cx);
            crate::fonts::register(cx).unwrap();
            let mut sidebar_handle = None;
            let mut fixture_handle = None;
            let window = cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                        gpui::point(gpui::px(20.), gpui::px(20.)),
                        gpui::size(gpui::px(1100.), gpui::px(1000.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    window.set_window_title("Grove Describe changes QA");
                    let (sidebar, source) = create_sidebar(window, cx);
                    sidebar_handle = Some(sidebar.clone());
                    let fixture = cx.new(|_| DescriptionReaderFixture { sidebar, source, rem: 16., width: 1100., height: 800., measured: Default::default() });
                    fixture_handle = Some(fixture.clone());
                    cx.new(|cx| gpui_component::Root::new(fixture, window, cx))
                },
            ).unwrap();
            let sidebar = sidebar_handle.unwrap();
            let fixture = fixture_handle.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (name, theme, rem, width, state) in [
                    ("result-dark", "grove-dark", 16., 1100., "result"),
                    ("expanded-dark", "grove-dark", 16., 1100., "expanded"),
                    ("diff-dark", "grove-dark", 16., 1100., "diff"),
                    ("diagram-dark", "grove-dark", 16., 1100., "diagram"),
                    ("preview-light-zoom", "grove-light", 20., 900., "preview"),
                    ("stale-narrow", "grove-dark", 16., 320., "stale"),
                    ("failure-dark", "grove-dark", 16., 1100., "error"),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        fixture.update(cx, |fixture, cx| { fixture.rem = rem; fixture.width = width; fixture.height = 800.; cx.notify(); });
                    });
                    cx.update_window(window.into(), |_, window, cx| {
                        window.resize(gpui::size(gpui::px(width), gpui::px(1000.)));
                        sidebar.update(cx, |sidebar, cx| {
                            let key = sidebar.descriptions.open.clone().unwrap();
                            let entry = sidebar.descriptions.entries.get_mut(&key).unwrap();
                            entry.checked_at = Some(Instant::now());
                            entry.freshness = Some(Ok(state != "stale"));
                            entry.expanded.clear();
                            entry.preview = None;
                            entry.diff = None;
                            entry.error = None;
                            entry.diagram = None;
                            entry.scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                            if state == "diagram" { entry.diagram = Some(cx.new(|cx| TextViewState::markdown("```mermaid\nflowchart LR\n A[\"Delivery\"] --> B[\"Claim ID\"]\n B --> C[\"Create order\"]\n```", cx))); }
                            if state == "expanded" { entry.expanded.insert(FIXTURE_FILE.into()); }
                            if state == "diff" {
                                let patch = &entry.result.as_ref().unwrap().snapshot.files[0].patch;
                                entry.diff = Some((FIXTURE_FILE.into(), cx.new(|cx| TextViewState::markdown(&format!("```diff\n{patch}\n```"), cx))));
                            }
                            if state == "error" { entry.error = Some("The provider disconnected. Your previous description is preserved.".into()); }
                            if state == "preview" { sidebar.open_description_preview(&key, window, cx); }
                            cx.notify();
                        });
                        fixture.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    }).unwrap();
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join(".grove-artifacts/describe-changes-native")
                        .join(format!("{name}-v3.png"));
                    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
                    let measured = cx.update(|cx| fixture.read(cx).measured.get());
                    assert!(measured.width > gpui::px(0.));
                    if state == "stale" { assert!(measured.width <= gpui::px(320.)); }
                    let mut metadata = std::fs::OpenOptions::new().create(true).append(true).open(output.parent().unwrap().join("capture-metadata-v3.txt")).unwrap();
                    writeln!(metadata, "{name}: requested {width}x800 logical px; measured {measured:?}").unwrap();
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove Describe changes QA"])
                        .arg(output)
                        .status().unwrap().success());
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
}

struct LinkedFixture {
    sidebar: Entity<Sidebar>,
    session: Entity<crate::entities::codex_session::CodexSession>,
    id: SessionId,
    actions: std::sync::mpsc::Receiver<grove_core::codex::Action>,
    _updates: std::sync::mpsc::Sender<grove_core::codex::Snapshot>,
    _store: tempfile::TempDir,
}
fn linked_fixture(
    cx: &mut gpui::TestAppContext,
    provider: grove_core::codex::Provider,
) -> (LinkedFixture, &mut gpui::VisualTestContext) {
    cx.update(initialize);
    let store_root = tempfile::tempdir().unwrap();
    let store = grove_core::change_description_store::Store::new(store_root.path().into());
    let description = frozen_description();
    store.save(&description).unwrap();
    let session_key = format!("description-navigation-{provider:?}");
    store
        .bind(
            &session_key,
            &grove_core::change_description_store::Source::from(&description),
        )
        .unwrap();
    let (handle, actions) = grove_core::codex::Handle::channel();
    let (updates, receive) = std::sync::mpsc::channel();
    let mut retained = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let (sidebar, source) = create_sidebar(window, cx);
        let mut saved = grove_core::codex::SavedSession::empty(
            session_key.clone(),
            provider,
            String::new(),
            FIXTURE_PATH.into(),
        );
        saved.draft = "Keep this follow-up draft while reading changes".into();
        saved.draft_images = vec!["retained-image-draft".into()];
        let mut snapshot = grove_core::codex::Snapshot::new(saved);
        snapshot.status = grove_core::codex::Status::Idle;
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                snapshot,
                grove_core::codex::Connection {
                    handle,
                    updates: receive.into(),
                },
                cx,
            )
        });
        let runtime = sidebar.read(cx).runtime.clone();
        let id = runtime.update(cx, |runtime, cx| {
            runtime.adopt_dashboard_session(session.clone(), cx)
        });
        sidebar.update(cx, |sidebar, cx| {
            let old = sidebar.descriptions.open.take().unwrap();
            let mut entry = sidebar.descriptions.entries.remove(&old).unwrap();
            sidebar.dashboard_open = false;
            sidebar.selection = Some(Selection::Session(id));
            sidebar.checkout_panel.chosen_root = Some(FIXTURE_PATH.into());
            sidebar.snapshot = runtime.update(cx, |runtime, cx| runtime.snapshot(cx));
            let target = key(
                &sidebar.selection,
                FIXTURE_PATH,
                &description.snapshot.scope,
            );
            entry.session = Some(id);
            entry.session_key = Some(session_key.clone());
            sidebar.descriptions.entries.insert(target.clone(), entry);
            sidebar.descriptions.linked.insert(id, target);
            sidebar.descriptions.hydrated.insert((id, session_key));
            sidebar.descriptions.store_root = Some(store_root.path().into());
            cx.notify();
        });
        retained = Some((sidebar.clone(), session, id));
        let fixture = cx.new(|_| DescriptionReaderFixture {
            sidebar,
            source,
            rem: 16.,
            width: 1100.,
            height: 1000.,
            measured: Default::default(),
        });
        gpui_component::Root::new(fixture, window, cx).bordered(false)
    });
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1000.)));
    draw(cx);
    let (sidebar, session, id) = retained.unwrap();
    (
        LinkedFixture {
            sidebar,
            session,
            id,
            actions,
            _updates: updates,
            _store: store_root,
        },
        cx,
    )
}
fn assert_linked_draft(fixture: &LinkedFixture, cx: &gpui::VisualTestContext) {
    fixture.session.read_with(cx, |session, _| {
        assert_eq!(
            session.state.saved.draft,
            "Keep this follow-up draft while reading changes"
        );
        assert_eq!(session.state.saved.draft_images, ["retained-image-draft"]);
    });
}

#[gpui::test]
fn linked_session_panel_reopens_description_without_local_changes_or_attached_pr(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let (fixture, cx) = linked_fixture(cx, provider);
        assert!(cx
            .debug_bounds("description-fixture-conversation")
            .is_some());
        assert!(cx.debug_bounds("description-reader").is_none());
        let open = cx.debug_bounds("describe-open").expect(
            "linked result must remain in the side panel with no live changes or attached PR",
        );
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("description-reader").is_some());
        fixture.sidebar.read_with(cx, |sidebar, cx| {
            assert!(sidebar.description_is_open(cx));
            assert_eq!(sidebar.selection, Some(Selection::Session(fixture.id)));
        });
        assert_linked_draft(&fixture, cx);
        let conversation = cx.debug_bounds("describe-back").unwrap();
        cx.simulate_click(conversation.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx
            .debug_bounds("description-fixture-conversation")
            .is_some());
        assert!(cx.debug_bounds("description-reader").is_none());
        assert_linked_draft(&fixture, cx);
        let open = cx.debug_bounds("describe-open").unwrap();
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("description-reader").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx
            .debug_bounds("description-fixture-conversation")
            .is_some());
        assert!(cx.debug_bounds("describe-open").is_some());
        cx.update(|window, cx| {
            let sidebar = fixture.sidebar.read(cx);
            assert!(sidebar.descriptions.open.is_none());
            assert!(sidebar
                .descriptions
                .open_source
                .as_ref()
                .unwrap()
                .is_focused(window));
        });
        assert_linked_draft(&fixture, cx);
        assert!(
            fixture.actions.try_iter().all(|action| !matches!(
                action,
                grove_core::codex::Action::Prompt(_)
                    | grove_core::codex::Action::Submit { .. }
                    | grove_core::codex::Action::SubmitWithMode { .. }
            )),
            "opening a saved description must not regenerate or submit another prompt"
        );
    }
}

#[gpui::test]
fn stale_linked_description_keeps_panel_action_and_never_leaks_to_another_session(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let (fixture, cx) = linked_fixture(cx, provider);
        cx.update(|_, cx| {
            fixture.sidebar.update(cx, |sidebar, cx| {
                let target = sidebar
                    .descriptions
                    .linked
                    .get(&fixture.id)
                    .unwrap()
                    .clone();
                sidebar
                    .descriptions
                    .entries
                    .get_mut(&target)
                    .unwrap()
                    .freshness = Some(Ok(false));
                cx.notify();
            })
        });
        draw(cx);
        assert!(
            cx.debug_bounds("status-mark-describe-panel-state")
                .is_some(),
            "the saved source needs a visible stale-state mark"
        );
        let open = cx.debug_bounds("describe-open").unwrap();
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("status-mark-description-stale").is_some());
        let publish = cx.debug_bounds("describe-publish").unwrap();
        cx.simulate_click(publish.center(), gpui::Modifiers::default());
        draw(cx);
        fixture.sidebar.read_with(cx, |sidebar, _| {
            let target = sidebar.descriptions.linked.get(&fixture.id).unwrap();
            assert!(
                sidebar
                    .descriptions
                    .entries
                    .get(target)
                    .unwrap()
                    .preview
                    .is_none(),
                "a stale description must not open a publishing preview"
            );
        });
        let other = cx.update(|_, cx| {
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession::empty(
                format!("unrelated-{provider:?}"),
                provider,
                String::new(),
                FIXTURE_PATH.into(),
            );
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    grove_core::codex::Snapshot::new(saved),
                    grove_core::codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let runtime = fixture.sidebar.read(cx).runtime.clone();
            let id = runtime.update(cx, |runtime, cx| {
                runtime.adopt_dashboard_session(session, cx)
            });
            fixture.sidebar.update(cx, |sidebar, cx| {
                sidebar.selection = Some(Selection::Session(id));
                sidebar
                    .descriptions
                    .hydrated
                    .insert((id, format!("unrelated-{provider:?}")));
                assert!(!sidebar.description_is_open(cx));
                cx.notify();
            });
            id
        });
        draw(cx);
        assert_ne!(other, fixture.id);
        assert!(cx.debug_bounds("description-reader").is_none());
        assert!(
            cx.debug_bounds("describe-open").is_none(),
            "an unrelated session must not borrow another session's description action"
        );
        cx.update(|_, cx| {
            fixture.sidebar.update(cx, |sidebar, cx| {
                sidebar.selection = Some(Selection::Session(fixture.id));
                assert!(sidebar.description_is_open(cx));
                cx.notify();
            })
        });
        draw(cx);
        assert!(cx.debug_bounds("description-reader").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("describe-open").is_some());
        assert!(cx
            .debug_bounds("status-mark-describe-panel-state")
            .is_some());
        assert_linked_draft(&fixture, cx);
    }
}

#[gpui::test]
fn publish_failure_keeps_edited_preview_and_never_offers_generation_retry(
    cx: &mut gpui::TestAppContext,
) {
    let (sidebar, cx) = fixture(cx);
    let edited = "Edited description for this exact PR.\nCloses #138.";
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let key = sidebar.descriptions.open.clone().unwrap();
            sidebar.open_description_preview(&key, window, cx);
            let entry = sidebar.descriptions.entries.get_mut(&key).unwrap();
            entry.preview.as_ref().unwrap().update(cx, |input, cx| {
                input.set_value(edited, window, cx);
            });
            entry.error =
                Some("The PR description changed before the update reached GitHub.".into());
            cx.notify();
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("description-retry").is_none());
    sidebar.read_with(cx, |sidebar, cx| {
        let entry = sidebar.descriptions.entries.values().next().unwrap();
        assert_eq!(
            entry.preview.as_ref().unwrap().read(cx).value().as_ref(),
            edited
        );
        assert!(entry.result.is_some());
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .descriptions
                .entries
                .values()
                .next()
                .unwrap()
                .preview_scroll
                .scroll_to_bottom();
            cx.notify();
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("description-publish-confirm").is_some());
    assert!(cx.debug_bounds("description-retry").is_none());
    sidebar.read_with(cx, |sidebar, cx| {
        assert_eq!(
            sidebar
                .descriptions
                .entries
                .values()
                .next()
                .unwrap()
                .preview
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .as_ref(),
            edited
        );
    });
}

#[cfg(target_os = "linux")]
#[path = "change_description_shell_tests.rs"]
mod shell;
