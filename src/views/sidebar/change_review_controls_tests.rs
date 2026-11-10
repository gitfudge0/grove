//! Native Review source-picker and permanent saved-result controls.
use super::tests::{create_sidebar, draw, frozen_review, initialize, FIXTURE_PATH};
use super::*;
const FIXTURE_PANEL_WIDTH: f32 = 280.;
/// The production controls with frozen availability; no provider or GitHub IO.
struct ReviewControlsFixture {
    sidebar: Entity<Sidebar>,
    width: f32,
    _observer: gpui::Subscription,
    local: bool,
    pr: bool,
    bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    drive_frames: bool,
}
fn controls_pr() -> grove_core::checkout_pr::PullRequest {
    serde_json::from_value(serde_json::json!({
        "number":142, "url":"https://github.com/example/northstar/pull/142",
        "state":"OPEN", "isDraft":false, "headRefOid":"fixture-head"
    }))
    .unwrap()
}
impl Render for ReviewControlsFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drive_frames {
            window.request_animation_frame();
        }
        let bounds = self.bounds.clone();
        let reference = super::super::checkout_panel::checkout_loading_row(
            "review-hover-reference",
            "Commit changes",
            "git",
            false,
            window,
            cx,
        );
        let controls = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_change_tools_sources(FIXTURE_PATH, self.local, self.pr.then(controls_pr));
            let local = sidebar.render_change_tools_heading(FIXTURE_PATH, true, window, cx);
            let pr = self
                .pr
                .then(|| sidebar.render_change_tools_heading(FIXTURE_PATH, false, window, cx));
            div()
                .flex()
                .flex_col()
                .child(local)
                .children(pr)
                .into_any_element()
        });
        div()
            .size_full()
            .bg(c::BG())
            .text_color(c::FG())
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .p(rpx(SPACE_3XL))
            .child(
                div()
                    .w(rpx(self.width))
                    .on_prepaint(move |rect, _, _| bounds.set(rect))
                    .rounded(rpx(RADIUS_PANEL))
                    .bg(c::FIELD_FILL())
                    .child(reference)
                    .child(controls),
            )
    }
}
#[gpui::test]
fn review_controls_only_offer_real_choices_and_keyboard_selection(cx: &mut gpui::TestAppContext) {
    cx.update(initialize);
    let mut handles = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let (sidebar, _) = create_sidebar(window, cx);
        sidebar.update(cx, |sidebar, _| {
            sidebar.reviews.open = None;
            sidebar.reviews.entries.clear();
        });
        let fixture = cx.new(|cx| ReviewControlsFixture {
            width: FIXTURE_PANEL_WIDTH,
            _observer: cx.observe(&sidebar, |_, _, cx| cx.notify()),
            sidebar: sidebar.clone(),
            local: true,
            pr: false,
            bounds: std::rc::Rc::default(),
            drive_frames: false,
        });
        handles = Some((sidebar, fixture.clone()));
        gpui_component::Root::new(fixture, window, cx).bordered(false)
    });
    let (sidebar, fixture) = handles.unwrap();
    draw(cx);
    assert!(
        cx.debug_bounds("review-source").is_none(),
        "section scope requires no picker"
    );
    let trigger = cx
        .debug_bounds("checkout-local-tools")
        .expect("local section keeps its own menu");
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |s, _| {
        assert!(s.checkout_panel.change_tools.menu.is_some());
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        let s = sidebar.read(cx);
        assert!(s.checkout_panel.change_tools.menu.is_none());
        assert!(s.checkout_panel.change_tools.focus[&true].is_focused(window));
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    sidebar.read_with(cx, |s, _| {
        assert!(s.checkout_panel.change_tools.menu.is_some());
    });
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        fixture.update(cx, |f, cx| {
            f.pr = true;
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("checkout-local-tools").is_some());
    assert!(cx.debug_bounds("checkout-pr-tools").is_some());
    cx.update(|window, cx| {
        let focus = sidebar.read(cx).checkout_panel.change_tools.focus[&true].clone();
        focus.focus(window, cx);
    });
    draw(cx);
    // Isolated fixtures omit Sidebar's unconsumed Tab router; inspect its native traversal order.
    cx.update(Window::focus_next);
    draw(cx);
    cx.update(|window, cx| {
        assert!(
            sidebar.read(cx).checkout_panel.change_tools.focus[&false].is_focused(window),
            "Tab reaches the PR section menu; actual={:?}, local={:?}, pr={:?}",
            window.focused(cx),
            sidebar.read(cx).checkout_panel.change_tools.focus[&true],
            sidebar.read(cx).checkout_panel.change_tools.focus[&false]
        );
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            let local = s
                .review_tool_state(FIXTURE_PATH, true, Some(Scope::Local), cx)
                .unwrap();
            let pr_scope = Scope::PullRequest {
                url: controls_pr().url,
            };
            let pr = s
                .review_tool_state(FIXTURE_PATH, false, Some(pr_scope.clone()), cx)
                .unwrap();
            assert_eq!(local.scope, Scope::Local);
            assert_eq!(pr.scope, pr_scope);
            assert_eq!(local.label, "Review changes");
            assert_eq!(pr.label, "Review changes");
            let target = key(&s.selection, FIXTURE_PATH, &Scope::Local);
            let mut result = frozen_review();
            result.snapshot.scope = Scope::Local;
            s.reviews.entries.insert(
                target.clone(),
                Entry {
                    scope: Some(Scope::Local),
                    result: Some(Arc::new(result)),
                    freshness: Some(Ok(false)),
                    checking: true,
                    ..Default::default()
                },
            );
            let saved = s
                .review_tool_state(FIXTURE_PATH, true, Some(Scope::Local), cx)
                .unwrap();
            assert_eq!(saved.label, "Open review");
            assert!(
                saved.status.is_some(),
                "staleness remains visible outside menus"
            );
            s.reviews.entries.get_mut(&target).unwrap().cancel =
                Some(Arc::new(CommitControl::default()));
            let busy = s
                .review_tool_state(FIXTURE_PATH, true, Some(Scope::Local), cx)
                .unwrap();
            assert_eq!(busy.label, "Open review progress");
            assert!(
                busy.status.is_some(),
                "progress remains visible outside menus"
            );
        });
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated controls window; requires artifact GROVE_CONFIG_DIR and 16 MiB stack"]
fn review_controls_linux_native_capture() {
    let config = std::path::PathBuf::from(std::env::var("GROVE_CONFIG_DIR").unwrap());
    assert!(
        config.is_absolute()
            && config
                .to_string_lossy()
                .contains(".grove-artifacts/describe-parity-review/controls/")
    );
    let output = config.parent().unwrap().to_path_buf();
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            initialize(cx);
            crate::fonts::register(cx).unwrap();
            let mut handles = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(20.), gpui::px(20.)),
                            gpui::size(gpui::px(480.), gpui::px(400.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove review controls QA");
                        window.set_rem_size(gpui::px(16.));
                        let (sidebar, _) = create_sidebar(window, cx);
                        sidebar.update(cx, |sidebar, _| {
                            sidebar.reviews.open = None;
                            sidebar.reviews.entries.clear();
                        });
                        let fixture = cx.new(|cx| ReviewControlsFixture {
                            width: FIXTURE_PANEL_WIDTH,
                            _observer: cx.observe(&sidebar, |_, _, cx| cx.notify()),
                            sidebar: sidebar.clone(),
                            local: true,
                            pr: false,
                            bounds: std::rc::Rc::default(),
                            drive_frames: true,
                        });
                        handles = Some((sidebar, fixture.clone()));
                        cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false))
                    },
                )
                .unwrap();
            let (sidebar, fixture) = handles.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
                for state in [
                    "local",
                    "hover",
                    "both",
                    "menu",
                    "selected",
                    "loading",
                    "saved",
                    "stale",
                    "light",
                    "light-hover",
                    "zoom",
                    "narrow",
                ] {
                    cx.update_window(window.into(), |_, window, cx| {
                        if matches!(state, "hover" | "light-hover") {
                            let bounds = fixture.read(cx).bounds.get();
                            let position = gpui::point(
                                bounds.left()
                                    + rpx(SPACE_2XL + ICON_LG).to_pixels(window.rem_size()),
                                bounds.top()
                                    + rpx(ROW_H + SPACE_LG + ROW_H / 2.)
                                        .to_pixels(window.rem_size()),
                            );
                            window.dispatch_event(
                                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                    position,
                                    pressed_button: None,
                                    modifiers: gpui::Modifiers::default(),
                                }),
                                cx,
                            );
                        }
                        if state == "both" {
                            fixture.update(cx, |fixture, cx| {
                                fixture.pr = true;
                                cx.notify();
                            });
                        }
                        if state == "menu" {
                            sidebar.update(cx, |sidebar, cx| {
                                sidebar
                                    .checkout_panel
                                    .change_tools
                                    .focus
                                    .get(&true)
                                    .unwrap()
                                    .focus(window, cx);
                            });
                            controls_press_key(window, cx, "enter");
                        }
                        if state == "selected" {
                            controls_press_key(window, cx, "escape");
                        }
                        if state == "loading" {
                            sidebar.update(cx, |sidebar, cx| {
                                let scope = Scope::PullRequest {
                                    url: controls_pr().url,
                                };
                                let target = key(&sidebar.selection, FIXTURE_PATH, &scope);
                                sidebar.reviews.entries.insert(
                                    target,
                                    Entry {
                                        scope: Some(scope),
                                        cancel: Some(Arc::new(CommitControl::default())),
                                        ..Default::default()
                                    },
                                );
                                cx.notify();
                            });
                        }
                        if state == "saved" {
                            fixture.update(cx, |fixture, cx| {
                                fixture.pr = false;
                                cx.notify();
                            });
                            sidebar.update(cx, |sidebar, cx| {
                                // Use the permanent session-linked navigation path.
                                let target = key(&sidebar.selection, FIXTURE_PATH, &Scope::Local);
                                let id = SessionId::from_raw(999999);
                                sidebar.dashboard_open = false;
                                sidebar.selection = Some(Selection::Session(id));
                                let target = Key {
                                    owner: owner(&sidebar.selection),
                                    ..target
                                };
                                let mut review = frozen_review();
                                review.snapshot.scope = Scope::Local;
                                sidebar.reviews.entries.insert(
                                    target.clone(),
                                    Entry {
                                        scope: Some(Scope::Local),
                                        result: Some(Arc::new(review)),
                                        freshness: Some(Ok(true)),
                                        checking: true,
                                        checked_at: Some(Instant::now()),
                                        ..Default::default()
                                    },
                                );
                                sidebar.reviews.linked.insert(id, target);
                                cx.notify();
                            });
                        }
                        if state == "stale" {
                            sidebar.update(cx, |sidebar, cx| {
                                let target = sidebar.linked_review_key().unwrap();
                                sidebar.reviews.entries.get_mut(&target).unwrap().freshness =
                                    Some(Ok(false));
                                cx.notify();
                            });
                        }
                        if state == "light" {
                            grove_core::theme::set_by_name("grove-light");
                            c::sync_component_theme(cx);
                        }
                        if state == "zoom" {
                            window.set_rem_size(gpui::px(20.));
                        }
                        if state == "narrow" {
                            fixture.update(cx, |fixture, cx| {
                                fixture.width = 180.;
                                cx.notify();
                            });
                            window.set_rem_size(gpui::px(16.));
                            window.resize(gpui::size(gpui::px(320.), gpui::px(480.)));
                        }
                        fixture.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove review controls QA"])
                        .arg(output.join(format!("controls-{state}.png")))
                        .status()
                        .unwrap()
                        .success());
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[cfg(target_os = "linux")]
fn controls_press_key(window: &mut Window, cx: &mut App, name: &str) {
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
