//! Opt-in real text/Metal captures of the production side chat, without opening
//! the installed app or launching a provider. These are UI fixtures, not live
//! Codex/Claude provider-boundary evidence.
use super::*;
use gpui::AppContext as _;
use std::{path::Path, sync::Arc};

struct NativeSideChat {
    sidebar: Entity<Sidebar>,
    rem: f32,
}

impl Render for NativeSideChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(gpui::px(self.rem));
        let panel = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.snapshot = sidebar
                .runtime
                .update(cx, |runtime, cx| runtime.snapshot(cx));
            sidebar.render_content(window, cx)
        });
        div()
            .size_full()
            .bg(c::BG())
            .font_family(crate::fonts::UI_FAMILY)
            .flex()
            .items_end()
            .justify_end()
            .child(panel)
    }
}

fn saved(path: &str, provider: grove_core::codex::Provider) -> grove_core::codex::SavedSession {
    grove_core::codex::SavedSession {
        session_review: Default::default(),
                        rich_plans: Default::default(),
        side_chats: HashMap::from([(
            path.to_owned(),
            grove_core::codex::SavedSideChat {
                session: None,
                draft: String::new(),
                messages: vec![
                    ("user".into(), "So, what are the statuses now?".into()),
                    (
                        "assistant".into(),
                        "The sidebar keeps four groups: **Needs you**, **Review**, **Working**, and **Idle**.\n\n| Group | Statuses |\n| --- | --- |\n| Needs you | Approval needed, Answer needed, Usage limit, Disconnected, Failed |\n| Review | Done — a completion you haven't viewed yet |\n| Working | Starting, Working |\n| Idle | Ready, Interrupted, Exited |\n\nPending approval or answer requests still require action when you're viewing the session.".into(),
                    ),
                ],
            },
        )]),
        claude_resume: None,
        claude_subagents: Default::default(),
        edit_checkpoints: Vec::new(),
        pending_edit: None,
            rewind_drafts: Vec::new(),
        provider,
        composer: Default::default(),
        context: None,
        key: "side-chat-native-capture".into(),
        project: "Side chat capture".into(),
        cwd: path.into(),
        label: provider.agent().label().into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: Some("side-chat-native-thread".into()),
        items: vec![serde_json::json!({"id":"main-history", "type":"agentMessage", "text":"Main transcript content."})].into(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        activity_reporting: false,
    }
}

#[cfg(target_os = "macos")]
fn capture_motion(
    cx: &mut gpui::HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    sidebar: &Entity<Sidebar>,
    output: &Path,
    provider: &str,
) {
    cx.update_window(window, |_, window, cx| {
        window.blur();
        cx.set_reduce_motion(false);
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            chat.hovered_messages.clear();
            let index = chat.messages.len();
            chat.messages.push((
                "assistant".into(),
                "A new answer appears while your draft stays in place.".into(),
            ));
            // This is the same state transition used for the first streamed text.
            chat.entrances.insert(index, cx.background_executor().now());
            chat.follow_latest = true;
            cx.notify();
        });
    })
    .unwrap();
    let mut images = Vec::new();
    let mut identity = None;
    for (stage, advance) in [
        ("start", 0),
        ("middle", MOTION_SLOW_MS / 2),
        ("end", MOTION_SLOW_MS / 2 + 1),
    ] {
        cx.advance_clock(Duration::from_millis(advance));
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        })
        .unwrap();
        if stage == "end" {
            // Bottom-follow shares the frame clock and may outlive the entrance.
            for _ in 0..80 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window, |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
        }
        let image = cx.capture_screenshot(window).unwrap();
        image
            .save(output.join(format!("{provider}-motion-{stage}.png")))
            .unwrap();
        images.push(image);
        cx.update(|cx| {
            let chat = sidebar
                .read(cx)
                .checkout_panel
                .side_chats
                .values()
                .next()
                .unwrap();
            let current = (
                chat.input.entity_id(),
                chat.message_views.last().unwrap().view.entity_id(),
            );
            if let Some(expected) = identity {
                assert_eq!(
                    current, expected,
                    "motion replaced the editor or message view"
                );
            } else {
                identity = Some(current);
            }
            assert_eq!(
                chat.input.read(cx).value().as_ref(),
                "Can you explain this?\nKeep the draft on two lines."
            );
            if stage == "end" {
                assert!(
                    chat.entrances.is_empty(),
                    "settled entrance retained animation state"
                );
            }
        });
    }
    assert!(
        images[0].as_raw() != images[1].as_raw(),
        "entrance did not advance from its first frame"
    );
    assert!(
        images[1].as_raw() != images[2].as_raw(),
        "entrance did not reach its final frame"
    );
    cx.advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    let settled = cx.capture_screenshot(window).unwrap();
    assert!(
        images[2].as_raw() == settled.as_raw(),
        "ordinary redraw replayed a settled entrance"
    );
    settled
        .save(output.join(format!("{provider}-motion-settled.png")))
        .unwrap();

    // Changing the preference during an active entrance must discard its state
    // and render the exact same endpoint immediately, without advancing the clock.
    cx.update_window(window, |_, window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            chat.entrances
                .insert(chat.messages.len() - 1, cx.background_executor().now());
            cx.notify();
        });
        cx.set_reduce_motion(true);
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    let reduced = cx.capture_screenshot(window).unwrap();
    reduced
        .save(output.join(format!("{provider}-motion-reduced.png")))
        .unwrap();
    assert!(
        settled.as_raw() == reduced.as_raw(),
        "Reduce motion did not render the immediate endpoint"
    );
    cx.update(|cx| {
        let chat = sidebar
            .read(cx)
            .checkout_panel
            .side_chats
            .values()
            .next()
            .unwrap();
        assert!(
            chat.entrances.is_empty(),
            "Reduce motion retained entrance state"
        );
        assert_eq!(chat.input.entity_id(), identity.unwrap().0);
        assert_eq!(
            chat.message_views.last().unwrap().view.entity_id(),
            identity.unwrap().1
        );
        assert_eq!(
            chat.input.read(cx).value().as_ref(),
            "Can you explain this?\nKeep the draft on two lines."
        );
    });
}

#[cfg(target_os = "macos")]
fn verify_stream_reveal(
    cx: &mut gpui::HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    sidebar: &Entity<Sidebar>,
    output: &Path,
    provider: &str,
) {
    let suffix = " A streamed continuation with Unicode 界🙂 and smooth text.".repeat(8);
    let mut initial = String::new();
    let mut identity = None;
    cx.update_window(window, |_, window, cx| {
        window.blur();
        cx.set_reduce_motion(false);
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            initial = chat.messages.last().unwrap().1.clone();
            identity = Some(chat.message_views.last().unwrap().view.entity_id());
            chat.messages.last_mut().unwrap().1.push_str(&suffix);
            chat.busy = true;
            // Isolate text reveal; capture_motion separately exercises follow.
            chat.follow_latest = false;
            cx.notify();
        });
        // Build production elements only inside Window::draw, which scopes and
        // clears GPUI's app element arena. An unused direct render here would
        // retain its Input/TextView handles in the thread-local fallback arena.
        window.refresh();
        let _ = window.draw(cx);
        let chat = sidebar
            .read(cx)
            .checkout_panel
            .side_chats
            .values()
            .next()
            .unwrap();
        let shown = chat.message_views.last().unwrap().stream_text.1;
        let full = &chat.messages.last().unwrap().1;
        assert!(
            shown >= initial.len() && shown < full.len(),
            "first render was not paced: shown={shown}, old={}, full={}",
            initial.len(),
            full.len()
        );
    })
    .unwrap();
    let mut partial_sources = std::collections::HashSet::new();
    for frame in 0..100 {
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.update(|cx| {
            let chat = sidebar
                .read(cx)
                .checkout_panel
                .side_chats
                .values()
                .next()
                .unwrap();
            let message = chat.message_views.last().unwrap();
            let shown = message.view.read(cx).source();
            let full = &chat.messages.last().unwrap().1;
            assert_eq!(message.view.entity_id(), identity.unwrap());
            assert!(
                shown.starts_with(&initial),
                "stream replayed an existing prefix"
            );
            assert!(
                full.starts_with(shown.as_ref()),
                "stream broke a UTF-8 prefix"
            );
            if shown.len() > initial.len() && shown.len() < full.len() {
                partial_sources.insert(shown.len());
            }
            if frame == 99 {
                assert_eq!(shown.as_ref(), full);
            }
        });
        if matches!(frame, 0 | 6 | 99) {
            cx.capture_screenshot(window)
                .unwrap()
                .save(output.join(format!("{provider}-stream-{frame}.png")))
                .unwrap();
        }
    }
    assert!(
        partial_sources.len() >= 2,
        "parsed Markdown never displayed successive partial prefixes: {partial_sources:?}"
    );
    cx.update_window(window, |_, window, cx| {
        cx.set_reduce_motion(true);
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            chat.messages.last_mut().unwrap().1.push_str(&suffix);
            cx.notify();
        });
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    // Parse the full reduced-motion endpoint without advancing animation time.
    cx.run_until_parked();
    cx.update(|cx| {
        let chat = sidebar
            .read(cx)
            .checkout_panel
            .side_chats
            .values()
            .next()
            .unwrap();
        assert_eq!(
            chat.message_views
                .last()
                .unwrap()
                .view
                .read(cx)
                .source()
                .as_ref(),
            &chat.messages.last().unwrap().1
        );
        assert!(
            !chat.motion_frame_pending.get(),
            "reduced motion scheduled a reveal frame"
        );
        assert_eq!(
            chat.input.read(cx).value().as_ref(),
            "Can you explain this?\nKeep the draft on two lines."
        );
    });
    cx.update_window(window, |_, _, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap()
                .busy = false;
            cx.notify();
        });
    })
    .unwrap();
}

#[cfg(target_os = "macos")]
fn verify_keyboard_routing(
    cx: &mut gpui::HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    sidebar: &Entity<Sidebar>,
    provider: grove_core::codex::Provider,
) {
    const DRAFT: &str = "Can you explain this?\nKeep the draft on two lines.";
    cx.update_window(window, |_, window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            chat.input.update(cx, |input, cx| {
                input.set_value(DRAFT, window, cx);
                input.focus(window, cx);
            });
        });
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    let key = gpui::Keystroke::parse("space").unwrap();
    cx.update_window(window, |_, window, cx| {
        // dispatch_keystroke sends KeyDown through the actual keymap and, only
        // if unconsumed, dispatches its text through the focused input handler.
        // A raw KeyDown alone cannot model native character insertion.
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke: key }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            let chat = sidebar
                .checkout_panel
                .side_chats
                .values_mut()
                .next()
                .unwrap();
            assert_eq!(
                chat.input.read(cx).value().as_ref(),
                format!(" {DRAFT}"),
                "native Space did not reach the focused side chat input"
            );
            chat.input
                .update(cx, |input, cx| input.set_value(DRAFT, window, cx));
        });
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
}

/// Run serially on macOS with GROVE_SIDE_CHAT_CAPTURE_OUTPUT set to a directory.
/// Writes wide, narrow and zoomed real native screenshots for both providers.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires real macOS text shaping and Metal; no installed app interaction"]
fn capture_native_side_chat() {
    let output = std::env::var("GROVE_SIDE_CHAT_CAPTURE_OUTPUT")
        .expect("set GROVE_SIDE_CHAT_CAPTURE_OUTPUT to an evidence directory");
    fs_err::create_dir_all(&output).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_string_lossy().into_owned();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["init", "-b", "main"])
        .output()
        .unwrap()
        .status
        .success());
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        for (name, width, height, rem) in [
            ("wide", 1280., 900., 16.),
            ("narrow", 760., 800., 16.),
            ("zoomed", 1600., 1125., 20.),
        ] {
            let mut cx = gpui::HeadlessAppContext::with_platform(
                gpui_platform::current_platform(true).text_system(),
                Arc::new(crate::assets::Assets),
                gpui_platform::current_headless_renderer,
            );
            cx.update(|cx| {
                gpui_component::init(cx);
                crate::fonts::register(cx).unwrap();
                cx.set_reduce_motion(true);
                assert!(grove_core::theme::set_by_name("grove-dark"));
                crate::theme::sync_component_theme(cx);
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    projects: vec![grove_core::storage::Project {
                        name: "Side chat capture".into(),
                        path: path.clone(),
                        scripts: Default::default(),
                        archived: false,
                        worktree_dir: None,
                    }],
                    ..Default::default()
                }));
                cx.set_global(crate::zoom::CurrentPtyDims::default());
                cx.set_global(crate::zoom::ZoomState::new(rem / crate::zoom::REM_BASE));
            });
            let (_updates, receive) = std::sync::mpsc::channel();
            let mut sidebar_entity = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(height)),
                    |window, cx| {
                        let runtime = cx.new(crate::runtime::Runtime::new);
                        let registry = runtime.read(cx).registry.clone();
                        let id = registry.update(cx, |registry, _| {
                            registry.insert_meta(
                                "Side chat capture".into(),
                                path.clone(),
                                provider.agent(),
                            )
                        });
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
                        let (handle, _actions) = grove_core::codex::Handle::channel();
                        let session = cx.new(|cx| {
                            crate::entities::codex_session::CodexSession::from_connection(
                                {
                                    let mut snapshot =
                                        grove_core::codex::Snapshot::new(saved(&path, provider));
                                    snapshot.status = grove_core::codex::Status::Idle;
                                    snapshot
                                },
                                grove_core::codex::Connection {
                                    handle: handle.clone(),
                                    updates: receive.into(),
                                },
                                cx,
                            )
                        });
                        registry
                            .update(cx, |registry, _| registry.attach_codex(id, session, handle));
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime, window, cx);
                            sidebar.selection = Some(Selection::Session(id));
                            sidebar.initial_selection_pending = false;
                            sidebar.checkout_panel.hidden = false;
                            sidebar.checkout_panel.side_chat_owner = sidebar.selection.clone();
                            sidebar
                        });
                        sidebar_entity = Some(sidebar.clone());
                        let probe = cx.new(|_| NativeSideChat { sidebar, rem });
                        cx.new(|cx| {
                            gpui_component::Root::new(probe, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            let sidebar = sidebar_entity.unwrap();
            for state in ["empty", "draft"] {
                if state == "draft" {
                    cx.update_window(window.into(), |_, window, cx| {
                        sidebar.update(cx, |sidebar, cx| {
                            let chat = sidebar
                                .checkout_panel
                                .side_chats
                                .values_mut()
                                .next()
                                .unwrap();
                            chat.input.update(cx, |input, cx| {
                                input.set_value(
                                    "Can you explain this?\nKeep the draft on two lines.",
                                    window,
                                    cx,
                                );
                                input.focus(window, cx);
                            });
                            chat.hovered_messages.insert(0);
                            cx.notify();
                        });
                    })
                    .unwrap();
                }
                for _ in 0..4 {
                    cx.advance_clock(Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                let image = cx.capture_screenshot(window.into()).unwrap();
                assert!(image.width() > 0 && image.height() > 0);
                let provider_name = match provider {
                    grove_core::codex::Provider::Codex => "codex",
                    grove_core::codex::Provider::Claude => "claude",
                };
                image
                    .save(Path::new(&output).join(format!("{provider_name}-{name}-{state}.png")))
                    .unwrap();
                cx.update(|cx| {
                    let chat = sidebar
                        .read(cx)
                        .checkout_panel
                        .side_chats
                        .values()
                        .next()
                        .unwrap();
                    assert!(
                        chat.entrances.is_empty(),
                        "reduced-motion fixture retained entrance state"
                    );
                    if state == "draft" {
                        assert_eq!(
                            chat.input.read(cx).value().as_ref(),
                            "Can you explain this?\nKeep the draft on two lines."
                        );
                    }
                });
            }
            if name == "wide" {
                capture_motion(
                    &mut cx,
                    window.into(),
                    &sidebar,
                    Path::new(&output),
                    provider.agent().label(),
                );
                verify_stream_reveal(
                    &mut cx,
                    window.into(),
                    &sidebar,
                    Path::new(&output),
                    provider.agent().label(),
                );
                verify_keyboard_routing(&mut cx, window.into(), &sidebar, provider);
            }
            // Release retained render-frame/editor handles while the app and its
            // executor are still alive, as in the other native capture fixtures.
            cx.update_window(window.into(), |_, window, cx| {
                cx.set_reduce_motion(true);
                window.blur();
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
            drop(sidebar);
            drop(_updates);
            cx.run_until_parked();
        }
    }
}
