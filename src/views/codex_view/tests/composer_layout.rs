use super::*;

#[gpui::test]
fn transcript_and_composer_edges_match_with_navigation_at_each_width_and_zoom(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for has_navigation in [false, true] {
            cx.update(|_, cx| {
                    cx.set_reduce_motion(true);
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.saved.items = if has_navigation {
                                vec![
                                    json!({"id":"align-user", "type":"userMessage", "content":[{"text":"Check alignment"}]}),
                                    json!({"id":"align-agent", "type":"agentMessage", "text":"Aligned response"}),
                                ].into()
                            } else {
                                vec![json!({"id":"align-agent", "type":"agentMessage", "text":"Aligned response"})].into()
                            };
                            cx.notify();
                        });
                    });
                });
            for (width, rem) in [(1200.0, 16.0), (600.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
                cx.update(|_, cx| {
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                });
                draw(cx);
                let anchor = cx
                    .debug_bounds(if has_navigation {
                        "codex-message-anchor-1"
                    } else {
                        "codex-message-anchor-0"
                    })
                    .unwrap();
                let composer = cx.debug_bounds("codex-composer").unwrap();
                assert!(
                        (anchor.left() - composer.left()).abs() <= gpui::px(1.0),
                        "left edge: {provider:?}, navigation={has_navigation}, width={width}, rem={rem}: {anchor:?} vs {composer:?}"
                    );
                assert!(
                        (anchor.right() - composer.right()).abs() <= gpui::px(1.0),
                        "right edge: {provider:?}, navigation={has_navigation}, width={width}, rem={rem}: {anchor:?} vs {composer:?}"
                    );
            }
        }
    }
}

#[gpui::test]
fn minimap_stays_anchored_when_tasks_and_composer_expand(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        cx.set_reduce_motion(true);
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"u1", "type":"userMessage", "content":[{"text":"First input"}]}),
                    json!({"id":"a1", "type":"agentMessage", "text":"First response"}),
                    json!({"id":"u2", "type":"userMessage", "content":[{"text":"Second input"}]}),
                    json!({"id":"tasks", "type":"plan", "plan":[
                            {"step":"Inspect", "status":"completed"},
                            {"step":"Implement", "status":"pending"},
                            {"step":"Verify", "status":"pending"}]}),
                ]
                .into();
                cx.notify();
            });
        });
    });
    for width in [1000.0, 400.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.tasks_expanded = false;
                view.composer_collapsed = true;
                view.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
        });
        draw(cx);
        let rail = minimap_bounds(cx, "codex-message-minimap").unwrap();
        let marker = minimap_bounds(cx, "codex-minimap-message-0").unwrap();
        let transcript = cx.debug_bounds("codex-messages").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        cx.update(|_, cx| view.update(cx, CodexView::toggle_tasks));
        draw(cx);
        assert!(cx.debug_bounds("codex-messages").unwrap().size.height < transcript.size.height);
        assert_eq!(minimap_bounds(cx, "codex-message-minimap").unwrap(), rail);
        assert_eq!(
            minimap_bounds(cx, "codex-minimap-message-0").unwrap(),
            marker
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.composer_collapsed = false;
                view.input.update(cx, |input, cx| {
                    input.set_value("A longer draft\n".repeat(20), window, cx);
                });
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-composer").unwrap().size.height > composer.size.height);
        assert_eq!(minimap_bounds(cx, "codex-message-minimap").unwrap(), rail);
        assert_eq!(
            minimap_bounds(cx, "codex-minimap-message-0").unwrap(),
            marker
        );
    }
}

#[gpui::test]
fn transcript_motion_coalesces_display_frames_and_stops_with_reduced_motion(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.items =
                    vec![json!({"id":"answer", "type":"agentMessage", "text":"old"})].into();
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.update(gpui::Window::simulate_next_frame);
    draw(cx);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let watch = cx.update(|_, cx| {
        cx.observe(&view, move |_, _| {
            observed.set(observed.get() + 1);
        })
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.saved.items[0]["text"] = json!(format!("old{}", "界".repeat(80)));
            });
            // Follow and reveal both need a frame. Repeated renders must share it.
            let _ = view.render(window, cx);
            let _ = view.render(window, cx);
            assert!(view.smooth_follow);
            assert!(view.stream_text["answer"].1 < view.stream_text["answer"].0.len());
            assert!(view.motion_frame_pending.get());
        });
    });
    notifications.set(0);
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        assert!(!view.read(cx).motion_frame_pending.get());
    });
    // Observe this one display callback before pumping the executor, which
    // deliberately drains every subsequent animation frame in GPUI tests.
    assert_eq!(
        notifications.get(),
        1,
        "follow and reveal share one notification"
    );
    cx.update(|_, cx| cx.set_reduce_motion(true));
    draw(cx);
    // Any already queued final follow frame settles with reduced motion.
    cx.update(gpui::Window::simulate_next_frame);
    draw(cx);
    notifications.set(0);
    for _ in 0..3 {
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    assert_eq!(notifications.get(), 0);
    view.read_with(cx, |view, _| {
        assert!(!view.motion_frame_pending.get());
        assert_eq!(
            view.stream_text["answer"].1,
            view.stream_text["answer"].0.len()
        );
    });
    drop(watch);
}

#[gpui::test]
fn empty_sessions_show_intro_above_bottom_composer(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.provider = provider;
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-empty").is_none());
        assert!(cx.debug_bounds("codex-empty-intro").is_some());
        assert!(cx.debug_bounds("codex-starter-review").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
    }
}

#[gpui::test]
fn context_meter_mounts_beside_composer_actions_only_when_usage_is_known(
    cx: &mut gpui::TestAppContext,
) {
    let (_view, cx, _, updates) = fixture(cx);
    draw(cx);
    assert!(cx.debug_bounds("codex-add").is_some());
    assert!(cx.debug_bounds("codex-context-meter").is_none());
    let mut state = snapshot();
    state.saved.context = Some(codex::ContextUsage {
        used: 76_000,
        window: 200_000,
    });
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let meter = cx.debug_bounds("codex-context-meter").unwrap();
    assert!(cx.debug_bounds("codex-composer-context").is_none());
    let strip = cx.debug_bounds("codex-composer").unwrap();
    let add = cx.debug_bounds("codex-add").unwrap();
    assert!(meter.right() <= add.left());
    assert!(meter.top() < add.bottom() && meter.bottom() > add.top());
    assert!(meter.left() >= strip.left() && meter.right() <= strip.right());
    assert!(meter.top() >= strip.top() && meter.bottom() <= strip.bottom());
    let mut state = snapshot();
    state.saved.context = Some(codex::ContextUsage { used: 1, window: 0 });
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-context-meter").is_none());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native X11 composer focus fixture"]
fn composer_ambient_focus_native_capture() {
    assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
    let output = std::path::PathBuf::from(std::env::var("GROVE_FOCUS_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute());
    fs_err::create_dir_all(&output).unwrap();
    let dashboard = std::env::var_os("GROVE_FOCUS_DASHBOARD").is_some();
    let animate = std::env::var_os("GROVE_FOCUS_ANIMATE").is_some();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_global(crate::settings::SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_reduce_motion(!animate);
            c::sync_component_theme(cx);
            let (handle, actions) = Handle::channel();
            let (updates, receive) = mpsc::channel();
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    gpui::point(gpui::px(30.), gpui::px(30.)), gpui::size(gpui::px(900.), gpui::px(700.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove composer focus QA");
                let mut state = snapshot();
                state.compacting = std::env::var_os("GROVE_FOCUS_COMPACTING").is_some();
                state.status = if state.compacting { Status::Working } else { Status::Done };
                state.saved.items = vec![json!({"id":"answer","type":"agentMessage","text":"Ready for your next instruction."})].into();
                if let Ok(phase) = std::env::var("GROVE_FOCUS_QUEUE_PHASE") {
                    assert!(matches!(phase.as_str(), "queued" | "completed" | "dispatched"));
                    state.compacting = phase == "queued";
                    state.status = if phase == "completed" {Status::Done} else {Status::Working};
                    if phase == "dispatched" {
                        state.saved.items.push(json!({"id":"queued-input","type":"userMessage","content":[{"type":"text","text":"Run this after compaction"}]}));
                    } else {
                        state.queued_prompts.push(codex::QueuedPrompt {id:41,text:"Run this after compaction".into(),images:vec![]});
                    }
                }
                if dashboard { state.saved.items = vec![].into(); }
                let session = cx.new(|cx| CodexSession::from_connection(state, Connection { handle, updates: receive.into() }, cx));
                let chat = cx.new(|cx| CodexView::new(session, window, cx));
                if dashboard {
                    chat.update(cx, |view, cx| {
                        view.dashboard_tray = Some(cx.new(|_| {
                            crate::views::sidebar::dashboard::DestinationTray::picker_test_tray("/unused".into())
                        }));
                    });
                }
                retained = Some(chat.clone());
                cx.new(|cx| gpui_component::Root::new(chat, window, cx).bordered(false))
            }).unwrap();
            let chat = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let _transport = (actions, updates);
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    let cases = if std::env::var_os("GROVE_FOCUS_QUEUE_PHASE").is_some() {vec![("grove-dark",900.,16.)]} else {vec![("grove-dark",900.,16.),("grove-light",900.,16.),("tokyo-night",900.,16.),("grove-dark",540.,20.)]};
                    for (theme, width, rem) in cases {
                        cx.update(|cx| { grove_core::theme::set_by_name(theme); c::sync_component_theme(cx); gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem); });
                        for focused in [true, false] {
                            cx.update_window(window.into(), |_, window, cx| {
                                window.resize(gpui::size(gpui::px(width),gpui::px(700.)));
                                window.set_rem_size(gpui::px(rem));
                                chat.update(cx, |view, cx| {
                                    view.session.update(cx, |session, cx| { session.state.saved.provider = provider; cx.notify(); });
                                    view.input.update(cx, |input, cx| input.set_value("Refine the composer focus treatment.",window,cx));
                                    if focused { view.input.focus_handle(cx).focus(window,cx); }
                                    else { view.option_focus[0].focus(window,cx); }
                                    assert_eq!(view.input.focus_handle(cx).is_focused(window), focused);
                                    cx.notify();
                                });
                                window.refresh();
                            }).unwrap();
                            cx.background_executor().timer(std::time::Duration::from_millis(350)).await;
                            cx.update_window(window.into(), |_, window, _| {
                                eprintln!("focus capture {provider:?} {theme}: viewport={:?}, rem={:?}", window.viewport_size(), window.rem_size());
                            }).unwrap();
                            // Flush theme/zoom changes before sampling the X11 backing pixmap.
                            cx.update_window(window.into(), |_, window, _| window.refresh()).unwrap();
                            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                            let path = output.join(format!("{provider:?}-{theme}-{width}-{rem}-{focused}.png"));
                            assert!(std::process::Command::new("import").args(["-window", "Grove composer focus QA"]).arg(path).status().unwrap().success());
                            if animate && focused && theme == "grove-dark" && rem == 16. {
                                for phase in ["peak", "trough"] {
                                    cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
                                    cx.update_window(window.into(), |_, window, _| window.refresh()).unwrap();
                                    cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                                    let path = output.join(format!("{provider:?}-pulse-{phase}.png"));
                                    assert!(std::process::Command::new("import").args(["-window", "Grove composer focus QA"]).arg(path).status().unwrap().success());
                                }
                                cx.update_window(window.into(), |_, window, cx| chat.read_with(cx, |view, cx| {
                                    assert!(view.input.focus_handle(cx).is_focused(window));
                                    assert_eq!(view.input.read(cx).value().to_string(), "Refine the composer focus treatment.");
                                })).unwrap();
                            }
                        }
                    }
                }
                cx.update_window(window.into(), |_, window, _| {window.blur();window.refresh();}).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
                drop(chat);
                cx.update(|cx| cx.quit());
            }).detach();
        });
}

#[gpui::test]
fn task_strip_docks_above_composer_toggles_and_jumps_to_task_start(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
    draw(cx);
    assert!(cx.debug_bounds("codex-task-strip").is_none());
    cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![
                        json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
                        json!({"id":"a0", "type":"agentMessage", "text":"Long reply.\n\n".repeat(60)}),
                        json!({"id":"grove-plan-t1", "type":"plan", "text":"", "explanation":"Keep the task register aligned.", "plan":[
                            {"step":"Inspect","status":"completed"},
                            {"step":"Implement","status":"inProgress"},
                            {"step":"Test","status":"pending"}]}),
                        json!({"id":"a1", "type":"agentMessage", "text":"More work.\n\n".repeat(60)}),
                    ].into();
                    cx.notify();
                });
            });
        });
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.tasks_expanded));
    assert!(cx.debug_bounds("codex-task-row-2").is_some());
    // Both providers use this native checklist; fixtures exercise rendering,
    // not live provider transport. Narrow widths and rem zoom keep its columns.
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (width, rem) in [(1000., 16.), (600., 16.), (600., 20.)] {
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        cx.notify();
                    });
                });
            });
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            draw(cx);
            let label = cx.debug_bounds("codex-task-header-label").unwrap();
            let mark = cx.debug_bounds("codex-task-header-mark").unwrap();
            for selector in [
                "codex-task-title-0",
                "codex-task-title-1",
                "codex-task-title-2",
                "codex-task-explanation-text",
            ] {
                assert_eq!(cx.debug_bounds(selector).unwrap().left(), label.left());
            }
            for selector in [
                "codex-task-row-status-0",
                "codex-task-row-status-1",
                "codex-task-row-status-2",
            ] {
                assert_eq!(cx.debug_bounds(selector).unwrap().left(), mark.left());
            }
        }
    }
    cx.update(|window, cx| {
        window.set_rem_size(gpui::px(16.));
        gpui_component::Theme::global_mut(cx).font_size = gpui::px(16.);
    });
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    draw(cx);
    let initial_header = cx.debug_bounds("codex-task-strip").unwrap();
    cx.simulate_click(initial_header.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(!view.read_with(cx, |view, _| view.tasks_expanded));
    let strip = cx.debug_bounds("codex-task-strip-panel").unwrap();
    let composer = cx.debug_bounds("codex-composer").unwrap();
    let header = cx.debug_bounds("codex-task-strip").unwrap();
    let rem_size = cx.update(|window, _| window.rem_size());
    assert_eq!(header.size.height, rpx(CONTROL_H).to_pixels(rem_size));
    assert_eq!(
        header.top() - strip.top(),
        rpx(SPACE_LG).to_pixels(rem_size) + gpui::px(1.0),
        "the header keeps the tray's top padding inside its border"
    );
    assert_eq!(
        header.top() - strip.top(),
        composer.top() - header.bottom(),
        "the collapsed header has equal visible top and bottom gaps"
    );
    assert!(strip.top() < composer.top());
    assert!(
        strip.left() > composer.left(),
        "tasks share the queue tray inset"
    );
    assert!(
        strip.right() < composer.right(),
        "tasks stay inside the composer rail"
    );
    assert!(cx.debug_bounds("codex-task-row-0").is_none());
    assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
    // Click and keyboard both toggle the in-memory expansion.
    let header = cx.debug_bounds("codex-task-strip").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.tasks_expanded));
    assert!(cx.debug_bounds("codex-task-row-2").is_some());
    let expanded = cx.debug_bounds("codex-task-strip-panel").unwrap();
    assert!(expanded.top() < strip.top(), "the strip grows upward");
    let expanded_header = cx.debug_bounds("codex-task-strip").unwrap();
    assert_eq!(
        header.top() - strip.top(),
        expanded_header.top() - expanded.top(),
        "collapsed and expanded headers keep the same top corner inset"
    );
    assert_eq!(
        header.left() - strip.left(),
        expanded_header.left() - expanded.left(),
        "collapsed and expanded headers keep the same horizontal corner inset"
    );
    let row = cx.debug_bounds("codex-task-row-1").unwrap();
    let jump = cx.debug_bounds("codex-task-jump-1").unwrap();
    let title = cx.debug_bounds("codex-task-title-1").unwrap();
    assert!(jump.right() <= row.right());
    assert!(
        title.right() <= jump.left(),
        "revealed jump action cannot cover the task title"
    );
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
    draw(cx);
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    // The jump resolves against the next layout after composer collapse.
    for _ in 0..3 {
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    let transcript = cx.debug_bounds("codex-messages").unwrap();
    let anchor = cx.debug_bounds("codex-message-anchor-3").unwrap();
    assert!(
            (anchor.top() - transcript.top()).abs() < gpui::px(1.0),
            "jump aligns the first transcript content after the hidden task update: anchor={anchor:?}, transcript={transcript:?}"
        );
    assert!(view.read_with(cx, |view, _| view.composer_collapsed));
    // Answering a clarification keeps the session checklist current.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.tasks_expanded = false;
            view.session.update(cx, |session, cx| {
                session.state.saved.items.push(
                    json!({"id":"u2", "type":"userMessage", "content":[{"text":"Both equally"}]}),
                );
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-task-strip").is_some());
    assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
    let list = view.read_with(cx, |view, _| {
        view.task_cache.as_ref().unwrap().list.clone().unwrap()
    });
    assert_eq!((list.done(), list.total()), (1, 3));
    assert_eq!(list.now_label(), Some("Implement"));
}

#[gpui::test]
fn composer_destination_strip_is_centered_at_ninety_five_percent(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.dashboard_tray = Some(cx.new(|_| {
                crate::views::sidebar::dashboard::DestinationTray::picker_test_tray(
                    "/unused".into(),
                )
            }));
            cx.notify();
        });
    });
    draw(cx);
    let input = cx.debug_bounds("codex-composer").unwrap();
    let strip = cx.debug_bounds("dashboard-destination").unwrap();
    assert!(
        (f32::from(strip.size.width)
            - f32::from(input.size.width) * COMPOSER_CONTEXT_WIDTH_FRACTION)
            .abs()
            <= 1.0
    );
    assert!((f32::from(strip.center().x - input.center().x)).abs() <= 1.0);
}

#[gpui::test]
fn composer_grows_to_limit_and_shrinks_after_clearing(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    draw(cx);
    let height = |cx: &mut gpui::VisualTestContext| {
        cx.debug_bounds("codex-composer-input").unwrap().size.height
    };
    let initial = height(cx);
    let set = |text: String, cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            });
        });
        draw(cx);
    };
    set("line\n".repeat(6), cx);
    let grown = height(cx);
    assert!(
        grown > initial,
        "typing multiline content must grow the visible composer"
    );
    set("line\n".repeat(14), cx);
    let capped = height(cx);
    assert!(capped > grown);
    set("line\n".repeat(30), cx);
    assert_eq!(height(cx), capped, "long drafts must stop growing");
    set(String::new(), cx);
    assert_eq!(
        height(cx),
        initial,
        "clearing restores compact composer height"
    );
}
