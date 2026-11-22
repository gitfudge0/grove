//! Queued prompt controls, draft restoration and row transitions.

use super::*;

impl CodexView {
    pub(super) fn queue_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let steering = if let Action::SteerQueued(id) = &action {
            self.session
                .read(cx)
                .state
                .queued_prompts
                .iter()
                .find(|prompt| prompt.id == *id)
                .cloned()
        } else {
            None
        };
        match self.session.read(cx).handle.send(action) {
            Ok(()) => {
                if let Some(prompt) = steering {
                    self.bubble_entrances
                        .retain(|pending| pending.submission != prompt.id);
                    self.record_submission_bubble(
                        prompt.id,
                        prompt.text,
                        prompt.images,
                        true,
                        true,
                        cx,
                    );
                    cx.notify();
                }
                true
            }
            Err(error) => {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(error);
                    cx.notify();
                });
                false
            }
        }
    }

    pub(super) fn begin_queue_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        self.dismiss_next_reply(cx);
        if self.plan_answer.is_some()
            || self.queue_editor.is_some()
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
            || !self.session.read(cx).state.queue_editable(id)
        {
            return;
        }
        let editor = QueueEditor {
            id,
            draft: self.input.read(cx).value().to_string(),
            images: self.images.clone(),
            tokens: self.picked_tokens.clone(),
            reference: None,
            loaded: false,
            saving: false,
        };
        let result = self.queue_action(Action::BeginQueuedEdit(id), cx);
        if result {
            self.queue_editor = Some(editor);
            self.queue_selected = Some(id);
        }
        cx.notify();
    }

    pub(super) fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self
            .queue_editor
            .as_mut()
            .filter(|editor| editor.loaded && !editor.saving)
        else {
            return;
        };
        let draft = self.input.read(cx).value().to_string();
        let text = editor
            .reference
            .as_ref()
            .map_or_else(|| draft.clone(), |reference| reference.serialize(&draft));
        if let Err(error) = codex::validate_prompt(&text, &self.images) {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
            return;
        }
        let id = editor.id;
        if self.queue_action(
            Action::SaveQueuedEdit {
                id,
                text,
                images: self.images.clone(),
            },
            cx,
        ) {
            if let Some(editor) = &mut self.queue_editor {
                editor.saving = true;
            }
        }
        cx.notify();
    }

    pub(super) fn restore_queue_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.queue_editor.take() {
            self.images = editor.images;
            for url in &self.images {
                if !self.previews.contains_key(url) {
                    if let Some(image) = image_from_url(url) {
                        self.previews.insert(url.clone(), image);
                    }
                }
            }
            self.picked_tokens = editor.tokens;
            self.input.update(cx, |input, cx| {
                input.set_value(editor.draft.clone(), window, cx);
            });
            self.session.update(cx, |session, cx| {
                session.send(Action::Draft(editor.draft), cx);
                session.send(Action::DraftImages(self.images.clone()), cx);
            });
            self.queue_selected = None;
            self.input.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_queue_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.queue_editor {
            if editor.saving {
                return;
            }
            let id = editor.id;
            if !self.queue_action(Action::CancelQueuedEdit(id), cx) {
                return;
            }
        }
        self.restore_queue_draft(window, cx);
    }

    pub(super) fn sync_queue_editor(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.queue_editor {
            if !editor.loaded && state.queue_edit == Some(editor.id) {
                if let Some(prompt) = state
                    .queued_prompts
                    .iter()
                    .find(|prompt| prompt.id == editor.id)
                {
                    // Capture the current unrelated draft at the moment the hold is granted.
                    // Pending input/attachment changes must survive the editor handoff.
                    editor.draft = self.input.read(cx).value().to_string();
                    editor.images.clone_from(&self.images);
                    editor.tokens.clone_from(&self.picked_tokens);
                    editor.loaded = true;
                    self.images = prompt.images.clone();
                    for url in &self.images {
                        if !self.previews.contains_key(url) {
                            if let Some(image) = image_from_url(url) {
                                self.previews.insert(url.clone(), image);
                            }
                        }
                    }
                    self.picked_tokens.clear();
                    let text = if let Some((reference, reply)) = reference_prompt(&prompt.text) {
                        editor.reference = Some(reference);
                        reply.to_owned()
                    } else {
                        prompt.text.clone()
                    };
                    self.input.update(cx, |input, cx| {
                        input.set_value(text, window, cx);
                    });
                    self.input.focus_handle(cx).focus(window, cx);
                }
            } else if editor.saving && state.queue_edit != Some(editor.id) {
                self.restore_queue_draft(window, cx);
            } else if !editor.loaded
                && !state.queue_editable(editor.id)
                && state.queue_edit != Some(editor.id)
            {
                // The provider won the dispatch race. Never overwrite the draft with a stale row.
                self.queue_editor = None;
                self.queue_selected = None;
            }
        }
        if self
            .queue_selected
            .is_some_and(|id| !state.queued_prompts.iter().any(|prompt| prompt.id == id))
        {
            self.queue_selected = None;
        }
    }

    pub(super) fn move_queue_selection(
        &mut self,
        up: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.input.focus_handle(cx).is_focused(window)
            || self.queue_editor.is_some()
            || !self.input.read(cx).value().is_empty()
            || self.submitted.is_some()
        {
            return false;
        }
        let state = &self.session.read(cx).state;
        let ids = state
            .queued_prompts
            .iter()
            .filter(|prompt| state.queue_editable(prompt.id))
            .map(|prompt| prompt.id)
            .collect::<Vec<_>>();
        let current = self
            .queue_selected
            .and_then(|id| ids.iter().position(|candidate| *candidate == id));
        self.queue_selected = match (up, current) {
            (true, None) => ids.last().copied(),
            (true, Some(index)) => ids.get(index.saturating_sub(1)).copied(),
            (false, Some(index)) => ids.get(index + 1).copied(),
            (false, None) => return false,
        };
        if ids.is_empty() {
            return false;
        }
        self.queue_collapsed = false;
        cx.notify();
        true
    }

    pub(super) fn sync_queue_motion(&mut self, state: &codex::Snapshot, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let reduced = cx.reduce_motion();
        self.queue_motion_rows.retain(|row| {
            !row.exiting
                || (!reduced
                    && now.duration_since(row.started)
                        < std::time::Duration::from_millis(MOTION_BASE_MS))
        });
        let mut animate = false;
        for row in &mut self.queue_motion_rows {
            if let Some(prompt) = state
                .queued_prompts
                .iter()
                .find(|prompt| prompt.id == row.prompt.id)
            {
                row.prompt = prompt.clone();
            } else if !row.exiting {
                row.exiting = true;
                row.started = now;
                animate = true;
            }
        }
        for prompt in &state.queued_prompts {
            if !self
                .queue_motion_rows
                .iter()
                .any(|row| row.prompt.id == prompt.id)
            {
                self.queue_motion_rows.push(QueueMotionRow {
                    prompt: prompt.clone(),
                    started: now,
                    exiting: false,
                });
                animate = true;
            }
        }
        if reduced {
            self.queue_motion_rows.retain(|row| !row.exiting);
        }
        if animate && !reduced {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    pub(super) fn queue_tray(
        &mut self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let steering_ids: HashSet<_> = self
            .bubble_entrances
            .iter()
            .filter(|pending| {
                pending
                    .optimistic
                    .as_ref()
                    .is_some_and(|item| item["localSteering"] == true)
            })
            .map(|pending| pending.submission)
            .collect();
        if self
            .queue_motion_rows
            .iter()
            .all(|row| steering_ids.contains(&row.prompt.id))
        {
            return div().into_any_element();
        }
        let count = state
            .queued_prompts
            .iter()
            .filter(|prompt| !steering_ids.contains(&prompt.id))
            .count();
        let pending_count = count;
        let resume_enabled =
            state.status != codex::Status::Disconnected && state.queue_uncertain.is_empty();
        let collapsed = self.queue_collapsed;
        let mut tray = composer_tray(window)
            .when(attached_above, gpui::Styled::rounded_none)
            .id("codex-queued-prompts")
            .debug_selector(|| "codex-queued-prompts".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(crate::icons::icon("queue", ICON_MD, c::FG_MUTE()))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Queued"),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(pending_count.to_string()),
                    )
                    .child(div().flex_1())
                    .when(!state.status.busy() && count > 0, |header| {
                        header.child(
                            header_control_with_label_tooltip(
                                "codex-resume-queue",
                                "Send next queued message",
                            )
                            .debug_selector(|| "codex-resume-queue".into())
                            .when(!resume_enabled, gpui::Styled::cursor_default)
                            .track_focus(
                                &self
                                    .queue_resume_focus
                                    .clone()
                                    .tab_stop(resume_enabled)
                                    .tab_index(0),
                            )
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon("arrow-up", ICON_MD, c::FG_DIM()))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if resume_enabled && action_key(event) {
                                        this.queue_action(Action::ResumeQueue, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if resume_enabled {
                                        this.queue_action(Action::ResumeQueue, cx);
                                    }
                                },
                            )),
                        )
                    })
                    .child(
                        header_control_with_label_tooltip(
                            "codex-collapse-queue",
                            if collapsed {
                                "Expand queued messages"
                            } else {
                                "Collapse queued messages"
                            },
                        )
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            if collapsed { "chev-right" } else { "chev-down" },
                            ICON_MD,
                            c::FG_MUTE(),
                        ))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) {
                                this.queue_collapsed = !this.queue_collapsed;
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.queue_collapsed = !this.queue_collapsed;
                            cx.notify();
                        })),
                    ),
            );
        if !collapsed {
            let mut rows = div()
                .id("codex-queue-rows")
                .max_h(rpx(QUEUED_PROMPTS_MAX_H))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for motion_row in &self.queue_motion_rows {
                let prompt = &motion_row.prompt;
                let id = prompt.id;
                if steering_ids.contains(&id) {
                    continue;
                }
                let exiting = motion_row.exiting;
                let locked = exiting || !state.queue_editable(id) || self.queue_editor.is_some();
                let steer = !locked && active_turn_redirect_available(state);
                let selected = self.queue_selected == Some(id);
                let label =
                    if let Some(plan) = state.saved.rich_plans.approval_for_payload(&prompt.text) {
                        format!(
                            "Plan approval · {} · Revision {}",
                            plan.document.title, plan.document.revision_id
                        )
                    } else if prompt.images.is_empty() {
                        collapsed_draft_preview(&reference_prompt_copy(&prompt.text))
                    } else {
                        format!(
                            "{} · {} images",
                            collapsed_draft_preview(&reference_prompt_copy(&prompt.text)),
                            prompt.images.len()
                        )
                    };
                let number = state
                    .queued_prompts
                    .iter()
                    .position(|prompt| prompt.id == id)
                    .map(|index| (index + 1).to_string())
                    .unwrap_or_default();
                let mut row = div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .pl(rpx(SPACE_LG))
                    .rounded(rpx(RADIUS_CONTROL))
                    .text_color(if selected { c::FG() } else { c::FG_DIM() })
                    .when(selected, |row| row.bg(c::MENU_HOVER()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .child(label),
                    );
                if state.queue_edit == Some(id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("Editing"),
                    );
                } else if state.queue_uncertain.contains(&id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("Delivery unconfirmed · Dismiss will not resend"),
                    );
                    row = row.child(
                        form_action("codex-dismiss-uncertain", "Dismiss", false, window, cx).icon(gpui_component::Icon::default().path("icons/close.svg").size(rpx(ICON_SM)))
                            .tooltip("Check the conversation first. Dismiss removes this unconfirmed entry without sending it again.")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue_action(Action::DismissUncertainQueued(id), cx);
                            })),
                    );
                } else if state.queue_sending.contains(&id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_MUTE())
                            .child(if state.queue_steering.contains(&id) {
                                "Steering…"
                            } else {
                                "Sending…"
                            }),
                    );
                }
                row = row
                    .child(
                        header_control_with_label_tooltip(
                            "codex-edit-queued",
                            "Edit queued message",
                        )
                        .tab_index(0)
                        .tab_stop(!locked)
                        .when(locked, gpui::Styled::cursor_default)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            "edit",
                            ICON_MD,
                            if locked { c::FG_MUTE() } else { c::FG_DIM() },
                        ))
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) && !locked {
                                this.begin_queue_edit(id, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !locked {
                                this.begin_queue_edit(id, cx);
                            }
                        })),
                    )
                    .child(
                        header_control_with_label_tooltip("codex-steer-queued", STEER_DESCRIPTION)
                            .w_auto()
                            .px(rpx(SPACE_SM))
                            .tab_index(0)
                            .tab_stop(steer)
                            .when(!steer, gpui::Styled::cursor_default)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(crate::icons::icon(
                                "steer",
                                ICON_MD,
                                if steer { c::FG_DIM() } else { c::FG_MUTE() },
                            ))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(if steer { c::FG_DIM() } else { c::FG_MUTE() })
                                    .child(STEER_LABEL),
                            )
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if action_key(event) && steer {
                                        this.queue_action(Action::SteerQueued(id), cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if steer {
                                    this.queue_action(Action::SteerQueued(id), cx);
                                }
                            })),
                    )
                    .child(
                        header_control_with_label_tooltip(
                            "codex-remove-queued",
                            "Remove queued message",
                        )
                        .tab_index(0)
                        .tab_stop(!locked)
                        .when(locked, gpui::Styled::cursor_default)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            "close",
                            ICON_MD,
                            if locked { c::FG_MUTE() } else { c::FG_DIM() },
                        ))
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) && !locked {
                                this.queue_action(Action::RemoveQueued(id), cx);
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !locked {
                                this.queue_action(Action::RemoveQueued(id), cx);
                            }
                        })),
                    );
                let row = div()
                    .id(format!("codex-queued-{id}"))
                    .debug_selector(move || format!("codex-queued-{id}"))
                    .h(rpx(CHROME_CONTROL_H + SPACE_SM))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .w(rpx(ICON_SM))
                            .flex_shrink_0()
                            .text_center()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_MUTE())
                            .child(number),
                    )
                    .child(row);
                let elapsed = motion_row.started.elapsed();
                let row = if !cx.reduce_motion()
                    && elapsed < std::time::Duration::from_millis(MOTION_BASE_MS)
                {
                    let exiting = motion_row.exiting;
                    row.with_animation(
                        format!("queue-row-motion-{id}-{exiting}"),
                        gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                            .with_easing(super::super::motion::ease_standard),
                        move |row, progress| {
                            let fraction = if exiting { 1.0 - progress } else { progress };
                            row.h(rpx((CHROME_CONTROL_H + SPACE_SM) * fraction))
                                .opacity(fraction)
                        },
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                };
                rows = rows.child(row);
            }
            tray = tray.child(rows);
        }
        if self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded)
        {
            let saving = self
                .queue_editor
                .as_ref()
                .is_some_and(|editor| editor.saving);
            tray = tray.child(
                div()
                    .flex()
                    .justify_end()
                    .gap(rpx(SPACE_SM))
                    .child(
                        header_control_with_label_tooltip(
                            "codex-cancel-queue-edit",
                            "Cancel queued message edit",
                        )
                        .debug_selector(|| "codex-cancel-queue-edit".into())
                        .tab_index(0)
                        .tab_stop(!saving)
                        .when(saving, gpui::Styled::cursor_default)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            "close",
                            ICON_MD,
                            if saving { c::FG_MUTE() } else { c::FG_DIM() },
                        ))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if action_key(event) && !saving {
                                    this.cancel_queue_edit(window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if !saving {
                                    this.cancel_queue_edit(window, cx);
                                }
                            },
                        )),
                    )
                    .child(
                        header_control_with_label_tooltip(
                            "codex-save-queue-edit",
                            "Save queued message edit",
                        )
                        .debug_selector(|| "codex-save-queue-edit".into())
                        .tab_index(0)
                        .tab_stop(!saving)
                        .when(saving, gpui::Styled::cursor_default)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            "check",
                            ICON_MD,
                            if saving { c::FG_MUTE() } else { c::FG_DIM() },
                        ))
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) && !saving {
                                this.save_queue_edit(cx);
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !saving {
                                this.save_queue_edit(cx);
                            }
                        })),
                    ),
            );
        }
        tray.into_any_element()
    }
}
