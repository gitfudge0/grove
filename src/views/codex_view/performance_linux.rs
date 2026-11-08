/// Native Linux text shaping and scene construction. Samples exclude GPU submission,
/// compositor presentation, timer waits, and screenshot capture; they are not FPS.
#[test]
#[ignore = "manual isolated Linux native transcript CPU workload"]
fn transcript_linux_frame_benchmark() {
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "use isolated configuration"
    );
    let workload = std::env::var("GROVE_PERF_WORKLOAD").unwrap_or_else(|_| "real".into());
    let original = workload_state(&workload);
    let item_count = original.saved.items.len();
    let sample_count = samples();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_global(crate::settings::SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_reduce_motion(true);
        c::sync_component_theme(cx);
        let (handle, actions) = Handle::channel();
        let (updates, receive) = mpsc::channel();
        let connection = Connection { handle, updates: receive.into() };
        let mut retained = None;
        let window = cx.open_window(gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                gpui::point(gpui::px(40.), gpui::px(40.)),
                gpui::size(gpui::px(1000.), gpui::px(900.)),
            ))),
            ..Default::default()
        }, |window, cx| {
            window.set_window_title("Grove performance QA");
            let session = cx.new(|cx| CodexSession::from_connection(original, connection, cx));
            let chat = cx.new(|cx| CodexView::new(session, window, cx));
            chat.update(cx, |view, cx| {
                let state = view.session.read(cx).state.shared();
                view.turn_history.expand_all_for_test(&state.saved.items);
                if std::env::var_os("GROVE_PERF_EXPAND_ALL").is_some() {
                    let state = view.session.read(cx).state.shared();
                    for index in 0..state.saved.items.len() {
                        view.expanded.insert(work_key_at(&state, index));
                    }
                }
            });
            retained = Some(chat.clone());
            cx.new(|cx| gpui_component::Root::new(chat, window, cx))
        }).unwrap();
        let chat = retained.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let _keep_transport = (actions, updates);
            cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
            for sample in 0..sample_count + 5 {
                cx.update_window(window.into(), |_, _, cx| {
                    chat.update(cx, |view, cx| {
                        let extent = view.scroll.max_offset().y;
                        let fraction = [0., 0.25, 0.5, 0.75, 1.][sample % 5];
                        view.scroll.set_offset(gpui::point(gpui::px(0.), -extent * fraction));
                        cx.notify();
                    });
                }).unwrap();
                let result = cx.update_window(window.into(), |_, window, cx| {
                    // Notify effects have flushed at the previous update boundary.
                    // A global refresh would bypass GPUI retained view caches.
                    super::super::super::native_plan::take_materialization_count();
                    super::super::super::native_plan::take_control_materialization_count();
                    super::super::super::mermaid::take_materialization_count();
                    let start = Instant::now();
                    let arena = window.draw(cx);
                    let elapsed = start.elapsed();
                    let plan_materializations = super::super::super::native_plan::take_materialization_count();
                    let control_materializations = super::super::super::native_plan::take_control_materialization_count();
                    let diagram_materializations = super::super::super::mermaid::take_materialization_count();
                    arena.clear();
                    let view = chat.read(cx);
                    assert_eq!(view.session.read(cx).state.saved.items.len(), item_count);
                    assert!(view.session.read(cx).state.error.is_none());
                    assert!(view.scroll.bounds().size.height > gpui::px(0.));
                    assert!(view.scroll.children_count() > 0);
                    assert_eq!(view.turn_history.start, 0, "native workload must expose full history");
                    (elapsed, view.scroll.offset().y, view.scroll.max_offset().y, window.viewport_size(), plan_materializations, control_materializations, diagram_materializations)
                }).unwrap();
                if sample >= 5 {
                    println!("TRANSCRIPT_PERF {}", json!({
                        "workload":workload,"operation":"scroll","sample":sample-5,
                        "ms":result.0.as_secs_f64()*1000.,"items":item_count,"completed":true,
                        "offset_y":f32::from(result.1),"max_offset_y":f32::from(result.2),
                        "viewport_width":f32::from(result.3.width),"viewport_height":f32::from(result.3.height),
                        "plan_materializations":result.4,"plan_control_materializations":result.5,"diagram_materializations":result.6,
                        "measurement":"linux_native_text_cpu_scene_build","compositor_fps_measured":false
                    }));
                }
                // Let the native loop submit each completed scene and process follow-up layout.
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            }
            if let Some(events) = std::env::var("GROVE_PERF_WHEEL_EVENTS").ok().and_then(|n| n.parse::<usize>().ok()).filter(|n| *n >= 2 && n % 2 == 0) {
                cx.update_window(window.into(), |_, _, cx| {
                    chat.update(cx, |view, cx| {
                        view.scroll.set_offset(gpui::Point::default());
                        cx.notify();
                    });
                }).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                gpui::set_frame_trace_enabled(true);
                let mut collector = gpui::FrameTimingCollector::new();
                let started = Instant::now();
                let mut furthest = gpui::px(0.);
                for event in 0..events {
                    cx.update_window(window.into(), |_, window, cx| {
                        let position = chat.read(cx).scroll.bounds().center();
                        window.dispatch_event(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                            position,
                            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(if event < events / 2 { -24. } else { 24. }))),
                            touch_phase: if event == 0 { gpui::TouchPhase::Started } else if event + 1 == events { gpui::TouchPhase::Ended } else { gpui::TouchPhase::Moved },
                            ..Default::default()
                        }), cx);
                        furthest = furthest.min(chat.read(cx).scroll.offset().y);
                    }).unwrap();
                    // Let the native event loop render normally: no manual draw
                    // or global refresh is issued for these wheel events.
                    let deadline = started + std::time::Duration::from_millis((event as u64 + 1) * 16);
                    cx.background_executor().timer(deadline.saturating_duration_since(Instant::now())).await;
                }
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let frames = collector.collect_unseen();
                gpui::set_frame_trace_enabled(false);
                assert!(!frames.is_empty(), "native wheel events must produce frames");
                assert!(furthest < gpui::px(-100.), "wheel input must scroll transcript content");
                let final_offset = cx.update_window(window.into(), |_, _, cx| {
                    let view = chat.read(cx);
                    assert_eq!(view.session.read(cx).state.saved.items.len(), item_count);
                    view.scroll.offset().y
                }).unwrap();
                assert!(final_offset.abs() < gpui::px(1.), "reverse wheel input must return to the start");
                for (index, frame) in frames.iter().enumerate() {
                    println!("TRANSCRIPT_WHEEL_FRAME {}", json!({
                        "sample":index, "ms":frame.draw_duration().as_secs_f64()*1000.,
                        "dirty_to_draw_ms":frame.dirty_to_draw_duration().map(|d|d.as_secs_f64()*1000.),
                        "draw_interval_ms":index.checked_sub(1).map(|previous|frame.draw_start.duration_since(frames[previous].draw_start).as_secs_f64()*1000.),
                        "invalidations":frame.invalidations,
                        "measurement":"native_event_loop_cpu_draw", "compositor_fps_measured":false
                    }));
                }
                println!("TRANSCRIPT_WHEEL_CHECK {}", json!({"events":events,"frames":frames.len(),"elapsed_ms":started.elapsed().as_secs_f64()*1000.,"furthest_y":f32::from(furthest),"final_y":f32::from(final_offset),"items":item_count,"completed":true}));
            }
            if let Ok(path) = std::env::var("GROVE_PERF_SCREENSHOT") {
                assert!(std::process::Command::new("import").args(["-window", "Grove performance QA", &path]).status().unwrap().success());
                eprintln!("Native performance capture: {path}");
            }
            cx.update_window(window.into(), |_, window, _| { window.blur(); window.refresh(); }).unwrap();
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
            drop(chat);
            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
