//! Session-owned, read-only side conversations.
use super::*;
use crate::views::components::{project_field_well, transcript_action};
use gpui_component::{
    input::{Input, InputState},
    Disableable,
};

const SIDE_CHAT_WIDTH: f32 = 280.;

pub(super) struct SideChat {
    input: Entity<InputState>,
    _input_subscription: gpui::Subscription,
    messages: Vec<(String, String)>,
    busy: bool,
    error: Option<String>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    scroll: gpui::ScrollHandle,
    follow_latest: bool,
    pending_scroll: Option<bool>,
}

impl Sidebar {
    fn persist_side_chat_draft(&self, key: &(SessionId, String), cx: &mut Context<Self>) {
        let Some(chat) = self.checkout_panel.side_chats.get(key) else {
            return;
        };
        let draft = chat.input.read(cx).value().to_string();
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned();
        if let Some(session) = session {
            session.update(cx, |session, cx| {
                session.send(
                    grove_core::codex::Action::SideChatDraft {
                        checkout: key.1.clone(),
                        draft,
                    },
                    cx,
                )
            });
        }
    }
    fn persist_side_chat(&self, key: &(SessionId, String), cx: &mut Context<Self>) {
        let Some(chat) = self.checkout_panel.side_chats.get(key) else {
            return;
        };
        let saved = grove_core::codex::SavedSideChat {
            draft: chat.input.read(cx).value().to_string(),
            messages: chat.messages.clone(),
        };
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned();
        if let Some(session) = session {
            session.update(cx, |session, cx| {
                session.send(
                    grove_core::codex::Action::SideChat {
                        checkout: key.1.clone(),
                        chat: saved,
                    },
                    cx,
                )
            });
        }
    }
    pub(super) fn toggle_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checkout_panel.side_chat_owner == self.selection {
            self.checkout_panel.side_chat_owner = None;
            self.checkout_panel.hidden = true;
        } else {
            self.dismiss_agent_peek(window, cx);
            self.dismiss_plan_peek(window, cx);
            self.dismiss_script_peek(window, cx);
            self.checkout_panel.side_chat_owner = self.selection.clone();
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
        }
        cx.notify();
    }

    fn send_side_chat(
        &mut self,
        key: (SessionId, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.selection, Some(Selection::Session(id) | Selection::Home(id)) if id == key.0)
            || super::checkout_panel::selected_checkout(
                &self.checkout_targets(cx),
                self.checkout_panel.chosen_root.as_deref(),
            )
            .as_ref()
            .map(|(_, path)| path)
                != Some(&key.1)
        {
            return;
        }
        let Some(session) = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned()
        else {
            return;
        };
        let saved = &session.read(cx).state.saved;
        let provider = saved.provider;
        let cwd = key.1.clone();
        let context = serde_json::to_string(&saved.items).unwrap_or_default();
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return;
        };
        let question = chat.input.read(cx).value().to_string();
        if chat.busy || question.trim().is_empty() {
            return;
        }
        chat.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        chat.messages.push(("user".into(), question));
        chat.error = None;
        chat.busy = true;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        chat.cancel = Some(cancel.clone());
        let conversation = chat.messages.clone();
        chat.follow_latest = true;
        self.persist_side_chat(&key, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    grove_core::side_chat::answer_with_cancel(
                        provider,
                        &cwd,
                        &context,
                        &conversation,
                        &cancel,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(chat) = this.checkout_panel.side_chats.get_mut(&key) {
                    chat.busy = false;
                    chat.cancel = None;
                    match result {
                        Ok(answer) => chat.messages.push(("assistant".into(), answer)),
                        Err(error) => chat.error = Some(error),
                    }
                }
                this.persist_side_chat(&key, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn retry_side_chat(
        &mut self,
        key: (SessionId, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.selection, Some(Selection::Session(id) | Selection::Home(id)) if id == key.0)
            || super::checkout_panel::selected_checkout(
                &self.checkout_targets(cx),
                self.checkout_panel.chosen_root.as_deref(),
            )
            .as_ref()
            .map(|(_, path)| path)
                != Some(&key.1)
        {
            return;
        }
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return;
        };
        let Some((role, question)) = chat.messages.last() else {
            return;
        };
        if role != "user" || chat.busy {
            return;
        }
        let question = question.clone();
        let draft = chat.input.read(cx).value().to_string();
        let input = chat.input.clone();
        chat.messages.pop();
        chat.input
            .update(cx, |input, cx| input.set_value(question, window, cx));
        self.send_side_chat(key, window, cx);
        input.update(cx, |input, cx| input.set_value(draft, window, cx));
    }

    pub(super) fn render_side_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Selection::Session(id) | Selection::Home(id)) = self.selection else {
            return div().into_any_element();
        };
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        // Only recover a previously bound conversation when no checkout is
        // available. Ambiguous multi-root sessions still require a selection.
        let recovery_paths = if targets.is_empty() {
            let mut paths = self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .codex(id)
                .map(|session| {
                    session
                        .read(cx)
                        .state
                        .saved
                        .side_chats
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            paths.extend(
                self.checkout_panel
                    .side_chats
                    .keys()
                    .filter(|(session, _)| *session == id)
                    .map(|(_, path)| path.clone()),
            );
            paths.sort();
            paths.dedup();
            paths
        } else {
            Vec::new()
        };
        let recovered = self
            .checkout_panel
            .chosen_root
            .as_ref()
            .filter(|path| recovery_paths.contains(path))
            .cloned()
            .or_else(|| (recovery_paths.len() == 1).then(|| recovery_paths[0].clone()));
        let available = selected.is_some();
        let Some(path) = selected.map(|(_, path)| path).or(recovered) else {
            let mut panel = div()
                .id("side-chat-checkout-required")
                .w(rpx(SIDE_CHAT_WIDTH))
                .h_full()
                .pt(rpx(APPBAR_TOTAL_H + SPACE_3XL))
                .p(rpx(SPACE_3XL))
                .text_size(rpx(TEXT_BODY))
                .text_color(c::FG_DIM())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG));
            if recovery_paths.is_empty() {
                panel = panel.child("Choose a checkout in the right panel before using Side chat.");
            } else {
                panel = panel.child(
                    "These checkouts are unavailable. Choose a conversation to view its history.",
                );
                for (index, path) in recovery_paths.into_iter().enumerate() {
                    let target = path.clone();
                    panel = panel.child(
                        transcript_action(("side-chat-recover", index), path, false, window, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout_panel.chosen_root = Some(target.clone());
                                cx.notify();
                            })),
                    );
                }
            }
            return panel.into_any_element();
        };
        let key = (id, path.clone());
        let provider = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .map(|session| session.read(cx).state.saved.provider);
        let supported = available && provider == Some(grove_core::codex::Provider::Codex);
        let context_disclosure = match (available, provider) {
            (false, _) => "Read-only · Context is unavailable; no context is sent.",
            (true, Some(grove_core::codex::Provider::Codex)) => "Read-only · Each question sends Codex a new, bounded snapshot of this session and worktree. It may omit files or history; answers may become stale as work continues.",
            (true, Some(grove_core::codex::Provider::Claude)) => "Read-only · Claude Code is unavailable; no context is sent.",
            (true, None) => "Read-only · Session context is unavailable; no context is sent.",
        };
        if !self.checkout_panel.side_chats.contains_key(&key) {
            let saved = self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .codex(id)
                .and_then(|session| session.read(cx).state.saved.side_chats.get(&path).cloned());
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .placeholder("Ask about this session or worktree…")
            });
            if let Some(saved) = &saved {
                input.update(cx, |input, cx| input.set_value(&saved.draft, window, cx));
            }
            let subscription_key = key.clone();
            let subscription = cx.subscribe(&input, move |this, _, event, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.persist_side_chat_draft(&subscription_key, cx);
                }
            });
            let interrupted = saved.as_ref().is_some_and(|saved| {
                saved
                    .messages
                    .last()
                    .is_some_and(|(role, _)| role == "user")
            });
            self.checkout_panel.side_chats.insert(
                key.clone(),
                SideChat {
                    input,
                    _input_subscription: subscription,
                    messages: saved.map(|saved| saved.messages).unwrap_or_default(),
                    busy: false,
                    error: interrupted.then(|| {
                        "The previous response was interrupted. Retry to continue.".into()
                    }),
                    cancel: None,
                    scroll: gpui::ScrollHandle::new(),
                    follow_latest: true,
                    pending_scroll: None,
                },
            );
        }
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return div().into_any_element();
        };
        if let Some(toward_bottom) = chat.pending_scroll.take() {
            chat.follow_latest = toward_bottom
                && chat.scroll.max_offset().y + chat.scroll.offset().y <= gpui::px(1.);
        }
        if chat.follow_latest {
            chat.scroll.scroll_to_bottom();
        }
        let scroll = chat.scroll.clone();
        let scroll_key = key.clone();
        let mut history = div()
            .id("side-chat-history")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Lines(delta) => delta.y,
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    };
                    if delta != 0.0 {
                        if let Some(chat) = this.checkout_panel.side_chats.get_mut(&scroll_key) {
                            chat.follow_latest = false;
                            chat.pending_scroll = Some(delta < 0.0);
                        }
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        if chat.messages.is_empty() {
            history = history.child(
                div()
                    .text_color(c::FG_DIM())
                    .child("Ask about this session or worktree. Context may be incomplete."),
            );
        }
        for (role, text) in &chat.messages {
            history = history.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(if role == "user" { "You" } else { "Assistant" }),
                    )
                    .child(div().whitespace_normal().child(text.clone())),
            );
        }
        if chat.busy {
            history = history.child(div().text_color(c::FG_DIM()).child("Responding…"));
        }
        let retry_key = key.clone();
        if let Some(error) = &chat.error {
            history = history
                .child(div().text_color(c::FORM_ERROR()).child(error.clone()))
                .child(
                    transcript_action("side-chat-retry", "Retry", false, window, cx)
                        .disabled(!supported)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.retry_side_chat(retry_key.clone(), window, cx)
                        })),
                );
        }
        let input = chat.input.clone();
        let busy = chat.busy;
        let action_key = key.clone();
        let mut recovery_choices = div().flex().flex_col().gap(rpx(SPACE_SM));
        if recovery_paths.len() > 1 {
            recovery_choices = recovery_choices.child("Other saved conversations:");
            for (index, candidate) in recovery_paths
                .iter()
                .enumerate()
                .filter(|(_, candidate)| *candidate != &path)
            {
                let target = candidate.clone();
                recovery_choices = recovery_choices.child(
                    transcript_action(
                        ("side-chat-recover", index),
                        candidate.clone(),
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout_panel.chosen_root = Some(target.clone());
                        cx.notify();
                    })),
                );
            }
        }
        let wide = Self::logical_window_width(window) >= super::checkout_panel::WIDE_VIEWPORT;
        div().id("side-chat-panel").debug_selector(|| "side-chat-panel".into())
            .flex_shrink_0().min_h_0()
            .when(wide, |panel| panel.w(rpx(SIDE_CHAT_WIDTH)).h_full().pt(rpx(APPBAR_TOTAL_H + SPACE_3XL)))
            .when(!wide, |panel| panel.w_full().h(rpx(SIDE_CHAT_WIDTH)).pt(rpx(SPACE_3XL)))
            .px(rpx(SPACE_3XL)).pb(rpx(SPACE_3XL)).flex().flex_col().gap(rpx(SPACE_2XL))
            .text_size(rpx(TEXT_BODY)).text_color(c::FG())
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Side chat"))
            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(path))
            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(context_disclosure))
            .when(!available, |panel| panel.child(div().text_color(c::FG_DIM()).child("This checkout is unavailable. History remains readable; restore the checkout to ask another question.")))
            .when(!available && recovery_paths.len() > 1, |panel| panel.child(recovery_choices))
            .when(available && !supported, |panel| panel.child(div().text_color(c::FG_DIM()).child("Side chat is unavailable for this provider until read-only access can be enforced.")))
            .child(history)
            .child(project_field_well(false, false).h_auto().flex().flex_col().gap(rpx(SPACE_LG))
                .child(Input::new(&input).aria_label("Ask about this session or worktree").appearance(false).bordered(false).focus_bordered(false).p_0())
                .child(if busy {
                    transcript_action("side-chat-stop", "Stop", false, window, cx)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(cancel) = this.checkout_panel.side_chats.get(&action_key).and_then(|chat| chat.cancel.as_ref()) {
                                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            cx.notify();
                        }))
                } else {
                    transcript_action("side-chat-send", "Send", true, window, cx).disabled(!supported)
                        .on_click(cx.listener(move |this, _, window, cx| this.send_side_chat(action_key.clone(), window, cx)))
                }))
            .into_any_element()
    }
}
