//! Transcript message, tool and work header row composition.

#[cfg(test)]
use super::measured_tool;
use super::{
    action_key, attachment_preview, c, codex, copy_control, header_control_frame,
    header_control_with_label_tooltip, image_from_url, is_tool_item, markdown_style, measured_slot,
    reference_prompt_copy, reference_prompt_markdown, relative_message_time, rpx, shimmer_label,
    snippet_shell_supported, snippets, tool_activity_icon, tool_activity_summary, tool_detail,
    tool_expansion_key, tool_failed, tool_is_running, tool_summary, tooltip, transcript_action,
    truncation_safe_label_width, Action, CodexView, PreparedItem, Rc, TextView, TextViewState,
    CHAT_MAX_W, TOOL_DETAIL_MAX_H, USER_BUBBLE_MAX_W,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, ICON_MD, ICON_SM, MOTION_SLOW_MS, RADIUS_CONTROL, RADIUS_PANEL, SPACE_2XL,
    SPACE_3XL, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS, TEXT_BODY, TEXT_SMALL, TEXT_TITLE,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AnimationExt as _;
use gpui::AppContext as _;
use gpui::{div, Context, FocusHandle, Window};
use gpui_component::WindowExt as _;

impl CodexView {
    pub(super) fn render_work_header(
        &mut self,
        index: usize,
        key: &str,
        label: &str,
        expanded: bool,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Section::header();
        let key = key.to_owned();
        let keyboard_key = key.clone();
        let label = label.to_owned();
        div()
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .pb(rpx(SPACE_XS))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(
                super::super::components::inline_disclosure(
                    "codex-work-header",
                    "Show work activity",
                )
                .id(("codex-work-header", index))
                .track_focus(focus)
                .aria_expanded(expanded)
                .debug_selector(move || format!("codex-work-header-{index}"))
                .w_full()
                .h_auto()
                .min_h(rpx(CHROME_CONTROL_H))
                .flex_wrap()
                .justify_start()
                .gap(rpx(SPACE_SM))
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .aria_label(label.clone())
                .child(shimmer_label(
                    ("codex-work-shimmer", index).into(),
                    label,
                    false,
                    cx,
                ))
                .child(crate::icons::icon(
                    if expanded { "chev-down" } else { "chev-right" },
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if action_key(event) {
                        this.toggle_expansion(&keyboard_key, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_expansion(&key, cx);
                })),
            )
            .into_any_element()
    }

    pub(super) fn prepared_item(&self, state: &codex::Snapshot, index: usize) -> Rc<PreparedItem> {
        self.transcript_cache
            .as_ref()
            .and_then(|cache| cache.item_data.get(index))
            .filter(|data| std::ptr::eq(data.source.as_ref(), state.saved.items[index].as_ref()))
            .cloned()
            .unwrap_or_else(|| Rc::new(PreparedItem::new(state.saved.items[index].clone())))
    }

    pub(super) fn render_tool_row(
        &mut self,
        detail_index: usize,
        state: &codex::Snapshot,
        shimmer: bool,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Section::tool();
        #[cfg(test)]
        measured_tool::MATERIALIZED.with(|count| count.set(count.get() + 1));
        let tool = state.saved.items[detail_index].as_ref();
        let tool_key = tool_expansion_key(tool, detail_index);
        let tool_expanded = self.expanded.contains(&tool_key);
        let keyboard_key = tool_key.clone();
        let copy_key = tool_key.clone();
        let failed = tool_failed(tool);
        let activity = tool_activity_summary(tool, tool_is_running(state, detail_index));
        let data = self.prepared_item(state, detail_index);
        let style = window.text_style();
        let font_size = rpx(TEXT_SMALL).to_pixels(window.rem_size());
        let label_width = {
            let mut cache = data.label_width.borrow_mut();
            if cache
                .as_ref()
                .is_none_or(|(label, old_style, old_size, _)| {
                    label != &activity || *old_style != style || *old_size != font_size
                })
            {
                let mut label_run = style.to_run(activity.len());
                label_run.font.family = crate::fonts::UI_FAMILY.into();
                let width = truncation_safe_label_width(
                    &activity,
                    font_size,
                    &label_run,
                    window.text_system(),
                );
                *cache = Some((activity.clone(), style, font_size, width));
            }
            cache
                .as_ref()
                .map_or(gpui::px(0.), |(_, _, _, width)| *width)
        };
        let mut detail_row = div()
            .id(("codex-tool-detail-row", detail_index))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .child(
                super::super::components::inline_disclosure(
                    "codex-toggle-tool",
                    if tool_expanded {
                        "Collapse tool call"
                    } else {
                        "Expand tool call"
                    },
                )
                .id(("codex-toggle-tool", detail_index))
                .debug_selector(move || format!("codex-toggle-tool-{detail_index}"))
                .w_full()
                .justify_start()
                .gap(rpx(SPACE_SM))
                .track_focus(focus)
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .when(tool_expanded, |style| style.text_color(c::FG()))
                .aria_label(tool_activity_summary(
                    tool,
                    tool_is_running(state, detail_index),
                ))
                .child(
                    div()
                        .id(("codex-tool-icon", detail_index))
                        .debug_selector(move || format!("codex-tool-icon-{detail_index}"))
                        .size(rpx(ICON_SM))
                        .flex_shrink_0()
                        .child(crate::icons::icon(
                            tool_activity_icon(tool),
                            ICON_SM,
                            if failed { c::FORM_ERROR() } else { c::FG_DIM() },
                        )),
                )
                .child(
                    div()
                        .id(("codex-tool-row-label", detail_index))
                        .debug_selector(move || format!("codex-tool-row-label-{detail_index}"))
                        .w(label_width)
                        .min_w_0()
                        .truncate()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(if tool_expanded { c::FG() } else { c::FG_DIM() })
                        .group_hover("neutral-control", |text| text.text_color(c::FG()))
                        .child(shimmer_label(
                            ("codex-tool-detail-shimmer", detail_index).into(),
                            activity,
                            shimmer,
                            cx,
                        )),
                )
                .when_some(tool["exitCode"].as_i64(), |row, code| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(if code == 0 {
                                c::FG_DIM()
                            } else {
                                c::FORM_ERROR()
                            })
                            .child(format!("· exit {code}")),
                    )
                })
                .aria_description(
                    tool["exitCode"]
                        .as_i64()
                        .map_or_else(String::new, |code| format!("Exit code {code}")),
                )
                .aria_expanded(tool_expanded)
                .child(
                    crate::icons::icon(
                        if tool_expanded {
                            "chev-down"
                        } else {
                            "chev-right"
                        },
                        ICON_SM,
                        c::FG_DIM(),
                    )
                    .flex_shrink_0(),
                )
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        this.toggle_expansion(&keyboard_key, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expansion(&tool_key, cx))),
            );
        if tool_expanded {
            let copy = data.detail();
            let copy_focus = self.ensure_control_focus(
                &format!(
                    "tool-copy:{}:{detail_index}:{copy_key}",
                    state.saved.thread_id.as_deref().unwrap_or_default()
                ),
                window,
                cx,
            );
            let markdown = data.detail_markdown();
            detail_row = detail_row.child(
                div()
                    .id(("codex-tool-detail", detail_index))
                    .debug_selector(move || format!("codex-tool-detail-{detail_index}"))
                    .max_h(rpx(TOOL_DETAIL_MAX_H))
                    .overflow_y_scroll()
                    .p(rpx(SPACE_LG))
                    .rounded(rpx(RADIUS_CONTROL))
                    .bg(c::BG_RAIL())
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        copy_control("codex-copy-tool", "Copy tool call", copy)
                            .track_focus(&copy_focus),
                    )
                    .child(
                        TextView::markdown(("codex-group-tool-text", detail_index), markdown)
                            .style(markdown_style(window))
                            .selectable(true),
                    ),
            );
        }
        detail_row.into_any_element()
    }

    pub(super) fn render_message_row(
        &mut self,
        index: usize,
        frame: &measured_slot::MessageFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let state = &frame.state;
        let item = state.saved.items[index].as_ref();
        let kind = item["type"].as_str().unwrap_or("tool");
        let tool = is_tool_item(item);
        let provider = state.saved.provider.label();
        let initial_transcript = frame.initial_transcript;
        let near_bottom = frame.near_bottom;
        let elapsed = frame.elapsed;
        let response_prose = &frame.prepared.response_prose;
        let response_groups = &frame.prepared.response_groups;
        let id = item["id"]
            .as_str()
            .map_or_else(|| index.to_string(), str::to_owned);
        let expanded = self.expanded.contains(&id);
        let plan_details = kind == "plan"
            && state.saved.provider == codex::Provider::Claude
            && item["tool"].is_string();
        let data = &frame.prepared.item_data[index];
        let text: gpui::SharedString = if self.review_scope.is_some() {
            grove_core::conversation_result::display_text(
                "review",
                "grove-review",
                kind,
                &data.text,
            )
            .into()
        } else if self.description_scope.is_some() {
            description_conversation_text(self.description_scope.as_ref(), kind, &data.text).into()
        } else {
            data.text.clone()
        };
        let approval = (kind == "userMessage")
            .then(|| state.saved.rich_plans.approval_for_payload(&text))
            .flatten();
        let role = match kind {
            "userMessage" => "You",
            "agentMessage" => provider,
            "plan" => "Plan",
            "commandExecution" => "Command",
            "fileChange" => "File changes",
            "reasoning" => "Reasoning",
            _ => "Tool",
        };
        let mut row = div()
            .id(("codex-item", index))
            .debug_selector(move || format!("codex-message-row-{index}"))
            .flex()
            .flex_col()
            .w_full()
            .max_w(rpx(if kind == "userMessage" {
                USER_BUBBLE_MAX_W
            } else {
                CHAT_MAX_W
            }))
            .gap(rpx(SPACE_LG))
            .when(kind == "userMessage", |row| {
                row.p(rpx(SPACE_2XL))
                    .rounded(rpx(RADIUS_PANEL))
                    .bg(c::BG_RAIL())
            });
        let text = if tool { data.detail() } else { text };
        let text = if kind == "userMessage" {
            frame
                .review_display
                .borrow_mut()
                .entry(index)
                .or_insert_with(|| state.saved.rich_plans.response_display_text(&text))
                .clone()
                .map_or(text.clone(), gpui::SharedString::from)
        } else {
            text
        };
        let copy = if kind == "userMessage" {
            gpui::SharedString::from(reference_prompt_copy(&text))
        } else {
            text.clone()
        };
        let text = if kind == "userMessage" {
            gpui::SharedString::from(reference_prompt_markdown(&text))
        } else {
            text
        };
        let text = if kind == "agentMessage" && state.status.busy() && !cx.reduce_motion() {
            let key = item["id"]
                .as_str()
                .map_or_else(|| format!("message-{index}"), str::to_owned);
            let entry = self.stream_text.entry(key).or_insert_with(|| {
                let shown = if state.status.busy()
                    && !initial_transcript
                    && index + 1 == state.saved.items.len()
                {
                    0
                } else {
                    text.len()
                };
                (text.to_string(), shown)
            });
            let revealed =
                super::super::motion::streamed_text(&text, entry, elapsed, state.status.busy());
            let revealed = if revealed.len() == text.len() {
                text.clone()
            } else {
                gpui::SharedString::from(revealed.to_owned())
            };
            if entry.1 < text.len() {
                self.smooth_follow |= near_bottom && !self.composer_collapsed;
                let owner = cx.entity().downgrade();
                let pending = self.motion_frame_pending.clone();
                if !pending.replace(true) {
                    window.on_next_frame(move |_, cx| {
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            }
            revealed
        } else {
            if kind == "agentMessage" {
                let key = item["id"]
                    .as_str()
                    .map_or_else(|| format!("message-{index}"), str::to_owned);
                let entry = self
                    .stream_text
                    .entry(key)
                    .or_insert_with(|| (text.to_string(), text.len()));
                if entry.0 != text.as_ref() {
                    entry.0 = text.to_string();
                }
                entry.1 = text.len();
            }
            text
        };
        let toggle = id.clone();
        if kind != "agentMessage" {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(
                                if kind == "userMessage"
                                    && self.bubble_entrances.iter().any(|pending| {
                                        pending.optimistic.as_ref().is_some_and(|bubble| {
                                            bubble["id"] == item["id"]
                                                && bubble["localSteering"] == true
                                        })
                                    })
                                {
                                    "You · Steering…".to_owned()
                                } else if tool {
                                    tool_summary(item)
                                } else {
                                    role.to_owned()
                                },
                            ),
                    )
                    .when(tool || plan_details, |row| {
                        let keyboard_toggle = toggle.clone();
                        row.child(
                            header_control_with_label_tooltip(
                                "codex-expand",
                                if expanded {
                                    "Collapse tool call"
                                } else {
                                    "Expand tool call"
                                },
                            )
                            .child(crate::icons::icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_MD,
                                c::FG_DIM(),
                            ))
                            .debug_selector(move || format!("codex-toggle-details-{index}"))
                            .aria_expanded(expanded)
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if action_key(event) {
                                        this.toggle_expansion(&keyboard_toggle, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.expanded.remove(&toggle) {
                                        this.expanded.insert(toggle.clone());
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .when(kind != "userMessage", |row| {
                        row.child(copy_control("codex-copy", "Copy message", copy.clone()))
                    }),
            );
        }
        if !tool {
            let html_message = item["id"]
                .as_str()
                .map_or_else(|| format!("message-{index}"), str::to_owned);
            let html_extensions = self
                .html
                .update(cx, |html, cx| html.extensions(&html_message, cx));
            if let Some(approval) = approval {
                let plan_key = (
                    approval.document.plan_id.clone(),
                    approval.document.revision_id.clone(),
                );
                let mut receipt = div()
                    .debug_selector(move || format!("plan-approval-receipt-{index}"))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(format!(
                                "Plan approved · Revision {}",
                                approval.document.revision_id
                            )),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .child(approval.document.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .child("Implementation authorized with your selected decisions."),
                    );
                let mut actions = div().flex().flex_wrap().gap(rpx(SPACE_LG));
                if let Some(plan) = self.native_plans.get(&plan_key) {
                    let plan = plan.clone();
                    actions = actions.child(
                        transcript_action(
                            "approval-view-plan",
                            "View approved plan",
                            false,
                            window,
                            cx,
                        )
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/zen.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .debug_selector(move || format!("plan-approval-open-{index}"))
                        .on_click(move |_, window, cx| {
                            plan.update(cx, |plan, cx| plan.reveal(window, cx));
                        }),
                    );
                }
                let detail_id = format!("approval-details-{id}");
                let details_open = self.expanded.contains(&detail_id);
                actions = actions.child(
                    transcript_action(
                        "approval-view-details",
                        if details_open {
                            "Hide approval details"
                        } else {
                            "View approval details"
                        },
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path(if details_open {
                                "icons/chev-down.svg"
                            } else {
                                "icons/chev-right.svg"
                            })
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(move || format!("plan-approval-details-toggle-{index}"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_expansion(&detail_id, cx)),
                    ),
                );
                receipt = receipt.child(actions);
                if details_open {
                    receipt = receipt.child(
                        div()
                            .debug_selector(move || format!("plan-approval-details-{index}"))
                            .child(
                                TextView::markdown(("approval-payload", index), text.clone())
                                    .style(markdown_style(window))
                                    .selectable(true),
                            ),
                    );
                }
                row = row.child(receipt);
            } else {
                row = row.child(
                    div()
                        .id(format!("codex-message-body-{index}"))
                        .debug_selector(move || format!("codex-message-body-{index}"))
                        .child({
                            let owner_key = format!(
                                "{}:{}",
                                state.saved.thread_id.as_deref().unwrap_or(""),
                                html_message
                            );
                            let owner = if let Some(owner) =
                                self.selection_views.get(&owner_key).filter(|owner| {
                                    let old = owner.read(cx);
                                    let prefix = text.starts_with(old.source().as_ref());
                                    // Keep paragraph measurement identities on an unselected
                                    // static replacement; selected/focused/dynamic bodies
                                    // retain the original new-owner invalidation path.
                                    let reusable = old.supports_layout_projection()
                                        && !old.layout_projection_focused(window, cx)
                                        && !window.is_a11y_active()
                                        && !window.is_inspector_picking(cx);
                                    prefix
                                        || (reusable
                                            && !gpui_component::text::has_active_text_selection(
                                                window, cx,
                                            )
                                            && window.text_selection_snapshot(cx).is_none())
                                }) {
                                owner.clone()
                            } else {
                                let owner = cx.new(|cx| TextViewState::markdown(&text, cx));
                                self.selection_subscriptions.insert(
                                    owner_key.clone(),
                                    cx.observe(&owner, |_, _, cx| cx.notify()),
                                );
                                self.selection_views
                                    .insert(owner_key.clone(), owner.clone());
                                owner
                            };
                            owner.update(cx, |owner, cx| owner.set_text_incremental(&text, cx));
                            let view = TextView::new(&owner)
                                .markdown_extensions(html_extensions)
                                .style(markdown_style(window))
                                .selectable(true);
                            if kind == "agentMessage" {
                                let owner = cx.entity().downgrade();
                                let source_digest = data.snippet_source_digest(&text);
                                let footer_owner = owner.clone();
                                let footer_key = owner_key.clone();
                                let footer_cwd = state.saved.cwd.clone();
                                let cwd = state.saved.cwd.clone();
                                let finalized = (item["groveCompleted"] == true
                                    || item["status"] == "completed")
                                    && item["status"] != "inProgress"
                                    && !self.transcript_only();
                                let project_code = self
                                    .message_slots
                                    .get(&format!("{owner_key}:{index}"))
                                    .is_none_or(|cache| cache.borrow().code_controls_static(self));
                                let inline_owner = owner.clone();
                                let inline_key = owner_key.clone();
                                view.code_block_layout_projection(project_code)
                                    .code_block_layout_revision(u64::from(finalized))
                                    .code_block_inline_actions(move |block, window, cx| {
                                        let code = block.code();
                                        if !finalized
                                            || !block.is_closed_fence()
                                            || code.trim().is_empty()
                                            || !block
                                                .lang()
                                                .is_some_and(|lang| snippet_shell_supported(&lang))
                                        {
                                            return None;
                                        }
                                        let key = snippets::key_with_digests(
                                            &inline_key,
                                            block.span.as_ref().map_or(0, |span| span.start),
                                            block.content_digest(),
                                            source_digest,
                                        );
                                        inline_owner
                                            .update(cx, |this, cx| {
                                                this.snippet_actions(&key, &code, &cwd, window, cx)
                                            })
                                            .ok()
                                    })
                                    .code_block_actions(move |block, _, cx| {
                                        let code = block.code();
                                        let key = snippets::key_with_digests(
                                            &owner_key,
                                            block.span.as_ref().map_or(0, |span| span.start),
                                            block.content_digest(),
                                            source_digest,
                                        );
                                        let focus = owner
                                            .update(cx, |this, cx| {
                                                this.message_focus
                                                    .entry(key.clone())
                                                    .or_insert_with(|| cx.focus_handle())
                                                    .clone()
                                            })
                                            .ok();
                                        let copied = owner.upgrade().is_some_and(|owner| {
                                            owner.read(cx).copied_code_blocks.contains_key(&key)
                                        });
                                        let label = if copied { "Copied" } else { "Copy code" };
                                        let keyboard_owner = owner.clone();
                                        let keyboard_key = key.clone();
                                        let keyboard_code = code.clone();
                                        let click_owner = owner.clone();
                                        let click_code = code.clone();
                                        let copy_key = key.clone();
                                        let copy = header_control_with_label_tooltip(
                                            "codex-copy-code",
                                            label,
                                        )
                                        .when_some(focus, |this, focus| this.track_focus(&focus))
                                        .debug_selector(move || {
                                            if copied {
                                                "codex-code-copied".into()
                                            } else {
                                                "codex-copy-code".into()
                                            }
                                        })
                                        .tab_index(0)
                                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                                        .child(crate::icons::icon(
                                            if copied { "check" } else { "copy" },
                                            ICON_MD,
                                            c::FG_DIM(),
                                        ))
                                        .on_key_down(move |event, _, cx| {
                                            if action_key(event) {
                                                let _ = keyboard_owner.update(cx, |this, cx| {
                                                    this.copy_code_block(
                                                        keyboard_key.clone(),
                                                        keyboard_code.clone(),
                                                        cx,
                                                    );
                                                });
                                                cx.stop_propagation();
                                            }
                                        })
                                        .on_click(move |_, _, cx| {
                                            let _ = click_owner.update(cx, |this, cx| {
                                                this.copy_code_block(
                                                    copy_key.clone(),
                                                    click_code.clone(),
                                                    cx,
                                                );
                                            });
                                        });
                                        div().flex().justify_end().child(copy).into_any_element()
                                    })
                                    .code_block_footer(move |block, window, cx| {
                                        let code = block.code();
                                        let language =
                                            block.lang().map(|s| s.to_string()).unwrap_or_default();
                                        let closed = block.is_closed_fence();
                                        if finalized
                                            && closed
                                            && !code.trim().is_empty()
                                            && snippet_shell_supported(&language)
                                        {
                                            let key = snippets::key_with_digests(
                                                &footer_key,
                                                block.span.as_ref().map_or(0, |span| span.start),
                                                block.content_digest(),
                                                source_digest,
                                            );
                                            footer_owner
                                                .update(cx, |this, cx| {
                                                    this.snippet_footer(
                                                        &key,
                                                        &code,
                                                        &footer_cwd,
                                                        window,
                                                        cx,
                                                    )
                                                })
                                                .unwrap_or_else(|_| div().into_any_element())
                                        } else {
                                            div().into_any_element()
                                        }
                                    })
                            } else {
                                view
                            }
                        }),
                );
            }
            let html_entity = self.html.clone();
            let linked = html_entity.update(cx, |html, cx| html.linked(&html_message, window, cx));
            row = row.children(linked);
            if kind == "agentMessage" && !self.history_readonly {
                for index in frame
                    .rich_messages
                    .plans
                    .get(&html_message)
                    .into_iter()
                    .flatten()
                {
                    let plan = &state.saved.rich_plans.revisions[*index];
                    let key = (
                        plan.document.plan_id.clone(),
                        plan.document.revision_id.clone(),
                    );
                    row = row.child(super::super::native_plan::inline(
                        &self.native_plans[&key],
                        window,
                        cx,
                    ));
                }
                for index in frame
                    .rich_messages
                    .errors
                    .get(&html_message)
                    .into_iter()
                    .flatten()
                {
                    let error = &state.saved.rich_plans.errors[*index];
                    if let Some(limit) = &error.size_limit {
                        let recovery_roots: Vec<std::path::PathBuf> = state
                            .saved
                            .roots
                            .iter()
                            .map(|root| root.wt_path.clone().into())
                            .collect();
                        let path = super::super::inline_html::recovery_path(
                            &limit.path,
                            std::path::Path::new(&state.saved.cwd),
                            &recovery_roots,
                        );
                        let recovery_cwd = std::path::PathBuf::from(&state.saved.cwd);
                        let actions = path
                            .into_iter()
                            .map(|path| {
                                let recovery_cwd = recovery_cwd.clone();
                                let recovery_roots = recovery_roots.clone();
                                transcript_action(
                                    format!("reveal-oversized-plan-{}", error.message_id),
                                    "Reveal file",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/folder.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .on_click(move |_, _, cx| {
                                    if let Some(path) = super::super::inline_html::recovery_path(
                                        &path,
                                        &recovery_cwd,
                                        &recovery_roots,
                                    ) {
                                        cx.reveal_path(&path);
                                    }
                                })
                                .into_any_element()
                            })
                            .collect();
                        let is_plan = grove_core::rich_plan::plan_link_path(
                            std::path::Path::new(&state.saved.cwd),
                            &error.source,
                            &recovery_roots,
                        )
                        .ok()
                        .as_ref()
                            == Some(&limit.path);
                        row = row.child(super::super::inline_html::oversized_card(
                        if is_plan { "Plan too large" } else { "Plan asset too large" },
                        limit,
                        if is_plan { "Move embedded images into separate assets and reference their paths." } else { "Reduce this asset’s size, then link a new plan revision." },
                        actions, cx,
                    ));
                        continue;
                    }
                    let message_id = error.message_id.clone();
                    let source = error.source.clone();
                    row = row.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_LG))
                            .p(rpx(SPACE_2XL))
                            .child(
                                div()
                                    .text_color(c::FORM_ERROR())
                                    .child(format!("Couldn’t open native plan: {}", error.error)),
                            )
                            .child(div().text_color(c::FG_DIM()).child(error.source.clone()))
                            .child(
                                transcript_action(
                                    format!("retry-native-plan-{message_id}"),
                                    "Retry plan import",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/refresh.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.session.update(cx, |session, cx| {
                                            session.send(
                                                Action::RetryPlanImport {
                                                    message_id: message_id.clone(),
                                                    source: source.clone(),
                                                },
                                                cx,
                                            );
                                        });
                                    },
                                )),
                            ),
                    );
                }
            }

            if kind == "userMessage" {
                if let Some(content) = item["content"].as_array() {
                    let images: Vec<_> = content
                        .iter()
                        .filter_map(|part| {
                            part["url"].as_str().or_else(|| part["image_url"].as_str())
                        })
                        .collect();
                    if !images.is_empty() {
                        let count = images.len();
                        let label =
                            format!("{count} attachment{}", if count == 1 { "" } else { "s" });
                        let mut thumbnails = div()
                            .id(("codex-message-images", index))
                            .debug_selector(move || format!("codex-message-images-{index}"))
                            .flex()
                            .flex_wrap()
                            .gap(rpx(SPACE_LG));
                        for (image_index, image) in images.into_iter().enumerate() {
                            let key = (index, image_index);
                            if let Some(image) = self
                                .transcript_previews
                                .get(&key)
                                .filter(|(source, _)| std::ptr::eq(source.as_ref(), item))
                                .map(|(_, preview)| preview.clone())
                                .or_else(|| {
                                    let preview = image_from_url(image)?;
                                    self.transcript_previews.insert(
                                        key,
                                        (state.saved.items[index].clone(), preview.clone()),
                                    );
                                    Some(preview)
                                })
                            {
                                thumbnails = thumbnails.child(
                                    attachment_preview(image)
                                        .id(format!("codex-message-image-{index}-{image_index}"))
                                        .debug_selector(move || {
                                            format!("codex-message-image-{index}-{image_index}")
                                        }),
                                );
                            }
                        }
                        row = row.child(
                            div()
                                .id(("codex-message-attachments", index))
                                .debug_selector(move || {
                                    format!("codex-message-attachments-{index}")
                                })
                                .flex()
                                .flex_col()
                                .gap(rpx(SPACE_LG))
                                .child(
                                    div()
                                        .id(("codex-message-attachment-count", index))
                                        .debug_selector(move || {
                                            format!("codex-message-attachment-count-{index}")
                                        })
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(label),
                                )
                                .child(thumbnails),
                        );
                    }
                }
            }
        } else if expanded {
            row = row.child(
                div()
                    .id(("codex-tool-detail", index))
                    .debug_selector(move || format!("codex-tool-detail-{index}"))
                    .child(
                        TextView::markdown(
                            ("codex-tool-text", index),
                            format!("```text\n{}\n```", text.replace("```", "` ` `")),
                        )
                        .style(markdown_style(window))
                        .selectable(true),
                    ),
            );
        }
        if plan_details && expanded {
            row = row.child(
                div()
                    .id(("codex-plan-detail", index))
                    .debug_selector(move || format!("codex-plan-detail-{index}"))
                    .child(
                        TextView::markdown(
                            ("codex-plan-tool-text", index),
                            format!(
                                "```text\n{}\n```",
                                tool_detail(item).replace("```", "` ` `")
                            ),
                        )
                        .style(markdown_style(window))
                        .selectable(true),
                    ),
            );
        }
        if kind == "agentMessage" {
            if let Some(group_index) = response_prose[index] {
                let group = &response_groups[group_index];
                let key = group.key.clone();
                let source_key = format!("{}:source:{id}", group.key);
                if !self.message_focus.contains_key(&source_key) {
                    let focus = cx.focus_handle();
                    self.focus_subscriptions.insert(
                        source_key.clone(),
                        (
                            cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                            cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
                        ),
                    );
                    self.message_focus.insert(source_key.clone(), focus);
                }
                let focus = self.message_focus[&source_key].clone();
                row = row.track_focus(&focus).tab_index(0).on_hover(cx.listener(
                    move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.message_hover.insert(key.clone());
                        } else {
                            this.message_hover.remove(&key);
                        }
                        cx.notify();
                    },
                ));
            }
        }
        if kind == "userMessage" {
            row = div()
                .id(("codex-message-block", index))
                .flex()
                .flex_col()
                .w_full()
                .max_w(rpx(USER_BUBBLE_MAX_W))
                .gap(rpx(SPACE_SM))
                .child(row);
            let key = format!(
                "{}:{id}",
                state.saved.thread_id.as_deref().unwrap_or_default()
            );
            if !self.message_focus.contains_key(&key) {
                let focus = cx.focus_handle();
                self.focus_subscriptions.insert(
                    key.clone(),
                    (
                        cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                        cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
                    ),
                );
                self.message_focus.insert(key.clone(), focus);
            }
            let focus = self.message_focus[&key].clone();
            let visible = self.message_hover.contains(&key) || focus.contains_focused(window, cx);
            let copied = self
                .copied_message
                .as_ref()
                .is_some_and(|(message, _)| message == &key);
            let hover_key = key.clone();
            row = row.track_focus(&focus).tab_index(0).on_hover(cx.listener(
                move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.message_hover.insert(hover_key.clone());
                    } else {
                        this.message_hover.remove(&hover_key);
                    }
                    cx.notify();
                },
            ));
            let mut actions = div()
                .id(("message-actions", index))
                .flex()
                .items_center()
                .gap(rpx(SPACE_MD))
                .h(rpx(CHROME_CONTROL_H))
                .justify_end();
            if let Some(timestamp) = item["groveReceivedAt"].as_u64() {
                actions = actions.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_MUTE())
                        .child(relative_message_time(timestamp)),
                );
            }
            actions = actions.child(self.fork_control(index, Some(id.clone()), true, window, cx));
            let source_index = self.source_item_index(&id, cx);
            let unavailable = source_index
                .and_then(|index| self.session.read(cx).state.rewind_target(index).err());
            if !self.transcript_only() && approval.is_none() && source_index.is_some() {
                let edit_id = id.clone();
                let click_id = id.clone();
                let edit_focus = self.ensure_control_focus(&format!("{key}:edit"), window, cx);
                let bounds = self
                    .message_action_bounds
                    .entry(key.clone())
                    .or_default()
                    .clone();
                actions = actions.child(
                    header_control_with_label_tooltip("codex-edit-message", "Edit sent message")
                        .relative()
                        .track_focus(&edit_focus)
                        .aria_description(unavailable.unwrap_or_else(|| "Edit from here".into()))
                        .debug_selector(move || format!("codex-edit-message-{index}"))
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon("edit", ICON_MD, c::FG_DIM()))
                        .child(
                            gpui::canvas(
                                move |bounds_now, _, _| {
                                    bounds.set(bounds_now);
                                },
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if action_key(event) {
                                    this.request_edit_item(&edit_id, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_edit_item(&click_id, window, cx);
                        })),
                );
            }
            let copy_focus = self.ensure_control_focus(&format!("{key}:copy"), window, cx);
            let keyboard_key = key.clone();
            let keyboard_text = copy.clone();
            let icon =
                crate::icons::icon(if copied { "check" } else { "copy" }, ICON_MD, c::FG_DIM());
            let mut copy_button = if copied {
                header_control_frame("codex-message-copy", "Copied", c::BG_HOVER())
            } else {
                header_control_with_label_tooltip("codex-message-copy", "Copy message")
            }
            .track_focus(&copy_focus)
            .relative()
            .debug_selector(|| "codex-copy".into())
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(if copied {
                super::super::motion::fast(icon, ("message-copy-check", index), cx)
            } else {
                icon.into_any_element()
            })
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    this.copy_message(keyboard_key.clone(), keyboard_text.to_string(), cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.copy_message(key.clone(), copy.to_string(), cx);
            }));
            if copied {
                copy_button = copy_button.child(
                    div()
                        .absolute()
                        .bottom(rpx(CHROME_CONTROL_H + SPACE_SM))
                        .right_0()
                        .child(tooltip("Copied", window).build(window, cx)),
                );
            }
            actions = actions.child(copy_button);
            // Keep the footer geometry and keyboard targets mounted while hidden.
            row = row.child(if visible || copied {
                super::super::motion::fast(actions, ("message-actions-show", index), cx)
            } else {
                actions.opacity(0.0).into_any_element()
            });
        }
        let row = if let Some((started, finished)) = self.animating_bubbles.get(&id) {
            let started = *started;
            let executor = cx.background_executor().clone();
            let finished = finished.clone();
            row.relative()
                .with_animation(
                    gpui::SharedString::from(format!("codex-send-{id}")),
                    gpui::Animation::new(std::time::Duration::from_millis(MOTION_SLOW_MS))
                        .with_easing(super::super::motion::ease_standard),
                    move |row, _progress| {
                        // One executor clock preserves progress across temporary unmounts
                        // and keeps virtual-time tests independent of wall-clock delays.
                        let elapsed = executor.now().duration_since(started).as_secs_f32()
                            / std::time::Duration::from_millis(MOTION_SLOW_MS).as_secs_f32();
                        let progress = super::super::motion::ease_standard(elapsed.min(1.0));
                        finished.set(elapsed >= 1.0);
                        // Rise from the composer, then settle with a small overshoot.
                        let remaining = 1.0 - progress;
                        row.top(rpx(SPACE_3XL
                            * remaining
                            * remaining
                            * (1.0 - 2.0 * progress)))
                    },
                )
                .into_any_element()
        } else {
            row.into_any_element()
        };
        row
    }
}

// Snapshot/schema transport is retained for recovery and model context, while
// the conversation presents its user-facing command and completion prose.
pub(super) fn description_conversation_text(
    scope: Option<&grove_core::change_description::Scope>,
    kind: &str,
    text: &str,
) -> String {
    let Some(scope) = scope else {
        return text.into();
    };
    if kind == "userMessage"
        && text.starts_with("Describe ONLY the supplied immutable change snapshot.")
        && text.contains("\n\nGrove request identity: ")
    {
        return match scope {
            grove_core::change_description::Scope::Local => "/describe-changes local".into(),
            grove_core::change_description::Scope::PullRequest { url } => {
                format!("/describe-changes {url}")
            }
        };
    }
    if kind != "agentMessage" {
        return text.into();
    }
    let mut result = Vec::new();
    let mut hidden = false;
    let mut other_fence = false;
    for line in text.lines() {
        let marker = line.trim();
        if hidden {
            if marker == "```" {
                hidden = false;
            }
            continue;
        }
        if other_fence {
            result.push(line);
            if marker == "```" {
                other_fence = false;
            }
            continue;
        }
        if marker == "```grove-description" {
            hidden = true;
            continue;
        }
        if marker.starts_with("```") {
            other_fence = true;
        }
        result.push(line);
    }
    let result = result.join("\n").trim().to_owned();
    if result.is_empty() && text.contains("```grove-description") {
        "Change description data".into()
    } else {
        result
    }
}
