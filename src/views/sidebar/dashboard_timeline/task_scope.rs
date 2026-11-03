//! Current task scope and collapsed history, projected from immutable provider snapshots.
use super::*;

impl Sidebar {
    fn task_scope_disclosure(
        &mut self,
        scope: Scope,
        group: String,
        label: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (scope, group.clone());
        let open = self.dashboard_timelines.group_opened.contains(&key);
        let focus = self
            .dashboard_timelines
            .group_focus
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let click_key = key.clone();
        let selector = format!("dashboard-task-group-{}-{}-{group}", scope.0, scope.1.raw());
        super::super::super::components::inline_disclosure(
            gpui::SharedString::from(selector.clone()),
            label.clone(),
        )
        .debug_selector(move || selector.clone())
        .w_full()
        .h_auto()
        .min_w_0()
        .flex_shrink_0()
        .justify_start()
        .gap(rpx(SPACE_LG))
        .py(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .line_height(rpx(TIMELINE_TASK_LINE_H))
        .track_focus(&focus)
        .tab_index(0)
        .tab_stop(true)
        .aria_expanded(open)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            focus.focus(window, cx);
            cx.stop_propagation();
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.dashboard_timelines.group_opened.remove(&click_key) {
                this.dashboard_timelines
                    .group_opened
                    .insert(click_key.clone());
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .capture_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if !this.dashboard_timelines.group_opened.remove(&key) {
                        this.dashboard_timelines.group_opened.insert(key.clone());
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
            }),
        )
        .child(
            icon(
                if open { "chev-down" } else { "chev-right" },
                ICON_SM,
                c::FG_DIM(),
            )
            .flex_shrink_0(),
        )
        .child(div().flex_1().min_w_0().whitespace_normal().child(label))
        .into_any_element()
    }

    pub(super) fn render_dashboard_tasks(
        &mut self,
        scope: Scope,
        data: &TimelineData,
        status: Status,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("{}-{}", scope.0, scope.1.raw());
        let projection = &data.task_scope;
        if data.list.is_none() && projection.history.is_empty() {
            return render_body(&key, data, status, cx);
        }
        let tasks = data
            .list
            .as_ref()
            .map_or(&[][..], |list| list.tasks.as_slice());
        let live_keys: HashSet<_> = tasks
            .iter()
            .chain(projection.history.iter().flat_map(|h| &h.tasks))
            .map(task_key)
            .collect();
        self.dashboard_timelines
            .task_opened
            .retain(|(owner, task)| *owner != scope || live_keys.contains(task));
        self.dashboard_timelines
            .task_focus
            .retain(|(owner, task), _| *owner != scope || live_keys.contains(task));
        self.dashboard_timelines
            .task_hovered
            .retain(|(owner, task)| *owner != scope || live_keys.contains(task));
        let mut body = div()
            .w_full()
            .min_w_0()
            .pl(rpx(SPACE_20))
            .py(rpx(SPACE_LG))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM));
        if projection.awaiting_update {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Awaiting task update · checklist predates your latest input"),
            );
        }
        if tasks.is_empty() {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("No current tasks"),
            );
        }
        if projection.added + projection.removed > 0 {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{} added · {} removed",
                        projection.added, projection.removed
                    )),
            );
        }
        body = body.child(self.render_dashboard_task_rows(scope, data, status, tasks, false, cx));
        let unassigned: Vec<_> = data
            .activity
            .iter()
            .filter(|activity| {
                activity.task.as_ref().is_none_or(|owner| {
                    !tasks
                        .iter()
                        .chain(projection.history.iter().flat_map(|h| &h.tasks))
                        .any(|task| task.owns_activity(owner))
                })
            })
            .cloned()
            .collect();
        let (current_unassigned, unassigned): (Vec<_>, Vec<_>) =
            unassigned.into_iter().partition(|activity| {
                activity.task.is_none()
                    && data
                        .current_turn
                        .as_deref()
                        .is_some_and(|turn| activity.turn_id.as_deref() == Some(turn))
            });
        if !current_unassigned.is_empty() {
            body = body.child(render_activity_with_current(
                &key,
                &format!("{key}-current-unassigned"),
                &current_unassigned,
                status,
                shimmering_activity(&current_unassigned, status),
                false,
                cx,
            ));
        }
        if !projection.history.is_empty() || !unassigned.is_empty() {
            body = body.child(self.task_scope_disclosure(
                scope,
                "history".into(),
                "Earlier work & revisions".into(),
                cx,
            ));
            if self
                .dashboard_timelines
                .group_opened
                .contains(&(scope, "history".into()))
            {
                for revision in &projection.history {
                    let group = format!("revision-{}", revision.item);
                    let completed = revision
                        .tasks
                        .iter()
                        .filter(|task| task.status == TaskStatus::Completed)
                        .count();
                    let removed = revision.tasks.len() - completed;
                    let label =
                        format!("Earlier scope · {completed} completed · {removed} removed");
                    let mut section = div()
                        .pl(rpx(ICON_LG + SPACE_LG))
                        .min_w_0()
                        .child(self.task_scope_disclosure(scope, group.clone(), label, cx));
                    if self
                        .dashboard_timelines
                        .group_opened
                        .contains(&(scope, group))
                    {
                        section = section.child(self.render_dashboard_task_rows(
                            scope,
                            data,
                            status,
                            &revision.tasks,
                            true,
                            cx,
                        ));
                    }
                    body = body.child(section);
                }
                if !unassigned.is_empty() {
                    body = body.child(render_activity_with_current(
                        &key,
                        &format!("{key}-unassigned"),
                        &unassigned,
                        Status::Idle,
                        None,
                        false,
                        cx,
                    ));
                }
            }
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct ScopeProbe {
        sidebar: Entity<Sidebar>,
        data: TimelineData,
        width: Option<f32>,
    }
    impl Render for ScopeProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let content = self.sidebar.update(cx, |sidebar, cx| {
                sidebar.render_dashboard_tasks(
                    (1, SessionId::from_raw(1)),
                    &self.data,
                    Status::Working,
                    window,
                    cx,
                )
            });
            div()
                .w_full()
                .when_some(self.width, |el, width| el.w(gpui::px(width)))
                .child(content)
        }
    }
    fn init(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    }
    fn fixture(revised: bool, stale: bool) -> TimelineData {
        let mut items = vec![
            json!({"type":"userMessage"}),
            json!({"type":"plan","id":"initial","plan":[
                {"id":"done","step":"Inspect task ownership for Codex and Claude","status":"completed"},
                {"id":"active","step":"Implement task grouping with long labels that wrap at narrow widths","status":"inProgress"},
                {"id":"removed","step":"Install the previous implementation","status":"pending"}
            ]}),
        ];
        if revised {
            items.push(json!({"type":"userMessage"}));
            items.push(json!({"type":"plan","id":"revision","plan":[
                {"id":"active","step":"Implement task grouping with long labels that wrap at narrow widths","status":"inProgress"},
                {"id":"verify","step":"Verify revisions for Codex and Claude","status":"pending"}
            ]}));
        }
        if stale {
            items.push(json!({"type":"userMessage"}));
        }
        let task_scope = Arc::new(grove_core::tasks::scope(&items));
        TimelineData {
            list: task_scope.current.clone().map(Arc::new),
            activity: Vec::new(),
            task_scope,
            current_turn: None,
        }
    }
    #[gpui::test]
    fn current_scope_disclosures_preserve_revisions_and_empty_history(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (probe, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            ScopeProbe {
                sidebar,
                data: fixture(false, false),
                width: None,
            }
        });
        cx.simulate_resize(gpui::size(gpui::px(300.), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let data = fixture(false, false);
        let done = &data.list.as_ref().unwrap().tasks[0];
        let selector: &'static str =
            Box::leak(format!("dashboard-task-label-1-1-{}", task_key(done)).into_boxed_str());
        assert!(
            cx.debug_bounds(selector).is_some(),
            "completed tasks remain in the list"
        );
        assert!(cx
            .debug_bounds("dashboard-task-group-1-1-completed")
            .is_none());
        // A follow-up marks the checklist stale while keeping current tasks visible.
        probe.update(cx, |probe, cx| {
            probe.data = fixture(false, true);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds(selector).is_some());
        probe.update(cx, |probe, cx| {
            probe.data = fixture(true, false);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let history = cx.debug_bounds("dashboard-task-group-1-1-history").unwrap();
        cx.simulate_click(history.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let revision = cx
            .debug_bounds("dashboard-task-group-1-1-revision-3")
            .unwrap();
        cx.simulate_click(revision.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds(selector).is_some(),
            "removed completed task stays in history"
        );
        assert!(
            cx.debug_bounds("dashboard-task-label-1-1-1-removed")
                .is_some(),
            "unfinished removed task is retained"
        );
        let label = cx
            .debug_bounds("dashboard-task-label-1-1-1-active")
            .unwrap();
        assert!(label.right() <= gpui::px(300.));
        // Clearing current tasks cannot expose old work as current or discard history.
        probe.update(cx, |probe, cx| {
            let mut data = fixture(true, false);
            data.list = None;
            let projection = Arc::make_mut(&mut data.task_scope);
            projection.current = None;
            probe.data = data;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx
            .debug_bounds("dashboard-task-label-1-1-1-active")
            .is_none());
        assert!(cx.debug_bounds(selector).is_some());
    }

    #[gpui::test]
    fn current_taskless_activity_after_clear_is_visible_outside_history(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            let mut data = fixture(true, false);
            data.list = None;
            Arc::make_mut(&mut data.task_scope).current = None;
            data.current_turn = Some("new-turn".into());
            for (index, turn) in [(40, "old-turn"), (41, "new-turn")] {
                data.activity.push(Activity {
                    index,
                    text: format!("Checking taskless work {index}"),
                    observed_active: None,
                    task: None,
                    explicit_status: Some(grove_core::activity::StepStatus::InProgress),
                    turn_id: Some(turn.into()),
                });
            }
            ScopeProbe {
                sidebar,
                data,
                width: None,
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-41").is_some());
        assert!(cx.debug_bounds("dashboard-activity-label-1-1-40").is_none());
    }

    #[test]
    #[ignore = "requires macOS Metal headless renderer; captures task scope interactions"]
    fn task_scope_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            init(cx);
            crate::fonts::register(cx).unwrap();
        });
        for (name, width, rem, theme) in [
            ("wide", 700., crate::zoom::REM_BASE, "grove-dark"),
            ("narrow", 300., crate::zoom::REM_BASE, "grove-dark"),
            ("zoom-light", 380., 20., "grove-light"),
        ] {
            cx.update(|cx| {
                grove_core::theme::set(grove_core::theme::by_name(theme).unwrap());
                c::sync_component_theme(cx);
            });
            let mut probe = None;
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let runtime = cx.new(Runtime::new);
                    let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
                    let view = cx.new(|_| ScopeProbe {
                        sidebar,
                        data: fixture(true, true),
                        width: None,
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
            for pose in [
                "collapsed",
                "history",
                "revision",
                "hover",
                "keyboard-collapsed",
            ] {
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                    let sidebar = probe.as_ref().unwrap().read(cx).sidebar.clone();
                    let group = match pose {
                        "history" => Some("history"),
                        "revision" => Some("revision-3"),
                        _ => None,
                    };
                    if let Some(group) = group {
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.dashboard_timelines.group_focus
                                [&((1, SessionId::from_raw(1)), group.into())]
                                .focus(window, cx);
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
                    if pose == "hover" {
                        window.dispatch_event(
                            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                position: gpui::point(gpui::px(40.), gpui::px(40.)),
                                pressed_button: None,
                                modifiers: gpui::Modifiers::default(),
                            }),
                            cx,
                        );
                    }
                    if pose == "keyboard-collapsed" {
                        window.dispatch_event(
                            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                keystroke: gpui::Keystroke::parse("space").unwrap(),
                                is_held: false,
                                prefer_character_input: false,
                            }),
                            cx,
                        );
                    }
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                let output = format!("/tmp/grove-task-scope-{name}-{pose}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&output)
                    .unwrap();
                eprintln!("Native task scope: {output}");
            }
        }
    }
    /// Real X11 window capture, without installing or opening a saved session.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated native X11 QA window and uses ImageMagick for screenshots"]
    fn task_scope_linux_window() {
        use gpui::AppContext as _;
        let width = std::env::var("GROVE_TASK_SCOPE_QA_WIDTH")
            .ok()
            .and_then(|n| n.parse::<f32>().ok())
            .unwrap_or(700.);
        let rem = std::env::var("GROVE_TASK_SCOPE_QA_REM")
            .ok()
            .and_then(|n| n.parse::<f32>().ok())
            .unwrap_or(crate::zoom::REM_BASE);
        let theme =
            std::env::var("GROVE_TASK_SCOPE_QA_THEME").unwrap_or_else(|_| "grove-dark".into());
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                init(cx);
                crate::fonts::register(cx).unwrap();
                grove_core::theme::set(grove_core::theme::by_name(&theme).unwrap());
                c::sync_component_theme(cx);
                let mut probe = None;
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(width), gpui::px(700.)),
                            ))),
                            titlebar: Some(gpui::TitlebarOptions {
                                title: Some("Grove task scope QA".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_rem_size(gpui::px(rem));
                            let runtime = cx.new(Runtime::new);
                            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
                            let view = cx.new(|_| ScopeProbe {
                                sidebar,
                                data: fixture(true, true),
                                width: Some(width),
                            });
                            probe = Some(view.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(view, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for pose in ["collapsed", "history", "revision", "keyboard-collapsed"] {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                        cx.update_window(window.into(), |_, window, cx| {
                            let sidebar = probe.as_ref().unwrap().read(cx).sidebar.clone();
                            let group = match pose {
                                "history" => Some("history"),
                                "revision" => Some("revision-3"),
                                _ => None,
                            };
                            if let Some(group) = group {
                                sidebar.update(cx, |sidebar, cx| {
                                    sidebar.dashboard_timelines.group_focus
                                        [&((1, SessionId::from_raw(1)), group.into())]
                                        .focus(window, cx);
                                });
                            }
                            if pose != "collapsed" {
                                window.dispatch_event(
                                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                        keystroke: gpui::Keystroke::parse(
                                            if pose == "keyboard-collapsed" {
                                                "space"
                                            } else {
                                                "enter"
                                            },
                                        )
                                        .unwrap(),
                                        is_held: false,
                                        prefer_character_input: false,
                                    }),
                                    cx,
                                );
                            }
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(250))
                            .await;
                        let output =
                            format!("/tmp/grove-task-scope-linux-{theme}-{width}-{pose}.png");
                        let status = std::process::Command::new("import")
                            .args(["-window", "Grove task scope QA", &output])
                            .status()
                            .unwrap();
                        assert!(status.success(), "native capture failed");
                        eprintln!("Native X11 task scope: {output}");
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }
}
