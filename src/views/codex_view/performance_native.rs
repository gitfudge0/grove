/// CPU scene-build timings with macOS CoreText and Grove's actual bundled fonts.
/// A Metal screenshot proves native rendering, but GPU/compositor presentation
/// is deliberately outside the measured update; this is not a displayed FPS test.
#[test]
#[ignore = "manual macOS CoreText transcript workload; may save a Metal screenshot"]
fn transcript_native_frame_benchmark() {
    use gpui::AppContext as _;
    let force_eager_rows = std::env::var("GROVE_PERF_EAGER_ROWS").as_deref() == Ok("1");
    crate::views::codex_view::measured_slot::FORCE_EAGER.with(|flag| flag.set(force_eager_rows));
    let workload = std::env::var("GROVE_PERF_WORKLOAD").unwrap_or_else(|_| "real".into());
    let state = workload_state(&workload);
    let item_count = state.saved.items.len();
    let last_prose = state
        .saved
        .items
        .iter()
        .rposition(|item| {
            item["type"] == "agentMessage" && !item["text"].as_str().unwrap_or_default().is_empty()
        })
        .unwrap();
    let last_id = state.saved.items[last_prose]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = Rc::new(state);
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(std::env::var("GROVE_PERF_MOTION").as_deref() != Ok("1"));
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    });
    let (handle, _actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let connection = Connection {
        handle,
        updates: receive.into(),
    };
    let mut retained = None;
    let cold = Instant::now();
    let width = std::env::var("GROVE_PERF_WIDTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000.);
    let window = cx
        .open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
            let session = cx
                .new(|cx| CodexSession::from_connection(original.as_ref().clone(), connection, cx));
            let chat = cx.new(|cx| CodexView::new(session, window, cx));
            if workload == "real" {
                chat.update(cx, |view, _| {
                    view.expanded.insert(work_key_at(&original, last_prose));
                });
            }
            retained = Some(chat.clone());
            cx.new(|cx| {
                gpui_component::Root::new(chat, window, cx)
                    .bordered(false)
                    .bg(c::BG())
            })
        })
        .unwrap();
    let view = retained.unwrap();
    cx.run_until_parked();
    println!(
        "TRANSCRIPT_PERF {}",
        json!({"workload":workload,"operation":"cold_mount","sample":0,"ms":cold.elapsed().as_secs_f64()*1000.,"update_ms":cold.elapsed().as_secs_f64()*1000.,"settle_ms":cold.elapsed().as_secs_f64()*1000.,"items":item_count,"completed":true,"measurement":"macos_coretext_cpu_update_and_auto_draw"})
    );
    for _ in 0..3 {
        cx.update_window(window.into(), |_, _, cx| {
            view.update(cx, |_, cx| cx.notify());
        })
        .unwrap();
        cx.run_until_parked();
    }
    cx.update_window(window.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            view.scroll.scroll_to_bottom();
            cx.notify();
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        println!("TRANSCRIPT_METADATA {}", json!({"a11y_active": window.is_a11y_active(), "reduce_motion": cx.reduce_motion(), "forced_row_header_footer_eager": force_eager_rows, "vendor_body_override": false, "native_text_system": "CoreText", "scope": "CPU action and automatic draw; compositor FPS unmeasured"}));
        let view = view.read(cx);
        let fixture_state = &view.session.read(cx).state;
        let (tool_controls, current_work_controls) = measured_slot::tool_detail_counts();
        println!("TRANSCRIPT_CONTEXT {}", json!({"fixture":fixture_context(fixture_state),"rendered_tool_detail_controls":tool_controls,"rendered_current_work_tool_details":current_work_controls,"latest_work_expanded":view.expanded.contains(&work_key_at(fixture_state,last_prose))}));
        if fixture_state.status.busy() && workload == "real" && fixture_state.saved.items.iter().any(|item| is_tool_item(item.as_ref())) {
            assert!(view.expanded.contains(&work_key_at(fixture_state,last_prose)), "Working fixture must keep expanded latest history");
            assert!(tool_controls > 0, "Working fixture must not hide all tool details");
            let expected_current = fixture_state.saved.items.iter().enumerate().filter(|(index,item)| *index >= active_work_start(fixture_state) && is_tool_item(item.as_ref())).count();
            assert_eq!(current_work_controls, expected_current, "Working fixture must retain every expanded current-work tool detail");
        }
        let pane = view.scroll.bounds();
        assert!(pane.size.height > gpui::px(0.));
        assert!(
            view.selection_views
                .keys()
                .any(|key| key.contains(&last_id)),
            "last prose must retain a native text owner"
        );
        assert!(view.scroll.children_count() > 0);
        assert!(
            (view.scroll.offset().y + view.scroll.max_offset().y).abs() < gpui::px(1.),
            "bottom navigation must reach the actual native layout extent"
        );
        assert_eq!(view.session.read(cx).state.saved.items.len(), item_count);
        assert!(view.session.read(cx).state.error.is_none());
    })
    .unwrap();
    if let Ok(path) = std::env::var("GROVE_PERF_SCREENSHOT") {
        let screenshot = cx.capture_screenshot(window.into()).unwrap();
        assert!(screenshot.width() > 0 && screenshot.height() > 0);
        screenshot.save(path).unwrap();
    }
    for sample in 0..samples() {
        let operations = if sample % 2 == 0 {
            ["redraw", "scroll", "snapshot", "append"]
        } else {
            ["append", "snapshot", "scroll", "redraw"]
        };
        for operation in operations {
            let start = Instant::now();
            // App::flush_effects automatically renders the dirty test window
            // before update_window returns: no redundant explicit draw here.
            cx.update_window(window.into(), |_, _, cx| {
                view.update(cx, |view, cx| {
                    match operation {
                        "snapshot" | "append" => {
                            let mut state = original.as_ref().clone();
                            state.model = Some(format!("perf-model-{sample}"));
                            if operation == "append" {
                                let text = state.saved.items[last_prose]["text"]
                                    .as_str()
                                    .unwrap()
                                    .to_owned();
                                state.saved.items[last_prose]["text"] =
                                    json!(format!("{text}\n\nPERF_COMPLETED_APPEND_{sample}"));
                            }
                            view.session.update(cx, |session, cx| {
                                session.state = state.into();
                                cx.notify();
                            });
                        }
                        "scroll" => {
                            view.scroll.set_offset(gpui::point(
                                gpui::px(0.),
                                gpui::px(if sample % 2 == 0 { -200. } else { -400. }),
                            ));
                        }
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .unwrap();
            let update_elapsed = start.elapsed();
            cx.run_until_parked();
            let settle_elapsed = start.elapsed();
            cx.update_window(window.into(), |_, _, cx| {
                let view = view.read(cx);
                assert_eq!(view.session.read(cx).state.saved.items.len(), item_count);
                assert!(view.session.read(cx).state.error.is_none());
                assert!(view.scroll.bounds().size.height > gpui::px(0.));
                if operation == "append" {
                    assert!(
                        view.selection_views.values().any(|owner| owner
                            .read(cx)
                            .source()
                            .contains(&format!("PERF_COMPLETED_APPEND_{sample}"))),
                        "appended content must reach a native CoreText owner"
                    );
                }
            })
            .unwrap();
            println!(
                "TRANSCRIPT_PERF {}",
                json!({"workload":workload,"operation":operation,"sample":sample,"ms":update_elapsed.as_secs_f64()*1000.,"update_ms":update_elapsed.as_secs_f64()*1000.,"settle_ms":settle_elapsed.as_secs_f64()*1000.,"items":item_count,"completed":true,"measurement":"macos_coretext_cpu_update_and_auto_draw"})
            );
        }
    }
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
