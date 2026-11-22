//! Message focus, copy, fork, expansion and stale control cleanup.

use super::*;

impl CodexView {
    pub(super) fn prune_transcript_controls(&mut self, transcript: &TranscriptRenderCache) {
        let revision = transcript.state.saved.items.revision();
        if self.focus_revision == Some(revision)
            && self.focus_thread == transcript.state.saved.thread_id
        {
            return;
        }
        if self.focus_thread != transcript.state.saved.thread_id {
            self.message_focus.clear();
            self.focus_subscriptions.clear();
        }
        self.focus_thread
            .clone_from(&transcript.state.saved.thread_id);
        self.focus_revision = Some(revision);
        let thread = transcript
            .state
            .saved
            .thread_id
            .as_deref()
            .unwrap_or_default();
        let prefix = format!("{thread}:");
        let mut valid: HashSet<String> = transcript
            .state
            .saved
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{thread}:{}",
                    item["id"]
                        .as_str()
                        .map_or_else(|| index.to_string(), str::to_owned)
                )
            })
            .collect();
        for group in &transcript.response_groups {
            valid.insert(group.key.clone());
            valid.extend(group.source_keys.iter().cloned());
        }
        let sources: HashMap<_, _> = transcript
            .state
            .saved
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                (
                    format!(
                        "{thread}:{}",
                        item["id"]
                            .as_str()
                            .map_or_else(|| index.to_string(), str::to_owned)
                    ),
                    item.clone(),
                )
            })
            .collect();
        let changed_code_owners: HashSet<_> = self
            .focus_sources
            .iter()
            .filter(|(owner, old)| {
                sources
                    .get(*owner)
                    .is_none_or(|new| !std::ptr::eq(old.as_ref(), new.as_ref()))
            })
            .map(|(owner, _)| owner.clone())
            .collect();
        self.focus_sources = sources;
        let count = transcript.state.saved.items.len();
        self.message_focus.retain(|key, _| {
            if key
                .rsplit_once(":code:")
                .is_some_and(|(owner, _)| changed_code_owners.contains(owner))
            {
                return false;
            }
            if let Some(boundary) = key.strip_prefix("fork:") {
                return boundary
                    .split(':')
                    .next()
                    .and_then(|v| v.parse::<usize>().ok())
                    .is_some_and(|index| index <= count);
            }
            for family in ["tool-row:", "tool-copy:", "work-header:"] {
                if let Some(row) = key.strip_prefix(family) {
                    return row
                        .strip_prefix(&prefix)
                        .and_then(|row| row.split(':').next())
                        .and_then(|v| v.parse::<usize>().ok())
                        .is_some_and(|index| index < count);
                }
            }
            if key.starts_with(&prefix) {
                let mut base = key.as_str();
                loop {
                    if valid.contains(base) {
                        return true;
                    }
                    let Some((parent, _)) = base.rsplit_once(':') else {
                        return false;
                    };
                    base = parent;
                }
            }
            true
        });
        self.focus_subscriptions
            .retain(|key, _| self.message_focus.contains_key(key));
    }

    pub(super) fn ensure_control_focus(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let focus = self
            .ensure_message_focus(key, window, cx)
            .tab_stop(true)
            .tab_index(0);
        self.message_focus.insert(key.to_owned(), focus.clone());
        focus
    }

    pub(super) fn ensure_message_focus(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(focus) = self.message_focus.get(key) {
            return focus.clone();
        }
        let focus = cx.focus_handle();
        self.focus_subscriptions.insert(
            key.to_owned(),
            (
                cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
            ),
        );
        self.message_focus.insert(key.to_owned(), focus.clone());
        focus
    }

    pub(super) fn fork_control(
        &mut self,
        boundary: usize,
        item_id: Option<String>,
        restore_user: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus =
            self.ensure_control_focus(&format!("fork:{boundary}:{restore_user}"), window, cx);
        let enabled = item_id
            .as_deref()
            .and_then(|id| self.fork_boundary(id, restore_user, cx))
            .is_some();
        let keyboard_id = item_id.clone();
        let control = if enabled {
            header_control_with_label_tooltip("fork-message", "Fork session from here")
        } else {
            super::super::components::header_control_with_hover_tooltip(
                "fork-message",
                "Fork unavailable at this point",
                gpui::transparent_black(),
            )
        };
        control
            .id((
                if restore_user {
                    "fork-user-message"
                } else {
                    "fork-response"
                },
                boundary,
            ))
            .track_focus(&focus)
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(crate::icons::icon("git-branch", ICON_MD, c::FG_DIM()))
            .when(!enabled, |control| control.opacity(0.4))
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled {
                    if let Some(id) = &item_id {
                        this.fork_from_item(id, restore_user, cx);
                    }
                }
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if enabled && action_key(event) {
                    if let Some(id) = &keyboard_id {
                        this.fork_from_item(id, restore_user, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }

    pub(super) fn response_copy_text(&self, group: &ResponseGroup) -> String {
        let text = group.copy_text();
        if self.review_scope.is_some() {
            grove_core::conversation_result::display_text(
                "review",
                "grove-review",
                "agentMessage",
                &text,
            )
        } else {
            description_conversation_text(self.description_scope.as_ref(), "agentMessage", &text)
        }
    }

    pub(super) fn response_footer(
        &mut self,
        group: &ResponseGroup,
        projected: &codex::Snapshot,
        flow_margin: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Section::footer();
        #[cfg(test)]
        measured_slot::RENDERED_FOOTERS.with(|keys| {
            keys.borrow_mut().insert(group.key.clone());
        });
        let key = group.key.clone();
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
        let visible = self.message_hover.contains(&key)
            || focus.contains_focused(window, cx)
            || group.source_keys.iter().any(|source| {
                self.message_focus
                    .get(source)
                    .is_some_and(|focus| focus.contains_focused(window, cx))
            });
        let copied = self
            .copied_message
            .as_ref()
            .is_some_and(|(message, _)| message == &key);
        let mut actions = div()
            .id(gpui::SharedString::from(format!("{}:actions", group.key)))
            .debug_selector(move || "codex-response-footer".into())
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .h(rpx(CHROME_CONTROL_H));
        if let Some(timestamp) = group.received_at {
            actions = actions.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_MUTE())
                    .child(relative_message_time(timestamp)),
            );
        }
        let copy_focus = self.ensure_control_focus(&format!("{key}:copy"), window, cx);
        let keyboard_key = key.clone();
        let copy_group = group.clone();
        let keyboard_group = group.clone();
        let mut button = if copied {
            header_control_frame("codex-message-copy", "Copied", c::BG_HOVER())
        } else {
            header_control_with_label_tooltip("codex-message-copy", "Copy entire response")
        }
        .track_focus(&copy_focus)
        .relative()
        .debug_selector(|| "codex-copy-response".into())
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .child(crate::icons::icon(
            if copied { "check" } else { "copy" },
            ICON_MD,
            c::FG_DIM(),
        ))
        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
            if action_key(event) {
                let text = this.response_copy_text(&keyboard_group);
                this.copy_message(keyboard_key.clone(), text, cx);
                cx.stop_propagation();
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
            let text = this.response_copy_text(&copy_group);
            this.copy_message(key.clone(), text, cx);
        }));
        if copied {
            button = button.child(
                div()
                    .absolute()
                    .bottom(rpx(CHROME_CONTROL_H + SPACE_SM))
                    .right_0()
                    .child(tooltip("Copied", window).build(window, cx)),
            );
        }
        actions = actions.child(button).child(
            self.fork_control(
                group.end,
                group
                    .anchor_index
                    .and_then(|index| projected.saved.items.get(index))
                    .and_then(|item| item["id"].as_str())
                    .map(str::to_owned),
                false,
                window,
                cx,
            ),
        );
        let hover_key = group.key.clone();
        div()
            .id(gpui::SharedString::from(group.key.clone()))
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .when(flow_margin, |footer| footer.mt(rpx(-SPACE_LG)))
            .track_focus(&focus)
            .tab_index(0)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.message_hover.insert(hover_key.clone());
                } else {
                    this.message_hover.remove(&hover_key);
                }
                cx.notify();
            }))
            .child(actions.when(!visible && !copied, |actions| actions.opacity(0.0)))
            .into_any_element()
    }

    pub(super) fn copy_code_block(
        &mut self,
        key: String,
        text: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.into().to_string()));
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        self.copied_code_blocks.insert(key.clone(), generation);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_code_blocks.get(&key) == Some(&generation) {
                    this.copied_code_blocks.remove(&key);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn copy_message(&mut self, key: String, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        self.copied_message = Some((key, generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(MOTION_SLOW_MS * 6))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .copied_message
                    .as_ref()
                    .is_some_and(|(_, current)| *current == generation)
                {
                    this.copied_message = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn source_item_index(&self, id: &str, cx: &App) -> Option<usize> {
        let state = &self.session.read(cx).state;
        if let Some(cache) = self
            .transcript_cache
            .as_ref()
            .filter(|cache| cache.source.saved.items.revision() == state.saved.items.revision())
        {
            return cache
                .id_index
                .get(id)
                .copied()
                .filter(|index| *index < state.saved.items.len());
        }
        state.saved.items.iter().position(|item| item["id"] == id)
    }

    pub(super) fn fork_boundary(&self, id: &str, restore_user: bool, cx: &App) -> Option<usize> {
        if self.transcript_only() {
            return None;
        }
        let boundary = self.source_item_index(id, cx)? + usize::from(!restore_user);
        self.session
            .read(cx)
            .state
            .can_fork_at(boundary, restore_user)
            .then_some(boundary)
    }

    pub(super) fn fork_from_item(&self, id: &str, restore_user: bool, cx: &mut Context<Self>) {
        if let Some(boundary) = self.fork_boundary(id, restore_user, cx) {
            cx.emit(CodexViewEvent::ForkRequested(boundary, restore_user));
        }
    }

    pub(super) fn toggle_expansion(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
        cx.notify();
    }
}
