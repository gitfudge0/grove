use super::*;
use serde_json::json;
use std::sync::Arc;

use super::activity::{activity_excerpt, ACTIVITY_HISTORY_LIMIT, ACTIVITY_TEXT_LIMIT};

struct EntranceProbe(Entity<Sidebar>);
impl Render for EntranceProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entrance = self.0.update(cx, |sidebar, cx| {
            let row = div().w_full().h(gpui::px(90.)).into_any_element();
            sidebar.dashboard_session_entrance((1, SessionId::from_raw(1)), row, cx)
        });
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "entrance-composer".into())
                    .h(gpui::px(40.)),
            )
            .child(entrance)
            .child(
                div()
                    .debug_selector(|| "entrance-following".into())
                    .h(gpui::px(30.)),
            )
    }
}
#[gpui::test]
fn dashboard_session_entrance_measures_once_and_finishes_without_replay(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        init_task_fixture(cx);
        cx.set_reduce_motion(false);
    });
    let (probe, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| {
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar
                .dashboard_timelines
                .adopt_session(1, SessionId::from_raw(1), false);
            sidebar
        });
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        EntranceProbe(sidebar)
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let composer = cx.debug_bounds("entrance-composer").unwrap();
    let initial_following = cx.debug_bounds("entrance-following").unwrap();
    assert!(initial_following.top() >= composer.bottom());
    assert!(initial_following.top() < composer.bottom() + gpui::px(90.));
    cx.run_until_parked();
    let key = (1, SessionId::from_raw(1));
    cx.update(|window, cx| {
        let sidebar = probe.read(cx).0.clone();
        sidebar.update(cx, |sidebar, cx| {
            let state = &sidebar.dashboard_timelines.entrances[&key];
            assert_eq!(state.measured, 90.);
            assert!(state.completion.is_some());
            cx.notify();
        });
        let _ = window.draw(cx);
    });
    let before_motion = cx.debug_bounds("entrance-following").unwrap().top();
    // GPUI Animation uses wall-clock Instant, independently of executor timers.
    std::thread::sleep(std::time::Duration::from_millis(30));
    cx.update(|window, cx| {
        probe.update(cx, |_, cx| cx.notify());
        let _ = window.draw(cx);
    });
    let during_motion = cx.debug_bounds("entrance-following").unwrap().top();
    assert!(during_motion > before_motion);
    assert!(during_motion < composer.bottom() + gpui::px(90.));
    assert_eq!(cx.debug_bounds("entrance-composer").unwrap(), composer);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(SESSION_ENTRANCE_MS - 1));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(probe
            .read(cx)
            .0
            .read(cx)
            .dashboard_timelines
            .is_entering(key.0, key.1));
    });
    assert_eq!(cx.debug_bounds("entrance-composer").unwrap(), composer);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(1));
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(!probe
            .read(cx)
            .0
            .read(cx)
            .dashboard_timelines
            .is_entering(key.0, key.1));
        let _ = window.draw(cx);
        assert!(!probe
            .read(cx)
            .0
            .read(cx)
            .dashboard_timelines
            .is_entering(key.0, key.1));
    });
    assert_eq!(cx.debug_bounds("entrance-composer").unwrap(), composer);
    assert_eq!(
        cx.debug_bounds("entrance-following").unwrap().top(),
        composer.bottom() + gpui::px(90.)
    );
}

#[test]
fn task_disclosure_states_use_visible_catalog_glyphs() {
    for open in [false, true] {
        let glyph = task_disclosure_glyph(open);
        let svg = crate::icons::svg_for_path(&format!("icons/{glyph}.svg")).unwrap();
        assert!(
            svg.contains("<path "),
            "task disclosure {open} must paint a shape: {glyph}"
        );
    }
    assert_ne!(task_disclosure_glyph(false), task_disclosure_glyph(true));
}

fn task_fixture() -> TimelineData {
    let report = |id: &str, task: &str, status: &str, label: &str| {
        let args = json!({"id":id,"task_id":task,"status":status,"label":label});
        json!({"type":"mcpToolCall","server":"grove_activity","tool":"report_activity","status":"completed","arguments":args,"result":{"structuredContent":{"accepted":true,"report":args,"turn_id":"fixture-turn"}}})
    };
    let items = vec![
        json!({"type":"plan","taskSnapshot":true,"plan":[{"id":"first","step":"Trace session closing and implement dashboard navigation","status":"inProgress"},{"id":"second","step":"Verify the change and commit it","status":"pending"}]}),
        report(
            "trace",
            "first",
            "completed",
            "Traced the session-close handler",
        ),
        report(
            "navigate",
            "first",
            "completed",
            "Added navigation back to the dashboard",
        ),
        json!({"type":"plan","taskSnapshot":true,"plan":[{"id":"first","step":"Trace session closing and implement dashboard navigation","status":"completed"},{"id":"second","step":"Verify the change and commit it","status":"inProgress"}]}),
        report(
            "check",
            "second",
            "completed",
            "Checked session-close navigation",
        ),
        report(
            "run",
            "second",
            "in_progress",
            "Running navigation checks with enough detail to wrap at narrow widths",
        ),
        report("failed", "second", "failed", "Narrow layout check failed"),
    ];
    let mut activity = recent_activity(&items);
    activity.push(Activity {
        index: 7,
        text: "Inspecting an interrupted check".into(),
        observed_active: Some(false),
        task: grove_core::tasks::current(&items)
            .unwrap()
            .tasks
            .get(1)
            .cloned(),
        explicit_status: Some(grove_core::activity::StepStatus::InProgress),
        turn_id: Some("older-turn".into()),
    });
    TimelineData {
        list: Some(Arc::new(grove_core::tasks::current(&items).unwrap())),
        activity,
        task_scope: Arc::default(),
        current_turn: None,
    }
}
struct TaskProbe {
    sidebar: gpui::Entity<Sidebar>,
    data: TimelineData,
}
impl gpui::Render for TaskProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scope = (1, SessionId::from_raw(1));
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.render_dashboard_tasks(scope, &self.data, Status::Working, window, cx)
        })
    }
}
fn init_task_fixture(cx: &mut App) {
    gpui_component::init(cx);
    cx.set_reduce_motion(true);
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(crate::settings::SettingsState::new(
        grove_core::storage::Store::default(),
    ));
}
#[gpui::test]
fn completed_task_hover_keyboard_and_click_preserve_geometry(cx: &mut gpui::TestAppContext) {
    cx.update(init_task_fixture);
    let (probe, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        sidebar.update(cx, |sidebar, _| {
            sidebar
                .dashboard_timelines
                .group_opened
                .insert(((1, SessionId::from_raw(1)), "completed".into()));
        });
        TaskProbe {
            sidebar,
            data: task_fixture(),
        }
    });
    let fixture = task_fixture();
    let tasks = &fixture.list.as_ref().unwrap().tasks;
    let completed_key = task_key(&tasks[0]);
    let active_key = task_key(&tasks[1]);
    let completed_label: &'static str =
        Box::leak(format!("dashboard-task-label-1-1-{completed_key}").into_boxed_str());
    let active_label: &'static str =
        Box::leak(format!("dashboard-task-label-1-1-{active_key}").into_boxed_str());
    let completed_row: &'static str =
        Box::leak(format!("dashboard-task-1-1-{completed_key}").into_boxed_str());
    let completed_chevron: &'static str =
        Box::leak(format!("dashboard-task-chevron-1-1-{completed_key}").into_boxed_str());
    let completed_status: &'static str =
        Box::leak(format!("dashboard-task-status-1-1-{completed_key}").into_boxed_str());
    for (width, rem) in [
        (620., crate::zoom::REM_BASE),
        (260., crate::zoom::REM_BASE),
        (320., 20.),
    ] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
        cx.update(|window, cx| {
            window.set_rem_size(gpui::px(rem));
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("dashboard-activity-label-1-1-1").is_none(),
            "completed steps start collapsed"
        );
        let label = cx.debug_bounds(completed_label).unwrap();
        let active = cx.debug_bounds(active_label).unwrap();
        assert_eq!(label.left(), active.left());
        assert!(label.right() <= gpui::px(width));
        let before = cx.debug_bounds(completed_row).unwrap();
        cx.simulate_mouse_move(before.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds(completed_chevron).is_some());
        assert_eq!(label, cx.debug_bounds(completed_label).unwrap());
        cx.simulate_click(before.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-1").is_some());
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-1").is_none());
        cx.simulate_mouse_move(
            gpui::point(gpui::px(width - 1.), gpui::px(690.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds(completed_chevron).is_none());
        assert!(
            cx.debug_bounds(completed_status).is_some(),
            "status returns when the pointer leaves, even with keyboard focus"
        );
        assert_eq!(label, cx.debug_bounds(completed_label).unwrap());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-1").is_some());
        assert!(cx.debug_bounds(completed_chevron).is_none());
        assert!(
            cx.debug_bounds(completed_status).is_some(),
            "expanded tasks retain their status when not hovered"
        );
        let child = cx.debug_bounds("dashboard-activity-label-1-1-1").unwrap();
        assert!(child.left() > label.left());
        assert!(child.right() <= gpui::px(width));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-1").is_none());
        // Expose the same data as Claude and Codex; provider boundary coverage
        // is deliberately owned by native_task_activity_smoke, not this fixture.
        let state = cx.update(|_, cx| {
            probe
                .read(cx)
                .sidebar
                .read(cx)
                .dashboard_timelines
                .task_opened
                .len()
        });
        assert_eq!(state, 0);
    }
}

#[test]
#[ignore = "requires macOS Metal renderer; captures actual task component and pointer/key interactions"]
fn task_activity_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        init_task_fixture(cx);
        crate::fonts::register(cx).unwrap();
        grove_core::theme::set(grove_core::theme::by_name("grove-dark").unwrap());
        c::sync_component_theme(cx);
    });
    for (name, width, rem) in [
        ("normal", 620., crate::zoom::REM_BASE),
        ("narrow", 260., crate::zoom::REM_BASE),
        ("zoom", 320., 20.),
        ("reduced-motion", 620., crate::zoom::REM_BASE),
    ] {
        cx.update(|cx| cx.set_reduce_motion(name == "reduced-motion"));
        let mut probe = None;
        let window = cx
            .open_window(gpui::size(gpui::px(width), gpui::px(520.)), |window, cx| {
                window.set_rem_size(gpui::px(rem));
                let runtime = cx.new(Runtime::new);
                let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
                sidebar.update(cx, |sidebar, _| {
                    sidebar
                        .dashboard_timelines
                        .group_opened
                        .insert(((1, SessionId::from_raw(1)), "completed".into()));
                });
                let view = cx.new(|_| TaskProbe {
                    sidebar,
                    data: task_fixture(),
                });
                probe = Some(view.clone());
                cx.new(|cx| {
                    gpui_component::Root::new(view, window, cx)
                        .bordered(false)
                        .bg(c::BG())
                })
            })
            .unwrap();
        cx.run_until_parked();
        let data = task_fixture();
        let task = &data.list.as_ref().unwrap().tasks[0];
        for pose in ["rest", "hover", "expanded", "collapsed"] {
            cx.update_window(window.into(), |_, window, cx| {
                let sidebar = probe.as_ref().unwrap().read(cx).sidebar.clone();
                if pose == "hover" {
                    // Real pointer enters the first task row, whose first text
                    // line occupies this token-derived inset.
                    let position = gpui::point(
                        rpx(SPACE_20 + ICON_LG + SPACE_LG + SPACE_LG).to_pixels(window.rem_size()),
                        rpx(SPACE_LG + SPACE_SM + TIMELINE_TASK_LINE_H / 2.)
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
                if pose == "expanded" || pose == "collapsed" {
                    sidebar.update(cx, |sidebar, cx| {
                        let focus = sidebar.dashboard_timelines.task_focus
                            [&((1, SessionId::from_raw(1)), task_key(task))]
                            .clone();
                        focus.focus(window, cx);
                    });
                    window.dispatch_event(
                        gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                            keystroke: gpui::Keystroke::parse("enter").unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        }),
                        cx,
                    );
                }
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            if pose == "expanded" || pose == "collapsed" {
                let opened = cx.update(|cx| {
                    let sidebar = probe.as_ref().unwrap().read(cx).sidebar.clone();
                    sidebar
                        .read(cx)
                        .dashboard_timelines
                        .task_opened
                        .contains(&((1, SessionId::from_raw(1)), task_key(task)))
                });
                assert_eq!(opened, pose == "expanded");
            }
            let output = format!("/tmp/grove-task-activity-{name}-{pose}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&output)
                .unwrap();
            eprintln!("Native task activity: {output}");
        }
    }
}

#[test]
fn recent_activity_groups_tools_under_short_provider_updates() {
    let mut items = vec![
        json!({"type":"agentMessage","text":"Inspecting hover styling. More detail is in the conversation."}),
        json!({"type":"commandExecution","command":"cat product/design.md"}),
        json!({"type":"reasoning","text":"private reasoning"}),
        json!({"type":"mcpToolCall","tool":"read_file"}),
        json!({"type":"agentMessage","text":"Removing the highlight\nMore detail"}),
    ];
    items.extend((0..1000).map(|_| json!({"type":"commandExecution","command":"raw command"})));
    let activity = recent_activity(&items);
    assert_eq!(
        activity.iter().map(|entry| entry.index).collect::<Vec<_>>(),
        vec![0, 4]
    );
    assert_eq!(activity[0].text, "Inspecting hover styling.");
    assert_eq!(activity[1].text, "Removing the highlight");
    assert_eq!(shimmering_activity(&activity, Status::Working), Some(4));
    assert!(recent_activity(&[json!({"type":"commandExecution","command":"pwd"})]).is_empty());
    assert!(activity_excerpt("```sh\npwd").is_none());
    assert!(
        activity_excerpt(&"界".repeat(1000))
            .unwrap()
            .chars()
            .count()
            <= ACTIVITY_TEXT_LIMIT + 1
    );
    let history = (0..100)
        .map(|n| json!({"type":"agentMessage","text":format!("Step {n}")}))
        .collect::<Vec<_>>();
    assert_eq!(recent_activity(&history).len(), ACTIVITY_HISTORY_LIMIT);
}
#[test]
fn dashboard_disclosure_scope_is_workspace_and_session_owned() {
    let first = SessionId::from_raw(1);
    let second = SessionId::from_raw(2);
    let mut state = DashboardTimelines::default();
    state.toggle((1, first));
    state.toggle((2, first));
    state.toggle((1, second));
    state.reveals.insert((1, second), TimelineReveal::default());
    assert!(!state.is_open(3, first));
    assert!(state.is_open(1, first));
    assert!(state.is_open(1, second));
    assert!(state.is_open(2, first));
    state.toggle((1, second));
    assert!(!state.is_open(1, second));
    assert!(state.is_open(1, first));
    state.retain(&HashSet::from([first]), &HashSet::from([1, 2]));
    assert!(!state.is_open(1, second));
    assert!(state.reveals.is_empty());
    state.retain(&HashSet::from([first]), &HashSet::from([1]));
    assert!(!state.is_open(2, first));
}
#[test]
fn reveal_reversal_uses_painted_height_and_stable_measurements_do_not_restart() {
    let mut state = TimelineReveal {
        visible: true,
        ..Default::default()
    };
    assert!(state.observe_height(214.));
    let revision = state.revision;
    for _ in 0..30 {
        assert!(!state.observe_height(214.));
    }
    assert_eq!(state.revision, revision);
    state.painted.set(121.28);
    state.target(0.);
    assert_eq!(state.from, 121.28);
    assert_eq!(state.to, 0.);
    state.target(214.);
    assert_eq!(state.from, 121.28);
    assert_eq!(state.to, 214.);
}
#[test]
fn activity_scope_warms_only_on_entry_membership_or_workspace_change() {
    let a = SessionId::from_raw(1);
    let b = SessionId::from_raw(2);
    let mut state = DashboardTimelines::default();
    assert!(state.needs_activity_warm(1, true, vec![a, b]));
    for _ in 0..60 {
        assert!(!state.needs_activity_warm(1, true, vec![a, b]));
    }
    assert!(
        !state.needs_activity_warm(1, true, vec![b, a, a]),
        "metadata order/duplicates must not rescan transcripts"
    );
    assert!(
        state.needs_activity_warm(1, true, vec![a]),
        "removed membership changes scope"
    );
    assert!(
        state.needs_activity_warm(1, true, vec![a, b]),
        "newly owned existing sessions need a warm"
    );
    assert!(
        state.needs_activity_warm(2, true, vec![a, b]),
        "workspace switch must not reuse another scope"
    );
    assert!(!state.needs_activity_warm(2, false, vec![a, b]));
    assert!(
        state.needs_activity_warm(2, true, vec![a, b]),
        "reopen must refresh updates missed while hidden"
    );
}
#[test]
fn only_newest_eligible_activity_shimmers_while_session_is_working() {
    let mut activity = recent_activity(&[
        json!({"type":"agentMessage","text":"First update"}),
        json!({"type":"agentMessage","text":"Latest streamed text"}),
    ]);
    assert_eq!(shimmering_activity(&activity, Status::Working), Some(1));
    assert_eq!(shimmering_activity(&activity, Status::Starting), Some(1));
    for status in [
        Status::Waiting,
        Status::Done,
        Status::Idle,
        Status::Failed,
        Status::Disconnected,
        Status::Interrupted,
    ] {
        assert_eq!(shimmering_activity(&activity, status), None);
    }
    activity[1].observed_active = Some(false);
    assert_eq!(
        shimmering_activity(&activity, Status::Working),
        None,
        "completed newest item must not move shimmer onto older items"
    );
    let revised = recent_activity(&[
        json!({"type":"agentMessage","text":"First update"}),
        json!({"type":"agentMessage","text":"Latest streamed text has progressed"}),
    ]);
    assert_eq!(
        activity[1].index, revised[1].index,
        "stream revisions preserve source identity"
    );
}
#[gpui::test]
fn unchanged_sidebar_sync_does_not_request_transcript_refresh(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        Sidebar::new(runtime, window, cx)
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.dashboard_open = true;
            sidebar.sync_task_glances(cx);
            let baseline = sidebar.dashboard_timelines.warm_requests;
            for _ in 0..60 {
                sidebar.sync_task_glances(cx);
            }
            assert_eq!(
                sidebar.dashboard_timelines.warm_requests, baseline,
                "unchanged render-time sync must not request saved.items reads"
            );
            sidebar.dashboard_open = false;
            sidebar.sync_task_glances(cx);
            sidebar.dashboard_open = true;
            sidebar.sync_task_glances(cx);
            assert_eq!(
                sidebar.dashboard_timelines.warm_requests,
                baseline + 1,
                "reopen refreshes activity missed while hidden"
            );
        });
    });
}
fn session_status_metadata(
    provider: grove_core::codex::Provider,
    label: &'static str,
) -> RowMetadata {
    RowMetadata {
        project: "Grove".into(),
        worktree: "Main checkout".into(),
        provider,
        model: "Fixture model".into(),
        effort: None,
        context: "Session state fixture".into(),
        // Deliberately differs from presentation: old raw runtime labels must
        // not hide review, reconnecting, plan-ready or explicit wait states.
        status: Status::Idle,
        display_status: label,
        review_completion: Some(7),
        age: "now".into(),
    }
}
#[test]
fn dashboard_review_action_requires_current_review_state_for_both_providers() {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        for label in [
            "Plan ready",
            "Reconnecting",
            "Waiting",
            "Ready",
            "Approval needed",
            "Failed",
        ] {
            assert_eq!(
                dashboard_review_completion(&session_status_metadata(provider, label)),
                None,
                "{provider:?} {label}"
            );
        }
        for label in ["Review", "Review · unread"] {
            let mut metadata = session_status_metadata(provider, label);
            assert_eq!(dashboard_review_completion(&metadata), Some(7));
            metadata.review_completion = None;
            assert_eq!(dashboard_review_completion(&metadata), None);
        }
    }
}

struct SessionStateProbe {
    sidebar: Entity<Sidebar>,
    metadata: RowMetadata,
}
impl gpui::Render for SessionStateProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.render_dashboard_progress(
                SessionId::from_raw(41),
                "A long session title that yields to status and available actions",
                &self.metadata,
                window,
                cx,
            )
        })
    }
}
#[gpui::test]
fn dashboard_codex_and_claude_session_states_preserve_review_and_narrow_layout(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(init_task_fixture);
    let (probe, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        SessionStateProbe {
            sidebar,
            metadata: session_status_metadata(grove_core::codex::Provider::Codex, "Ready"),
        }
    });
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        for label in [
            "Plan ready",
            "Reconnecting",
            "Waiting",
            "Review",
            "Review · unread",
            "Ready",
        ] {
            for (width, rem) in [(620., crate::zoom::REM_BASE), (280., 20.)] {
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    probe.update(cx, |probe, cx| {
                        probe.metadata = session_status_metadata(provider, label);
                        cx.notify();
                    });
                });
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let status = cx.debug_bounds("dashboard-session-status-41").unwrap();
                let open = cx.debug_bounds("dashboard-open-session-41").unwrap();
                assert!(
                    status.right() <= open.left(),
                    "{provider:?} {label}: status overlaps open action"
                );
                assert!(
                    open.right() <= gpui::px(width),
                    "{provider:?} {label}: open action overflows"
                );
                let review = cx.debug_bounds("dashboard-mark-reviewed-41");
                assert_eq!(
                    review.is_some(),
                    matches!(label, "Review" | "Review · unread"),
                    "{provider:?} {label}"
                );
                let done = cx.debug_bounds("dashboard-mark-done-41");
                assert_eq!(done.is_some(), label == "Ready", "{provider:?} {label}");
                if let Some(done) = done {
                    assert!(done.right() <= open.left());
                }
                if let Some(review) = review {
                    assert!(review.right() <= gpui::px(width));
                }
            }
        }
    }
}

#[test]
fn metadata_shows_model_and_formats_only_actual_effort() {
    let mut metadata = RowMetadata {
        project: "Actual project".into(),
        worktree: sidebar_worktree_name("root", true).into(),
        provider: grove_core::codex::Provider::Codex,
        model: "GPT-6.1".into(),
        effort: None,
        context: String::new(),
        status: Status::Idle,
        display_status: "Ready",
        review_completion: None,
        age: String::new(),
    };
    assert_eq!(metadata.worktree, "Main checkout");
    assert_eq!(metadata.model_label(), "GPT-6.1");
    metadata.effort = Some("xhigh".into());
    assert_eq!(metadata.model_label(), "GPT-6.1 Extra high");
    metadata.provider = grove_core::codex::Provider::Claude;
    metadata.model = "Claude Opus".into();
    metadata.effort = Some("high".into());
    assert_eq!(metadata.model_label(), "Claude Opus High");
    metadata.effort = Some(String::new());
    assert_eq!(metadata.model_label(), "Claude Opus");
}
#[gpui::test]
fn metadata_model_uses_available_width_and_wraps_when_narrow(cx: &mut gpui::TestAppContext) {
    struct MetadataProbe;
    impl gpui::Render for MetadataProbe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            render_metadata(
                &RowMetadata {
                    project: "Project".into(),
                    worktree: "Main checkout".into(),
                    provider: grove_core::codex::Provider::Codex,
                    model: "GPT-6.1 Terra".into(),
                    effort: Some("high".into()),
                    context: String::new(),
                    status: Status::Idle,
                    display_status: "Ready",
                    review_completion: None,
                    age: String::new(),
                },
                window,
            )
        }
    }
    cx.update(gpui_component::init);
    let (_, cx) = cx.add_window_view(|_, _| MetadataProbe);
    for (width, zoom) in [(440., 1.0), (440., 0.7), (110., 1.0)] {
        cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * zoom)));
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(500.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let label = cx.debug_bounds("dashboard-meta-label-codex").unwrap();
        let checkout = cx.debug_bounds("dashboard-meta-label-git-branch").unwrap();
        assert!(label.right() <= gpui::px(width));
        if width > 200. {
            assert!(
                checkout.size.height
                    <= rpx(SESSION_META_LINE_H)
                        .to_pixels(gpui::px(crate::zoom::REM_BASE * zoom))
                        .ceil(),
                "zoom {zoom}: {checkout:?}"
            );
            cx.update(|window, _| {
                let text = "GPT-6.1 Terra High";
                let mut run = window.text_style().to_run(text.len());
                run.font.family = crate::fonts::UI_FAMILY.into();
                let required = super::super::super::components::truncation_safe_label_width(
                    text,
                    rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                    &run,
                    window.text_system(),
                );
                assert!(label.size.width >= required);
            });
        } else {
            assert!(
                label.size.height
                    > rpx(SESSION_META_LINE_H).to_pixels(gpui::px(crate::zoom::REM_BASE))
            );
        }
    }
}
#[test]
fn prior_turn_and_final_answers_never_shimmer_as_current_work() {
    for items in [
        vec![json!({"type":"agentMessage","text":"Final outcome","phase":"final_answer"})],
        vec![
            json!({"type":"agentMessage","text":"Prior step"}),
            json!({"type":"userMessage"}),
            json!({"type":"commandExecution","command":"pwd"}),
        ],
    ] {
        let activity = recent_activity(&items);
        assert_eq!(shimmering_activity(&activity, Status::Working), None);
    }
}
#[gpui::test]
fn expanded_activity_history_preserves_columns_at_narrow_width_and_zoom(
    cx: &mut gpui::TestAppContext,
) {
    struct HistoryProbe(TimelineData);
    impl gpui::Render for HistoryProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .child(render_body("history", &self.0, Status::Waiting, cx))
        }
    }
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
    });
    let items = (0..5).map(|n| json!({"type":"agentMessage","text":format!("Observed step {n} with a longer description that wraps in narrow layouts")})).collect::<Vec<_>>();
    let (_, cx) = cx.add_window_view(|_, _| {
        HistoryProbe(TimelineData {
            list: None,
            activity: recent_activity(&items),
            task_scope: Arc::default(),
            current_turn: None,
        })
    });
    for width in [440., 240.] {
        for rem in [crate::zoom::REM_BASE, 20.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                let _ = window.draw(cx);
            });
            let first = cx
                .debug_bounds("dashboard-activity-label-history-0")
                .unwrap();
            for selector in [
                "dashboard-activity-label-history-1",
                "dashboard-activity-label-history-2",
                "dashboard-activity-label-history-3",
                "dashboard-activity-label-history-4",
            ] {
                let label = cx.debug_bounds(selector).unwrap();
                assert_eq!(first.left(), label.left());
                assert!(label.right() <= gpui::px(width));
            }
        }
    }
}
