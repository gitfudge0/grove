//! Subagent visibility, disclosure, and provider presentation fixtures.

use super::super::agents::{AGENT_LINE_H, AGENT_STATUS_W};
use super::*;

/// Native Metal screenshots with channel-backed provider fixtures; no model calls.
#[test]
#[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn checkout_agents_native_snapshots() {
    use gpui::AppContext as _;
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
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Preview fixture".into(),
                path: "/tmp/grove-agent-preview-project".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    for (provider, label, width, rem, light) in [
        (
            grove_core::codex::Provider::Codex,
            "codex-wide",
            1280.,
            16.,
            false,
        ),
        (
            grove_core::codex::Provider::Claude,
            "claude-narrow",
            760.,
            16.,
            false,
        ),
        (
            grove_core::codex::Provider::Claude,
            "claude-light-zoom",
            1280.,
            20.,
            true,
        ),
    ] {
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(if light {
                c::DEFAULT_LIGHT_THEME
            } else {
                c::DEFAULT_DARK_THEME
            }));
            c::sync_component_theme(cx);
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
        });
        let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
        let owner_for_window = owner.clone();
        let window = cx.open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let id = runtime.read(cx).registry.clone().update(cx, |registry, _| registry.insert_meta("Preview fixture".into(), "/tmp/grove-agent-preview-fixture".into(), provider.agent()));
                let (handle, _actions) = grove_core::codex::Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession {
                html_policy: grove_core::visuals::HtmlPolicy::Legacy,
                visuals: Vec::new(),
                session_review: grove_core::codex::SessionReview::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: std::collections::HashMap::default(),
                fork_origin: None,
            claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
            rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
            context_details: None,
                key: "agents-fixture".into(),
                project: "Preview fixture".into(),
                cwd: "/tmp/grove-agent-preview-fixture".into(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };

                let mut snapshot = grove_core::codex::Snapshot::new(saved);
                snapshot.saved.items = vec![serde_json::json!({"type":"userMessage", "text":"Investigate authentication retries"}), serde_json::json!({"type":"reasoning", "text":"Thinking"})].into();
                snapshot.saved.work_periods = vec![grove_core::codex::WorkPeriod { start_item: 0, end_item: None, duration_ms: None, subagents: vec![grove_core::codex::Subagent { id: "explorer".into(), name: "Explorer".into(), role: Some("Investigation".into()), task: Some("Investigate authentication retry failures and report the owning layer".into()), activity: "Reading authentication tests".into(), status: grove_core::codex::SubagentStatus::Working }] }];
                for (id, name, status) in [
                    ("interrupted", "Interrupted agent", grove_core::codex::SubagentStatus::Interrupted),
                    ("done", "Completed agent", grove_core::codex::SubagentStatus::Completed),
                    ("failed", "Failed agent", grove_core::codex::SubagentStatus::Failed),
                    ("unknown", "Unknown agent with a long name that must truncate", grove_core::codex::SubagentStatus::Unknown),
                ] {
                    snapshot.saved.work_periods[0].subagents.push(grove_core::codex::Subagent {
                        id: id.into(), name: name.into(), role: None, task: None,
                        activity: "Observed activity".into(), status,
                    });
                }
                snapshot.status = grove_core::codex::Status::Working;
                let observed = Instant::now();
                let mut activity = grove_core::codex::SubagentActivity::default();
                activity.items = (0..24).map(|index| serde_json::json!({"id":format!("comment-{index}"),"type":"agentMessage","text":format!("### Investigation step {index}\n\nFollowing the authentication retry path and checking the owning layer.\n\n- Preserve the request identifier\n- Inspect retry status and cancellation")})).collect();
                activity.items.push(serde_json::json!({"id":"tool-completed","type":"commandExecution","command":"rg retry tests/auth","status":"completed","aggregatedOutput":"tests/auth/retry.rs:42: retry_after_rejection\ntests/auth/retry.rs:87: cancellation_preserves_request"}));
                activity.items.push(serde_json::json!({"id":"latest-comment","type":"agentMessage","text":"## Retry entry point found\n\nThe request reaches **the authentication owner** before retrying. This detailed investigation preserves markdown, `request_id`, and the complete visible message.\n\n```rust\nassert_eq!(retry.request_id, original.request_id);\n```\n\nNext I will run the focused regression and inspect its full output."}));
                activity.items.push(serde_json::json!({"id":"tool-active","type":"commandExecution","command":"cargo test authentication_retry -- --nocapture","status":"inProgress","aggregatedOutput":"running 3 tests\ntest retry_after_rejection ... ok\ntest cancellation_preserves_request ... ok"}));
                activity.observed_at = Some(observed);
                snapshot.subagent_activity.insert("explorer".into(), activity);
                let session = cx.new(|cx| crate::entities::codex_session::CodexSession::from_connection(snapshot, grove_core::codex::Connection { handle:handle.clone(), updates:receive.into() },cx));
                runtime.read(cx).registry.clone().update(cx, |registry,_| registry.attach_codex(id,session,handle));
                let sidebar = cx.new(|cx| { let mut sidebar=Sidebar::new(runtime,window,cx); sidebar.selection=Some(Selection::Session(id)); sidebar.initial_selection_pending=false; sidebar.checkout_panel.expanded=true; sidebar });
                *owner_for_window.borrow_mut()=Some((sidebar.clone(),id));
                cx.new(|cx| gpui_component::Root::new(sidebar,window,cx).bordered(false).bg(c::BG()))
            }).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        let (sidebar, id) = owner.borrow().as_ref().unwrap().clone();
        cx.update_window(window.into(), |_, window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert_eq!(
                    sidebar.selection,
                    Some(Selection::Session(id)),
                    "fixture session remains selected"
                );
                assert!(
                    sidebar.checkout_targets(cx).is_empty(),
                    "fixture has no registered checkout"
                );
                assert!(
                    sidebar
                        .checkout_panel
                        .agent_bounds
                        .get("explorer")
                        .is_some_and(|bounds| bounds.get().size.width > gpui::px(0.)),
                    "fixture agent row was actually rendered"
                );
                sidebar.open_agent_peek(id, "explorer".into(), true, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        let path = format!("/tmp/grove-agent-preview-{label}.png");
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&path)
            .unwrap();
        eprintln!("Native fixture (provider channels, not live): {path}");
        // Capture retention states independently of provider channels. The live
        // fixture above remains the original complete/streaming transcript.
        for retention in ["awaiting", "unavailable", "restored"] {
            cx.update_window(window.into(), |_, window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        let session = sidebar.runtime.read(cx).registry.read(cx).codex(id).cloned().unwrap();
                        session.update(cx, |session, cx| {
                            if retention == "awaiting" {
                                session.state.subagent_activity.clear();
                            } else if retention == "unavailable" {
                                session.state = grove_core::codex::Snapshot::new(session.state.saved.clone()).into();
                            } else {
                                let mut activity = grove_core::codex::SubagentActivity::default();
                                activity.items = vec![serde_json::json!({"id":"saved-reply", "type":"agentMessage", "text":"## Saved investigation\n\nThe authentication owner preserves the original request identifier."})].into();
                                session.state.subagent_activity.insert("explorer".into(), activity);
                            }
                            let (freshness, empty) = super::super::agent_preview::child_transcript_labels(&session.state, "explorer");
                            match retention {
                                "awaiting" => assert_eq!(empty, Some("No child messages received yet")),
                                "unavailable" => assert_eq!(empty, Some("Historical child messages are unavailable")),
                                _ => {
                                    assert!(empty.is_none());
                                    assert_eq!(freshness, "Saved child transcript · observation time unavailable");
                                }
                            }
                            cx.notify();
                        });
                        sidebar.open_agent_peek(id, "explorer".into(), true, window, cx);
                    });
                    let _ = window.draw(cx);
                }).unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let path = format!("/tmp/grove-agent-preview-{label}-{retention}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native retention fixture (not live): {path}");
        }
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[gpui::test]
fn codex_and_claude_first_child_appears_without_registered_checkout(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Unregistered fixture".into(),
                path: "/tmp/grove-unregistered-project".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "agents-fixture".into(),
            project: "Unregistered fixture".into(),
            cwd: "/tmp/grove-unregistered-agent-fixture".into(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };

        let mut initial = grove_core::codex::Snapshot::new(saved.clone());
        initial.status = grove_core::codex::Status::Working;
        let (handle, _actions) = grove_core::codex::Handle::channel();
        let (updates, receive) = std::sync::mpsc::channel();
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta(
                    "Unregistered fixture".into(),
                    "/tmp/grove-unregistered-agent-fixture".into(),
                    provider.agent(),
                )
            });
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    initial,
                    grove_core::codex::Connection {
                        handle: handle.clone(),
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, _| registry.attach_codex(id, session, handle));
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.checkout_targets(cx).is_empty()));
        assert!(cx.debug_bounds("checkout-agents").is_none());
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .checkout_panel
            .agent_observer
            .is_some()));
        let mut next = grove_core::codex::Snapshot::new(saved);
        next.status = grove_core::codex::Status::Working;
        next.saved.work_periods = vec![grove_core::codex::WorkPeriod {
            start_item: 0,
            end_item: None,
            duration_ms: None,
            subagents: vec![grove_core::codex::Subagent {
                id: "first-child".into(),
                name: "Explorer".into(),
                task: Some("Inspect source".into()),
                role: None,
                activity: "Reading files".into(),
                status: grove_core::codex::SubagentStatus::Working,
            }],
        }];
        let mut second = next.saved.work_periods[0].subagents[0].clone();
        second.id = "second-child".into();
        second.name = "Reviewer".into();
        next.saved.work_periods[0].subagents.push(second);
        // Only the provider update channel changes; do not notify Sidebar.
        updates.send(next).unwrap();
        cx.executor().advance_clock(Duration::from_millis(41));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("checkout-agents").is_some(),
            "first child makes independent right panel visible for {}",
            provider.label()
        );
        assert!(cx.debug_bounds("checkout-finder").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                cx.set_reduce_motion(false);
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("fixture session");
                };
                assert!(
                    !sidebar.html_overlay_open(),
                    "resting transcript permits HTML"
                );
                sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
                assert!(
                    !sidebar.html_overlay_open(),
                    "preview must preserve native HTML outside its mask"
                );
                sidebar.dismiss_agent_peek(window, cx);
                assert!(sidebar.checkout_panel.peek.as_ref().unwrap().closing);
                assert!(
                    !sidebar.html_overlay_open(),
                    "HTML remains visible during dismissal"
                );
                assert!(sidebar.checkout_panel.agent_focus["first-child"].is_focused(window));
                sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_FAST_MS) * 2);
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(
                    !sidebar.checkout_panel.peek.as_ref().unwrap().closing,
                    "old dismissal must not remove reopened {} preview",
                    provider.label()
                );
                assert!(!sidebar.html_overlay_open(), "reopening preserves HTML");
                sidebar.dismiss_agent_peek(window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_FAST_MS - 1));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()),
            "exit remains painted until the fast-motion deadline"
        );
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.checkout_panel.peek.is_none());
                assert!(!sidebar.html_overlay_open(), "HTML returns after dismissal");
                cx.set_reduce_motion(true);
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("fixture session");
                };
                sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
                assert!(
                    !sidebar.html_overlay_open(),
                    "static preview preserves native HTML"
                );
                sidebar.dismiss_agent_peek(window, cx);
                assert!(sidebar.checkout_panel.peek.is_none());
                assert!(
                    !sidebar.html_overlay_open(),
                    "static dismissal restores HTML"
                );
                sidebar.agent_peek_hover(id, "first-child".into(), true, window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(220));
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("fixture session");
                };
                assert_eq!(
                    sidebar.checkout_panel.peek.as_ref().unwrap().child,
                    "first-child"
                );
                // Enter can arrive before the old row/preview's exit.
                sidebar.agent_peek_hover(id, "second-child".into(), true, window, cx);
                sidebar.agent_peek_hover(id, "first-child".into(), false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(220));
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("fixture session");
                };
                assert_eq!(
                    sidebar.checkout_panel.peek.as_ref().unwrap().child,
                    "second-child",
                    "quick hover handoff opens the next {} agent",
                    provider.label()
                );
                sidebar.agent_peek_hover(id, "first-child".into(), false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(260));
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let Some(Selection::Session(id)) = sidebar.selection else {
                    panic!("fixture session");
                };
                assert_eq!(
                    sidebar.checkout_panel.peek.as_ref().unwrap().child,
                    "second-child",
                    "stale exits cannot dismiss the new preview"
                );
                sidebar.agent_peek_hover(id, "second-child".into(), false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(260));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()),
            "leaving the current agent still dismisses its preview"
        );
    }
}

#[gpui::test]
fn codex_and_claude_checkout_agents_follow_session_updates_and_disclosures(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(fixture.path())
        .args(["init", "-b", "main"])
        .output()
        .unwrap()
        .status
        .success());
    fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
    let fixture_path = fixture.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        // Immediate interaction checks; the separate lifecycle test covers exit motion.
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Checkout QA".into(),
                path: fixture_path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let path = fixture_path.clone();
        let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
        });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path,
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.selection = Some(Selection::Session(id));
        sidebar.initial_selection_pending = false;
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let id = cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let id = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, _| {
                        registry.insert_meta(
                            "Checkout QA".into(),
                            fixture_path.clone(),
                            provider.agent(),
                        )
                    });
                sidebar.selection = Some(Selection::Session(id));
                id
            })
        });
        let (handle, _actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "agents-fixture".into(),
            project: "Checkout QA".into(),
            cwd: fixture_path.clone(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                grove_core::codex::Snapshot::new(saved),
                grove_core::codex::Connection {
                    handle: handle.clone(),
                    updates: receive.into(),
                },
                cx,
            )
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, _| {
                        registry.attach_codex(id, session.clone(), handle);
                    });
                sidebar.checkout_panel.expanded = true;
                cx.notify();
            });
        });
        cx.run_until_parked();
        // The observer is installed even before a child has appeared.
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|_, cx| {
                session.update(cx, |session, cx| {
                    use grove_core::codex::{Subagent, SubagentStatus, WorkPeriod};
                    let agent = |id: &str, status| Subagent {
                        id: id.into(),
                        name: format!("A deliberately long agent name for {id}"),
                        activity: "Inspecting checkout layout".into(),
                        role: None,
                        task: Some("Inspect checkout layout and verify long task headings remain readable at narrow widths".into()),
                        status,
                    };
                    session.state.saved.items = vec![
                        serde_json::json!({"type":"userMessage", "text":"Inspect checkout"}),
                        serde_json::json!({"type":"reasoning", "text":"Inspecting"}),
                        serde_json::json!({"type":"userMessage", "text":"Continue checkout"}),
                        serde_json::json!({"type":"reasoning", "text":"Continuing"}),
                    ].into();
                    session.state.saved.work_periods = vec![
                        WorkPeriod {
                            start_item: 0,
                            end_item: Some(2),
                            duration_ms: None,
                            subagents: vec![
                                agent("old-running", SubagentStatus::Working),
                                agent("old-done", SubagentStatus::Completed),
                                agent("updated", SubagentStatus::Working),
                            ],
                        },
                        WorkPeriod {
                            start_item: 2,
                            end_item: None,
                            duration_ms: None,
                            subagents: (0..6)
                                .map(|index| {
                                    agent(
                                        &format!("current-{index}"),
                                        if index == 0 {
                                            SubagentStatus::Completed
                                        } else {
                                            SubagentStatus::Working
                                        },
                                    )
                                })
                                .chain(std::iter::once(agent("updated", SubagentStatus::Failed)))
                                .collect(),
                        },
                    ];
                    cx.notify();
                });
            });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        // Exercise both provider fixtures at normal and narrow viewport sizes and UI zoom.
        for (width, scale) in [(1280., 1.), (500., 1.), (500., 1.1)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(crate::zoom::REM_BASE * scale));
                sidebar.update(cx, |sidebar, _| {
                    // This fixture inspects an explicitly reopened panel,
                    // including widths where automatic fitting hides it.
                    sidebar.checkout_panel.width_override =
                        Some(Sidebar::logical_window_width(window));
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("checkout-agents").is_some());
            let card = cx.debug_bounds("checkout-card").unwrap();
            let name = cx.debug_bounds("checkout-agent-name-old-running").unwrap();
            let status = cx
                .debug_bounds("checkout-agent-status-old-running")
                .unwrap();
            assert!(status.right() < card.right());
            assert!((status.size.width - gpui::px(AGENT_STATUS_W * scale)).abs() < gpui::px(0.5));
            let activity = cx
                .debug_bounds("checkout-agent-activity-old-running")
                .unwrap();
            assert_eq!(name.top(), status.top());
            assert!(name.right() <= status.left());
            // One foreground identity/status line, then one smaller activity line.
            assert!((activity.top() - name.bottom()).abs() < gpui::px(0.5));
            assert!((activity.size.height - gpui::px(AGENT_LINE_H * scale)).abs() < gpui::px(0.5));
            let next_name = cx.debug_bounds("checkout-agent-name-updated").unwrap();
            assert!(
                (next_name.top()
                    - name.top()
                    - gpui::px((AGENT_LINE_H * 2. + SPACE_SM * 2.) * scale))
                .abs()
                    < gpui::px(1.)
            );
            assert!(activity.left() >= card.left());
            assert!(activity.right() < card.right());
            let pane = sidebar.read_with(cx, |sidebar, cx| {
                sidebar.codex_views[&id].read(cx).transcript_bounds.get()
            });
            assert!(
                pane.size.height >= gpui::px(80.),
                "{} transcript must remain usable with checkout expanded: {pane:?}",
                provider.label()
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_agent_peek(id, "old-running".into(), true, window, cx);
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let preview = cx.debug_bounds("agent-peek").unwrap_or_else(|| {
                    let (open, visible, source, pane) = sidebar.read_with(cx, |sidebar, cx| {
                        (
                            sidebar.checkout_panel.peek.is_some(),
                            sidebar.checkout_panel.agents_visible,
                            sidebar.checkout_panel.agent_bounds.get("old-running").map(|b| b.get()),
                            sidebar.codex_views.get(&id).map(|view| view.read(cx).transcript_bounds.get()),
                        )
                    });
                    let composer = cx.debug_bounds("codex-composer");
                    let input = cx.debug_bounds("codex-composer-input");
                    panic!(
                        "{} preview missing at width {width} and scale {scale}: open={open}, visible={visible}, source={source:?}, pane={pane:?}, composer={composer:?}, input={input:?}",
                        provider.label()
                    );
                });
            assert_eq!(
                cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
                Some(preview),
                "{} preview mask follows narrow layout and zoom",
                provider.label()
            );
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(
                cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
                None
            );
        }
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.update(|window, cx| {
            window.set_rem_size(gpui::px(crate::zoom::REM_BASE));
            let _ = window.draw(cx);
        });
        let row = cx.debug_bounds("checkout-agent-old-running").unwrap();
        // Pointer intent does not open immediately or steal composer focus.
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                gpui::Focusable::focus_handle(sidebar.codex_views[&id].read(cx), cx)
                    .focus(window, cx);
            });
        });
        let outside = gpui::point(gpui::px(1.), gpui::px(1.));
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
        cx.executor().advance_clock(Duration::from_millis(221));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .checkout_panel
            .peek
            .as_ref()
            .is_some_and(|peek| !peek.explicit)));
        assert!(
            cx.update(|window, cx| sidebar.read(cx).codex_views[&id]
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)),
            "hover keeps composer focus"
        );
        let hover_preview = cx.debug_bounds("agent-peek").unwrap();
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            Some(hover_preview),
            "{} preview mask matches the painted panel",
            provider.label()
        );
        // A short crossing outside both surfaces must not dismiss the preview.
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()));
        cx.simulate_mouse_move(hover_preview.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()),
            "entering preview cancels transit dismissal"
        );
        // Root Escape must work while hover has left focus in the composer.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            None,
            "dismissal clears the native mask"
        );
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(221));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(261));
        cx.run_until_parked();
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()),
            "hover dismissal runs after transit grace"
        );
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .checkout_panel
            .peek
            .as_ref()
            .is_some_and(|peek| peek.explicit)));
        let preview = cx.debug_bounds("agent-peek").unwrap();
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            Some(preview)
        );
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(preview.bottom() <= composer.top());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            None,
            "dismissal clears the native mask"
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.checkout_panel.agent_focus["old-running"].focus(window, cx);
            });
        });
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let card = cx.debug_bounds("checkout-card").unwrap();
        let divider = cx.debug_bounds("checkout-session-divider").unwrap();
        assert!(divider.left() <= card.left() + gpui::px(SPACE_XS));
        assert!(divider.right() >= card.right() - gpui::px(SPACE_XS));
        assert!(
            !sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                [&id]
                .show_all)
        );
        assert!(
            !sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                [&id]
                .previous)
        );
        // Taller two-line tasks may put Show more below the checkout viewport.
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-agents-more").is_some());
        let more = cx.debug_bounds("checkout-agents-more").unwrap();
        cx.simulate_click(more.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                [&id]
                .show_all)
        );
        // Expanded activity rows use the existing checkout scroll region.
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let previous = cx.debug_bounds("checkout-agents-previous").unwrap();
        assert!(
            previous.bottom() <= panel.bottom(),
            "scroll reveals previous agents"
        );
        cx.simulate_click(previous.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                [&id]
                .previous)
        );
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(800.)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.checkout_panel.width_override = Some(Sidebar::logical_window_width(window));
                sidebar.checkout_panel.expanded = false;
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
            .read(cx)
            .agents_in_checkout));
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, _| {
                sidebar.checkout_panel.width_override = Some(Sidebar::logical_window_width(window));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
            .read(cx)
            .agents_in_checkout));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("checkout-summary").is_some(),
            "Agents remains reachable in Zen"
        );
        assert!(
            cx.debug_bounds("checkout-finder").is_none(),
            "Zen contains session agents only"
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_agent_peek(id, "old-running".into(), true, window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx
            .update(|_, cx| crate::views::inline_html::native_exclusion(cx))
            .is_some());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.selection = Some(Selection::Project(0));
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(
            cx.update(|_, cx| crate::views::inline_html::native_exclusion(cx)),
            None,
            "leaving the session clears its preview mask"
        );
    }
}

#[test]
fn checkout_agent_presentation_uses_observed_text_without_duplicates() {
    use grove_core::codex::{Subagent, SubagentStatus};
    let mut agent = Subagent {
        id: "one".into(),
        name: " Agent 1 ".into(),
        task: Some("  Fix\n retries  ".into()),
        activity: " Inspecting\n tests ".into(),
        role: None,
        status: SubagentStatus::Working,
    };
    let working = checkout_agent_presentation(&agent);
    assert_eq!(working.name, "Agent 1");
    assert_eq!(working.activity.as_deref(), Some("Inspecting tests"));
    assert!(working.detail.contains("Task: Fix retries"));
    for (status, label) in [
        (SubagentStatus::Completed, "Done"),
        (SubagentStatus::Failed, "Failed"),
        (SubagentStatus::Interrupted, "Interrupted"),
        (SubagentStatus::Unknown, "Unknown"),
    ] {
        agent.status = status;
        let row = checkout_agent_presentation(&agent);
        assert_eq!(row.status, label);
        assert_eq!(row.activity.as_deref(), Some("Inspecting tests"));
    }
    agent.activity = "fix RETRIES".into();
    assert_eq!(
        checkout_agent_presentation(&agent).activity.as_deref(),
        Some("fix RETRIES")
    );
    agent.activity = "Activity unavailable".into();
    assert_eq!(
        checkout_agent_presentation(&agent).activity.as_deref(),
        Some("Fix retries")
    );
    agent.activity = "fix RETRIES".into();
    agent.task = Some(" \n ".into());
    let fallback = checkout_agent_presentation(&agent);
    assert_eq!(fallback.activity.as_deref(), Some("fix RETRIES"));
    for placeholder in ["", " \n ", " Activity unavailable ", "ACTIVITY UNAVAILABLE"] {
        agent.activity = placeholder.into();
        assert_eq!(checkout_agent_presentation(&agent).name, "Agent 1");
        assert_eq!(checkout_agent_presentation(&agent).activity, None);
    }
    agent.name.clear();
    assert_eq!(checkout_agent_presentation(&agent).name, "Agent");
    agent.task = Some("Long task ".repeat(100));
    agent.activity = "Long observation ".repeat(100);
    let long = checkout_agent_presentation(&agent);
    assert!(long.activity.as_ref().unwrap().len() > 900);
    assert!(long.detail.contains(agent.task.as_deref().unwrap().trim()));
    assert!(long.detail.contains(agent.activity.trim()));
}

#[test]
fn checkout_agent_groups_keep_latest_terminal_observations_and_independent_children() {
    use grove_core::codex::{Subagent, SubagentStatus, WorkPeriod};
    let agent = |id: &str, status| Subagent {
        id: id.into(),
        name: id.into(),
        activity: String::new(),
        role: None,
        task: None,
        status,
    };
    let mut periods = vec![
        WorkPeriod {
            start_item: 0,
            end_item: Some(1),
            duration_ms: None,
            subagents: vec![
                agent("running", SubagentStatus::Working),
                agent("unknown", SubagentStatus::Unknown),
                agent("done", SubagentStatus::Completed),
                agent("changed", SubagentStatus::Working),
            ],
        },
        WorkPeriod {
            start_item: 1,
            end_item: None,
            duration_ms: None,
            subagents: vec![
                agent("current-done", SubagentStatus::Completed),
                agent("changed", SubagentStatus::Failed),
            ],
        },
    ];
    let agents = checkout_agents(&periods);
    assert_eq!(agents.len(), 5);
    let (main, previous) = checkout_agent_groups(&agents, &periods);
    assert_eq!(
        main.iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>(),
        ["running", "unknown", "changed", "current-done"]
    );
    assert_eq!(previous[0].id, "done");
    assert_eq!(main[2].status, SubagentStatus::Failed);
    periods.push(WorkPeriod {
        start_item: 2,
        end_item: None,
        duration_ms: None,
        subagents: Vec::new(),
    });
    let (main, previous) = checkout_agent_groups(&agents, &periods);
    assert_eq!(main.len(), 2);
    assert_eq!(previous.len(), 3);
}
