use super::*;
struct LinkFixture {
    session: Entity<HtmlSession>,
    text: String,
    width: Option<gpui::Pixels>,
    _subscription: gpui::Subscription,
    opened: Rc<std::cell::RefCell<Vec<PathBuf>>>,
}
impl Render for LinkFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let extensions = self
            .session
            .update(cx, |session, cx| session.extensions("saved-message", cx));
        div()
            .w_full()
            .when_some(self.width, |row, width| row.w(width).max_w_full())
            .bg(c::BG())
            .text_color(c::FG())
            .text_size(rpx(TEXT_BODY))
            .font_family(crate::fonts::UI_FAMILY)
            .child(
                TextView::markdown("html-link-fixture", self.text.clone())
                    .markdown_extensions(extensions)
                    .style(super::super::codex_view::markdown_style(window))
                    .selectable(true),
            )
            .children(render_expanded(window, cx))
    }
}
fn link_fixture(
    cx: &mut gpui::TestAppContext,
) -> (Entity<LinkFixture>, &mut gpui::VisualTestContext) {
    cx.update(gpui_component::init);
    cx.add_window_view(|_, cx| {
        let session =
            cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
        let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = opened.clone();
        let subscription = cx.subscribe(&session, move |_, _, event: &OpenLocalFile, _| {
            observed.borrow_mut().push(event.0.path.clone());
        });
        LinkFixture {
            session,
            text: String::new(),
            width: None,
            _subscription: subscription,
            opened,
        }
    })
}
fn draw_links(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
}

fn attachment_reference() -> grove_core::visuals::Reference {
    use grove_core::visuals::{Presentation, PresentationOptions, Reference};
    Reference {
        scope: "0".repeat(64), publication_id: "1".repeat(64),
        visual_id: "1".repeat(64), revision_id: "1".repeat(64),
        source_digest: "2".repeat(64),
        title: "Project identity — six visual directions, symbols, wordmarks and typography for the next design review".into(),
        item_id: "tool".into(), turn_id: "turn".into(),
        presentation: PresentationOptions { mode: Presentation::Document, width: 900, height: 900, preview_region: None },
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native attachment fixture; requires X11 and import"]
fn publication_attachment_native_capture() {
    use gpui::AppContext as _;
    struct Fixture {
        artifact: Entity<HtmlArtifact>,
        _session: Entity<HtmlSession>,
        width: Rc<Cell<f32>>,
        rem: Rc<Cell<f32>>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(gpui::px(self.rem.get()));
            div()
                .size_full()
                .p(rpx(SPACE_3XL))
                .bg(c::BG())
                .text_color(c::FG())
                .font_family(crate::fonts::UI_FAMILY)
                .text_size(rpx(TEXT_BODY))
                .child(
                    div()
                        .w(gpui::px(self.width.get()))
                        .max_w_full()
                        .child(self.artifact.clone()),
                )
                .children(render_expanded(window, cx))
        }
    }
    assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
    let output = PathBuf::from(std::env::var("GROVE_ARTIFACT_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        let mut retained = None;
        let pane_width = Rc::new(Cell::new(808.));
        let pane_rem = Rc::new(Cell::new(16.));
        let window = cx.open_window(gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                gpui::point(gpui::px(30.), gpui::px(30.)), gpui::size(gpui::px(840.), gpui::px(540.)),
            ))), ..Default::default()
        }, |window, cx| {
            window.set_window_title("Grove artifact attachment QA");
            let session = cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
            let artifact = cx.new(|cx| {
                let mut artifact = HtmlArtifact::new(String::new(), String::new(), None, session.downgrade(), Rc::default(), ScrollHandle::new(), cx);
                artifact.publication = Some(attachment_reference());
                artifact.version.active = "<!doctype html><html><body><h1>Project identity</h1><input value='Retained reader input'></body></html>".into();
                artifact
            });
            retained = Some(artifact.clone());
            let fixture = cx.new(|_| Fixture { artifact, _session: session, width: pane_width.clone(), rem: pane_rem.clone() });
            cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false))
        }).unwrap();
        let artifact = retained.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            for (name, width, light, rem, state) in [
                ("wide-dark",840.,false,16.,"ready"),
                ("narrow-light-zoom",360.,true,20.,"ready"),
                ("loading",840.,false,16.,"loading"),
                ("failed",360.,true,16.,"failed"),
                ("viewer",840.,false,16.,"viewer"),
            ] {
                pane_width.set(width - 32.);
                pane_rem.set(rem);
                cx.update_window(window.into(), |_, window, cx| {
                    grove_core::theme::set_by_name(if light { c::DEFAULT_LIGHT_THEME } else { c::DEFAULT_DARK_THEME });
                    c::sync_component_theme(cx);
                    window.set_rem_size(gpui::px(rem));
                    artifact.update(cx, |artifact, cx| {
                        artifact.loading_file = state == "loading";
                        artifact.publication_failed = state == "failed";
                        artifact.error = (state == "failed").then(|| "Saved artifact is unavailable or corrupt.".into());
                        if state == "viewer" { artifact.toggle_expand(window, cx); }
                        cx.notify();
                    });
                    window.resize(gpui::size(gpui::px(width), gpui::px(540.)));
                    window.refresh();
                }).unwrap();
                cx.background_executor().timer(Duration::from_millis(800)).await;
                assert!(std::process::Command::new("import").args(["-window", "Grove artifact attachment QA"]).arg(output.join(format!("{name}.png"))).status().unwrap().success());
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
            drop(artifact);
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

#[gpui::test]
fn publication_attachment_metadata_for_every_presentation(cx: &mut gpui::TestAppContext) {
    use grove_core::visuals::{Presentation, PresentationOptions, Reference};
    let (fixture, cx) = link_fixture(cx);
    fixture.update(cx, |fixture, cx| {
        fixture.text = "```html\n<html><body>Visual</body></html>\n```".into();
        cx.notify();
    });
    draw_links(cx);
    let artifact = fixture.read_with(cx, |fixture, cx| {
        fixture
            .session
            .read(cx)
            .artifacts
            .values()
            .next()
            .unwrap()
            .clone()
    });
    for mode in [
        Presentation::Inline,
        Presentation::Dashboard,
        Presentation::Canvas,
        Presentation::Document,
    ] {
        for (width, rem) in [(280., 16.), (280., 20.), (900., 16.)] {
            cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
            fixture.update(cx, |fixture, cx| {
                fixture.width = Some(gpui::px(width));
                cx.notify();
            });
            cx.update(|_window, cx| {
                artifact.update(cx, |artifact, cx| {
                    let (width, height) = mode.viewport();
                    artifact.publication = Some(Reference {
                        scope: "0".repeat(64),
                        publication_id: "1".repeat(64),
                        visual_id: "1".repeat(64),
                        revision_id: "1".repeat(64),
                        source_digest: "2".repeat(64),
                        title: "A deliberately long artifact title ".repeat(20),
                        item_id: "tool".into(),
                        turn_id: "turn".into(),
                        presentation: PresentationOptions {
                            mode,
                            width,
                            height,
                            preview_region: None,
                        },
                    });
                    artifact.surface = None;
                    artifact.report_height = 10_000.;
                    cx.notify();
                });
            });
            draw_links(cx);
            let bounds = cx.debug_bounds("html-publication-attachment").unwrap();
            let open = cx.debug_bounds("html-publication-open").unwrap();
            let browser = cx.debug_bounds("html-artifact-browser").unwrap();
            let title = cx.debug_bounds("html-artifact-title").unwrap();
            assert_eq!(open.top(), browser.top());
            assert!(title.right() <= open.left() || title.bottom() <= open.top());
            assert!(open.right() <= browser.left());
            assert!(browser.right() <= bounds.right());
            assert!(f32::from(bounds.size.width) <= width);
            assert!(cx.debug_bounds("html-document-body").is_none());
            assert!(artifact.read_with(cx, |artifact, _| artifact.surface.is_none()));
            assert!(cx.debug_bounds("html-artifact-format").is_some());
            assert!(cx.debug_bounds("html-artifact-full-title").is_none());
            assert!(cx.debug_bounds("html-artifact-details").is_none());
            assert_eq!(
                artifact.read_with(cx, |artifact, _| artifact
                    .publication
                    .as_ref()
                    .unwrap()
                    .presentation
                    .width),
                mode.viewport().0
            );
        }
    }
}

#[gpui::test]
fn publication_attachment_open_and_disabled_states(cx: &mut gpui::TestAppContext) {
    let (fixture, cx) = link_fixture(cx);
    fixture.update(cx, |fixture, cx| {
        fixture.text = "```html\n<html>Retained reader</html>\n```".into();
        cx.notify();
    });
    draw_links(cx);
    let artifact = fixture.read_with(cx, |fixture, cx| {
        fixture
            .session
            .read(cx)
            .artifacts
            .values()
            .next()
            .unwrap()
            .clone()
    });
    artifact.update(cx, |artifact, cx| {
        artifact.publication = Some(attachment_reference());
        artifact.surface = None;
        cx.notify();
    });
    for (loading, failed) in [(true, false), (false, true)] {
        artifact.update(cx, |artifact, cx| {
            artifact.loading_file = loading;
            artifact.publication_failed = failed;
            artifact.error = failed.then(|| "Saved artifact is corrupt".into());
            cx.notify();
        });
        draw_links(cx);
        let open = cx.debug_bounds("html-publication-open").unwrap();
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw_links(cx);
        assert!(cx.debug_bounds("html-expanded-overlay").is_none());
        assert!(artifact.read_with(cx, |a, _| a.surface.is_none()));
    }
    artifact.update(cx, |artifact, cx| {
        artifact.loading_file = false;
        artifact.publication_failed = false;
        artifact.error = None;
        cx.notify();
    });
    draw_links(cx);
    let open = cx.debug_bounds("html-publication-open").unwrap();
    cx.simulate_click(open.center(), gpui::Modifiers::default());
    draw_links(cx);
    assert!(cx.debug_bounds("html-expanded-overlay").is_some());
    cx.simulate_keystrokes("escape");
    draw_links(cx);
    assert!(cx.debug_bounds("html-expanded-overlay").is_none());
    cx.update(|window, cx| {
        assert!(artifact.read(cx).expand_focus.is_focused(window));
    });
    // Button activation requires a full key-down/key-up pair.
    draw_links(cx);
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    draw_links(cx);
    assert!(cx.debug_bounds("html-expanded-overlay").is_some());
    assert!(artifact.read_with(cx, |a, _| a.version.active.contains("Retained reader")));
}

#[gpui::test]
fn expanded_html_escape_uses_its_own_focus_scope(cx: &mut gpui::TestAppContext) {
    let (fixture, cx) = link_fixture(cx);
    fixture.update(cx, |fixture, cx| {
        fixture.text = "```html\n<html><body>Retained source</body></html>\n```".into();
        cx.notify();
    });
    draw_links(cx);
    let artifact = fixture.read_with(cx, |fixture, cx| {
        fixture
            .session
            .read(cx)
            .artifacts
            .values()
            .next()
            .unwrap()
            .clone()
    });
    cx.update(|window, cx| {
        artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
    });
    draw_links(cx);
    draw_links(cx);
    assert!(cx.debug_bounds("html-expanded-overlay").is_some());
    cx.simulate_keystrokes("escape");
    draw_links(cx);
    assert!(cx.debug_bounds("html-expanded-overlay").is_none());
    assert!(artifact.read_with(cx, |artifact, _| artifact
        .version
        .active
        .contains("Retained source")));
}

#[gpui::test]
fn html_transcript_links_keep_viewer_and_browser_actions(cx: &mut gpui::TestAppContext) {
    let (fixture, cx) = link_fixture(cx);
    let screenshot_link = format!(
        "[Open interactive mock](<{}>)",
        std::env::current_dir()
            .unwrap()
            .join(".grove-artifacts/composer-suggestions/index.html")
            .display()
    );
    let cases = [
        (screenshot_link.as_str(), true, true),
        ("[**Preview**](mock.htm)", true, true),
        ("[A longer interactive mock label with several words for narrow transcript layout](mock.html)", true, true),
        ("[A mock with spaces](<mock file.HTML>)", true, true),
        ("[Line](mock.html:12)", true, true),
        ("[Fragment](mock.html#L12)", true, true),
        ("[Open in browser](mock.html)", false, true),
        ("[**OPEN IN BROWSER**](mock.htm)", false, true),
        ("[Source](mock.rs)", true, false),
        ("[External](https://example.com/mock.html)", false, false),
    ];
    for width in [800., 280.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
        for (markdown, viewer, browser) in cases {
            fixture.update(cx, |fixture, cx| {
                fixture.text = markdown.into();
                cx.notify();
            });
            draw_links(cx);
            assert_eq!(
                cx.debug_bounds("transcript-file-link").is_some(),
                viewer,
                "{markdown}"
            );
            assert_eq!(
                cx.debug_bounds("transcript-browser-link").is_some(),
                browser,
                "{markdown}"
            );
            if let Some(bounds) = cx.debug_bounds("transcript-file-link") {
                assert!(
                    bounds.right() <= gpui::px(width + 1.),
                    "viewer overflow: {markdown}: {bounds:?}"
                );
            }
            if let Some(bounds) = cx.debug_bounds("transcript-browser-link") {
                assert!(bounds.size.width > gpui::px(0.));
                assert!(
                    bounds.right() <= gpui::px(width + 1.),
                    "{markdown}: {bounds:?}"
                );
            }
        }
    }
    fixture.update(cx, |fixture, cx| {
        fixture.session =
            cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
        fixture.text = "[First](one.html) and [Second](two.htm)".into();
        cx.notify();
    });
    draw_links(cx);
    fixture.read_with(cx, |fixture, cx| {
        let session = fixture.session.read(cx);
        assert_eq!(
            session.file_focus.len(),
            4,
            "both HTML links have independent viewer/browser controls"
        );
        assert!(
            session.opened.is_empty(),
            "actions do not depend on an auto preview"
        );
    });
    // A fresh session renders the browser action from saved Markdown alone.
    fixture.update(cx, |fixture, cx| {
        fixture.session =
            cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
        fixture.text = "[Restored](mock.html)".into();
        cx.notify();
    });
    draw_links(cx);
    assert!(cx.debug_bounds("transcript-browser-link").is_some());
}

#[gpui::test]
fn html_transcript_link_activation_and_missing_file_feedback(cx: &mut gpui::TestAppContext) {
    let (fixture, cx) = link_fixture(cx);
    let missing = tempfile::tempdir().unwrap().path().join("missing.html");
    fixture.update(cx, |fixture, cx| {
        fixture.text = format!("[Mock]({})", missing.display());
        cx.notify();
    });
    draw_links(cx);
    let viewer = cx.debug_bounds("transcript-file-link").unwrap();
    let browser = cx.debug_bounds("transcript-browser-link").unwrap();
    assert!(viewer.right() <= browser.left() || viewer.bottom() <= browser.top());
    cx.simulate_click(viewer.center(), gpui::Modifiers::default());
    assert_eq!(
        fixture.read_with(cx, |fixture, _| fixture.opened.borrow().clone()),
        vec![missing.clone()]
    );
    // Mouse focuses the independent browser action. Repeated keyboard retries
    // must not invoke the viewer, and missing-file errors remain visible.
    cx.simulate_click(browser.center(), gpui::Modifiers::default());
    draw_links(cx);
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(key);
        draw_links(cx);
        fixture.read_with(cx, |fixture, cx| {
            assert_eq!(fixture.opened.borrow().len(), 1);
            assert!(fixture.session.read(cx).browser_errors[&(
                "saved-message".into(),
                missing.to_string_lossy().into_owned()
            )]
                .contains("File unavailable"));
        });
    }
}
#[gpui::test]
fn html_transcript_browser_launch_errors_are_visible(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("preview.html");
    fs_err::write(&file, "<html>Preview</html>").unwrap();
    let (fixture, cx) = link_fixture(cx);
    fixture.update(cx, |fixture, cx| {
        fixture.text = format!("[Open in browser]({})", file.display());
        cx.notify();
    });
    draw_links(cx);
    assert!(cx.debug_bounds("transcript-file-link").is_none());
    let browser = cx.debug_bounds("transcript-browser-link").unwrap();
    cx.simulate_click(browser.center(), gpui::Modifiers::default());
    draw_links(cx);
    assert!(cx.debug_bounds("transcript-browser-error").is_some());
    fixture.read_with(cx, |fixture, cx| {
        assert!(fixture.opened.borrow().is_empty());
        let error = &fixture.session.read(cx).browser_errors
            [&("saved-message".into(), file.to_string_lossy().into_owned())];
        assert!(error.starts_with("Couldn’t open in browser:"));
        assert!(
            !error.contains("File unavailable"),
            "the test backend rejects real browser launches"
        );
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native X11 HTML link fixture; requires import"]
fn html_transcript_links_native_capture() {
    use gpui::AppContext as _;
    assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
    let output = PathBuf::from(std::env::var("GROVE_HTML_LINK_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    let selected_case =
        std::env::var("GROVE_HTML_LINK_CAPTURE_CASE").unwrap_or_else(|_| "wide-dark".into());
    assert!(matches!(
        selected_case.as_str(),
        "wide-dark" | "narrow-light"
    ));
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        let mut retained = None;
        let window = cx.open_window(gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                gpui::point(gpui::px(20.), gpui::px(20.)), gpui::size(gpui::px(840.), gpui::px(540.)),
            ))), ..Default::default()
        }, |window, cx| {
            window.set_window_title("Grove HTML link QA");
            let session = cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
            let fixture = cx.new(|cx| {
                let subscription = cx.subscribe(&session, |_, _, _: &OpenLocalFile, _| {});
                LinkFixture { session, text: String::new(), width: None, _subscription: subscription, opened: Rc::default() }
            });
            retained = Some(fixture.clone());
            cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false))
        }).unwrap();
        let fixture = retained.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            for (name, narrow, light) in [("wide-dark", false, false), ("narrow-light", true, true)] {
                if name != selected_case { continue; }
                cx.update_window(window.into(), |_, window, cx| {
                    grove_core::theme::set_by_name(if light { c::DEFAULT_LIGHT_THEME } else { c::DEFAULT_DARK_THEME });
                    c::sync_component_theme(cx);
                    window.set_rem_size(gpui::px(16.));
                    fixture.update(cx, |fixture, cx| {
                        fixture.width = Some(gpui::px(if narrow { 280. } else { 700. }));
                        fixture.text = if narrow {
                            "[A longer interactive mock label with several words for narrow transcript layout](preview.htm)\n\n[Open in browser](preview.html)"
                        } else {
                            "[Open interactive mock](.grove-artifacts/composer-suggestions/index.html)\n\nTry typing, clearing the draft, switching with arrows, and accepting with Tab.\n\n[First](one.html) and [Second](two.htm)"
                        }.into();
                        cx.notify();
                    });
                    window.resize(gpui::size(gpui::px(if narrow { 300. } else { 840. }), gpui::px(540.)));
                    window.refresh();
                }).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                assert!(std::process::Command::new("import").args(["-window", "Grove HTML link QA"]).arg(output.join(format!("{name}.png"))).status().unwrap().success());
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
            drop(fixture);
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native UI and captures the production screenshot pipeline"]
fn native_visual_preview_pipeline() {
    use base64::Engine as _;
    use gpui::AppContext as _;
    assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
    let output = PathBuf::from(std::env::var("GROVE_VISUAL_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        let mut retained = None;
        let window = cx.open_window(gpui::WindowOptions::default(), |window, cx| {
            window.set_window_title("Grove native visual capture QA");
            window.set_rem_size(gpui::px(16.));
            let session = cx.new(|_| HtmlSession::new(std::env::current_dir().unwrap(), ScrollHandle::new()));
            let fixture = cx.new(|cx| {
                let subscription = cx.subscribe(&session, |_, _, _: &OpenLocalFile, _| {});
                LinkFixture { session, text: "Native screenshot pipeline QA".into(), width: None, _subscription: subscription, opened: Rc::default() }
            });
            retained = Some(fixture.clone());
            cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false))
        }).unwrap();
        let fixture = retained.unwrap();
        grove_core::visual_preview::enable();
        cx.spawn(async move |cx| {
            for (width, light) in [(728, false), (360, true)] {
                cx.update_window(window.into(), |_, window, cx| {
                    grove_core::theme::set_by_name(if light { c::DEFAULT_LIGHT_THEME } else { c::DEFAULT_DARK_THEME });
                    c::sync_component_theme(cx);
                    window.set_rem_size(gpui::px(if light { 20. } else { 16. }));
                }).unwrap();
                let input: grove_core::visuals::Input = serde_json::from_value(serde_json::json!({"title":"Native preview QA","html":"<!doctype html><html><head><style>body{padding:var(--grove-gap)}h1{font-size:var(--grove-title)}.bar{height:24px;background:var(--grove-info);width:70%;margin-bottom:var(--grove-gap)}input{font:inherit;color:var(--grove-fg);background:var(--grove-surface);border:1px solid var(--grove-border)}</style></head><body><h1>Native visual preview</h1><div class='bar'></div><input value='Retained input'><script>console.warn('Preview diagnostic');fetch('https://example.com/blocked').catch(()=>console.error('Network blocked'))</script></body></html>"})).unwrap();
                let html = grove_core::visuals::prepare(&input, Path::new("/tmp"), &[], false).unwrap();
                let expected_digest = grove_core::visuals::digest(html.as_bytes());
                let (send, receive) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let result = grove_core::visual_preview::capture(html, width, 560, &std::sync::atomic::AtomicBool::new(false), &std::sync::atomic::AtomicBool::new(false));
                    let _ = send.send(result);
                });
                let deadline = std::time::Instant::now() + Duration::from_secs(22);
                let result = loop {
                    cx.update_window(window.into(), |_, window, cx| {
                        crate::platform::visual_capture::pump_native();
                        pump_capture(window, cx);
                    }).unwrap();
                    if let Ok(result) = receive.try_recv() { break result.unwrap(); }
                    assert!(std::time::Instant::now() < deadline, "capture pipeline timed out");
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                };
                assert_eq!(result["source_digest"], expected_digest);
                assert_eq!(result["width"], width);
                let content_height = result["contentHeight"].as_f64().unwrap();
                assert!(content_height > 0. && content_height < 500., "capture must measure content in standards mode, not the viewport: {content_height}");
                let messages = result["consoleMessages"].as_array().unwrap();
                assert!(messages.iter().any(|m| m["text"] == "Preview diagnostic"));
                assert!(messages.iter().any(|m| m["text"] == "Network blocked"));
                let png = base64::engine::general_purpose::STANDARD.decode(result["png"].as_str().unwrap()).unwrap();
                assert!(png.starts_with(b"\x89PNG"));
                let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
                let background = pixels.get_pixel(0, 0);
                if light { assert!(background[0] > 230 && background[1] > 230 && background[2] > 230); }
                else { assert!(background[0] < 60 && background[1] < 60 && background[2] < 60, "display preference/theme composition must actually apply: {background:?}"); }

                fs_err::write(output.join(format!("preview-{width}.png")), png).unwrap();
                let mut metadata = result; metadata.as_object_mut().unwrap().remove("png");
                fs_err::write(output.join(format!("preview-{width}.json")), serde_json::to_vec_pretty(&metadata).unwrap()).unwrap();
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
            drop(fixture);
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

#[gpui::test]
fn os_text_scale_keeps_native_and_report_sizes_aligned(cx: &mut gpui::TestAppContext) {
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().text_size(rpx(TEXT_BODY)).child("Text preference")
        }
    }
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::ZoomState::new(1.2));
    });
    let (_, cx) = cx.add_window_view(|_, _| Fixture);
    let display_scale = cx.update(|window, _| window.scale_factor());
    for os_scale in [0.8182, 1., 1.5] {
        cx.update(|window, cx| {
            let state = crate::zoom::ZoomState::new(1.2).with_text_scale(os_scale);
            let rem = state.rem_size();
            cx.set_global(state);
            crate::platform::text_preferences::sync_component_size(cx);
            window.set_rem_size(gpui::px(rem));
            window.draw(cx).clear();
            let body = f32::from(rpx(TEXT_BODY).to_pixels(window.rem_size()));
            assert_eq!(body, TEXT_BODY * (rem / crate::zoom::REM_BASE));
            assert!(report_script(window, false).contains(&format!("--grove-body:{body}px")));
            assert_eq!(gpui_component::Theme::global(cx).font_size, gpui::px(rem));
            assert_eq!(window.scale_factor(), display_scale);
            assert_eq!(cx.global::<crate::zoom::ZoomState>().zoom, 1.2);
        });
    }
}

#[test]
fn browser_labels_include_descriptive_transcript_links() {
    assert!(browser_link_label("Open the interactive mock in browser"));
    assert!(browser_link_label("Open in browser"));
    assert!(!browser_link_label("Preview mock"));
}

#[test]
fn integrated_reports_require_the_exact_marker() {
    assert!(!integrated_report("<html>ordinary document</html>"));
    assert!(!integrated_report("<html>grove:integrated-report</html>"));
    assert!(integrated_report(&format!("<html>{REPORT_MARKER}</html>")));
}

#[test]
fn menu_and_tooltip_exclusions_stay_within_the_document() {
    let rect = |x, y, w, h| {
        Bounds::new(
            gpui::point(gpui::px(x), gpui::px(y)),
            gpui::size(gpui::px(w), gpui::px(h)),
        )
    };
    let clip = rect(300., 40., 500., 300.);
    let menu = rect(250., 60., 150., 200.);
    let sidebar_tooltip = rect(100., 80., 180., 30.);
    assert_eq!(
        merge_exclusions(Some(menu), Some(sidebar_tooltip), clip),
        Some(rect(300., 60., 100., 200.))
    );
    assert_eq!(merge_exclusions(None, Some(sidebar_tooltip), clip), None);
    assert_eq!(merge_exclusions(None, None, clip), None);
    let tooltip = rect(320., 80., 180., 30.);
    assert_eq!(
        merge_exclusions(Some(menu), Some(tooltip), clip),
        Some(rect(300., 60., 200., 200.))
    );
}

#[gpui::test]
fn native_html_surface_lease_survives_arena_clear_and_retires_when_omitted(
    cx: &mut gpui::TestAppContext,
) {
    struct SurfaceFixture {
        surface: Rc<Surface>,
        mounted: bool,
    }
    impl Render for SurfaceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().when(self.mounted, |root| {
                root.child(NativeElement {
                    integrated: false,
                    artifact: cx.entity_id(),
                    surface: self.surface.clone(),
                    paused: Rc::new(Cell::new(false)),
                })
            })
        }
    }
    let visible = Rc::new(Cell::new(false));
    let probe = visible.clone();
    let (fixture, cx) = cx.add_window_view(|_, _| SurfaceFixture {
        surface: Rc::new(Surface {
            initialized: Cell::new(false),
            native: NativeDocument::with_visibility_probe(probe),
            generation: Cell::new(0),
        }),
        mounted: true,
    });
    cx.update(|window, cx| window.draw(cx).clear());
    assert!(
        visible.get(),
        "clearing the just-presented GPUI arena must not retire its visible native surface"
    );
    cx.update(|window, cx| {
        fixture.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    assert!(
        visible.get(),
        "replacement frames must preserve the surface after their arena clears"
    );
    cx.update(|window, cx| {
        fixture.update(cx, |fixture, cx| {
            fixture.mounted = false;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert!(
        !visible.get(),
        "omission must retire the prior frame-owned lease"
    );
    cx.update(|window, cx| {
        fixture.update(cx, |fixture, cx| {
            fixture.mounted = true;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert!(
        visible.get(),
        "the retained document can attach again after omission"
    );
}

#[gpui::test]
fn expansion_selects_only_one_surface_and_return_restores_scroll(cx: &mut gpui::TestAppContext) {
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    cx.update(gpui_component::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
        let owner = session.downgrade();
        let paused = Rc::new(Cell::new(false));
        let a = cx.new(|cx| {
            HtmlArtifact::new(
                "<html>A</html>".into(),
                "A".into(),
                None,
                owner.clone(),
                paused.clone(),
                ScrollHandle::new(),
                cx,
            )
        });
        let b = cx.new(|cx| {
            HtmlArtifact::new(
                "<html>B</html>".into(),
                "B".into(),
                None,
                owner,
                paused,
                ScrollHandle::new(),
                cx,
            )
        });
        set_context(Some(session.entity_id()), false, cx);
        assert!(surface_is_selected(a.entity_id(), None));
        assert!(surface_is_selected(b.entity_id(), None));
        a.update(cx, |a, cx| a.toggle_expand(window, cx));
        let selected = cx
            .global::<HtmlUi>()
            .expanded
            .as_ref()
            .map(|entity| entity.entity_id());
        assert!(surface_is_selected(a.entity_id(), selected));
        assert!(!surface_is_selected(b.entity_id(), selected));
        a.update(cx, |a, cx| {
            assert_eq!(
                a.owner,
                session.entity_id(),
                "fenced artifact owner must be the session"
            );
            a.return_offset = Some(gpui::point(gpui::px(0.), gpui::px(-120.)));
            a.toggle_expand(window, cx);
            assert_eq!(a.scroll.offset().y, gpui::px(-120.));
            assert!(a.expand_focus.is_focused(window));
        });
        assert!(cx.global::<HtmlUi>().expanded.is_none());
        a.update(cx, |a, cx| a.toggle_expand(window, cx));
        set_context(None, false, cx);
        assert!(
            cx.global::<HtmlUi>().expanded.is_none(),
            "switching session dismisses expansion"
        );
        Empty
    });
    cx.run_until_parked();
}

#[gpui::test]
fn snapshot_open_releases_owner_before_expansion_and_restores_source_focus(
    cx: &mut gpui::TestAppContext,
) {
    struct Source {
        focus: FocusHandle,
    }
    impl Render for Source {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).child("Native plan source")
        }
    }
    cx.update(gpui_component::init);
    let mut owner = None;
    let mut source_focus = None;
    let (_, cx) = cx.add_window_view(|_, cx| {
        let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
        set_context(Some(session.entity_id()), false, cx);
        let focus = cx.focus_handle();
        source_focus = Some(focus.clone());
        owner = Some(session);
        Source { focus }
    });
    let owner = owner.unwrap();
    let source_focus = source_focus.unwrap();
    let source = "<html><body>Immutable plan exhibit</body></html>";
    cx.update(|window, cx| {
        source_focus.focus(window, cx);
        owner.update(cx, |session, cx| {
            session.open_snapshot("saved-plan-asset".into(), source.into(), window, cx);
            assert!(cx.global::<HtmlUi>().expanded.is_none());
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let artifact = owner.read(cx).artifacts["saved-plan-asset"].clone();
        assert!(expanded(&artifact, cx));
        assert_eq!(artifact.read(cx).owner, owner.entity_id());
        assert_eq!(artifact.read(cx).version.active, source);
        artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
        assert!(cx.global::<HtmlUi>().expanded.is_none());
        assert!(source_focus.is_focused(window));
        assert_eq!(owner.read(cx).artifacts.len(), 1);
        owner.update(cx, |session, cx| {
            session.open_snapshot(
                "saved-plan-asset".into(),
                "changed source".into(),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let artifact = owner.read(cx).artifacts["saved-plan-asset"].clone();
        assert!(expanded(&artifact, cx));
        assert_eq!(artifact.read(cx).version.active, source);
        assert_eq!(owner.read(cx).artifacts.len(), 1);
        artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
        assert!(source_focus.is_focused(window));
    });
}

#[gpui::test]
fn file_references_in_later_replies_capture_independent_source(cx: &mut gpui::TestAppContext) {
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    cx.update(gpui_component::init);
    let checkout = tempfile::tempdir().unwrap();
    let path = checkout.path().join("same.html");
    fs_err::write(&path, "<html>First version</html>").unwrap();
    let mut session = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let owner = cx.new(|_| HtmlSession::new(checkout.path().into(), ScrollHandle::new()));
        owner.update(cx, |session, cx| {
            session.open_link("first", "same.html", window, cx);
        });
        session = Some(owner);
        Empty
    });
    let session = session.unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let first = session.read(cx).opened["first"][0].clone();
        assert_eq!(
            session.read(cx).artifacts[&first].read(cx).version.active,
            "<html>First version</html>"
        );
    });
    fs_err::write(&path, "<html>Updated version</html>").unwrap();
    cx.update(|window, cx| {
        session.update(cx, |session, cx| {
            session.open_link("second", "same.html", window, cx);
            session.open_link("first", "same.html", window, cx);
            session.open_link("second", "same.html", window, cx);
            assert_eq!(session.opened_count(), 2);
            assert_eq!(session.artifact_ids().len(), 2);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let session = session.read(cx);
        let first = &session.artifacts[&session.opened["first"][0]];
        let second = &session.artifacts[&session.opened["second"][0]];
        assert_ne!(first.entity_id(), second.entity_id());
        assert_eq!(first.read(cx).version.active, "<html>First version</html>");
        assert!(first.read(cx).version.pending.is_none());
        assert_eq!(
            second.read(cx).version.active,
            "<html>Updated version</html>"
        );
    });
}

#[gpui::test]
fn inline_documents_omit_toolbar_and_expanded_documents_retain_controls(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_component::init);
    let mut owner = None;
    let (artifact, cx) = cx.add_window_view(|_, cx| {
        let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
        owner = Some(session.clone());
        HtmlArtifact::new(
            "<html>Ordinary</html>".into(),
            String::new(),
            None,
            session.downgrade(),
            Rc::default(),
            ScrollHandle::new(),
            cx,
        )
    });
    cx.update(|window, cx| window.draw(cx).clear());
    assert!(cx.debug_bounds("html-document-toolbar").is_none());
    cx.update(|window, cx| {
        artifact.update(cx, |artifact, cx| {
            artifact.report = true;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert!(cx.debug_bounds("html-document-toolbar").is_none());
    cx.update(|window, cx| {
        artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
        window.draw(cx).clear();
    });
    assert!(cx.debug_bounds("html-document-toolbar").is_some());
    drop(owner);
}

#[test]
fn explicit_browser_links_are_not_auto_previewed() {
    let cwd = Path::new("/tmp/project");
    assert!(output_html_link("[Open in browser](large.html)", cwd).is_none());
    assert!(output_html_link("[**Open in browser**](large.html)", cwd).is_none());
    assert!(output_html_link("[  OPEN IN BROWSER  ](large.html)", cwd).is_none());
    assert_eq!(
        output_html_link(
            "[Small mock](small.html) [Open in browser](large.html)",
            cwd
        ),
        Some("small.html".into())
    );
}

#[cfg(unix)]
#[test]
fn fifo_and_symlink_to_fifo_are_rejected_without_opening() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("out.html");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let link = dir.path().join("linked.html");
    symlink(&fifo, &link).unwrap();
    assert!(read_document(&fifo)
        .unwrap_err()
        .message
        .contains("regular file"));
    assert!(read_document(&link)
        .unwrap_err()
        .message
        .contains("regular file"));
    assert!(read_document(dir.path())
        .unwrap_err()
        .message
        .contains("regular file"));
}

#[test]
fn complete_fences_and_documents_only() {
    for source in [
        "```html\n<html></html>\n```",
        "~~~~html\n<!doctype html><html></html>\n~~~~~",
    ] {
        assert!(closed_fence(source));
    }
    for source in [
        "```html\n<html></html>",
        "````html\n<html></html>\n```",
        "```html\n<html></html>\n~~~",
        "```html\n<html></html>\n```bad",
    ] {
        assert!(!closed_fence(source));
    }
    assert!(complete_document(
        "<!DOCTYPE html><html><body></body></html>"
    ));
    for source in [
        "<!-- grove:integrated-report -->\n<!doctype html><html></html>",
        "\u{feff}  <!-- a --><!--b-->\n<HTML></HTML>",
    ] {
        assert!(complete_document(source));
    }
    assert!(!complete_document("<!-- unterminated <html></html>"));
    assert!(!complete_document(
        "<!-- note --><div>fragment</div></html>"
    ));
    assert!(!complete_document("<div>fragment</div>"));
    assert!(!complete_document("<html><body>streaming"));
    assert!(!complete_document(&format!(
        "<html>{}</html>",
        "x".repeat(MAX_HTML_BYTES)
    )));
}
#[test]
fn fast_completion_opens_without_a_rendered_busy_snapshot() {
    for restored_done in [false, true] {
        let mut auto = AutoOpen::default();
        assert!(auto
            .observe(
                false,
                restored_done,
                None,
                Some(("restored", "[old](old.html)"))
            )
            .is_none());
        assert!(auto
            .observe(false, true, None, Some(("restored", "[old](old.html)")))
            .is_none());
        assert_eq!(
            auto.observe(false, true, None, Some(("new-final", "[new](new.html)"))),
            Some(("new-final".into(), "[new](new.html)".into()))
        );
        assert!(auto
            .observe(false, true, None, Some(("new-final", "[new](new.html)")))
            .is_none());
    }
}

#[test]
fn previous_turn_output_is_not_reopened_after_a_run_with_no_new_answer() {
    let mut auto = AutoOpen::default();
    auto.observe(false, true, None, Some(("old", "[old](old.html)")));
    auto.observe(true, false, Some("next"), Some(("old", "[old](old.html)")));
    assert!(auto
        .observe(false, true, None, Some(("old", "[old](old.html)")))
        .is_none());
}
#[test]
fn browser_url_keeps_fragment_and_query_while_read_path_does_not() {
    let cwd = Path::new("/tmp/project");
    assert_eq!(
        local_html_url("preview.html?theme=dark#tasks", cwd)
            .unwrap()
            .as_str(),
        "file:///tmp/project/preview.html?theme=dark#tasks"
    );
    assert_eq!(
        local_html_path("preview.html?theme=dark#tasks", cwd).unwrap(),
        PathBuf::from("/tmp/project/preview.html")
    );
}
#[test]
fn new_versions_require_explicit_adoption() {
    let mut version = Version {
        active: "first".into(),
        markdown: "one".into(),
        pending: None,
    };
    version.offer("second".into(), "two".into());
    assert_eq!(version.active, "first");
    version.offer("second".into(), "two".into());
    assert_eq!(version.active, "first");
    assert!(version.adopt());
    assert_eq!(version.active, "second");
    assert!(!version.adopt());
    version.offer("second".into(), "two".into());
    assert!(version.pending.is_none());
}
#[test]
fn local_link_resolution_keeps_browser_and_disk_targets_distinct() {
    let cwd = Path::new("/tmp/project");
    for target in [
        "https://example.com/a.html",
        "//example.com/a.html",
        "file://remote/tmp/a.html",
        "mailto:a.html",
        "a.rs",
    ] {
        assert!(local_html_path(target, cwd).is_none(), "{target}");
    }
    for (target, expected) in [
        ("file:///tmp/a.htm#section", "/tmp/a.htm"),
        ("/tmp/A%20B.html#x", "/tmp/A B.html"),
        ("./out/demo.HTML", "/tmp/project/out/demo.HTML"),
        ("../demo.html", "/tmp/demo.html"),
    ] {
        assert_eq!(local_html_path(target, cwd), Some(PathBuf::from(expected)));
    }
}
#[test]
fn auto_open_needs_observed_completed_run_and_deduplicates() {
    let mut auto = AutoOpen::default();
    assert!(auto
        .observe(false, true, None, Some(("old", "old")))
        .is_none());
    assert!(auto.observe(true, false, Some("t1"), None).is_none());
    assert!(auto
        .observe(true, false, Some("t1"), Some(("new", "partial")))
        .is_none());
    assert_eq!(
        auto.observe(false, true, None, Some(("new", "final"))),
        Some(("new".into(), "final".into()))
    );
    assert!(auto
        .observe(false, true, None, Some(("new", "final")))
        .is_none());
    auto.observe(true, false, Some("t2"), None);
    assert!(auto
        .observe(false, false, None, Some(("failed", "text")))
        .is_none());
}
#[test]
fn output_detection_uses_markdown_links_not_tool_mentions() {
    let cwd = Path::new("/tmp");
    assert_eq!(
        output_html_link("Done: [Preview](out.html)", cwd),
        Some("out.html".into())
    );
    assert!(
        output_html_link("tool wrote out.html\n```sh\necho '[x](out.html)'\n```", cwd).is_none()
    );
    assert!(output_html_link("[site](https://example.com/out.html)", cwd).is_none());
}
#[test]
fn sandbox_policy_precedes_author_bytes_and_blocks_nested_and_transport_loads() {
    let hostile = "<html><head><meta http-equiv='Content-Security-Policy' content='default-src *'></head><body><iframe srcdoc=\"<script>fetch('https://example.com')</script>\"></iframe></body></html>";
    let html = sandboxed(hostile, false);
    assert!(html.find(CSP).unwrap() < html.find(hostile).unwrap());
    for directive in [
        "connect-src 'none'",
        "frame-src 'none'",
        "worker-src 'none'",
        "object-src 'none'",
        "form-action 'none'",
        "base-uri 'none'",
    ] {
        assert!(CSP.contains(directive));
    }
}
#[test]
fn near_limit_sizes_remain_visibly_greater_than_the_limit() {
    assert_eq!(
        artifact_size(2 * 1024 * 1024 + 1, Some(2 * 1024 * 1024)),
        "2097153 bytes"
    );
    assert_eq!(
        artifact_size(1024 * 1024 + 1, Some(1024 * 1024)),
        "1048577 bytes"
    );
    assert_eq!(artifact_size(3_565_158, Some(2 * 1024 * 1024)), "3.4 MiB");
    assert_eq!(artifact_size(2 * 1024 * 1024, None), "2 MiB");
}

#[test]
fn recovery_paths_use_only_the_owning_session_roots() {
    let checkout = tempfile::tempdir().unwrap();
    let additional = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let artifact = additional.path().join("artifact.html");
    let blocked = outside.path().join("blocked.html");
    fs_err::write(&artifact, "<html></html>").unwrap();
    fs_err::write(&blocked, "<html></html>").unwrap();
    assert!(recovery_path(&artifact, checkout.path(), &[]).is_none());
    let roots = vec![additional.path().to_owned()];
    assert_eq!(
        recovery_path(&artifact, checkout.path(), &roots),
        Some(artifact.canonicalize().unwrap())
    );
    assert!(recovery_path(&blocked, checkout.path(), &roots).is_none());
    #[cfg(unix)]
    {
        let link = checkout.path().join("changed.html");
        std::os::unix::fs::symlink(&blocked, &link).unwrap();
        assert!(recovery_path(&link, checkout.path(), &roots).is_none());
    }
}

#[test]
fn file_reads_reject_invalid_utf8_and_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.html");
    fs_err::write(&path, "<html>ok</html>").unwrap();
    assert_eq!(read_document(&path).unwrap(), "<html>ok</html>");
    fs_err::write(&path, [255]).unwrap();
    assert!(read_document(&path).is_err());
    fs_err::write(&path, vec![b'x'; MAX_HTML_BYTES + 1]).unwrap();
    let failure = read_document(&path).unwrap_err();
    let size = failure.size_limit.unwrap();
    assert_eq!(size.bytes, MAX_HTML_BYTES as u64 + 1);
    assert_eq!(size.limit, MAX_HTML_BYTES as u64);
    assert_eq!(size.path, path);
    fs_err::write(&path, "<html>fixed</html>").unwrap();
    assert_eq!(read_document(&path).unwrap(), "<html>fixed</html>");
    assert!(read_document(&dir.path().join("missing.html"))
        .unwrap_err()
        .size_limit
        .is_none());
}
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn oversized_recovery_native_snapshots() {
    use gpui::AppContext as _;
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let html = grove_core::rich_plan::FileSizeLimit {
                path: "mock-comparison-with-a-long-artifact-name.html".into(),
                bytes: 3_565_158,
                limit: MAX_HTML_BYTES as u64,
            };
            let plan = grove_core::rich_plan::FileSizeLimit {
                path: "review.grove-plan.json".into(),
                bytes: 1_363_149,
                limit: 1024 * 1024,
            };
            let browser =
                recovery_action("qa-browser", "Open in browser", "external-link", window, cx)
                    .into_any_element();
            let reveal_html =
                recovery_action("qa-html-reveal", "Reveal file", "folder", window, cx)
                    .into_any_element();
            let updates = recovery_action("qa-updates", "Check for updates", "refresh", window, cx)
                .into_any_element();
            let reveal_plan =
                recovery_action("qa-plan-reveal", "Reveal file", "folder", window, cx)
                    .into_any_element();
            div()
                .w_full()
                .h_full()
                .p(rpx(SPACE_2XL))
                .bg(c::BG())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(oversized_card(
                    "Preview too large",
                    &html,
                    "Open the full document in your browser.",
                    vec![browser, reveal_html, updates],
                    cx,
                ))
                .child(oversized_card(
                    "Plan too large",
                    &plan,
                    "Move embedded images into separate assets and reference their paths.",
                    vec![reveal_plan],
                    cx,
                ))
        }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
    });
    let previous_theme = grove_core::theme::current().name.clone();
    let previous_font = cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size);
    for appearance in ["dark", "light"] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&format!(
                "grove-{appearance}"
            )));
            c::sync_component_theme(cx);
        });
        for (width, zoom) in [(800., 1.), (400., 1.), (400., 1.25)] {
            cx.update(|cx| {
                gpui_component::Theme::global_mut(cx).font_size =
                    gpui::px(crate::zoom::REM_BASE * zoom);
            });
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(650.)), |window, cx| {
                    let fixture = cx.new(|_| Fixture);
                    cx.new(|cx| {
                        gpui_component::Root::new(fixture, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                })
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let path =
                format!("/tmp/grove-oversized-recovery-{appearance}-{width:.0}-{zoom:.2}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Oversized recovery fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
    cx.update(|cx| {
        assert!(grove_core::theme::set_by_name(&previous_theme));
        c::sync_component_theme(cx);
        gpui_component::Theme::global_mut(cx).font_size = previous_font;
    });
}
