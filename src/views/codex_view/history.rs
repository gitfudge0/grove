//! A presentation window over the full, lossless transcript.
use super::*;

const HISTORY_BATCH_TURNS: usize = 10;

// Each user message and its following agent response count separately. Keep
// commentary and tools together with that response rather than counting events.
pub(super) fn history_boundaries(
    items: &[codex::TranscriptItem],
    entries: &[MessageMinimapEntry],
) -> Vec<usize> {
    let mut boundaries = Vec::with_capacity(entries.len() * 2);
    for (index, entry) in entries.iter().enumerate() {
        boundaries.push(entry.item_index);
        let end = entries
            .get(index + 1)
            .map_or(items.len(), |next| next.item_index);
        if entry.item_index + 1 < end {
            boundaries.push(entry.item_index + 1);
        }
    }
    boundaries
}

#[derive(Default)]
pub(super) struct TurnHistory {
    pub(super) start: usize,
    anchor: Option<codex::TranscriptItem>,
    pub(super) expanded: bool,
    initialized: bool,
}

impl TurnHistory {
    #[cfg(test)]
    pub(super) fn expand_all_for_test(&mut self, items: &[codex::TranscriptItem]) {
        self.set_start(0, items);
        self.expanded = true;
    }

    fn latest(entries: &[usize]) -> usize {
        if entries.len() <= HISTORY_BATCH_TURNS {
            0
        } else {
            entries[entries.len() - HISTORY_BATCH_TURNS]
        }
    }

    fn set_start(&mut self, start: usize, items: &[codex::TranscriptItem]) {
        self.start = start;
        self.anchor = items.get(start).cloned();
        self.initialized = true;
    }

    fn same_source(item: &codex::TranscriptItem, anchor: &codex::TranscriptItem) -> bool {
        match (item["id"].as_str(), anchor["id"].as_str()) {
            (Some(id), Some(anchor_id)) => id == anchor_id,
            _ => item == anchor,
        }
    }

    pub(super) fn sync(
        &mut self,
        items: &[codex::TranscriptItem],
        entries: &[MessageMinimapEntry],
        following: bool,
    ) -> usize {
        let entries = history_boundaries(items, entries);
        let latest = Self::latest(&entries);
        if !self.initialized || (following && !self.expanded) {
            self.set_start(latest, items);
        } else if let Some(anchor) = &self.anchor {
            if !items
                .get(self.start)
                .is_some_and(|item| Self::same_source(item, anchor))
            {
                // Compaction/rewind may replace or move source indices. Never
                // apply a stale boundary to an unrelated conversational turn.
                if let Some(index) = items
                    .iter()
                    .position(|item| Self::same_source(item, anchor))
                {
                    self.set_start(index, items);
                } else {
                    *self = Self::default();
                    self.set_start(latest, items);
                }
            }
        }
        if self.start > latest {
            self.set_start(latest, items);
        }
        self.start
    }

    fn reveal(&mut self, items: &[codex::TranscriptItem], entries: &[MessageMinimapEntry]) {
        let entries = history_boundaries(items, entries);
        let hidden = entries.partition_point(|entry| *entry < self.start);
        let start = hidden
            .checked_sub(HISTORY_BATCH_TURNS)
            .filter(|index| *index > 0)
            .map_or(0, |index| entries[index]);
        self.set_start(start, items);
        self.expanded = true;
    }
}

impl CodexView {
    pub(super) fn jump_to_transcript_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(transcript) = &self.transcript_cache {
            while index < self.turn_history.start {
                self.turn_history
                    .reveal(&transcript.state.saved.items, &transcript.minimap_entries);
            }
        }
        self.pending_history_jump = Some(index);
        self.smooth_follow = false;
        self.follow_after_submit = false;
        self.composer_pin_bottom = false;
        self.pending_composer_scroll = None;
        self.set_composer_collapsed(true, cx);
        // The item-to-child mapping is rebuilt after revealing the batch.
        let owner = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = owner.update(cx, |_, cx| cx.notify());
        });
        cx.notify();
    }

    fn reveal_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(transcript) = &self.transcript_cache {
            self.turn_history
                .reveal(&transcript.state.saved.items, &transcript.minimap_entries);
        }
        if self.turn_history.start == 0 {
            self.history_latest_focus.focus(window, cx);
        } else {
            self.history_earlier_focus.focus(window, cx);
        }
        self.smooth_follow = false;
        self.follow_after_submit = false;
        self.composer_pin_bottom = false;
        self.scroll.scroll_to_top_of_item(0);
        cx.notify();
    }

    fn collapse_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.turn_history = TurnHistory::default();
        if self.transcript_cache.as_ref().is_some_and(|cache| {
            history_boundaries(&cache.state.saved.items, &cache.minimap_entries).len()
                > HISTORY_BATCH_TURNS
        }) {
            self.history_earlier_focus.focus(window, cx);
        } else {
            self.input.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.smooth_follow = false;
        self.pending_history_jump = None;
        self.scroll.scroll_to_top_of_item(0);
        cx.notify();
    }

    pub(super) fn render_history_disclosure(
        &self,
        hidden: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut row = div()
            .id("codex-history-window")
            .debug_selector(|| "codex-history-window".into())
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(SPACE_2XL))
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM());
        if hidden > 0 {
            let label = format!("Earlier conversation · {hidden} turns");
            row = row.child(
                super::super::components::inline_disclosure("codex-history-earlier", label.clone())
                    .debug_selector(|| "codex-history-earlier".into())
                    .track_focus(&self.history_earlier_focus)
                    .w_auto()
                    .h_auto()
                    .min_h(rpx(CONTROL_H))
                    .gap(rpx(SPACE_SM))
                    .tab_index(0)
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .aria_expanded(false)
                    .aria_description(format!(
                        "Show {} earlier turns",
                        hidden.min(HISTORY_BATCH_TURNS)
                    ))
                    .child(crate::icons::icon("chev-right", ICON_SM, c::FG_DIM()))
                    .child(label)
                    .on_click(cx.listener(|this, _, window, cx| this.reveal_history(window, cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.reveal_history(window, cx);
                            cx.stop_propagation();
                        }
                    })),
            );
        } else {
            row = row.child("Beginning of conversation");
        }
        row = row.child(
            div()
                .flex_1()
                .min_w(rpx(SPACE_20))
                .border_t_1()
                .border_color(c::BORDER_SOFT()),
        );
        if self.turn_history.expanded {
            row = row.child(
                super::super::components::inline_disclosure(
                    "codex-history-latest",
                    "Collapse to latest 10",
                )
                .debug_selector(|| "codex-history-latest".into())
                .track_focus(&self.history_latest_focus)
                .w_auto()
                .h_auto()
                .min_h(rpx(CONTROL_H))
                .gap(rpx(SPACE_SM))
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(crate::icons::icon("chev-up", ICON_SM, c::FG_DIM()))
                .child("Collapse to latest 10")
                .on_click(cx.listener(|this, _, window, cx| this.collapse_history(window, cx)))
                .on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.collapse_history(window, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn items(turns: usize) -> Vec<codex::TranscriptItem> {
        (0..turns)
            .flat_map(|turn| {
                [
                    json!({"id":format!("user-{turn}"),"type":"userMessage","text":"Request"})
                        .into(),
                    json!({"id":format!("answer-{turn}"),"type":"agentMessage","text":"Answer"})
                        .into(),
                ]
            })
            .collect()
    }

    #[test]
    fn batches_follow_browse_and_reconcile_surviving_sources() {
        let mut items = items(84);
        let mut entries = message_minimap_entries(&items);
        let mut history = TurnHistory::default();
        assert_eq!(history.sync(&items, &entries, true), 158);
        let appended = [
            json!({"id":"new-user","type":"userMessage"}).into(),
            json!({"id":"new-answer","type":"agentMessage"}).into(),
        ];
        items.extend(appended);
        entries = message_minimap_entries(&items);
        assert_eq!(
            history.sync(&items, &entries, false),
            158,
            "browsing pins the source"
        );
        assert_eq!(
            history.sync(&items, &entries, true),
            160,
            "following advances the cutoff"
        );
        history.reveal(&items, &entries);
        assert_eq!(history.start, 150);
        assert_eq!(
            history.sync(&items, &entries, true),
            150,
            "manual expansion survives follow"
        );
        items.drain(..10);
        entries = message_minimap_entries(&items);
        assert_eq!(
            history.sync(&items, &entries, false),
            140,
            "source survives compaction at a new index"
        );
        items.truncate(8);
        entries = message_minimap_entries(&items);
        assert_eq!(
            history.sync(&items, &entries, false),
            0,
            "rewind removes a stale boundary"
        );
        assert!(!history.expanded);
    }

    #[test]
    fn agent_activity_counts_once_and_keeps_tools_with_the_response() {
        let mut items = items(5);
        items.push(json!({"id":"next-user","type":"userMessage","text":"Next"}).into());
        let mut history = TurnHistory::default();
        assert_eq!(
            history.sync(&items, &message_minimap_entries(&items), true),
            1,
            "eleventh turn hides the first user but retains its answer"
        );
        items.push(json!({"id":"tool","type":"commandExecution"}).into());
        items.push(
            json!({"id":"comment","type":"agentMessage","phase":"commentary","text":"Working"})
                .into(),
        );
        items.push(
            json!({"id":"answer","type":"agentMessage","phase":"final_answer","text":"Done"})
                .into(),
        );
        let entries = message_minimap_entries(&items);
        assert_eq!(history_boundaries(&items, &entries).len(), 12);
        assert_eq!(history.sync(&items, &entries, true), 2);
        assert_eq!(
            history_boundaries(&items, &entries).last(),
            Some(&11),
            "the agent boundary includes its preceding tool activity"
        );
        history.reveal(&items, &entries);
        assert_eq!(history.start, 0);
    }

    #[test]
    fn legacy_rows_without_ids_reconcile_the_boundary_after_compaction() {
        let mut items: Vec<codex::TranscriptItem> = (0..84)
            .map(|turn| json!({"type":"userMessage", "text":format!("Request {turn}")}).into())
            .collect();
        let mut history = TurnHistory::default();
        assert_eq!(
            history.sync(&items, &message_minimap_entries(&items), true),
            74
        );
        history.reveal(&items, &message_minimap_entries(&items));
        items.drain(..10);
        assert_eq!(
            history.sync(&items, &message_minimap_entries(&items), false),
            54
        );
        assert_eq!(items[history.start]["text"], "Request 64");
    }

    #[test]
    fn short_and_exact_batch_histories_keep_their_preamble() {
        for count in [0, 1, 5, 6, 10, 11] {
            let mut items = items(count);
            items.insert(
                0,
                json!({"id":"preamble","type":"providerNotice","text":"Notice"}).into(),
            );
            let entries = message_minimap_entries(&items);
            let mut history = TurnHistory::default();
            let start = history.sync(&items, &entries, true);
            assert_eq!(start, if count <= 5 { 0 } else { (count * 2) - 10 + 1 });
            while history.start > 0 {
                history.reveal(&items, &entries);
            }
            assert_eq!(history.start, 0);
            assert_eq!(items[0]["id"], "preamble");
        }
    }
}
