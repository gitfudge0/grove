//! Snapshot synchronization, draft acknowledgements, and transcript follow state.

use super::{
    codex, composer_markdown_decorations, composer_markdown_replacements, composer_scope, rpx,
    CodexView, Rc, TranscriptRenderCache, TurnHistory,
};
use crate::views::tokens::{MOTION_FAST_MS, SPACE_20};
use gpui::{Context, Focusable, Window};

pub(super) struct TranscriptFollow {
    pub(super) near_bottom: bool,
    pub(super) initial_transcript: bool,
    pub(super) elapsed: f32,
}

impl CodexView {
    pub(super) fn prepare_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Rc<TranscriptRenderCache> {
        if !self.selection_root_observed {
            if let Some(root) = window.root::<gpui_component::Root>().flatten() {
                self.subscriptions
                    .push(cx.observe(&root, |_, _, cx| cx.notify()));
                self.selection_root_observed = true;
            }
        }
        self.validate_stop_confirmation(cx);
        self.flush_dashboard_submission(window, cx);
        let source = self.input.read(cx).value().to_string();
        let generation = grove_core::theme::generation();
        if self
            .markdown_source
            .as_ref()
            .is_none_or(|(previous, theme, _)| previous != &source || *theme != generation)
        {
            let decorations = composer_markdown_decorations(&source);
            let replacements = composer_markdown_replacements(&source);
            self.input.update(cx, |input, cx| {
                input.set_text_replacements(replacements, cx);
            });
            self.markdown_source = Some((source, generation, decorations));
        }
        // An identical programmatic replacement may adjust decorations even
        // though source bytes did not change. Restore cached spans as well.
        if let Some((_, _, decorations)) = &self.markdown_source {
            self.markdown_decorations.set(decorations.clone(), cx);
        }
        self.restore_image_tokens(window, cx);
        let state = if let Some(child) = &self.preview_child {
            let source = self.session.read(cx).state.shared();
            if self
                .preview_snapshot
                .as_ref()
                .is_none_or(|(old, id, _)| !Rc::ptr_eq(old, &source) || id != child)
            {
                let projected = Rc::new(source.subagent_snapshot(child));
                self.preview_snapshot = Some((source.clone(), child.clone(), projected));
            }
            self.preview_snapshot.as_ref().map_or_else(
                || Rc::new(source.subagent_snapshot(child)),
                |(_, _, projected)| projected.clone(),
            )
        } else {
            self.session.read(cx).state.shared()
        };
        if self.edit_confirmation.as_ref().is_some_and(|target| {
            target.thread_id != state.saved.thread_id
                || state
                    .saved
                    .items
                    .get(target.index)
                    .is_none_or(|item| item["id"].as_str() != Some(&target.message_id))
        }) {
            self.edit_confirmation = None;
        }
        if self
            .composer_menu_scope
            .as_ref()
            .is_some_and(|scope| *scope != composer_scope(&state))
        {
            self.close_menu(cx);
        }
        self.sync_native_plans(window, cx);
        self.sync_plan_composer_pending(cx);

        if state.edited_messages != self.edited_messages {
            self.edited_messages = state.edited_messages;
            self.edit_request_pending = false;
            self.rewind_attempt = None;
            self.edit_confirmation = None;
            self.load_restored_composer(&state, window, cx);
            self.bubble_entrances.clear();
            self.animating_bubbles.clear();
            self.stream_text.clear();
            self.set_composer_collapsed(false, cx);
        }

        if self
            .rewind_attempt
            .is_some_and(|attempt| state.rewind_attempts > attempt)
            && (state.error.is_some() || state.saved.pending_edit.is_none())
        {
            self.edit_request_pending = false;
            self.rewind_attempt = None;
        }
        // checkout_context contains chrome labels such as “Main checkout”, not
        // filesystem paths. Resolve document links against the saved worktree.
        let html_cwd = std::path::PathBuf::from(&state.saved.cwd);
        self.file_browser
            .update(cx, |browser, cx| browser.set_root(html_cwd.clone(), cx));
        let html_paused = self.menu.is_some()
            || self.popup.is_some()
            || !self.file_suggestions.is_empty()
            || self.edit_confirmation.is_some();
        self.html.update(cx, |html, cx| {
            html.sync(&state, html_cwd, html_paused, window, cx);
        });
        self.sync_queue_editor(&state, window, cx);
        self.sync_queue_motion(&state, cx);
        if self
            .pending_clear
            .as_ref()
            .is_some_and(|(generation, _, _)| state.cleared_conversations > *generation)
        {
            if let Some((_, draft, images)) = self.pending_clear.take() {
                if self.input.read(cx).value().as_ref() == draft {
                    self.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    self.picked_tokens.clear();
                    self.file_suggestions.clear();
                    self.file_query.clear();
                    self.file_lookup = None;
                    self.dismissed_suggestions = None;
                }
                if self.images == images {
                    self.images.clear();
                }
            }
            self.reference = None;
            self.submitted_reference = None;
            self.selection_views.clear();
            self.selection_subscriptions.clear();
            self.message_focus.clear();
            self.focus_subscriptions.clear();
            self.focus_sources.clear();
            self.focus_revision = None;
            self.selection_dismissed = None;
            self.submitted = None;
            self.submitted_ack = None;
            self.expanded.clear();
            self.menu = None;
            self.popup = None;
            self.popup_subscription = None;
            self.interrupt_requested_for_turn = None;
            self.bubble_entrances.clear();
            self.animating_bubbles.clear();
            self.approval_drafts.clear();
            self.question_drafts.clear();
            self.question_receipts.clear();
            self.elicitation_drafts.clear();
            self.follow_after_submit = true;
        }
        let scope = format!("{:?}:{}", state.saved.thread_id, state.interaction_epoch);
        if self.interaction_scope != scope {
            let restore_focus = self.approval_drafts.values().any(|draft| {
                draft.activity_anchor.focus.is_focused(window)
                    || draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window)
            }) || self.elicitation_drafts.values().any(|draft| {
                draft.activity_anchor.focus.is_focused(window)
                    || draft.fields.iter().any(|field| {
                        field.input.focus_handle(cx).is_focused(window)
                            || field
                                .choice_focus
                                .values()
                                .any(|focus| focus.is_focused(window))
                    })
            }) || self
                .question_drafts
                .values()
                .any(|draft| draft.activity_anchor.focus.is_focused(window));
            self.approval_drafts.clear();
            self.question_drafts.clear();
            self.question_receipts.clear();
            self.elicitation_drafts.clear();
            self.interaction_scope = scope;
            if restore_focus {
                self.input.focus_handle(cx).focus(window, cx);
            }
        }
        self.sync_bubble_entrances(&state, cx.reduce_motion(), cx.background_executor().now());
        self.sync_questions(&state, window, cx);
        self.sync_approvals(&state, window, cx);
        self.sync_elicitations(&state, window, cx);
        if self
            .submitted_ack
            .is_some_and(|target| state.submission_accepted(target))
        {
            let reference_matches =
                self.reference.as_ref().map(|quote| &quote.id) == self.submitted_reference.as_ref();
            if reference_matches {
                self.reference = None;
                self.reference_expanded = false;
            }
            self.submitted_reference = None;
            let submitted = self.submitted.take();
            self.submitted_ack = None;
            if reference_matches
                && submitted.as_deref() == Some(self.input.read(cx).value().as_ref())
            {
                self.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            if reference_matches && self.images == self.submitted_images {
                self.images.clear();
            }
        }
        if self
            .submitted_ack
            .is_some_and(|id| state.rejected_submission == Some(id))
            || state.status == codex::Status::Disconnected
        {
            self.submitted = None;
            self.submitted_ack = None;
        }
        // Submission receipts and restored/queued drafts must settle before
        // deciding whether the newly completed response has an empty composer.
        self.sync_next_reply(&state, window, cx);
        let optimistic_items: Vec<_> = self
            .bubble_entrances
            .iter()
            .filter_map(|pending| pending.optimistic.clone())
            .filter(|item| {
                !state
                    .saved
                    .items
                    .iter()
                    .any(|saved| saved["id"] == item["id"])
            })
            .collect();
        let transcript = match &self.transcript_cache {
            Some(cache) if cache.matches(&state, &optimistic_items) => cache.clone(),
            _ => {
                let cache = Rc::new(TranscriptRenderCache::refreshed(
                    state,
                    optimistic_items,
                    self.transcript_cache.as_deref(),
                ));
                self.transcript_previews.retain(|(index, _), (source, _)| {
                    cache
                        .state
                        .saved
                        .items
                        .get(*index)
                        .is_some_and(|item| std::ptr::eq(source.as_ref(), item.as_ref()))
                });
                self.transcript_cache = Some(cache.clone());
                cache
            }
        };
        self.prune_transcript_controls(&transcript);
        transcript
    }

    pub(super) fn update_transcript_follow(
        &mut self,
        transcript: &TranscriptRenderCache,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TranscriptFollow {
        let state = &transcript.state;
        if transcript.has_timestamp && !self.timestamp_tick_pending.replace(true) {
            let pending = self.timestamp_tick_pending.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                pending.set(false);
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        // Tool output/status and same-length streaming replacements also change layout.
        let content = Some(transcript.content);
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y
            <= rpx(SPACE_20).to_pixels(window.rem_size());
        if let Some(toward_bottom) = self.pending_composer_scroll.take() {
            if self.transcript_only() {
                self.preview_follow_latest = toward_bottom
                    && self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(1.);
            } else {
                // Collapsing enlarges the transcript viewport and can clamp its offset to
                // the new bottom. That layout change must not reverse an upward gesture.
                let restoring_bottom = toward_bottom && near_bottom && self.composer_collapsed;
                if restoring_bottom || (!toward_bottom && !near_bottom) {
                    self.set_composer_collapsed(!restoring_bottom, cx);
                }
                if restoring_bottom {
                    self.composer_pin_bottom = true;
                    self.smooth_follow = true;
                }
            }
        }
        // A blocking edit confirmation owns the visible source. Session updates
        // must not restart bottom-follow and pull that source out from its tray.
        if self.edit_confirmation.is_some() {
            self.smooth_follow = false;
            self.follow_after_submit = false;
        }
        if self.follow_after_submit {
            self.composer_pin_bottom = true;
            self.set_composer_collapsed(false, cx);
        }
        let changed_thread = self.stream_thread != state.saved.thread_id;
        if changed_thread {
            self.turn_history = TurnHistory::default();
            self.pending_history_jump = None;
            self.message_slots.clear();
            self.work_header_slots.clear();
            self.tool_slots.clear();
            self.selection_views.clear();
            self.selection_dismissed = None;
            if self
                .reference
                .as_ref()
                .is_some_and(|quote| quote.thread_id != state.saved.thread_id)
            {
                self.reference.clone_from(&state.saved.draft_reference);
                self.reference_expanded = false;
            }
            self.stream_text.clear();
            self.stream_thread.clone_from(&state.saved.thread_id);
        }
        if cx.reduce_motion() {
            // A later preference toggle must not resurrect an older partial prefix.
            for (text, shown) in self.stream_text.values_mut() {
                *shown = text.len();
            }
        }
        let initial_transcript = self.last_content.is_none() || changed_thread;
        self.stream_text.retain(|key, _| {
            transcript.id_index.contains_key(key)
                || key
                    .strip_prefix("message-")
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| state.saved.items.get(index))
                    .is_some_and(|item| !item["id"].is_string())
        });
        if self.follow_after_submit {
            self.last_follow_offset = None;
        }
        let now = cx.background_executor().now();
        let elapsed = self
            .motion_frame
            .replace(now)
            .map_or(0.0, |previous| now.duration_since(previous).as_secs_f32());
        if !self.transcript_only()
            && self.edit_confirmation.is_none()
            && (self.follow_after_submit
                || (content != self.last_content
                    && (near_bottom || self.smooth_follow)
                    && !self.composer_collapsed))
        {
            self.smooth_follow = true;
            self.follow_after_submit = false;
        }
        if self.scroll.max_offset().y == self.last_follow_max
            && self
                .last_follow_offset
                .is_some_and(|offset| offset != self.scroll.offset().y)
            && !near_bottom
        {
            self.smooth_follow = false;
        }
        if self.smooth_follow {
            let target = -self.scroll.max_offset().y;
            let current = self.scroll.offset();
            let fraction = if cx.reduce_motion() {
                1.0
            } else {
                1.0 - (-elapsed / (MOTION_FAST_MS as f32 / 1000.0)).exp()
            };
            let next = current.y + (target - current.y) * fraction;
            let settled = (target - next).abs() <= gpui::px(0.5);
            self.scroll
                .set_offset(gpui::point(current.x, if settled { target } else { next }));
            self.last_follow_offset = Some(self.scroll.offset().y);
            self.last_follow_max = self.scroll.max_offset().y;
            if !settled || content != self.last_content {
                let owner = cx.entity().downgrade();
                let pending = self.motion_frame_pending.clone();
                if !pending.replace(true) {
                    window.on_next_frame(move |_, cx| {
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            }
        }
        self.last_content = content;
        TranscriptFollow {
            near_bottom,
            initial_transcript,
            elapsed,
        }
    }

    pub(super) fn set_composer_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if !collapsed {
            self.pending_composer_scroll = None;
        }
        if self.composer_collapsed == collapsed {
            return;
        }
        self.composer_transition_from =
            (!cx.reduce_motion()).then(|| self.composer_body_height.get());
        self.composer_transition_generation = self.composer_transition_generation.wrapping_add(1);
        self.composer_motion_finished.set(cx.reduce_motion());
        self.composer_collapsed = collapsed;
        let min_rows = if self.dashboard_tray.is_some() { 2 } else { 3 };
        self.input.update(cx, |input, cx| {
            if collapsed {
                input.set_auto_grow(1, 1, cx);
            } else {
                input.set_auto_grow(min_rows, 10, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn transcript_scrolled(&mut self, toward_bottom: bool, cx: &mut Context<Self>) {
        // The next render observes the final offset after native wheel dispatch.
        // Keep this separate from layout/content updates, which never enter browse mode.
        self.smooth_follow = false;
        if self.transcript_only() {
            self.preview_follow_latest = toward_bottom
                && self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(1.);
            // Native dispatch applies the wheel delta after this listener. Recheck
            // the resulting offset next render without routing into composer browse.
            self.pending_composer_scroll = Some(toward_bottom);
            cx.notify();
            return;
        }
        self.pending_composer_scroll = Some(toward_bottom);
        self.composer_pin_bottom = false;
        cx.notify();
    }
}
