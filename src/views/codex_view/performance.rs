/// Deterministic GPUI structural workload using NoopTextSystem font metrics.
/// The separate native workload below uses actual CoreText and bundled fonts.
/// Neither CPU workload measures compositor/GPU presentation or displayed FPS.
mod performance {
    use super::*;
    use std::time::Instant;

    fn samples() -> usize {
        std::env::var("GROVE_PERF_SAMPLES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(31)
    }

    #[derive(Clone, Copy)]
    struct Timings {
        elapsed: std::time::Duration,
        total: std::time::Duration,
        first: std::time::Duration,
        update: std::time::Duration,
    }

    fn emit(workload: &str, operation: &str, sample: usize, items: usize, timing: Timings) {
        let Timings {
            elapsed,
            total,
            first,
            update,
        } = timing;
        println!(
            "TRANSCRIPT_PERF {}",
            json!({
                "workload":workload, "operation":operation, "sample":sample,
                "ms":elapsed.as_secs_f64()*1000.0, "settle_ms":total.as_secs_f64()*1000.0, "first_frame_ms":first.as_secs_f64()*1000.0, "update_ms":update.as_secs_f64()*1000.0, "items":items,
                "completed":true, "measurement":"gpui_cpu_render_layout_paint",
            })
        );
    }

    fn workload_state(workload: &str) -> Snapshot {
        let mut state = snapshot();
        if workload == "real" {
            let path = std::env::var("GROVE_PERF_SESSION")
                .expect("real workload needs GROVE_PERF_SESSION");
            let saved: SavedSession = serde_json::from_slice(&fs_err::read(path).unwrap()).unwrap();
            state = Snapshot::new(saved);
            state.status = Status::Idle;
        } else if let Some(count) = workload.strip_prefix("many-") {
            let count: usize = count.parse().unwrap();
            state.saved.items = (0..count).map(|i| json!({
                "id":format!("perf-{i}"), "type":if i%4==0 {"userMessage"} else {"agentMessage"},
                "content":[{"type":"text", "text":format!("Input {i}")}],
                "text":format!("## Response {i}\n\nA measured **Markdown** paragraph with `inline code`, details and a [link](https://example.com).\n\nA second paragraph preserves the complete response."),
                "groveCompleted":true, "phase":"final_answer",
            })).collect();
        } else if let Some(count) = workload.strip_prefix("long-") {
            let count: usize = count.parse().unwrap();
            let text = (0..count).fold(String::new(), |mut text, i| {
                writeln!(text, "Paragraph {i}: Representative **Markdown** prose with `code`, several words and full text retained.").unwrap();
                text.push('\n');
                text
            });
            state.saved.items = vec![json!({"id":"long-response", "type":"agentMessage", "text":text, "groveCompleted":true, "phase":"final_answer"})].into();
        } else if workload.starts_with("giant-") || workload.starts_with("code-") {
            let count: usize = workload.split_once('-').unwrap().1.parse().unwrap();
            let text = if workload.starts_with("code-") {
                format!(
                    "```rust\n{}\n```",
                    (0..count).fold(String::new(), |mut text, i| {
                        writeln!(text, "let variable_{i} = {i};").unwrap();
                        text
                    })
                )
            } else {
                "single paragraph of representative prose with words and formatting ".repeat(count)
            };
            state.saved.items = vec![json!({"id":"giant-response", "type":"agentMessage", "text":text, "groveCompleted":true, "phase":"final_answer"})].into();
        } else {
            panic!("unknown workload {workload}");
        }
        if std::env::var("GROVE_PERF_BUSY").as_deref() == Ok("1") {
            state.status = Status::Working;
            state.turn_id = Some("perf-working-turn".into());
            let now = Instant::now();
            state.work_started = Some(
                now.checked_sub(std::time::Duration::from_mins(1))
                    .unwrap_or(now),
            );
            if let Some(period) = state.saved.work_periods.last_mut() {
                period.end_item = None;
                period.duration_ms = None;
            } else {
                let start_item = state
                    .saved
                    .items
                    .iter()
                    .rposition(|item| item["type"] == "userMessage")
                    .unwrap_or(0);
                state.saved.work_periods.push(codex::WorkPeriod {
                    start_item,
                    end_item: None,
                    duration_ms: None,
                    subagents: vec![],
                });
            }
        }
        if std::env::var("GROVE_PERF_PROVIDER").as_deref() == Ok("claude") {
            state.saved.provider = codex::Provider::Claude;
        }
        state
    }

    #[cfg(target_os = "macos")]
    fn fixture_context(state: &Snapshot) -> serde_json::Value {
        let tools: Vec<_> = state
            .saved
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| is_tool_item(item.as_ref()).then_some(index))
            .collect();
        let start = active_work_start(state);
        let shimmer = tools
            .iter()
            .filter(|index| work_is_active_at(state, **index) && tool_visible(state, **index))
            .count();
        if state.status.busy() && !tools.is_empty() {
            assert!(state.work_started.is_some() && state.turn_id.is_some());
            assert!(
                state
                    .saved
                    .work_periods
                    .last()
                    .is_some_and(|period| period.end_item.is_none())
            );
            assert_eq!(
                shimmer, 1,
                "Working fixture must retain the newest active tool presentation"
            );
        }
        json!({"busy":state.status.busy(),"status":state.status.label(),"work_started":state.work_started.is_some(),"turn_context":state.turn_id.is_some(),"active_work_start":start,"total_source_tool_items":tools.len(),"current_work_source_tool_items":tools.iter().filter(|index| **index>=start).count(),"current_tool_shimmer_candidates":shimmer})
    }

    #[gpui::test]
    #[ignore = "manual structural transcript CPU workload with NoopTextSystem"]
    fn transcript_frame_benchmark(cx: &mut gpui::TestAppContext) {
        let workload = std::env::var("GROVE_PERF_WORKLOAD").unwrap_or_else(|_| "many-200".into());
        let state = workload_state(&workload);
        let item_count = state.saved.items.len();
        let last_prose = state
            .saved
            .items
            .iter()
            .rposition(|item| {
                item["type"] == "agentMessage"
                    && !item["text"].as_str().unwrap_or_default().is_empty()
            })
            .unwrap();
        let expected_last = state.saved.items.last().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let original = Rc::new(state);
        let (view, cx, _, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(900.)));
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                if workload == "real" {
                    view.expanded.insert(work_key_at(&original, last_prose));
                }
                view.session.update(cx, |session, cx| {
                    session.state = original.as_ref().clone().into();
                    cx.notify();
                });
                cx.notify();
            });
        });
        let cold = Instant::now();
        draw(cx);
        emit(
            &workload,
            "cold_mount",
            0,
            item_count,
            Timings {
                elapsed: cold.elapsed(),
                total: cold.elapsed(),
                first: std::time::Duration::ZERO,
                update: std::time::Duration::ZERO,
            },
        );
        for _ in 0..3 {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            draw(cx);
        }
        assert!(cx.debug_bounds("codex-messages").is_some());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        draw(cx);
        let pane = cx.debug_bounds("codex-messages").unwrap();
        let last_anchor = cx
            .debug_bounds(Box::leak(
                format!("codex-message-anchor-{last_prose}").into_boxed_str(),
            ))
            .expect("last prose anchor must render");
        assert!(
            last_anchor.bottom() <= pane.bottom() + gpui::px(1.),
            "bottom navigation must expose final prose"
        );
        assert!(
            last_anchor.bottom() > pane.top(),
            "last prose must be visible"
        );
        view.read_with(cx, |view, cx| {
            let session = view.session.read(cx);
            assert_eq!(session.state.saved.items.len(), item_count);
            assert_eq!(
                session.state.saved.items.last().unwrap()["id"],
                expected_last
            );
            assert!(session.state.error.is_none());
        });
        // A fresh dirty notification is mandatory for every measured frame.
        // Alternate operation order to reduce warm-cache/thermal ordering bias.
        for sample in 0..samples() {
            let operations = if sample % 2 == 0 {
                ["redraw", "scroll", "snapshot", "append"]
            } else {
                ["append", "snapshot", "scroll", "redraw"]
            };
            for operation in operations {
                let start = Instant::now();
                cx.update(|_, cx| {
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
                                let y = if sample % 2 == 0 {
                                    gpui::px(-200.)
                                } else {
                                    gpui::px(-400.)
                                };
                                view.scroll.set_offset(gpui::point(gpui::px(0.), y));
                            }
                            _ => {}
                        }
                        cx.notify();
                    });
                });
                let update_elapsed = start.elapsed();
                let first_start = Instant::now();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let first_elapsed = first_start.elapsed();
                draw(cx);
                let settled = start.elapsed();
                // Measure exactly one forced dirty GPUI frame separately from
                // the update/parser/event-loop settling work above.
                cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
                let frame_start = Instant::now();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let elapsed = frame_start.elapsed();
                if operation == "append" {
                    view.read_with(cx, |view, cx| {
                        assert!(
                            view.selection_views.values().any(|owner| owner
                                .read(cx)
                                .source()
                                .contains(&format!("PERF_COMPLETED_APPEND_{sample}"))),
                            "appended text must reach native TextView source"
                        );
                    });
                }
                assert!(cx.debug_bounds("codex-messages").unwrap().size.height > gpui::px(0.));
                emit(
                    &workload,
                    operation,
                    sample,
                    item_count,
                    Timings {
                        elapsed,
                        total: settled,
                        first: first_elapsed,
                        update: update_elapsed,
                    },
                );
            }
        }
        // Verify completed work rather than accepting fast skipped/failed frames.
        view.read_with(cx, |view, cx| {
            let session = view.session.read(cx);
            assert_eq!(session.state.saved.items.len(), item_count);
            assert_eq!(
                session.state.saved.items.last().unwrap()["id"],
                expected_last
            );
            assert!(session.state.error.is_none());
            assert!(
                view.selection_views
                    .values()
                    .any(|owner| !owner.read(cx).source().is_empty())
            );
        });
    }
    #[cfg(target_os = "macos")]
    include!("performance_native.rs");
    #[cfg(target_os = "macos")]
    include!("performance_native_spread.rs");
}
