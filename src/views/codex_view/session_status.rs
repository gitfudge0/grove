//! Connection recovery, context usage, review summary, and provider notices.

use super::*;

/// Context share, in whole percent, at which the meter switches to the warning colour.
const CONTEXT_WARN_PERCENT: u32 = 80;
/// The context ring's stroke, the icon catalog's optical stroke.
const CONTEXT_RING_STROKE: f32 = 1.5;
/// Polyline segments per full turn of the context ring.
const CONTEXT_RING_SEGMENTS: f32 = 48.0;
/// Keep the review dock an overview; detailed evidence stays in the transcript.
pub(super) fn review_overview(response: &str) -> String {
    super::super::review_summary::ReviewSummary::parse(response, std::path::Path::new("/")).text
}

/// The meter's whole percent and colour: muted at rest, warning at the threshold.
pub(super) fn context_tone(usage: codex::ContextUsage) -> (u32, gpui::Hsla) {
    let percent = usage.percent();
    let color = if percent >= CONTEXT_WARN_PERCENT {
        c::AMBER()
    } else {
        c::FG_MUTE()
    };
    (percent, color)
}

/// The meter's hover detail, e.g. "76k / 200k tokens".
pub(super) fn context_detail(usage: codex::ContextUsage) -> String {
    format!(
        "{} / {} tokens",
        codex::compact_tokens(usage.used),
        codex::compact_tokens(usage.window)
    )
}

/// Context-window occupancy: a vector progress ring and its percentage.
pub(super) fn context_meter(usage: codex::ContextUsage) -> impl IntoElement {
    let (percent, color) = context_tone(usage);
    let detail = context_detail(usage);
    div()
        .id("codex-context-meter")
        .debug_selector(|| "codex-context-meter".into())
        .role(gpui::Role::Meter)
        .aria_label(format!("Context {percent}% used, {detail}"))
        .flex_shrink_0()
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .text_color(color)
        .child(context_ring(percent as f32 / 100.0, color))
        .child(format!("{percent}%"))
}

/// A track circle with a clockwise arc from twelve o'clock. Drawn as vector paths because
/// the bundled fonts carry no geometric-shape glyphs.
fn context_ring(fraction: f32, color: gpui::Hsla) -> impl IntoElement {
    let track = c::BORDER();
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let stroke = rpx(CONTEXT_RING_STROKE).to_pixels(window.rem_size());
            let center = bounds.center();
            let radius = bounds.size.width.min(bounds.size.height) / 2.0 - stroke / 2.0;
            let arc = |sweep: f32| {
                let steps = (CONTEXT_RING_SEGMENTS * sweep).ceil().max(1.0) as usize;
                let mut path = gpui::PathBuilder::stroke(stroke);
                for step in 0..=steps {
                    let angle = std::f32::consts::TAU * sweep * step as f32 / steps as f32
                        - std::f32::consts::FRAC_PI_2;
                    let point = gpui::point(
                        center.x + radius * angle.cos(),
                        center.y + radius * angle.sin(),
                    );
                    if step == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                if sweep >= 1.0 {
                    path.close();
                }
                path.build().ok()
            };
            if let Some(path) = arc(1.0) {
                window.paint_path(path, track);
            }
            if fraction > 0.0 {
                if let Some(path) = arc(fraction.min(1.0)) {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(rpx(ICON_MD))
    .flex_shrink_0()
}

impl CodexView {
    pub(super) fn sync_status_cards(&mut self, state: &codex::Snapshot) {
        let mut keys = HashSet::new();
        if state.session_status() == "Review" {
            if let Some(period) = state.review_completion() {
                keys.insert(format!("review:{period}"));
            }
        }
        if state.error.is_some() || state.status == codex::Status::Disconnected {
            keys.insert(format!("notice:{:?}:{:?}", state.status, state.error));
        }
        if state.plan_ready() {
            for plan in &state.saved.rich_plans.revisions {
                keys.insert(format!(
                    "plan:{}:{}",
                    plan.document.plan_id, plan.document.revision_id
                ));
            }
        }
        for approval in &state.approvals {
            keys.insert(Self::approval_key(approval, state.interaction_epoch));
        }
        for question in &state.questions {
            keys.insert(question_key(state, question));
        }
        for request in &state.elicitations {
            keys.insert(format!("elicitation:{request:?}"));
        }
        if keys.difference(&self.status_card_keys).next().is_some() {
            self.tasks_expanded = false;
        }
        self.status_card_keys = keys;
    }

    pub(super) fn status_card_header(
        &self,
        key: &'static str,
        label: &'static str,
        kind: StatusKind,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let expanded = !self.expanded.contains(key);
        super::super::components::inline_disclosure(key, label)
            .debug_selector(move || key.into())
            .w_full()
            .h(rpx(TASK_STRIP_HEAD_H))
            .justify_start()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .aria_expanded(expanded)
            .child(crate::icons::icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                c::FG_DIM(),
            ))
            .child(status::label(key, kind, label, ICON_SM, cx))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    if !this.expanded.remove(key) {
                        this.expanded.insert(key.into());
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(key) {
                    this.expanded.insert(key.into());
                }
                cx.notify();
            }))
    }

    /// Current session feedback shares the task dock’s exact horizontal inset.
    pub(super) fn session_review(
        &self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.transcript_only() || state.session_status() != "Review" {
            return None;
        }
        let work_period = state.review_completion()?;
        let summary = self.transcript_cache.as_ref().map_or_else(
            || {
                super::super::review_summary::ReviewSummary::from_snapshot(state)
                    .map(|summary| review_overview(&summary.session_text()))
                    .filter(|summary| !summary.is_empty())
            },
            |cache| {
                cache
                    .review_overview
                    .get_or_init(|| {
                        super::super::review_summary::ReviewSummary::from_snapshot(state)
                            .map(|summary| review_overview(&summary.session_text()))
                            .filter(|summary| !summary.is_empty())
                    })
                    .clone()
            },
        );
        let expanded = summary.is_some() && !self.expanded.contains("session-review-disclosure");
        Some(
            composer_tray(window)
                .when(attached_above, gpui::Styled::rounded_none)
                // Keep intrinsic height; the containing rail removes its gap.
                .mb_0()
                .pb(rpx(SPACE_LG))
                .when(!expanded, |panel| panel.bg(c::BG()))
                .id("session-review")
                .debug_selector(|| "session-review".into())
                .role(gpui::Role::Group)
                .aria_label("Ready for review")
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(summary.is_some(), |header| {
                                    header.child(self.status_card_header(
                                        "session-review-disclosure",
                                        "Ready for review",
                                        StatusKind::Success,
                                        cx,
                                    ))
                                })
                                .when(summary.is_none(), |header| {
                                    header.child(
                                        div()
                                            .h(rpx(TASK_STRIP_HEAD_H))
                                            .flex()
                                            .items_center()
                                            .child(status::label(
                                                "session-review-status",
                                                StatusKind::Success,
                                                "Ready for review",
                                                ICON_SM,
                                                cx,
                                            )),
                                    )
                                }),
                        )
                        .child(
                            form_action(
                                "session-mark-reviewed",
                                "Mark reviewed",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/check.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "session-mark-reviewed".into())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let current = &this.session.read(cx).state;
                                    if current.session_status() == "Review"
                                        && current.review_completion() == Some(work_period)
                                    {
                                        this.session.update(cx, |session, cx| {
                                            session.send(Action::MarkReviewed { work_period }, cx);
                                        });
                                        this.input.focus_handle(cx).focus(window, cx);
                                    }
                                },
                            )),
                        ),
                )
                .when(expanded, |panel| {
                    panel.child(
                        div()
                            .id("session-review-summary")
                            .debug_selector(|| "session-review-summary".into())
                            .line_clamp(2)
                            .pl(rpx(TASK_MARK_INSET))
                            .pb(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(summary.unwrap_or_default()),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn session_notice(
        &self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        // Catalog failures already have an actionable home in the Dashboard slot.
        if self.dashboard_tray.is_some()
            && self.session.read(cx).catalog_error.is_some()
            && self.session.read(cx).catalog_error == state.error
        {
            return None;
        }
        let disconnected = state.status == codex::Status::Disconnected;
        let notice = state.effective_notice();
        if !disconnected && notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::Retrying) {
            return None;
        }
        let body = state
            .error
            .as_deref()
            .or(disconnected.then_some("The session is disconnected."))?;
        let title = if disconnected {
            "Connection lost"
        } else if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit) {
            "Usage limit reached"
        } else if state.status.busy() {
            "Provider error"
        } else {
            "Request stopped"
        };
        let notice_kind = if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit)
        {
            StatusKind::Warning
        } else {
            StatusKind::Failed
        };
        let expanded = self.expanded.contains("session-notice-details");
        let (summary, details) =
            if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit) {
                (
                    "The provider’s usage limit prevents this request from continuing.",
                    Some(body),
                )
            } else if notice.is_some() {
                body.split_once('\n')
                    .map_or((body, None), |(first, rest)| (first, Some(rest)))
            } else {
                (body, None)
            };
        let mut content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .when(disconnected || notice.is_some(), |content| {
                content.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(notice_kind.color())
                        .child(title),
                )
            })
            .child(literal_notice_text("session-notice-body", summary));
        if let Some(reset) = notice.and_then(|n| n.reset_at) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let seconds = reset.saturating_sub(now).max(0);
            let reset_text = if seconds == 0 {
                "The reported reset time has passed.".to_owned()
            } else if seconds < 60 {
                "Usage resets in less than a minute.".to_owned()
            } else if seconds < 3600 {
                format!("Usage resets in {} minutes.", (seconds + 59) / 60)
            } else {
                format!("Usage resets in about {} hours.", (seconds + 3599) / 3600)
            };
            content = content.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(reset_text),
            );
        }
        let copy_body = body.to_owned();
        let mut actions = div().flex().flex_wrap().gap(rpx(SPACE_SM)).child(
            form_action("session-notice-copy", "Copy message", false, window, cx)
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/copy.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(|| "session-notice-copy".into())
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_body.clone()));
                }),
        );
        let lower = body.to_ascii_lowercase();
        if [
            "sign in",
            "login",
            "log in",
            "not logged",
            "authentication",
            "unauthorized",
            "oauth",
        ]
        .iter()
        .any(|word| lower.contains(word))
        {
            let provider = state.saved.provider;
            let cwd = state.saved.cwd.clone();
            let args = state.saved.config_args.clone();
            actions = actions.child(
                form_action("session-connections", "Manage sign-in", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        cx.emit(CodexViewEvent::OpenConnections(
                            provider,
                            cwd.clone(),
                            args.clone(),
                            window.focused(cx),
                        ));
                    })),
            );
        }
        if disconnected {
            actions = actions.child(
                form_action("codex-reconnect", "Reconnect", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(|| "codex-reconnect".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.session.read(cx).state.status == codex::Status::Disconnected {
                            this.session
                                .update(cx, |session, cx| session.send(Action::Reconnect, cx));
                        }
                    })),
            );
        } else if let Some(url) = notice.and_then(|n| n.usage_url) {
            actions = actions.child(
                form_action("session-notice-usage", "Manage usage", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(move |_, _, cx| cx.open_url(url)),
            );
        }
        if details.is_some() {
            actions = actions.child(
                form_action(
                    "session-notice-details",
                    if expanded {
                        "Hide details"
                    } else {
                        "Show details"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(if expanded {
                            "icons/chev-down.svg"
                        } else {
                            "icons/chev-right.svg"
                        })
                        .size(rpx(ICON_SM)),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.expanded.remove("session-notice-details") {
                        this.expanded.insert("session-notice-details".into());
                    }
                    cx.notify();
                })),
            );
        }
        content = content.child(actions);
        if expanded {
            if let Some(details) = details {
                content = content.child(
                    div()
                        .id("session-notice-diagnostic-scroll")
                        .max_h(rpx(TOOL_DETAIL_MAX_H))
                        .overflow_y_scroll()
                        .child(literal_notice_text("session-notice-diagnostic", details)),
                );
            }
        }
        Some(
            composer_tray(window)
                .when(attached_above, gpui::Styled::rounded_none)
                // Negative tray overlap can erase the rail's intrinsic height.
                .mb_0()
                .pb(rpx(SPACE_LG))
                .id("codex-error")
                .debug_selector(|| "codex-error".into())
                .role(gpui::Role::Alert)
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(self.status_card_header("session-notice-disclosure", title, notice_kind, cx))
                .when(
                    !self.expanded.contains("session-notice-disclosure"),
                    |panel| {
                        panel.child(
                            div()
                                .pl(rpx(TASK_LABEL_INSET))
                                .pb(rpx(SPACE_LG))
                                .child(content),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_provider_notices(
        &mut self,
        index: usize,
        end: usize,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let notices = &state.saved.items[index..end];
        let key = format!(
            "provider-notices:{}:{}",
            state.saved.thread_id.as_deref().unwrap_or_default(),
            notices[0]["id"]
                .as_str()
                .map_or_else(|| index.to_string(), str::to_owned)
        );
        let expanded = self.expanded.contains(&key);
        let mcp = notices.iter().all(|notice| {
            notice["title"].as_str().is_some_and(|title| {
                title.starts_with("MCP server ") && title.ends_with(" is unavailable")
            })
        });
        let mut label = if mcp {
            format!(
                "{} MCP server{} unavailable",
                notices.len(),
                if notices.len() == 1 { "" } else { "s" }
            )
        } else if notices.len() == 1 {
            notices[0]["title"]
                .as_str()
                .unwrap_or("Provider warning")
                .to_owned()
        } else {
            format!("{} provider warnings", notices.len())
        };
        if mcp
            && notices.iter().all(|notice| {
                notice["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Authenticate this MCP server"))
            })
        {
            label.push_str(" · Authentication required");
        }
        let focus = self
            .ensure_message_focus(&key, window, cx)
            .tab_stop(true)
            .tab_index(0);
        let keyboard_key = key.clone();
        let mut group = div()
            .id(("provider-notice", index))
            .debug_selector(move || format!("provider-notice-{index}"))
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .child(
                super::super::components::inline_disclosure(
                    "provider-notice-toggle",
                    label.clone(),
                )
                .id(("provider-notice-toggle", index))
                .debug_selector(move || format!("provider-notice-toggle-{index}"))
                .track_focus(&focus)
                .tab_index(0)
                .aria_expanded(expanded)
                .w_full()
                .h_auto()
                .min_h(rpx(CHROME_CONTROL_H))
                .justify_start()
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(crate::icons::icon(
                    if expanded { "chev-down" } else { "chev-right" },
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .child(crate::icons::icon("message-warning", ICON_MD, c::AMBER()))
                .child(div().flex_1().min_w_0().child(label))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if action_key(event) {
                        this.toggle_expansion(&keyboard_key, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    focus.focus(window, cx);
                    this.toggle_expansion(&key, cx);
                })),
            );
        // Expansion is immediate input feedback, matching the transcript tool disclosures.
        // Keep literal details intact and show identical recovery text once per group.
        if expanded {
            let mut details = div()
                .w_full()
                .min_w_0()
                .pl(rpx(ICON_MD + SPACE_LG))
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG));
            let shared_text = notices
                .iter()
                .all(|notice| notice["text"] == notices[0]["text"]);
            for (offset, notice) in notices.iter().enumerate() {
                let item_index = index + offset;
                details = details.child(
                    div().w_full().min_w_0().text_size(rpx(TEXT_SMALL)).child(
                        notice["title"]
                            .as_str()
                            .unwrap_or("Provider warning")
                            .to_owned(),
                    ),
                );
                if !shared_text || offset + 1 == notices.len() {
                    details = details.child(literal_notice_text(
                        gpui::SharedString::from(format!("provider-notice-text-{item_index}")),
                        notice["text"].as_str().unwrap_or_default(),
                    ));
                }
            }
            group = group.child(details);
        }
        if mcp {
            let provider = state.saved.provider;
            let cwd = state.saved.cwd.clone();
            let args = state.saved.config_args.clone();
            group = group.child(
                super::super::components::form_action(
                    ("provider-connections", index),
                    "Manage connection",
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/external-link.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(move || format!("provider-connections-{index}"))
                .on_click(cx.listener(move |_, _, window, cx| {
                    cx.emit(CodexViewEvent::OpenConnections(
                        provider,
                        cwd.clone(),
                        args.clone(),
                        window.focused(cx),
                    ));
                })),
            );
        }
        group.into_any_element()
    }
}
