//! Selected-session context occupancy and provider-backed attribution.
use super::*;
use crate::views::components::{form_action, project_field_well};
use gpui_component::Disableable;
use grove_core::provider::{self, ContextCategoryKind, ContextMeasurement, Provider, Status};

const METER_H: f32 = 5.;
const WARNING_PERCENT: u32 = 80;
const REFRESH_W: f32 = 28.;
/// Quiet measurement provenance, smaller than the category rows.
const FOOTER_TEXT: f32 = 10.;

#[derive(Default)]
pub(super) struct ContextPanel {
    observer: Option<(gpui::EntityId, gpui::Subscription)>,
    measured: HashMap<SessionId, (gpui::EntityId, u64)>,
    feedback: super::action_feedback::Feedbacks,
    expiry: HashMap<SessionId, gpui::Task<()>>,
}

fn text(value: impl Into<SharedString>) -> Div {
    div()
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM())
        .child(value.into())
}
fn metric(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(rpx(SPACE_LG))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(rpx(TEXT_SMALL))
                .child(label.into()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(rpx(TEXT_SMALL))
                .child(value.into()),
        )
}
fn meter(used: u64, window: u64, warn: bool) -> Div {
    let fraction = if window == 0 {
        0.
    } else {
        (used as f32 / window as f32).clamp(0., 1.)
    };
    div()
        .h(rpx(METER_H))
        .w_full()
        .rounded_full()
        .bg(c::BORDER())
        .overflow_hidden()
        .child(div().h_full().w(gpui::relative(fraction)).bg(
            if warn && fraction >= WARNING_PERCENT as f32 / 100. {
                c::AMBER()
            } else {
                c::FG_MUTE()
            },
        ))
}

/// Missing/zero denominators and inconsistent subcounts do not become zero bars.
fn share_ratio(count: Option<u64>, parent: Option<u64>) -> Option<f32> {
    let (count, parent) = (count?, parent?);
    (parent > 0 && count <= parent).then_some(count as f32 / parent as f32)
}

/// Only known used categories enter the five summary groups. Missing is not zero.
fn summary_categories(details: &provider::ContextDetails) -> Vec<(&'static str, u64)> {
    const LABELS: [&str; 5] = [
        "Messages",
        "System prompt",
        "Tools & MCP instructions",
        "Memory files",
        "Skills & agents",
    ];
    let mut counts = [None::<u64>; 5];
    for category in &details.categories {
        if category.kind != ContextCategoryKind::Used {
            continue;
        }
        let group = match category.id.as_str() {
            "messages" => 0,
            "system_prompt" => 1,
            "system_tools" | "mcp_tools" | "mcp_instructions" => 2,
            "memory_files" => 3,
            "skills" | "custom_agents" => 4,
            _ => continue,
        };
        counts[group] = Some(counts[group].unwrap_or(0).saturating_add(category.tokens));
    }
    LABELS
        .into_iter()
        .zip(counts)
        .filter_map(|(label, count)| count.map(|count| (label, count)))
        .collect()
}

fn share_metric(label: &str, count: u64, parent: Option<u64>) -> gpui::Stateful<Div> {
    let share = share_ratio(Some(count), parent);
    let mut row = div()
        .id(SharedString::from(format!("context-category-{label}")))
        .debug_selector({
            let label = label.to_owned();
            move || format!("context-category-{label}")
        })
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .child(metric(label.to_owned(), provider::compact_tokens(count)));
    if let Some(fraction) = share {
        row = row.child(
            div()
                .id(SharedString::from(format!("share-{label}")))
                .role(gpui::Role::Meter)
                .aria_label(format!(
                    "{label}: {count} of {} tokens, {:.0}%",
                    parent.unwrap_or(0),
                    fraction * 100.
                ))
                .child(meter(count, parent.unwrap_or(0), false)),
        );
    }
    row
}

fn measured_age(at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match now.saturating_sub(at) {
        0..=59 => "just now".into(),
        seconds @ 60..=3599 => format!("{} minutes ago", seconds / 60),
        seconds @ 3600..=86399 => format!("{} hours ago", seconds / 3600),
        seconds => format!("{} days ago", seconds / 86400),
    }
}

impl Sidebar {
    pub(super) fn render_context_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.dashboard_open {
            self.context_panel.observer = None;
            return None;
        }
        let Some(Selection::Session(id)) = self.selection else {
            self.context_panel.observer = None;
            return None;
        };
        let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
        let Some(session) = session else {
            self.context_panel.observer = None;
            return None;
        };
        if self
            .context_panel
            .observer
            .as_ref()
            .is_none_or(|(owner, _)| *owner != session.entity_id())
        {
            self.context_panel.observer = Some((
                session.entity_id(),
                cx.observe(&session, |_, _, cx| cx.notify()),
            ));
        }
        let has_transport = session.read(cx).has_transport();
        let state = session.read(cx).state.shared();
        // Provider events update occupancy during work. Count full Claude categories only
        // once per new idle generation; idle time/redraws alone never poll the counter API.
        let generation = (session.entity_id(), state.context_measurement_revision());
        if has_transport
            && state.saved.provider == Provider::Claude
            && matches!(state.status, Status::Idle | Status::Done)
            && !state.compacting
            && !state.reconnecting
            && !state.context_query_pending
            && (!state.context_measurement_live
                || state
                    .saved
                    .context_details
                    .as_ref()
                    .is_none_or(|details| details.source == ContextMeasurement::ProviderUsage))
            && self.context_panel.measured.get(&id) != Some(&generation)
        {
            self.context_panel.measured.insert(id, generation);
            let target = session.clone();
            cx.defer(move |cx| {
                target.update(cx, |session, cx| {
                    session.send(provider::Action::RefreshContext, cx)
                })
            });
        }
        let usage = state.saved.context.filter(|u| u.window > 0);
        let details = state.saved.context_details.as_ref();
        let disconnected =
            matches!(state.status, Status::Disconnected | Status::Starting) || state.reconnecting;
        let mut panel = project_field_well(false, false)
            .id("session-context")
            .debug_selector(|| "session-context".into())
            .w_full()
            .min_w_0()
            .h_auto()
            .flex()
            .flex_col()
            .p(rpx(SPACE_2XL))
            .gap(rpx(SPACE_LG));
        let title = if state.compacting {
            "Compacting context"
        } else {
            "Context"
        };
        let mut header = div()
            .flex()
            .w_full()
            .items_center()
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rpx(TEXT_BODY))
                    .child(title),
            );
        if let Some(usage) = usage {
            header = header.child(
                div()
                    .flex_shrink_0()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(if usage.percent() >= WARNING_PERCENT {
                        c::AMBER()
                    } else {
                        c::FG_DIM()
                    })
                    .child(format!("{}% used", usage.percent())),
            );
        }
        if state.saved.provider == Provider::Claude {
            let owner = format!("context-{}-{:?}", id.raw(), session.entity_id());
            let previous = self.context_panel.feedback.get(&owner, "context");
            if state.context_query_pending
                && previous.as_ref().is_none_or(|f| f.completed.is_some())
            {
                self.context_panel
                    .feedback
                    .begin(&owner, "context", "Measuring context");
            } else if !state.context_query_pending {
                if let Some(previous) = previous.filter(|f| f.completed.is_none()) {
                    let success = state.context_query_error.is_none()
                        && state.context_measurement_live
                        && details.is_some_and(|d| d.source == ContextMeasurement::Full);
                    self.context_panel.feedback.finish(
                        &owner,
                        "context",
                        previous.token,
                        success.then_some("Context measured"),
                        Instant::now(),
                    );
                    if success {
                        self.context_panel.expiry.insert(
                            id,
                            cx.spawn(async |this, cx| {
                                cx.background_executor()
                                    .timer(super::action_feedback::LIFETIME)
                                    .await;
                                let _ = this.update(cx, |_, cx| cx.notify());
                            }),
                        );
                    }
                }
            }
            let feedback = self.context_panel.feedback.get(&owner, "context");
            let refresh_session = session.clone();
            header = header.child(
                form_action("context-refresh", "Refresh context", false, window, cx)
                    .debug_selector(|| "context-refresh".into())
                    .w(rpx(REFRESH_W))
                    .icon_only()
                    .disabled(
                        !has_transport
                            || disconnected
                            || state.context_query_pending
                            || state.compacting
                            || state.status.busy(),
                    )
                    .child(feedback.as_ref().map_or_else(
                        || icon("refresh", ICON_SM, c::FG_DIM()).into_any_element(),
                        |feedback| {
                            super::action_feedback::mark(
                                "context-refresh-feedback",
                                feedback,
                                ICON_SM,
                                cx,
                            )
                        },
                    ))
                    .tooltip("Measure context details")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        refresh_session.update(cx, |session, cx| {
                            session.send(provider::Action::RefreshContext, cx)
                        });
                    })),
            );
        }
        panel = panel.child(header);
        let source = details.map_or("Waiting for measurement", |details| match details.source {
            ContextMeasurement::Full => "",
            ContextMeasurement::Estimate => "Estimated breakdown",
            ContextMeasurement::ProviderUsage => "",
        });
        let model = details.and_then(|d| d.model.as_deref());
        let provenance = if (usage.is_some() || details.is_some())
            && (disconnected || !state.context_measurement_live)
        {
            "Last known"
        } else {
            source
        };
        let mut identity = format!(
            "{}{}",
            state.saved.provider.label(),
            model.map_or(String::new(), |m| format!(" · {m}"))
        );
        if !provenance.is_empty() {
            identity.push_str(&format!(" · {provenance}"));
        }
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(icon(
                    state.saved.provider.agent().icon_name(),
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .child(text(identity).flex_1().min_w_0()),
        );
        if let Some(usage) = usage {
            panel = panel
                .child(
                    div()
                        .id("session-context-meter")
                        .role(gpui::Role::Meter)
                        .aria_label(format!(
                            "Context {}% used, {} of {} tokens",
                            usage.percent(),
                            usage.used,
                            usage.window
                        ))
                        .child(meter(usage.used, usage.window, true)),
                )
                .child(metric(
                    format!("{} used", provider::compact_tokens(usage.used)),
                    format!("{} window", provider::compact_tokens(usage.window)),
                ));
        } else {
            panel = panel.child(text(
                "Usage has not been reported yet. Instructions and tools also take space.",
            ));
        }
        if state.compacting {
            panel = panel.child(text(
                "Summarizing older conversation. Waiting for a new measurement.",
            ));
        } else if (usage.is_some() || details.is_some())
            && (disconnected || !state.context_measurement_live)
        {
            panel = panel.child(text(
                "Saved measurement; usage may have changed. Reconnect to update.",
            ));
        }
        if let Some(error) = &state.context_query_error {
            panel = panel.child(text(format!("Could not measure details: {error}")));
        }
        let categories = details.map(summary_categories).unwrap_or_default();
        if state.saved.provider == Provider::Claude && !categories.is_empty() {
            panel = panel
                .child(div().text_size(rpx(TEXT_SMALL)).child("What uses context"))
                .children(categories.into_iter().map(|(label, count)| {
                    share_metric(label, count, usage.map(|usage| usage.used))
                }));
        } else if state.saved.provider == Provider::Claude {
            panel = panel.child(text(
                "No breakdown reported yet. Use Refresh to measure available details.",
            ));
        }
        if let Some(details) = details {
            panel = panel.child(
                div()
                    .text_size(rpx(FOOTER_TEXT))
                    .text_color(c::FG_DIM())
                    .child(format!("Measured {}", measured_age(details.measured_at))),
            );
        }
        Some(panel.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;
    use grove_core::provider::{
        ContextCategory, ContextDetails, ContextEntry, ContextUsage, SavedSession,
    };

    fn attach(
        sidebar: &mut Sidebar,
        provider: Provider,
        cx: &mut Context<Sidebar>,
    ) -> (
        SessionId,
        Entity<crate::entities::codex_session::CodexSession>,
    ) {
        let registry = sidebar.runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta(String::new(), "/tmp".into(), provider.agent())
        });
        let mut saved = SavedSession::empty(
            format!("context-{}", id.raw()),
            provider,
            String::new(),
            "/tmp".into(),
        );
        saved.context = Some(ContextUsage {
            used: 128_000,
            window: 200_000,
        });
        if provider == Provider::Claude {
            saved.context_details = Some(ContextDetails {
                model: Some("claude-test-model".into()),
                measured_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                source: ContextMeasurement::Full,
                categories: vec![
                    ContextCategory {
                        id: "messages".into(),
                        label: "Messages with a long category name".into(),
                        tokens: 94000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![ContextEntry {
                            label: "Tool results".into(),
                            tokens: 68000,
                        }],
                    },
                    ContextCategory {
                        id: "memory_files".into(),
                        label: "Memory files".into(),
                        tokens: 6000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![ContextEntry {
                            label: "/project/CLAUDE.md".into(),
                            tokens: 6000,
                        }],
                    },
                    ContextCategory {
                        id: "system_prompt".into(),
                        label: "System prompt".into(),
                        tokens: 12000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "system_tools".into(),
                        label: "System tools".into(),
                        tokens: 6000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "mcp_tools".into(),
                        label: "MCP tools".into(),
                        tokens: 6000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "mcp_instructions".into(),
                        label: "MCP instructions".into(),
                        tokens: 2000,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "skills".into(),
                        label: "Skills".into(),
                        tokens: 1500,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "custom_agents".into(),
                        label: "Agents".into(),
                        tokens: 500,
                        kind: ContextCategoryKind::Used,
                        entries: vec![],
                    },
                    ContextCategory {
                        id: "free_space".into(),
                        label: "Free space".into(),
                        tokens: 72000,
                        kind: ContextCategoryKind::Free,
                        entries: vec![],
                    },
                ],
                auto_compact_enabled: Some(true),
                auto_compact_threshold: None,
                accounting: None,
            });
        }
        if provider == Provider::Codex {
            let counters = provider::ContextTokenCounters {
                input_tokens: Some(120000),
                cached_input_tokens: Some(90000),
                cache_write_input_tokens: Some(0),
                output_tokens: Some(8000),
                reasoning_output_tokens: Some(2000),
                total_tokens: Some(128000),
            };
            saved.context_details = Some(ContextDetails {
                model: Some("codex-test-model".into()),
                measured_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                source: ContextMeasurement::ProviderUsage,
                categories: Vec::new(),
                auto_compact_enabled: None,
                auto_compact_threshold: None,
                accounting: Some(provider::ContextTokenAccounting {
                    last: Some(counters),
                    total: Some(provider::ContextTokenCounters {
                        input_tokens: Some(240000),
                        cached_input_tokens: Some(180000),
                        output_tokens: Some(16000),
                        reasoning_output_tokens: Some(4000),
                        total_tokens: Some(256000),
                        ..counters
                    }),
                }),
            });
            saved.items.push(serde_json::json!({"id":"shell","type":"commandExecution","status":"completed","durationMs":1200}));
            saved.items.push(serde_json::json!({"id":"mcp","type":"mcpToolCall","server":"repository","tool":"search","status":"completed","durationMs":800}));
        }
        let session = cx.new(|cx| crate::entities::codex_session::CodexSession::history(saved, cx));
        registry.update(cx, |registry, _| {
            registry.attach_codex(id, session.clone(), provider::Handle::channel().0)
        });
        sidebar.selection = Some(Selection::Session(id));
        sidebar.dashboard_open = false;
        (id, session)
    }

    #[test]
    fn usage_shares_keep_parent_subcounts_and_unknown_values_distinct() {
        assert_eq!(share_ratio(Some(90000), Some(120000)), Some(0.75));
        assert_eq!(share_ratio(Some(2000), Some(8000)), Some(0.25));
        assert_eq!(share_ratio(Some(0), Some(120000)), Some(0.));
        assert_eq!(share_ratio(None, Some(120000)), None);
        assert_eq!(share_ratio(Some(0), Some(0)), None);
        assert_eq!(share_ratio(Some(130000), Some(120000)), None);
    }

    #[test]
    fn summary_groups_only_known_used_categories_without_child_double_counting() {
        let mut details = ContextDetails {
            accounting: None,
            model: None,
            measured_at: 0,
            source: ContextMeasurement::Full,
            categories: vec![],
            auto_compact_enabled: None,
            auto_compact_threshold: None,
        };
        for (id, tokens, kind) in [
            ("messages", 94, ContextCategoryKind::Used),
            ("system_prompt", 12, ContextCategoryKind::Used),
            ("system_tools", 6, ContextCategoryKind::Used),
            ("mcp_tools", 6, ContextCategoryKind::Used),
            ("mcp_instructions", 2, ContextCategoryKind::Used),
            ("memory_files", 6, ContextCategoryKind::Used),
            ("skills", 1, ContextCategoryKind::Used),
            ("custom_agents", 1, ContextCategoryKind::Used),
            ("mcp_tools", 100, ContextCategoryKind::Deferred),
            ("free_space", 72, ContextCategoryKind::Free),
            ("autocompact_buffer", 10, ContextCategoryKind::Buffer),
            ("future_category", 20, ContextCategoryKind::Used),
        ] {
            details.categories.push(ContextCategory {
                id: id.into(),
                label: "Provider label".into(),
                tokens,
                kind,
                entries: vec![ContextEntry {
                    label: "Included child".into(),
                    tokens: 99,
                }],
            });
        }
        assert_eq!(
            summary_categories(&details),
            vec![
                ("Messages", 94),
                ("System prompt", 12),
                ("Tools & MCP instructions", 14),
                ("Memory files", 6),
                ("Skills & agents", 2)
            ]
        );
        details.categories.clear();
        assert!(summary_categories(&details).is_empty());
        details.categories.push(ContextCategory {
            id: "messages".into(),
            label: "Messages".into(),
            tokens: 0,
            kind: ContextCategoryKind::Used,
            entries: vec![],
        });
        assert_eq!(summary_categories(&details), vec![("Messages", 0)]);
    }

    #[gpui::test]
    fn codex_and_claude_context_panel_follows_selected_session_with_summary_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.initial_selection_pending = false;
            attach(&mut sidebar, Provider::Claude, cx);
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-context").is_some());
        assert!(cx.debug_bounds("context-category-Messages").is_some());
        assert!(cx.debug_bounds("context-entries-0").is_none());
        assert!(cx.debug_bounds("context-category-Memory files").is_some());
        assert!(cx.debug_bounds("context-entries-1").is_none());
        let first = sidebar.read_with(cx, |sidebar, _| match sidebar.selection {
            Some(Selection::Session(id)) => id,
            _ => panic!(),
        });
        let second = cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| attach(sidebar, Provider::Codex, cx).0)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-context").is_some());
        assert!(cx.debug_bounds("context-refresh").is_none());
        assert!(cx.debug_bounds("context-accounting-latest").is_none());
        assert!(cx.debug_bounds("context-accounting-total").is_none());
        assert!(cx.debug_bounds("context-tool-activity").is_none());
        assert!(cx.debug_bounds("context-entries-0").is_none());
        assert_ne!(first, second);
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.selection = Some(Selection::Session(first));
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("context-refresh").is_some());
        assert!(cx.debug_bounds("context-accounting-latest").is_none());
        assert!(cx.debug_bounds("context-tool-activity").is_none());
        assert!(cx.debug_bounds("context-category-Messages").is_some());
        assert!(cx.debug_bounds("context-entries-0").is_none());
        // Incoming provider updates redraw the selected panel without changing sessions.
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let session = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(first)
                    .unwrap()
                    .clone();
                session.update(cx, |session, cx| {
                    session.state.saved.context = None;
                    session.state.saved.context_details = None;
                    cx.notify();
                });
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-context-meter").is_none());
        assert!(cx.debug_bounds("context-entries-0").is_none());
        // A selected idle transport is measured once, not on every redraw.
        let (handle, actions) = provider::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = SavedSession::empty(
            "live-context-fixture".into(),
            Provider::Claude,
            String::new(),
            "/tmp".into(),
        );
        let mut snapshot = provider::Snapshot::new(saved);
        snapshot.status = Status::Idle;
        let live = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                snapshot,
                provider::Connection {
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
                        registry.attach_codex(first, live.clone(), handle)
                    });
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(matches!(
            actions.try_recv().unwrap(),
            provider::Action::RefreshContext
        ));
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
        }
        assert!(
            actions.try_recv().is_err(),
            "redraw must not repeat full token-counting queries"
        );
        // Occupancy updates arrive while working; full measurement waits for idle.
        // Use the shared normalized event boundary; this is a fixture, not a Claude wire event.
        cx.update(|_, cx| {
            live.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.thread_id = Some("context-thread".into());
                assert!(session.state.apply(&serde_json::json!({"method":"thread/tokenUsage/updated","params":{"threadId":"context-thread","tokenUsage":{"last":{"totalTokens":12000},"modelContextWindow":200000}}})));
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(
            actions.try_recv().is_err(),
            "working must not trigger a full context query"
        );
        assert_eq!(
            live.read_with(cx, |session, _| session.state.saved.context.unwrap().used),
            12000
        );
        cx.update(|_, cx| {
            live.update(cx, |session, cx| {
                session.state.status = Status::Done;
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(matches!(
            actions.try_recv().unwrap(),
            provider::Action::RefreshContext
        ));
        // Same reported count is still a new generation and must refresh after the next turn.
        cx.update(|_, cx| { live.update(cx, |session, cx| {
            session.state.status = Status::Working;
            session.state.apply(&serde_json::json!({"method":"thread/tokenUsage/updated","params":{"threadId":"context-thread","tokenUsage":{"last":{"totalTokens":12000},"modelContextWindow":200000}}}));
            session.state.context_query_pending = true;
            cx.notify();
        }); });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(actions.try_recv().is_err());
        cx.update(|_, cx| {
            live.update(cx, |session, cx| {
                session.state.status = Status::Done;
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(
            actions.try_recv().is_err(),
            "in-flight measurements must coalesce"
        );
        cx.update(|_, cx| {
            live.update(cx, |session, cx| {
                session.state.context_query_pending = false;
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(matches!(
            actions.try_recv().unwrap(),
            provider::Action::RefreshContext
        ));
        assert!(actions.try_recv().is_err());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.dashboard_open = true;
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-context").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires native X11 display; captures the real context panel"]
    fn context_panel_linux_capture() {
        struct Probe(Entity<Sidebar>, ScrollHandle, f32);
        impl gpui::Render for Probe {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let body = self
                    .0
                    .update(cx, |sidebar, cx| sidebar.render_context_panel(window, cx));
                div()
                    .id("context-qa-scroll")
                    .track_scroll(&self.1)
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    .p(rpx(SPACE_3XL))
                    .bg(c::BG())
                    .font_family(crate::fonts::UI_FAMILY)
                    .child(div().w(rpx(self.2)).children(body))
            }
        }
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(|cx| {
                gpui_component::init(cx);
                crate::fonts::register(cx).unwrap();
                cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
                cx.set_global(crate::zoom::CurrentPtyDims::default());
                cx.set_global(crate::zoom::ZoomState::new(1.0));
                cx.set_reduce_motion(true);
                let mut owner = None;
                let mut probe_owner = None;
                let capture_scroll = ScrollHandle::new();
                let view_scroll = capture_scroll.clone();
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(340.), gpui::px(1050.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove context panel QA");
                            let runtime = cx.new(crate::runtime::Runtime::new);
                            let sidebar = cx.new(|cx| {
                                let mut sidebar = Sidebar::new(runtime, window, cx);
                                attach(&mut sidebar, Provider::Claude, cx);
                                sidebar
                            });
                            owner = Some(sidebar.clone());
                            let probe = cx.new(|_| Probe(sidebar, view_scroll, 380.));
                            probe_owner = Some(probe.clone());
                            cx.new(|cx| gpui_component::Root::new(probe, window, cx))
                        },
                    )
                    .unwrap();
                let owner = owner.unwrap();
                let probe = probe_owner.unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (name, theme, rem, provider, width) in [
                        ("claude-dark", "grove-dark", 16., Provider::Claude, 380.),
                        (
                            "claude-light-zoom",
                            "grove-light",
                            20.,
                            Provider::Claude,
                            300.,
                        ),
                        ("codex-dark", "grove-dark", 16., Provider::Codex, 380.),
                        (
                            "codex-light-zoom",
                            "grove-light",
                            20.,
                            Provider::Codex,
                            300.,
                        ),
                    ] {
                        cx.update(|cx| {
                            probe.update(cx, |probe, cx| {
                                probe.2 = width;
                                cx.notify();
                            });
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                            owner.update(cx, |sidebar, cx| {
                                attach(sidebar, provider, cx);
                                cx.notify();
                            });
                        });
                        cx.update_window(window.into(), |_, window, _| {
                            capture_scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                            window.set_rem_size(gpui::px(rem));
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(Duration::from_millis(500))
                            .await;
                        let output = format!(
                            "{}/.grove-artifacts/context-panel/native-{name}.png",
                            env!("CARGO_MANIFEST_DIR")
                        );
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove context panel QA", &output])
                            .status()
                            .unwrap()
                            .success());
                        if provider == Provider::Codex {
                            cx.update_window(window.into(), |_, window, _| {
                                capture_scroll.scroll_to_bottom();
                                window.refresh();
                            })
                            .unwrap();
                            cx.background_executor()
                                .timer(Duration::from_millis(250))
                                .await;
                            let output = format!(
                                "{}/.grove-artifacts/context-panel/native-{name}-bottom.png",
                                env!("CARGO_MANIFEST_DIR")
                            );
                            assert!(std::process::Command::new("import")
                                .args(["-window", "Grove context panel QA", &output])
                                .status()
                                .unwrap()
                                .success());
                        }
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }
}
