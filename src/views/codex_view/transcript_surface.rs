//! History and message layout shared by editable conversations and read-only previews.

use super::{
    c, codex, compaction_divider, history, is_tool_item, measured_slot, measured_tool, rpx, tasks,
    tool_activity_label, tool_expansion_key, tool_is_running, CodexView, HashSet, Rc,
    TranscriptRenderCache, WorkIndex, CHAT_MAX_W,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, ICON_SM, MODAL_W_LG, SPACE_20, SPACE_2XL, SPACE_3XL, SPACE_LG, SPACE_SM,
    SPACE_XS, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{div, Context, Focusable, Window};
use gpui_component::WindowExt as _;

pub(super) enum TranscriptSurface {
    ReadOnly(gpui::AnyElement),
    Interactive(InteractiveTranscript),
}

pub(super) struct InteractiveTranscript {
    pub(super) transcript: gpui::AnyElement,
    pub(super) minimap: gpui::AnyElement,
    pub(super) placement: ComposerPlacement,
}

#[derive(Clone, Copy)]
pub(super) struct ComposerPlacement {
    pub(super) content_inset: f32,
    pub(super) has_minimap: bool,
    pub(super) focused_launch: bool,
}

impl CodexView {
    pub(super) fn render_transcript(
        &mut self,
        transcript: &Rc<TranscriptRenderCache>,
        follow: super::frame_state::TranscriptFollow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TranscriptSurface {
        let super::frame_state::TranscriptFollow {
            near_bottom,
            initial_transcript,
            elapsed,
        } = follow;
        let state = &transcript.state;
        let history_start = {
            let following = near_bottom
                && !self.composer_collapsed
                && self.edit_confirmation.is_none()
                && !gpui_component::text::has_active_text_selection(window, cx)
                && window.text_selection_snapshot(cx).is_none();
            self.turn_history
                .sync(&state.saved.items, &transcript.minimap_entries, following)
        };
        let history_start =
            self.reveal_activity_request(history_start, state, transcript, window, cx);
        let response_groups = &transcript.response_groups;
        let response_ends = &transcript.response_ends;
        let minimap_entries = &transcript.minimap_entries;
        let mut minimap_anchors = Vec::new();
        // The first rendered child at or after each item, for jump-to-start.
        let mut item_children = vec![0; state.saved.items.len() - history_start];
        let mut message_child = 0;
        let mut rendered_selection_keys = HashSet::new();
        let mut rendered_slot_keys = HashSet::new();
        let mut rendered_header_keys = HashSet::new();
        let mut rendered_tool_keys = HashSet::new();
        let message_frame = Rc::new(measured_slot::MessageFrame {
            rich_messages: transcript.rich_messages.clone(),
            review_display: transcript.review_display.clone(),
            state: state.clone(),
            prepared: transcript.prepared.clone(),
            initial_transcript,
            near_bottom,
            elapsed,
        });
        // Use the allocated pane, rather than the entire app window (split/grid).
        let pane_width = f32::from(self.transcript_bounds.get().size.width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE);
        let content_inset = if pane_width > 0. && pane_width < MODAL_W_LG {
            SPACE_LG
        } else {
            SPACE_20
        };
        let mut messages = div()
            .id("codex-messages")
            .debug_selector(|| "codex-messages".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let delta_y = match event.delta {
                    gpui::ScrollDelta::Lines(delta) => delta.y,
                    gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y),
                };
                if delta_y != 0.0 {
                    this.transcript_scrolled(delta_y < 0.0, cx);
                }
            }))
            .flex()
            .flex_col()
            .items_center()
            .px(rpx(content_inset))
            .py(rpx(SPACE_3XL))
            .gap(rpx(SPACE_20));
        if history_start > 0 || self.turn_history.expanded {
            let hidden = history::history_boundaries(&state.saved.items, minimap_entries)
                .partition_point(|index| *index < history_start);
            messages = messages.child(self.render_history_disclosure(hidden, cx));
            message_child += 1;
        }
        let focused_launch = state.saved.items.is_empty()
            && state.questions.is_empty()
            && self.question_receipts.is_empty()
            && state.approvals.is_empty()
            && state.elicitations.is_empty()
            && (!state.status.busy()
                || (state.status == codex::Status::Starting
                    && state.saved.provider == codex::Provider::Claude));
        for (index, item) in state
            .saved
            .items
            .iter()
            .map(AsRef::as_ref)
            .chain(std::iter::once(&serde_json::Value::Null))
            .enumerate()
            .skip(history_start)
        {
            // Flush before inspecting the next source item: the previous final item
            // may be hidden commentary, a checklist, or a collapsed tool.
            if let Some(group_index) = response_ends[index].filter(|_| index > history_start) {
                let group = &response_groups[group_index];
                let footer = if window.is_a11y_active()
                    || measured_slot::force_eager()
                    || measured_slot::force_footer_eager()
                {
                    self.response_footer(group, state, true, window, cx)
                } else {
                    let focus = self.ensure_message_focus(&group.key, window, cx);
                    let copy_focus =
                        self.ensure_control_focus(&format!("{}:copy", group.key), window, cx);
                    measured_slot::MeasuredResponseFooter::new(
                        cx.entity().downgrade(),
                        message_frame.clone(),
                        group_index,
                        focus,
                        copy_focus,
                    )
                    .into_any_element()
                };
                messages = messages.child(footer);
                message_child += 1;
            }
            if index < state.saved.items.len() {
                item_children[index - history_start] = message_child;
                let blocks = self.question_blocks_at(index, state, window, cx);
                message_child += blocks.len();
                messages = messages.children(blocks);
            }
            // Group only adjacent notices; never cross a question or another message.
            // Skip before work headers so grouped diagnostics do not leave empty work rows.
            if index > history_start
                && index < state.saved.items.len()
                && item["type"] == "providerNotice"
                && state.saved.items[index - 1]["type"] == "providerNotice"
                && !self.has_question_anchor(index)
            {
                continue;
            }
            if let Some((key, label, active)) =
                transcript.work.header(state, &transcript.prepared, index)
            {
                message_child += 1;
                let expanded = self.expanded.contains(&key);
                let focus_key = format!(
                    "work-header:{}:{index}:{key}",
                    state.saved.thread_id.as_deref().unwrap_or_default()
                );
                // The original button used an implicit tabbable handle. An
                // explicit tracked handle must preserve that metadata itself.
                let focus = self
                    .ensure_message_focus(&focus_key, window, cx)
                    .tab_stop(true)
                    .tab_index(0);
                self.message_focus.insert(focus_key.clone(), focus.clone());
                let header = if !active
                    && !expanded
                    && !window.is_a11y_active()
                    && !measured_slot::force_eager()
                    && !measured_slot::force_header_eager()
                {
                    rendered_header_keys.insert(focus_key.clone());
                    let cache = self.work_header_slots.entry(focus_key).or_default().clone();
                    measured_slot::MeasuredWorkHeader::new(
                        cx.entity().downgrade(),
                        index,
                        key.clone(),
                        label.clone(),
                        focus,
                        cache,
                    )
                    .into_any_element()
                } else {
                    self.render_work_header(index, &key, &label, expanded, &focus, cx)
                };
                messages = messages.child(header);
                if expanded && !transcript.work.parent_activity[index] {
                    message_child += 1;
                    messages = messages.child(
                        div()
                            .id(("codex-work-empty", index))
                            .debug_selector(move || format!("codex-work-empty-{index}"))
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if active {
                                "No parent activity has been reported yet."
                            } else {
                                "No parent activity was reported."
                            }),
                    );
                }
                if active && state.work_started.is_some() && !self.work_tick_pending.replace(true) {
                    let owner = cx.entity().downgrade();
                    let pending = self.work_tick_pending.clone();
                    cx.spawn(async move |_, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                }
            }
            if index == state.saved.items.len() {
                continue;
            }
            let kind = item["type"].as_str().unwrap_or("tool");
            if kind == "subAgentActivity" {
                continue;
            }
            // Keep completed commentary with the work it describes; the answer stays inline.
            if transcript.commentary[index]
                && !self
                    .expanded
                    .contains(WorkIndex::key(&transcript.prepared, index))
                && !transcript.work.active[index]
            {
                continue;
            }
            if kind == codex::COMPACTION_ITEM {
                // A running compaction is shown by the live activity line instead.
                if item["status"] != "inProgress" {
                    message_child += 1;
                    messages = messages.child(compaction_divider(index, codex::item_text(item)));
                }
                continue;
            }
            if kind == "providerNotice" {
                let end = (index + 1..state.saved.items.len())
                    .find(|next| {
                        state.saved.items[*next]["type"] != "providerNotice"
                            || self.has_question_anchor(*next)
                    })
                    .unwrap_or(state.saved.items.len());
                message_child += 1;
                messages =
                    messages.child(self.render_provider_notices(index, end, state, window, cx));
                continue;
            }
            if tasks::is_checklist(item) || grove_core::activity::is_internal(item) {
                // Checklist updates belong in the docked panel, not the transcript.
                continue;
            }
            let tool = is_tool_item(item);
            let has_visual = |index: usize| {
                state.saved.items.get(index).is_some_and(|item| {
                    item["id"]
                        .as_str()
                        .is_some_and(|id| transcript.visual_items.contains(id))
                })
            };
            if tool && has_visual(index) {
                let focus = self.ensure_control_focus(&format!("visual-tool-{index}"), window, cx);
                let mut publication = div()
                    .id(("codex-visual-publication", index))
                    .debug_selector(move || format!("codex-visual-publication-{index}"))
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(self.render_tool_row(index, state, false, &focus, window, cx));
                for reference in state
                    .saved
                    .visuals
                    .iter()
                    .filter(|r| item["id"] == r.item_id)
                {
                    publication = publication.child(
                        self.html
                            .update(cx, |html, cx| html.published(reference, window, cx)),
                    );
                }
                messages = messages.child(publication);
                message_child += 1;
                continue;
            }
            if tool {
                if index > 0
                    && is_tool_item(&state.saved.items[index - 1])
                    && !has_visual(index - 1)
                    && (state.saved.items[index - 1]["type"] == "reasoning")
                        == (kind == "reasoning")
                    && !self.has_question_anchor(index)
                {
                    continue;
                }
                let end = state.saved.items[index..]
                    .iter()
                    .position(|item| {
                        item["id"]
                            .as_str()
                            .is_some_and(|id| transcript.visual_items.contains(id))
                            || !is_tool_item(item)
                            || (item["type"] == "reasoning") != (kind == "reasoning")
                    })
                    .map_or(state.saved.items.len(), |offset| index + offset);
                let end = (index + 1..end)
                    .find(|anchor| self.has_question_anchor(*anchor))
                    .unwrap_or(end);
                let tools = state.saved.items[index..end]
                    .iter()
                    .enumerate()
                    .filter(|(offset, _)| {
                        let item_index = index + offset;
                        self.expanded
                            .contains(WorkIndex::key(&transcript.prepared, item_index))
                            || (transcript.work.tool_visible(state, item_index)
                                && transcript.work.active[item_index])
                    })
                    .map(|(offset, item)| (index + offset, item))
                    .collect::<Vec<_>>();
                if tools.is_empty() {
                    continue;
                }
                let group_id = format!("tools-{}", item["id"].as_str().unwrap_or_default());
                let group_id = if item["id"].as_str().is_some() {
                    group_id
                } else {
                    format!("tools-{index}")
                };
                let expanded = !self.expanded.contains(&group_id);
                let running = tools
                    .iter()
                    .any(|(index, _)| tool_is_running(state, *index));
                let label = if tools
                    .iter()
                    .all(|(_, item)| item["type"] == "commandExecution")
                {
                    if running {
                        "Running commands…"
                    } else {
                        "Ran commands"
                    }
                } else if running {
                    "Working…"
                } else {
                    "Activity"
                }
                .to_owned();
                let summary = div()
                    .id(("codex-tool-summary", index))
                    .debug_selector(move || format!("codex-tool-summary-{index}"))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(crate::icons::icon("terminal", ICON_SM, c::FG_DIM()))
                    .child(
                        div()
                            .id(("codex-tool-label", index))
                            .debug_selector(move || format!("codex-tool-label-{index}"))
                            .min_w_0()
                            .truncate()
                            .child(tool_activity_label(label.clone(), index, false, cx)),
                    )
                    .child(
                        div()
                            .id(("codex-tool-chevron", index))
                            .debug_selector(move || format!("codex-tool-chevron-{index}"))
                            .size(rpx(ICON_SM))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(crate::icons::icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            )),
                    );
                let mut group = div()
                    .id(("codex-tool-group", index))
                    .debug_selector(move || format!("codex-tool-group-{index}"))
                    // Adjacent activity groups share the compact rhythm of their rows;
                    // prose keeps the transcript's normal paragraph separation.
                    .when(
                        state
                            .saved
                            .items
                            .get(end)
                            .is_some_and(|item| is_tool_item(item))
                            && transcript.work.tool_visible(state, end)
                            && (transcript.work.active[end]
                                || self
                                    .expanded
                                    .contains(WorkIndex::key(&transcript.prepared, end))),
                        |group| group.mb(-rpx(SPACE_2XL)),
                    )
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS));
                let keyboard_group = group_id.clone();
                if tools.len() > 1 {
                    group = group.child(
                        super::super::components::inline_disclosure(
                            "codex-toggle-tools",
                            if expanded {
                                "Collapse tool calls"
                            } else {
                                "Expand tool calls"
                            },
                        )
                        .id(("codex-toggle-tools", index))
                        .debug_selector(move || format!("codex-toggle-tools-{index}"))
                        .aria_expanded(expanded)
                        .w_full()
                        .justify_start()
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .when(expanded, |style| style.text_color(c::FG()))
                        .aria_label(label)
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                if !this.expanded.remove(&keyboard_group) {
                                    this.expanded.insert(keyboard_group.clone());
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child(summary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&group_id) {
                                this.expanded.insert(group_id.clone());
                            }
                            cx.notify();
                        })),
                    );
                }
                if expanded || tools.len() == 1 {
                    for (detail_index, tool) in tools {
                        #[cfg(test)]
                        measured_slot::observe_tool_detail(
                            detail_index >= transcript.work.active_start,
                        );
                        let tool_key = tool_expansion_key(tool, detail_index);
                        let focus_key = format!(
                            "tool-row:{}:{detail_index}:{tool_key}",
                            state.saved.thread_id.as_deref().unwrap_or_default()
                        );
                        let focus = self.ensure_control_focus(&focus_key, window, cx);
                        let shimmer = transcript.work.active[detail_index]
                            && transcript.work.tool_visible(state, detail_index);
                        let detail_expanded = self.expanded.contains(&tool_key);
                        let eager = tool_is_running(state, detail_index)
                            || shimmer
                            || window.is_a11y_active()
                            || gpui_component::text::has_active_text_selection(window, cx)
                            || window.is_inspector_picking(cx)
                            || measured_tool::force_eager();
                        let row = if eager {
                            self.render_tool_row(detail_index, state, shimmer, &focus, window, cx)
                        } else {
                            rendered_tool_keys.insert(focus_key.clone());
                            let cache = self.tool_slots.entry(focus_key).or_default().clone();
                            measured_tool::MeasuredTool::new(
                                cx.entity().downgrade(),
                                state.clone(),
                                detail_index,
                                tool_key,
                                focus,
                                detail_expanded,
                                cache,
                            )
                            .into_any_element()
                        };
                        group = group.child(row);
                    }
                }
                message_child += 1;
                messages = messages.child(group);
                continue;
            }
            let message_id = item["id"]
                .as_str()
                .map_or_else(|| format!("message-{index}"), str::to_owned);
            let owner_key = format!(
                "{}:{}",
                state.saved.thread_id.as_deref().unwrap_or(""),
                message_id
            );
            let receipt = kind == "userMessage"
                && state
                    .saved
                    .rich_plans
                    .approval_for_payload(&codex::item_text(item))
                    .is_some();
            if !tool && !receipt {
                rendered_selection_keys.insert(owner_key.clone());
            }
            let slot_key = format!("{owner_key}:{index}");
            // Streaming and entrance animations update follow state during Render;
            // keep those effects before the parent's scroll-follow decisions.
            let tail_stream = state.status.busy() && index + 1 == state.saved.items.len();
            let entrance = self
                .animating_bubbles
                .contains_key(item["id"].as_str().unwrap_or(""))
                || self.bubble_entrances.iter().any(|pending| {
                    pending
                        .optimistic
                        .as_ref()
                        .is_some_and(|bubble| bubble["id"] == item["id"])
                });
            let row = if matches!(kind, "userMessage" | "agentMessage")
                && !receipt
                && !tail_stream
                && !entrance
                && !window.is_a11y_active()
            {
                rendered_slot_keys.insert(slot_key.clone());
                let cache = self.message_slots.entry(slot_key).or_default().clone();
                measured_slot::MeasuredTranscriptSlot::new(
                    cx.entity().downgrade(),
                    index,
                    message_frame.clone(),
                    owner_key,
                    cache,
                )
                .into_any_element()
            } else {
                self.render_message_row(index, &message_frame, window, cx)
            };
            if kind == "userMessage" {
                minimap_anchors.push((index, message_child));
            }
            message_child += 1;
            messages = messages.child(
                div()
                    .id(("codex-message-anchor", index))
                    .debug_selector(move || format!("codex-message-anchor-{index}"))
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .when(kind == "userMessage", gpui::Styled::justify_end)
                    // Only assistant prose gives back the old per-item footer gap;
                    // tool groups and user-message boundaries retain their spacing.
                    .when(kind == "agentMessage", |anchor| anchor.mb(rpx(-SPACE_LG)))
                    .child(row),
            );
        }
        if let Some(index) = self.pending_history_jump.take() {
            let child = item_children
                .get(index.saturating_sub(history_start))
                .copied()
                .unwrap_or(message_child.saturating_sub(1));
            self.jump_to_message(child, window, cx);
        }
        self.prune_idle_snippets(&rendered_selection_keys);
        if self.transcript_only() {
            self.message_slots
                .retain(|key, _| rendered_slot_keys.contains(key));
            self.work_header_slots
                .retain(|key, _| rendered_header_keys.contains(key));
            self.tool_slots
                .retain(|key, _| rendered_tool_keys.contains(key));
            self.selection_views
                .retain(|key, _| rendered_selection_keys.contains(key));
            self.selection_subscriptions
                .retain(|key, _| self.selection_views.contains_key(key));
            // Stay pinned through layout growth until a wheel gesture starts browsing.
            // Returning to the bottom resumes following without affecting the composer.
            if self.preview_follow_latest {
                self.scroll.scroll_to_bottom();
            }
            return TranscriptSurface::ReadOnly(
                div()
                    .debug_selector(|| "codex-readonly-transcript".into())
                    .track_focus(&self.input.focus_handle(cx))
                    .size_full()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_color(c::FG())
                    .child(messages.px(rpx(SPACE_SM)))
                    .into_any_element(),
            );
        }
        messages =
            messages.children(self.question_blocks_at(state.saved.items.len(), state, window, cx));
        // Question edits and composer reflow can change the scroll range without
        // changing the transcript items. Pin against the new layout so the
        // footer or receipt stays reachable when the reader was at the bottom.
        if (!state.questions.is_empty() || self.has_question_anchor(state.saved.items.len()))
            && !self.composer_collapsed
            && self.edit_confirmation.is_none()
            && (self.composer_pin_bottom
                || self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(0.5))
        {
            self.scroll.scroll_to_bottom();
        }
        // Hidden, replaced and removed message owners must be dropped so the
        // native selection controller cannot copy their last painted text.
        self.selection_views
            .retain(|key, _| rendered_selection_keys.contains(key));
        self.selection_subscriptions
            .retain(|key, _| self.selection_views.contains_key(key));
        self.message_slots
            .retain(|key, _| rendered_slot_keys.contains(key));
        self.tool_slots
            .retain(|key, _| rendered_tool_keys.contains(key));
        self.work_header_slots
            .retain(|key, _| rendered_header_keys.contains(key));
        let minimap =
            self.render_message_minimap(transcript.prepared.clone(), &minimap_anchors, window, cx);
        let selection_overlay = self.render_selection_actions(state, window, cx);
        let transcript_bounds = self.transcript_bounds.clone();
        let bounds_owner = cx.entity().downgrade();
        let transcript = div()
            .id("codex-transcript")
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        if transcript_bounds.replace(bounds) != bounds {
                            let _ = bounds_owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.notify()),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    let selection = window.text_selection_snapshot(cx);
                    if selection.is_some() && selection != this.selection_dismissed {
                        this.selection_dismissed = selection;
                        this.clear_stop_confirmation(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .when(!minimap_entries.is_empty(), |transcript| {
                transcript.pl(rpx(CHROME_CONTROL_H))
            })
            .child(messages)
            .children(selection_overlay);
        TranscriptSurface::Interactive(InteractiveTranscript {
            transcript: transcript.into_any_element(),
            minimap,
            placement: ComposerPlacement {
                content_inset,
                has_minimap: !minimap_entries.is_empty(),
                focused_launch,
            },
        })
    }

    pub(super) fn jump_to_message(
        &mut self,
        child: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.smooth_follow = false;
        self.follow_after_submit = false;
        self.composer_pin_bottom = false;
        self.pending_composer_scroll = None;
        self.set_composer_collapsed(true, cx);
        // GPUI resolves this against the next layout, including any composer resize.
        self.scroll.scroll_to_top_of_item(child);
        let owner = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = owner.update(cx, |_, cx| cx.notify());
        });
        cx.notify();
    }
}
