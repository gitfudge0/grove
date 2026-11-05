//! Native production-reader fixtures. They use frozen source data and never
//! start a provider or perform a GitHub write.
use super::*;
use gpui::AppContext as _;
use std::io::Write as _;

const FIXTURE_PATH: &str = "/grove-review-fixture";
const FIXTURE_FILE: &str = "src/webhooks/handler.ts";

struct ReviewReaderFixture {
    sidebar: Entity<Sidebar>,
    source: FocusHandle,
    rem: f32,
    width: f32,
    height: f32,
    measured: std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}
impl Render for ReviewReaderFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(gpui::px(self.rem));
        let reader = self
            .sidebar
            .update(cx, |sidebar, cx| sidebar.render_review(window, cx));
        let sidebar = self.sidebar.clone();
        let measured = self.measured.clone();
        div()
            .debug_selector(|| "review-fixture-viewport".into())
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
                        sidebar.update(cx, |sidebar, cx| sidebar.close_review(window, cx));
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
fn frozen_review() -> Review {
    Review {
        snapshot: grove_core::change_description::Snapshot {
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
                grove_core::change_description::SnapshotFile {
                    path: FIXTURE_FILE.into(),
                    added: 1,
                    removed: 0,
                    patch: "diff --git a/src/webhooks/handler.ts b/src/webhooks/handler.ts\n@@ -1,3 +1,4 @@\n export async function deliver(event) {\n   await createOrder(event.data);\n+  await createOrder(event.data);\n }\n".into(),
                    omitted: None,
                },
                grove_core::change_description::SnapshotFile {
                    path: "tests/webhooks.test.ts".into(),
                    added: 12,
                    removed: 0,
                    patch: "@@ -0,0 +1 @@\n+it('ignores duplicate deliveries', async () => {});\n".into(),
                    omitted: None,
                },
            ],
        },
        summary: "The snapshot has one correctness concern in delivery handling.".into(),
        reviewed_files: vec![FIXTURE_FILE.into(), "tests/webhooks.test.ts".into()],
        findings: vec![domain::Finding { priority: 1, title: "Avoid duplicate orders on every delivery".into(), body: "The added second createOrder call creates two orders for each delivery.".into(), path: FIXTURE_FILE.into(), side: "new".into(), line_start: 3, line_end: 3, suggestion: "Remove the second createOrder call.".into() }],
        files: vec![
            grove_core::change_description::FileDescription {
                path: FIXTURE_FILE.into(),
                title: "One finding in this captured patch".into(),
                summary: vec!["### P1 · Avoid duplicate orders on every delivery\n\n`src/webhooks/handler.ts:3` (new side)\n\nThe added second createOrder call creates two orders for each delivery.\n\n**Next step:** Remove the second createOrder call.".into()],
                category: "P1 findings".into(),
            },
            grove_core::change_description::FileDescription {
                path: "tests/webhooks.test.ts".into(),
                title: "No findings in this captured patch".into(),
                summary: vec!["No actionable defects identified in the supplied patch. Tests were not executed.".into()],
                category: "Reviewed without findings".into(),
            },
        ],
    }
}
fn create_sidebar(window: &mut Window, cx: &mut App) -> (Entity<Sidebar>, FocusHandle) {
    let runtime = cx.new(crate::runtime::Runtime::new);
    let mut source = None;
    let sidebar = cx.new(|cx| {
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.initial_selection_pending = false;
        sidebar.selection = Some(Selection::Worktree(0, FIXTURE_PATH.into()));
        let review = Arc::new(frozen_review());
        let key = key(&sidebar.selection, FIXTURE_PATH, &review.snapshot.scope);
        let text = cx.new(|cx| TextViewState::markdown(&review.summary_markdown(), cx));
        let file_text = review
            .files
            .iter()
            .map(|file| {
                let text = format!("**{}**\n\n{}", file.title, file.summary.join("\n\n"));
                (
                    file.path.clone(),
                    cx.new(|cx| TextViewState::markdown(&text, cx)),
                )
            })
            .collect();
        let return_focus = cx.focus_handle();
        source = Some(return_focus.clone());
        sidebar.reviews.entries.insert(
            key.clone(),
            Entry {
                scope: Some(review.snapshot.scope.clone()),
                result: Some(review),
                freshness: Some(Ok(true)),
                // Frozen snapshots never launch a subprocess, even on a slow renderer.
                checking: true,
                checked_at: Some(Instant::now()),
                text: Some(text),
                file_text,
                ..Default::default()
            },
        );
        sidebar.reviews.open = Some(key);
        sidebar.active_change_reader = Some("review");
        sidebar.reviews.return_focus = Some(return_focus);
        sidebar.reviews.focus_pending = true;
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
        let fixture = cx.new(|_| ReviewReaderFixture {
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
    assert!(cx.debug_bounds("review-reader").is_some());
    let file = cx
        .debug_bounds("review-file-src/webhooks/handler.ts")
        .unwrap();
    cx.simulate_click(file.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.reviews.entries.values().next().unwrap();
        assert!(entry.expanded.contains(FIXTURE_FILE));
    });
    let diff = cx
        .debug_bounds("review-diff-src/webhooks/handler.ts")
        .unwrap();
    cx.simulate_click(diff.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.reviews.entries.values().next().unwrap();
        assert_eq!(entry.diff.as_ref().unwrap().0, FIXTURE_FILE);
        assert_eq!(entry.result.as_ref().unwrap().snapshot.head, "fixture-head");
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    sidebar.read_with(cx, |sidebar, _| {
        let entry = sidebar.reviews.entries.values().next().unwrap();
        assert!(entry.diff.is_none());
        assert!(entry.expanded.contains(FIXTURE_FILE));
    });
}

#[gpui::test]
fn hidden_checkout_skill_uses_selected_worktree_without_a_mounted_panel(
    cx: &mut gpui::TestAppContext,
) {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path().to_string_lossy().into_owned();
    assert!(std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repository.path())
        .output()
        .unwrap()
        .status
        .success());
    cx.update(|cx| {
        initialize(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Review skill fixture".into(),
                path: path.clone(),
                scripts: Default::default(),
                archived: false,
                worktree_dir: None,
            }],
            default_agent: Some(Agent::Codex),
            ..Default::default()
        }));
    });
    let mut sidebar_handle = None;
    let mut source_handle = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
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
        let source = cx.focus_handle();
        source_handle = Some(source.clone());
        let sidebar = cx.new(|cx| {
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.initial_selection_pending = false;
            sidebar.selection = Some(Selection::Worktree(0, path.clone()));
            sidebar.snapshot = sidebar
                .runtime
                .update(cx, |runtime, cx| runtime.snapshot(cx));
            // No Checkout rendering occurs in this fixture. Its cached path and
            // Git/PR state remain uninitialized, reproducing the hidden-panel bug.
            sidebar.checkout_panel.hidden = true;
            let local = key(&sidebar.selection, &path, &Scope::Local);
            sidebar.reviews.entries.insert(
                local,
                Entry {
                    scope: Some(Scope::Local),
                    // Join a preseeded run so the test never starts a model process.
                    cancel: Some(Arc::new(CommitControl::default())),
                    ..Default::default()
                },
            );
            sidebar
        });
        sidebar_handle = Some(sidebar.clone());
        let fixture = cx.new(|_| ReviewReaderFixture {
            sidebar,
            source,
            rem: 16.,
            width: 1100.,
            height: 1000.,
            measured: Default::default(),
        });
        gpui_component::Root::new(fixture, window, cx).bordered(false)
    });
    let sidebar = sidebar_handle.unwrap();
    let source = source_handle.unwrap();
    for preference in [ScopePreference::Local, ScopePreference::Current] {
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.reviews.open = None;
                sidebar.reviews.scopes.clear();
                sidebar.files_active = true;
                sidebar.checkout_panel.side_chat_owner = sidebar.selection.clone();
                source.focus(window, cx);
                sidebar.invoke_review_skill(preference, source.clone(), cx);
                let expected = key(&sidebar.selection, &path, &Scope::Local);
                assert_eq!(sidebar.reviews.open.as_ref(), Some(&expected));
                assert!(sidebar.review_is_open(cx));
                assert!(!sidebar.files_active);
                assert!(sidebar.checkout_panel.side_chat_owner.is_none());
                assert_eq!(sidebar.reviews.return_focus.as_ref(), Some(&source));
                sidebar.toggle_files(window, cx);
                assert!(sidebar.files_active);
                assert_eq!(sidebar.active_change_reader, None);
                assert!(!sidebar.review_is_open(cx));
                // Cached review stays intact but navigation owns the canvas.
                assert!(sidebar.reviews.open.is_some());
                sidebar.invoke_review_skill(preference, source.clone(), cx);
                assert!(sidebar.review_is_open(cx));
            })
        });
        draw(cx);
        assert!(cx.debug_bounds("review-reader").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).reviews.open.is_none());
            assert!(source.is_focused(window));
        });
    }
}

#[gpui::test]
fn stale_review_remains_readable_and_source_focus_returns(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    let source = sidebar.read_with(cx, |s, _| s.reviews.return_focus.clone().unwrap());
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            s.reviews.entries.values_mut().next().unwrap().freshness = Some(Ok(false));
            cx.notify();
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("review-copy").is_some());
    assert!(cx.debug_bounds("review-regenerate").is_some());
    assert!(cx.debug_bounds("review-publish").is_none());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(sidebar.read(cx).reviews.open.is_none());
        assert!(source.is_focused(window));
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated Linux native fixture window; run under Xvfb"]
fn review_changes_linux_native_capture() {
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            initialize(cx);
            crate::fonts::register(cx).unwrap();
            let mut sidebar_handle = None;
            let mut fixture_handle = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(20.), gpui::px(20.)),
                            gpui::size(gpui::px(1100.), gpui::px(1000.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove Review changes QA");
                        let (sidebar, source) = create_sidebar(window, cx);
                        sidebar_handle = Some(sidebar.clone());
                        let fixture = cx.new(|_| ReviewReaderFixture {
                            sidebar,
                            source,
                            rem: 16.,
                            width: 1100.,
                            height: 800.,
                            measured: Default::default(),
                        });
                        fixture_handle = Some(fixture.clone());
                        cx.new(|cx| gpui_component::Root::new(fixture, window, cx))
                    },
                )
                .unwrap();
            let sidebar = sidebar_handle.unwrap();
            let fixture = fixture_handle.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (name, theme, rem, width, state) in [
                    ("result-dark", "grove-dark", 16., 1100., "result"),
                    ("finding-dark", "grove-dark", 16., 1100., "expanded"),
                    ("diff-light-zoom", "grove-light", 20., 900., "diff"),
                    ("stale-narrow", "grove-dark", 16., 320., "stale"),
                    ("failure-dark", "grove-dark", 16., 1100., "error"),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        fixture.update(cx, |f, cx| {
                            f.rem = rem;
                            f.width = width;
                            cx.notify();
                        });
                    });
                    cx.update_window(window.into(), |_, window, cx| {
                        window.resize(gpui::size(gpui::px(width), gpui::px(1000.)));
                        sidebar.update(cx, |s, cx| {
                            let entry = s.reviews.entries.values_mut().next().unwrap();
                            entry.freshness = Some(Ok(state != "stale"));
                            entry.expanded.clear();
                            entry.diff = None;
                            entry.error = None;
                            entry
                                .scroll
                                .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                            if state == "expanded" {
                                entry.expanded.insert(FIXTURE_FILE.into());
                            }
                            if state == "diff" {
                                entry.diff = Some((
                                    FIXTURE_FILE.into(),
                                    cx.new(|cx| {
                                        TextViewState::markdown(
                                            &format!(
                                                "```diff\n{}\n```",
                                                entry.result.as_ref().unwrap().snapshot.files[0]
                                                    .patch
                                            ),
                                            cx,
                                        )
                                    }),
                                ));
                            }
                            if state == "error" {
                                entry.error = Some(
                                    "Review model unavailable. Previous snapshot retained.".into(),
                                );
                            }
                            cx.notify();
                        });
                        fixture.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(Duration::from_millis(800))
                        .await;
                    let output = std::path::PathBuf::from(
                        std::env::var("GROVE_REVIEW_CAPTURE_DIR").unwrap(),
                    )
                    .join(format!("{name}.png"));
                    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
                    let measured = cx.update(|cx| fixture.read(cx).measured.get());
                    let mut meta = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(output.parent().unwrap().join("metadata.txt"))
                        .unwrap();
                    writeln!(meta, "{name}: requested {width}; measured {measured:?}").unwrap();
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove Review changes QA"])
                        .arg(output)
                        .status()
                        .unwrap()
                        .success());
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
