/// Additional final-only stress: visit distant history positions, rather than
/// repeatedly scrolling an already painted top viewport. This has no comparative
/// speedup denominator and does not change the frozen near-top benchmark.
#[test]
#[ignore = "manual native history-spread stress; optional Metal screenshots"]
fn transcript_native_spread_benchmark() {
    use gpui::AppContext as _;
    let force_eager_rows = std::env::var("GROVE_PERF_EAGER_ROWS").as_deref() == Ok("1");
    crate::views::codex_view::measured_slot::FORCE_EAGER.with(|flag| flag.set(force_eager_rows));
    let workload = std::env::var("GROVE_PERF_WORKLOAD").unwrap_or_else(|_| "many-2000".into());
    let state = workload_state(&workload);
    let expected = state.saved.items.len();
    let last_prose = state
        .saved
        .items
        .iter()
        .rposition(|item| {
            item["type"] == "agentMessage" && !item["text"].as_str().unwrap_or_default().is_empty()
        })
        .unwrap();
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
    let width = std::env::var("GROVE_PERF_WIDTH")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1000.);
    let (handle, _actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let connection = Connection {
        handle,
        updates: receive.into(),
    };
    let mut retained = None;
    let window = cx
        .open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
            let session = cx.new(|cx| CodexSession::from_connection(state, connection, cx));
            let chat = cx.new(|cx| CodexView::new(session, window, cx));
            if workload == "real" {
                chat.update(cx, |view, cx| {
                    let key = work_key_at(&view.session.read(cx).state, last_prose);
                    view.expanded.insert(key);
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
    for _ in 0..3 {
        cx.update_window(window.into(), |_, _, cx| {
            view.update(cx, |_, cx| cx.notify());
        })
        .unwrap();
        cx.run_until_parked();
    }
    cx.update_window(window.into(), |_, window, cx| {
        println!("TRANSCRIPT_METADATA {}", json!({"a11y_active": window.is_a11y_active(), "reduce_motion": cx.reduce_motion(), "forced_row_header_footer_eager": force_eager_rows, "vendor_body_override": false, "native_text_system": "CoreText", "width": width}));
    }).unwrap();
    for sample in 0..samples() {
        let fraction = [0., 0.13, 0.5, 0.87, 1., 0.37, 0.65][sample % 7];
        let mut requested = gpui::Pixels::ZERO;
        let start = Instant::now();
        cx.update_window(window.into(), |_, _, cx| {
            view.update(cx, |view, cx| {
                view.follow_after_submit = false;
                view.smooth_follow = false;
                view.composer_pin_bottom = false;
                requested = -view.scroll.max_offset().y * fraction;
                view.scroll.set_offset(gpui::point(gpui::px(0.), requested));
                cx.notify();
            });
        })
        .unwrap();
        let elapsed = start.elapsed();
        cx.run_until_parked();
        let settled = start.elapsed();
        let actual = cx
            .update_window(window.into(), |_, _, cx| {
                let view = view.read(cx);
                assert_eq!(view.session.read(cx).state.saved.items.len(), expected);
                assert!(view.session.read(cx).state.error.is_none());
                assert!(view.scroll.bounds().size.height > gpui::px(0.));
                assert!(
                    (view.scroll.offset().y - requested).abs() < gpui::px(1.),
                    "spread scrolling must paint the requested position, fraction={fraction}"
                );
                view.scroll.offset().y
            })
            .unwrap();
        if sample < 7 {
            if let Ok(directory) = std::env::var("GROVE_PERF_SCREENSHOT_DIR") {
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(
                        std::path::Path::new(&directory)
                            .join(format!("spread-{workload}-{sample}.png")),
                    )
                    .unwrap();
            }
        }
        println!(
            "TRANSCRIPT_SPREAD {}",
            json!({"workload":workload,"sample":sample,
            "fraction":fraction,"requested_offset":f32::from(requested),"actual_offset":f32::from(actual),
            "update_ms":elapsed.as_secs_f64()*1000.,"settle_ms":settled.as_secs_f64()*1000.,
            "items":expected,"completed":true,"measurement":"macos_coretext_cpu_history_spread_scroll"})
        );
    }
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
