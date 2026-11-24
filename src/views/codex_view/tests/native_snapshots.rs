//! Native screenshots for conversation states on the supported renderer.

use super::*;

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn speed_card_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    let previous_theme = grove_core::theme::current().name.clone();
    for appearance in ["dark", "light"] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&format!(
                "grove-{appearance}"
            )));
            c::sync_component_theme(cx);
        });
        for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                for available in [true, false] {
                    let mut state = snapshot();
                    state.saved.provider = provider;
                    state.saved.draft = "Compare the two refresh paths.".into();
                    state.saved.items.clear();
                    state.saved.composer.speed = Some(codex::Speed::Fast);
                    state.model = Some("speed-fixture".into());
                    state.effort = Some("high".into());
                    state.models = vec![
                        json!({"model":"speed-fixture", "displayName":if available {"Selected model"} else {"A very long configured model name with unavailable fast mode"}, "supportsFastMode":available, "serviceTiers":if available {vec![json!({"id":"fast"})]} else {vec![]}, "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}),
                    ].into();
                    if provider == codex::Provider::Claude && available {
                        state.reported_speed = Some("cooldown".into());
                    }
                    let (handle, _actions) = Handle::channel();
                    let (_updates, receive) = mpsc::channel();
                    let connection = Connection {
                        handle,
                        updates: receive.into(),
                    };
                    let mut retained = None;
                    let window = cx
                        .open_window(
                            gpui::size(gpui::px(width), gpui::px(720.0)),
                            |window, cx| {
                                window.set_rem_size(gpui::px(rem));
                                let session = cx
                                    .new(|cx| CodexSession::from_connection(state, connection, cx));
                                let chat = cx.new(|cx| CodexView::new(session, window, cx));
                                retained = Some(chat.clone());
                                cx.new(|cx| {
                                    gpui_component::Root::new(chat, window, cx)
                                        .bordered(false)
                                        .bg(c::BG())
                                })
                            },
                        )
                        .unwrap();
                    cx.update_window(window.into(), |_, window, cx| {
                        retained.as_ref().unwrap().update(cx, |view, cx| {
                            view.open_menu(ComposerMenu::Settings, window, cx);
                        });
                    })
                    .unwrap();
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    cx.update_window(window.into(), |_, _window, cx| {
                        let view = retained.as_ref().unwrap().read(cx);
                        assert_eq!(
                            view.input.read(cx).value().as_ref(),
                            "Compare the two refresh paths."
                        );
                    })
                    .unwrap();
                    let path = format!(
                        "/tmp/grove-speed-{appearance}-{provider:?}-{width:.0}-{rem:.0}-{available}.png"
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native speed card fixture: {path}");
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
    }
    cx.update(|cx| {
        assert!(grove_core::theme::set_by_name(&previous_theme));
        c::sync_component_theme(cx);
    });
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn effort_card_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    let previous_theme = grove_core::theme::current().name.clone();
    for appearance in ["dark", "light"] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&format!(
                "grove-{appearance}"
            )));
            c::sync_component_theme(cx);
        });
        for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
            cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                for effort in ["low", "high"] {
                    let mut state = snapshot();
                    state.saved.provider = provider;
                    state.saved.draft = "Compare the two refresh paths.".into();
                    state.saved.items.clear();
                    state.saved.composer.speed = Some(codex::Speed::Fast);
                    state.model = Some("speed-fixture".into());
                    state.effort = Some(effort.into());
                    state.models = vec![
                        json!({"model":"speed-fixture", "displayName":"Selected model", "supportsFastMode":true, "serviceTiers":vec![json!({"id":"fast"})], "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}),
                    ].into();
                    if provider == codex::Provider::Claude {
                        state.reported_speed = Some("cooldown".into());
                    }
                    let (handle, _actions) = Handle::channel();
                    let (_updates, receive) = mpsc::channel();
                    let connection = Connection {
                        handle,
                        updates: receive.into(),
                    };
                    let mut retained = None;
                    let window = cx
                        .open_window(
                            gpui::size(gpui::px(width), gpui::px(720.0)),
                            |window, cx| {
                                window.set_rem_size(gpui::px(rem));
                                let session = cx
                                    .new(|cx| CodexSession::from_connection(state, connection, cx));
                                let chat = cx.new(|cx| CodexView::new(session, window, cx));
                                retained = Some(chat.clone());
                                cx.new(|cx| {
                                    gpui_component::Root::new(chat, window, cx)
                                        .bordered(false)
                                        .bg(c::BG())
                                })
                            },
                        )
                        .unwrap();
                    cx.update_window(window.into(), |_, window, cx| {
                        retained.as_ref().unwrap().update(cx, |view, cx| {
                            view.open_menu(ComposerMenu::Settings, window, cx);
                        });
                    })
                    .unwrap();
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    cx.update_window(window.into(), |_, window, cx| {
                        assert_eq!(
                            window.rem_size(),
                            gpui::px(rem),
                            "native fixture must render the requested zoom"
                        );
                        let view = retained.as_ref().unwrap().read(cx);
                        assert_eq!(
                            view.input.read(cx).value().as_ref(),
                            "Compare the two refresh paths."
                        );
                    })
                    .unwrap();
                    let path = format!(
                        "/tmp/grove-effort-{appearance}-{provider:?}-{width:.0}-{rem:.0}-{effort}.png"
                    );
                    let screenshot = cx.capture_screenshot(window.into()).unwrap();
                    screenshot.save(&path).unwrap();
                    if effort == "low" {
                        // The larger thumb fully covers a correctly rounded track cap.
                        // A short fill with radius clamped by its width leaks a crescent.
                        let (bounds, viewport, blue) = cx
                            .update_window(window.into(), |_, window, cx| {
                                let bounds =
                                    retained.as_ref().unwrap().read(cx).effort_bounds.get();
                                (bounds, window.viewport_size(), gpui::Rgba::from(c::BLUE()))
                            })
                            .unwrap();
                        let scale = screenshot.width() as f32 / f32::from(viewport.width);
                        let inset = ICON_MD * rem / crate::zoom::REM_BASE;
                        let blue_pixels = screenshot
                            .enumerate_pixels()
                            .filter(|(x, y, pixel)| {
                                let x = *x as f32 / scale;
                                let y = *y as f32 / scale;
                                x >= f32::from(bounds.left()) - inset
                                    && x <= f32::from(bounds.right()) + inset
                                    && y >= f32::from(bounds.top()) - inset
                                    && y <= f32::from(bounds.bottom()) + inset
                                    && (f32::from(pixel[0]) / 255.0 - blue.r).abs() < 0.02
                                    && (f32::from(pixel[1]) / 255.0 - blue.g).abs() < 0.02
                                    && (f32::from(pixel[2]) / 255.0 - blue.b).abs() < 0.02
                            })
                            .count();
                        assert_eq!(
                            blue_pixels, 0,
                            "low effort must not leak a blue cap behind the thumb: {path}"
                        );
                    }
                    eprintln!("Native effort card fixture: {path}");
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
    }
    cx.update(|cx| {
        assert!(grove_core::theme::set_by_name(&previous_theme));
        c::sync_component_theme(cx);
    });
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn composer_markdown_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for width in [1024.0, 600.0, 360.0] {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            state.saved.items = vec![
                json!({"type":"agentMessage", "id":"snippet", "text":"Copy this code:\n\n```sh\n  cargo test --workspace\n```", "groveCompleted":true}),
            ].into();
            state.saved.draft = "## Heading 2\n- This is a **bold** list item\n- Second item with `code` and [link](https://example.com)\n1. Ordered 🦀 é item\nNormal paragraph after heading and lists, with enough additional Unicode text é🦀 to wrap at narrow widths.".into();
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(600.0)),
                    |window, cx| {
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        retained = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            {
                cx.update_window(window.into(), |_, window, cx| {
                    retained.as_ref().unwrap().update(cx, |view, cx| {
                        view.input.update(cx, |input, cx| {
                            let source = "## Heading 2\n- This is a **bold** list item\n> 9. [x] Completed é🦀";
                            input.set_value(source, window, cx);
                            input.set_selected_range(source.len()..source.len(), cx);
                            input.focus(window, cx);
                        });
                    });
                    window.dispatch_action(Box::new(gpui_component::input::Enter { secondary: false, shift: true }), cx);
                }).unwrap();
                cx.run_until_parked();
                cx.update(|cx| {
                    let view = retained.as_ref().unwrap().read(cx);
                    assert!(view.input.read(cx).value().ends_with("\n> 10. [ ] "));
                });
                for _ in 0..4 {
                    cx.advance_clock(std::time::Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                let path = format!("/tmp/grove-composer-markdown-{provider:?}-{width:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native composer Markdown fixture: {path}");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn snippet_copy_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    // Channel-backed provider-shaped views: no provider launch or script activation.
    let checkout = std::env::temp_dir().join(format!(
        "grove-snippet-metal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs_err::create_dir_all(&checkout).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&checkout)
        .status()
        .unwrap()
        .success());
    let checkout = checkout.canonicalize().unwrap();
    let cwd = checkout.to_string_lossy().into_owned();
    for available in [true, false] {
        cx.update(|cx| {
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store {
                    projects: if available {
                        vec![grove_core::storage::Project {
                            name: "Snippet Metal fixture".into(),
                            path: cwd.clone(),
                            scripts: grove_core::storage::ProjectScripts::default(),
                            archived: false,
                            worktree_dir: None,
                        }]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
            ));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(16.0);
        });
        for width in [1024.0, 600.0] {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let mut state = snapshot();
                state.saved.provider = provider;
                state.saved.cwd = cwd.clone();
                state.saved.items = vec![
                    json!({"type":"agentMessage", "id":"snippet", "text":"Copy this code:\n\n```sh\n  cargo test --workspace\n```", "groveCompleted":true}),
                ].into();
                let thread = state.saved.thread_id.clone().unwrap_or_default();
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(600.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let runtime = cx.new(crate::runtime::Runtime::new);
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            chat.update(cx, |view, cx| view.set_script_runtime(runtime, cx));
                            retained = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                for copied in [false, true] {
                    if copied {
                        cx.update_window(window.into(), |_, _, cx| {
                            retained.as_ref().unwrap().update(cx, |view, cx| {
                                view.copy_code_block(
                                    format!("{thread}:snippet:code:17"),
                                    "  cargo test --workspace\n",
                                    cx,
                                );
                            });
                        })
                        .unwrap();
                    }
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            assert_eq!(window.rem_size(), gpui::px(16.0));
                            assert!(
                                retained.as_ref().unwrap().read(cx).script_runtime.is_some(),
                                "both availability states must render the generated Run path"
                            );
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    let path =
                    format!("/tmp/grove-snippet-copy-{provider:?}-{width:.0}-available-{available}-copied-{copied}.png");
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Headless Metal provider-shaped fixture (not live provider): provider={provider:?} width={width:.0} rem=16 generated_runtime=true available={available} copied={copied} path={path}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
    fs_err::remove_dir_all(checkout).unwrap();
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn composer_access_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    let mut baseline_heights = HashMap::new();
    for (width, rem) in [
        (1024.0, 16.0),
        (600.0, 16.0),
        (600.0, 20.0),
        (400.0, 16.0),
        (400.0, 20.0),
    ] {
        // Root reapplies the component theme's font size on every render.
        // Set its source of truth rather than a rem value Root will overwrite.
        cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
        for kind in ["configured", "codex-yolo", "claude-yolo"] {
            let mut state = snapshot();
            state.sandbox = Some(json!({"type":"dangerFullAccess"}));
            state.approval_policy = Some(json!("untrusted"));
            if kind == "codex-yolo" {
                state.access = Some("full-access".into());
            } else if kind == "claude-yolo" {
                state.saved.provider = codex::Provider::Claude;
                state.access = Some("bypassPermissions".into());
            }
            if kind != "configured" {
                let provider = state.saved.provider;
                state.saved.config_args = provider
                    .launch_options(&provider.agent().launch_args_for_backend(true, true, false))
                    .unwrap()
                    .0;
            }
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(800.0)),
                    |window, cx| {
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        retained = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            for _ in 0..12 {
                cx.advance_clock(std::time::Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            cx.update_window(window.into(), |_, window, cx| {
                assert_eq!(
                    window.rem_size(),
                    gpui::px(rem),
                    "Root must retain requested zoom"
                );
                let view = retained.as_ref().unwrap().read(cx);
                let model = view.option_bounds[0].get();
                assert!(model.left() >= gpui::px(0.) && model.right() <= gpui::px(width));
                if rem == 16.0 {
                    baseline_heights.insert((kind, width as u32), model.size.height);
                } else {
                    let baseline = baseline_heights[&(kind, width as u32)].into();
                    assert!(
                        model.size.height > baseline,
                        "zoom must change actual control geometry: {model:?} vs {baseline:?}"
                    );
                }
                if kind == "configured" {
                    let access = view.option_bounds[2].get();
                    assert!(
                        access.left() >= rpx(SPACE_20 + SPACE_2XL).to_pixels(window.rem_size()) + gpui::px(1.)
                            && access.right() <= gpui::px(width) - rpx(SPACE_20 + SPACE_2XL).to_pixels(window.rem_size()) - gpui::px(1.),
                        "entire access button, including its reserved chevron, must fit composer content: {access:?}"
                    );
                    assert!(
                        access.left() >= model.right() || access.top() >= model.bottom(),
                        "wrapping controls must not overlap: {model:?}, {access:?}"
                    );
                }
                eprintln!(
                    "Native composer geometry {kind}/{width}/{rem}: model={model:?}, rem={:?}",
                    window.rem_size()
                );
            })
            .unwrap();
            let path = format!("/tmp/grove-composer-access-{kind}-{width:.0}-{rem:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native composer access fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// Opt-in real text shaping / Metal snapshots, independent of installed Grove.
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn approval_tray_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for width in [1024.0, 600.0] {
        for kind in ["computer", "command", "command-prefix", "claude"] {
            let mut state = snapshot();
            state.status = Status::Waiting;
            state.saved.items = vec![
                json!({"id":"intro","type":"agentMessage","text":"I’ll inspect the launcher’s current layout."}),
                json!({"id":"plan","type":"plan","plan":[{"step":"Inspect launcher","status":"completed"},{"step":"Verify interaction states","status":"inProgress"}]}),
            ].into();
            if kind == "computer" {
                state.elicitations.push(ElicitationRequest::parse_codex(json!("computer"), &json!({
                    "serverName":"cua_repl", "message":"Allow Computer Use to use \"Grove Launcher QA\"?",
                    "requestedSchema":{"type":"object","properties":{}}
                })).unwrap());
            } else if kind == "claude" {
                state.saved.provider = codex::Provider::Claude;
                state.approvals.push(json!({"id":"claude", "method":"item/commandExecution/requestApproval",
                    "groveAgent":"Reviewer", "claudeInput":{"command":"cargo test --lib controls::tests", "description":"Verify approval response mapping"},
                    "claudeRequest":{"tool_name":"Bash","input":{"command":"cargo test --lib controls::tests"}}
                }));
            } else {
                state.approvals.push(json!({"id":"command","method":"item/commandExecution/requestApproval","params":{
                    "reason":"May I resolve the vendored selection test dependencies to compile and run its focused regression tests?",
                    "command":"/bin/zsh -lc 'cargo test --manifest-path vendor/gpui-component/ui/Cargo.toml --lib text::window_selection::tests --no-run --locked'",
                    "cwd":"/Users/digvijaymahapatra/Sandbox/grove",
                    "proposedExecpolicyAmendment":["cargo","test","--manifest-path","vendor/gpui-component/ui/Cargo.toml","--lib","text::window_selection::tests","--no-run","--locked"]
                }}));
            }
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let mut capture_view = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(800.0)),
                    |window, cx| {
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        capture_view = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                if kind == "command-prefix" {
                    capture_view.as_ref().unwrap().update(cx, |view, cx| {
                        for draft in view.approval_drafts.values_mut() {
                            draft.policy_expanded = true;
                        }
                        cx.notify();
                    });
                }
                let _ = window.draw(cx);
            })
            .unwrap();
            let path = format!("/tmp/grove-permission-{kind}-{width:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native approval fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// Opt-in real text shaping / Metal snapshots, independent of installed Grove.

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn plan_approval_receipt_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let mut captured = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(900.0)),
                    |window, cx| {
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        let session = cx.new(|cx| {
                            CodexSession::from_connection(
                                approval_receipt_snapshot(provider),
                                Connection {
                                    handle,
                                    updates: receive.into(),
                                },
                                cx,
                            )
                        });
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        captured = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            cx.run_until_parked();
            for details in [false, true] {
                cx.update_window(window.into(), |_, window, cx| {
                    captured.as_ref().unwrap().update(cx, |view, cx| {
                        if details {
                            view.expanded
                                .insert("approval-details-approval-message".into());
                        }
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                })
                .unwrap();
                cx.run_until_parked();
                let path = format!(
                    "/tmp/grove-plan-approval-{provider:?}-{width:.0}-{rem:.0}-{}.png",
                    if details { "details" } else { "receipt" }
                );
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native plan approval receipt: {path}");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn transcript_activity_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for width in [1024.0, 600.0] {
        for kind in ["codex", "claude"] {
            let mut state = snapshot();
            state.status = Status::Working;
            if kind == "claude" {
                state.saved.provider = codex::Provider::Claude;
            }
            state.saved.items = vec![
                json!({"id":"question","type":"userMessage","content":[{"type":"text","text":"Inspect the working tree"}]}),
                json!({"id":"think","type":"reasoning","text":"Checking files","status":"completed"}),
                json!({"id":"done","type":"commandExecution","command":"pwd","status":"completed","aggregatedOutput":"/workspace"}),
                json!({"id":"failed","type":"commandExecution","command":"git fetch","status":"failed","exitCode":1}),
                json!({"id":"active","type":"commandExecution","command":"git status --short","status":"inProgress"}),
            ].into();
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let mut capture_view = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(800.0)),
                    |window, cx| {
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        capture_view = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                capture_view.as_ref().unwrap().update(cx, |view, cx| {
                    view.expanded.insert("work-0".into());
                    cx.notify();
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            let path = format!("/tmp/grove-transcript-activity-{kind}-{width:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native transcript activity fixture: {path}");
            cx.update_window(window.into(), |_, window, cx| {
                capture_view.as_ref().unwrap().update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.items.truncate(1);
                        session.state.saved.items.push(json!({
                            "id":"child-started", "type":"subAgentActivity", "kind":"started",
                            "agentThreadId":"child-fixture", "agentPath":"root/reviewer"
                        }));
                        cx.notify();
                    });
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            let path = format!("/tmp/grove-transcript-empty-work-{kind}-{width:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native empty work fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// Opt-in native Metal rendering without modifying the installed app.
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn selection_reference_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    let previous_theme = grove_core::theme::current().name.clone();
    for appearance in ["dark", "light"] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&format!(
                "grove-{appearance}"
            )));
            c::sync_component_theme(cx);
        });
        for width in [1024.0, 400.0] {
            let quote = codex::ReferenceDraft {
                id: "native-reference".into(),
                thread_id: Some("thread-ui".into()),
                sources: vec![codex::ReferenceSource {
                    message_id: "answer".into(),
                    role: "Assistant".into(),
                }],
                text: "Drafts survive reconnection.".into(),
            };
            let mut state = snapshot();
            state.saved.draft = "Can you explain how this works?".into();
            state.saved.draft_reference = Some(quote.clone());
            state.saved.items = vec![
                json!({"id":"question", "type":"userMessage", "content":[{"type":"text", "text":"What happens if the agent disconnects while I’m writing?"}]}),
                json!({"id":"answer", "type":"agentMessage", "text":"Your conversation stays open while Grove reconnects to the agent.\n\nYour message and attached images stay in the composer. Drafts survive reconnection.\n\nYou can continue editing your message while you wait."}),
            ].into();
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let retained = Rc::new(std::cell::RefCell::new(None));
            let captured = retained.clone();
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(800.0)),
                    move |window, cx| {
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        *captured.borrow_mut() = Some(chat.clone());
                        cx.new(|cx| gpui_component::Root::new(chat, window, cx).bordered(false))
                    },
                )
                .unwrap();
            // Native text shaping and Input auto-grow publish layout after
            // the initial frame. Drive those actual frames before capture.
            for _ in 0..12 {
                cx.advance_clock(std::time::Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
                let chat = retained.borrow();
                let view = chat.as_ref().unwrap().read(cx);
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Can you explain how this works?"
                );
                assert_eq!(view.reference.as_ref().unwrap().text, quote.text);
                let input = view.input.read(cx).range_to_bounds(&(0..1)).expect("native input text shaped");
                let pane = view.transcript_bounds.get();
                let viewport = window.viewport_size();
                eprintln!("Native quote layout {width}/{appearance}: transcript={pane:?}, input={input:?}, viewport={viewport:?}");
                assert!(input.top() >= gpui::px(0.) && input.bottom() <= viewport.height, "composer input outside screenshot viewport");
                assert!(pane.bottom() < input.top() && pane.bottom() < viewport.height - gpui::px(100.), "quote/composer must reserve visible space below transcript");
                assert!(view
                    .selection_views
                    .values()
                    .all(|owner| !owner.read(cx).source().contains("\"message_id\":")));
            })
            .unwrap();
            let path = format!("/tmp/grove-selection-reply-{width:.0}-{appearance}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native selection reference fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
    cx.update(|cx| {
        assert!(grove_core::theme::set_by_name(&previous_theme));
        c::sync_component_theme(cx);
    });
}

#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn question_receipt_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (width, rem) in [(1024.0, 16.0), (520.0, 16.0), (520.0, 24.0)] {
            cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            let mut state = snapshot();
            state.saved.provider = provider;
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(1000.0)),
                    |window, cx| {
                        window.set_rem_size(gpui::px(rem));
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        retained = Some(chat.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            // Initial synchronization clears state from the preceding session.
            // Insert the acknowledged receipt only after that first render.
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
                retained.as_ref().unwrap().update(cx, |view, cx| {
                    view.question_receipts = vec![wrapped_question_receipt(cx)];
                    cx.notify();
                });
            })
            .unwrap();
            for _ in 0..4 {
                cx.advance_clock(std::time::Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            cx.update_window(window.into(), |_, window, cx| {
                assert_eq!(window.rem_size(), gpui::px(rem));
                let view = retained.as_ref().unwrap().read(cx);
                assert_eq!(
                    view.question_receipts.len(),
                    1,
                    "native fixture must retain its receipt"
                );
            })
            .unwrap();
            let path = format!("/tmp/grove-question-receipt-{provider:?}-{width:.0}-{rem:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native question receipt fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// Real native task rendering with synthetic provider snapshots; no live transport.
#[test]
#[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn task_strip_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for (width, rem) in [(1024., 16.), (600., 16.), (600., 20.)] {
            cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            let mut state = snapshot();
            state.saved.provider = provider;
            state.status = Status::Working;
            state.saved.items = vec![
                json!({"id":"u","type":"userMessage","content":[{"text":"Align the task register"}]}),
                json!({"id":"tasks","type":"plan","explanation":"Keep task labels aligned with the header while preserving jump actions.","plan":[
                    {"step":"Inspect the existing task dock","status":"completed"},
                    {"step":"Align task text and stable status slots","status":"inProgress"},
                    {"step":"Verify narrow widths and larger text","status":"pending"}
                ]}),
            ].into();
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = mpsc::channel();
            let connection = Connection {
                handle,
                updates: receive.into(),
            };
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(700.)), |window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let session = cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                    let chat = cx.new(|cx| CodexView::new(session, window, cx));
                    cx.new(|cx| {
                        gpui_component::Root::new(chat, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                })
                .unwrap();
            for _ in 0..3 {
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            let path = format!("/tmp/grove-task-strip-{provider:?}-{width:.0}-{rem:.0}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native task strip fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
