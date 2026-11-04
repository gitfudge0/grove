//! A session-owned, independent Activity composer. Never borrows the chat draft.
use super::*;
use gpui::AnimationExt as _;
use gpui_component::input::{Input, InputEvent};
use grove_core::codex::{Snapshot, Status, SubmissionMode};

type Scope = (u64, SessionId);
const REPLY_MIN_H: f32 = 128.;
const REPLY_ERROR_MIN_H: f32 = 192.;
const REPLY_MOTION_MS: u64 = MOTION_SLOW_MS * 2;
const REPLY_RECEIPT_DURATION: Duration = Duration::from_secs(3);
const REPLY_ACK_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct ActivityReply {
    progress: std::rc::Rc<std::cell::Cell<f32>>,
    generation: u64,
    closing: bool,
    motion_task: Option<gpui::Task<()>>,
    input: Entity<InputState>,
    focus: FocusHandle,
    close_focus: FocusHandle,
    send_focus: FocusHandle,
    pending: Option<u64>,
    error: Option<String>,
    receipt: Option<&'static str>,
    receipt_closing: bool,
    receipt_height: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    mode: SubmissionMode,
    submitted_mode: SubmissionMode,
    turn: Option<String>,
    _subscriptions: Vec<gpui::Subscription>,
    timeout: Option<gpui::Task<()>>,
}

impl Sidebar {
    fn open_activity_reply(&mut self, key: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if key.0 != self.active_workspace || !self.dashboard_owns_session(key.1) {
            return;
        }
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.1)
            .cloned();
        let Some(session) = session else {
            return;
        };
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.dashboard_timelines.replies.entry(key)
        {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(2, 2)
                    .submit_on_enter(true)
                    .markdown_auto_continue(true)
                    .placeholder("Add an instruction…")
            });
            let subscription = cx.subscribe_in(
                &input,
                window,
                move |this, _, event, window, cx| match event {
                    InputEvent::PressEnter {
                        secondary,
                        shift: false,
                    } => this.submit_activity_reply(key, *secondary, window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
            let observer = cx.observe_in(&session, window, move |this, session, window, cx| {
                this.sync_activity_reply(key, &session.read(cx).state.shared(), window, cx);
                cx.notify();
            });
            entry.insert(ActivityReply {
                progress: Default::default(),
                generation: 0,
                closing: false,
                motion_task: None,
                input,
                focus: cx.focus_handle(),
                close_focus: cx.focus_handle(),
                send_focus: cx.focus_handle(),
                pending: None,
                error: None,
                receipt: None,
                receipt_closing: false,
                receipt_height: Default::default(),
                mode: SubmissionMode::Steer,
                submitted_mode: SubmissionMode::Steer,
                turn: None,
                _subscriptions: vec![subscription, observer],
                timeout: None,
            });
        }
        if let Some(old) = self
            .dashboard_timelines
            .reply_open
            .filter(|old| *old != key)
        {
            self.close_activity_reply(old, window, cx);
        }
        let reply = self.dashboard_timelines.replies.get_mut(&key).unwrap();
        reply.generation += 1;
        reply.closing = false;
        reply.motion_task = None;
        self.dashboard_timelines.reply_open = Some(key);
        self.dashboard_timelines.replies[&key]
            .input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }
    fn close_activity_reply(&mut self, key: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if self.dashboard_timelines.reply_open == Some(key) {
            self.dashboard_timelines.reply_open = None;
        }
        if let Some(reply) = self.dashboard_timelines.replies.get_mut(&key) {
            reply.generation += 1;
            reply.closing = !cx.reduce_motion();
            if reply.closing {
                reply.motion_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(REPLY_MOTION_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(reply) = this.dashboard_timelines.replies.get_mut(&key) {
                            reply.closing = false;
                            reply.progress.set(0.);
                        }
                        cx.notify();
                    });
                }));
            } else {
                reply.progress.set(0.);
            }
            reply.focus.focus(window, cx);
        }
        cx.notify();
    }
    fn submit_activity_reply(
        &mut self,
        key: Scope,
        _steer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if key.0 != self.active_workspace
            || !self.dashboard_open
            || !self.dashboard_owns_session(key.1)
        {
            return;
        }
        let Some(reply) = self.dashboard_timelines.replies.get(&key) else {
            return;
        };
        if reply.pending.is_some() || self.dashboard_timelines.reply_open != Some(key) {
            return;
        }
        let text = reply.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        // Use the mode and turn actually shown, rather than silently changing a
        // request that raced a provider update.
        let mode = SubmissionMode::Steer;
        let turn = reply.turn.clone();
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.1)
            .cloned();
        let result = session
            .ok_or_else(|| "This session is no longer available.".to_string())
            .and_then(|session| {
                session.update(cx, |session, cx| {
                    session.activity_prompt(text, mode, turn, cx)
                })
            });
        match result {
            Ok(id) => {
                let timeout = cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(REPLY_ACK_TIMEOUT).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(reply) = this.dashboard_timelines.replies.get_mut(&key) {
                            if reply.pending == Some(id) {
                                reply.error = Some(
                                    "Delivery unconfirmed. Open the session before resending."
                                        .into(),
                                );
                            }
                        }
                        cx.notify();
                    });
                });
                let Some(reply) = self.dashboard_timelines.replies.get_mut(&key) else {
                    return;
                };
                reply.submitted_mode = mode;
                reply.pending = Some(id);
                reply.error = None;
                reply.receipt = None;
                reply.receipt_closing = false;
                reply.timeout = Some(timeout);
            }
            Err(error) => {
                if let Some(reply) = self.dashboard_timelines.replies.get_mut(&key) {
                    reply.error = Some(error);
                }
            }
        }
        cx.notify();
    }
    fn sync_activity_reply(
        &mut self,
        key: Scope,
        state: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(reply) = self.dashboard_timelines.replies.get_mut(&key) else {
            return;
        };
        let Some(id) = reply.pending else {
            return;
        };
        if state.queue_uncertain.contains(&id) || state.status == Status::Disconnected {
            reply.error = Some("Delivery unconfirmed. Open the session before resending.".into());
            // Retain the submission lock: reconnect must never replay a side effect.
        } else if state.accepted_submission == Some(id) {
            reply.pending = None;
            reply.timeout = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(REPLY_RECEIPT_DURATION).await;
                let animate = this
                    .update(cx, |this, cx| {
                        if let Some(reply) = this.dashboard_timelines.replies.get_mut(&key) {
                            if cx.reduce_motion() {
                                reply.receipt = None;
                            } else {
                                reply.receipt_closing = true;
                            }
                        }
                        cx.notify();
                        !cx.reduce_motion()
                    })
                    .unwrap_or(false);
                if !animate {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(REPLY_MOTION_MS))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(reply) = this.dashboard_timelines.replies.get_mut(&key) {
                        reply.receipt = None;
                        reply.receipt_closing = false;
                    }
                    cx.notify();
                });
            }));
            reply.error = None;
            reply.receipt_closing = false;
            reply.receipt = Some("Session is being steered");
            reply
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
            if self.dashboard_timelines.reply_open == Some(key) {
                self.close_activity_reply(key, window, cx);
            }
        } else if state.rejected_submission == Some(id) {
            reply.pending = None;
            reply.timeout = None;
            reply.error = Some(
                state
                    .error
                    .clone()
                    .unwrap_or_else(|| "Reply rejected. Your draft is retained.".into()),
            );
        }
    }
    pub(super) fn activity_reply_button(
        &mut self,
        key: Scope,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self
            .dashboard_timelines
            .reply_focus
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let click_focus = focus.clone();
        super::super::components::header_control_frame(
            ("activity-respond", key.1.raw()),
            "Respond to session",
            c::BG_HOVER(),
        )
        .debug_selector(move || format!("activity-respond-{}", key.1.raw()))
        .track_focus(&focus)
        .tab_index(0)
        .tab_stop(true)
        .focus_visible(|s| s.bg(c::BG_HOVER()))
        .tooltip(|window, cx| {
            super::super::components::tooltip("Respond", window).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            click_focus.focus(window, cx);
            this.open_activity_reply(key, window, cx);
            cx.stop_propagation();
        }))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.open_activity_reply(key, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }),
        )
        .child(icon("reply", ICON_SM, c::FG_DIM()))
        .into_any_element()
    }
    pub(super) fn wrap_activity_reply(
        &mut self,
        key: Scope,
        _title: &str,
        row: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.dashboard_timelines.reply_open == Some(key);
        let height = self
            .dashboard_timelines
            .reply_heights
            .entry(key)
            .or_default()
            .clone();
        let measured_height = height.clone();
        let owner = cx.entity().downgrade();
        let mut frame = div()
            .debug_selector(move || format!("activity-reply-frame-{}", key.1.raw()))
            .w_full()
            .child(row);
        if let Some(reply) = self.dashboard_timelines.replies.get(&key) {
            if let Some(receipt) = reply.receipt {
                let height = reply.receipt_height.clone();
                let measured = height.clone();
                let label = div()
                    .pl(rpx(ICON_LG + SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .child(receipt)
                    .on_prepaint(move |bounds, _, _| measured.set(bounds.size.height));
                let body = div().w_full().overflow_hidden().child(label);
                frame = if reply.receipt_closing && !cx.reduce_motion() {
                    frame.child(
                        body.with_animation(
                            format!("activity-receipt-exit-{}", key.1.raw()),
                            gpui::Animation::new(Duration::from_millis(REPLY_MOTION_MS))
                                .with_easing(motion::ease_standard),
                            move |body, t| body.h(height.get() * (1. - t)).opacity(1. - t),
                        ),
                    )
                } else if reply.receipt_closing {
                    frame
                } else {
                    frame.child(body)
                };
            }
        }
        let closing = self
            .dashboard_timelines
            .replies
            .get(&key)
            .is_some_and(|r| r.closing);
        if !active && !closing {
            return frame.into_any_element();
        }
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.1)
            .cloned();
        let state = session.as_ref().map(|s| s.read(cx).state.shared());
        let Some(reply) = self.dashboard_timelines.replies.get_mut(&key) else {
            return frame.into_any_element();
        };
        let busy = state
            .as_ref()
            .is_some_and(|s| s.status.busy() || s.turn_id.is_some());
        reply.mode = SubmissionMode::Steer;
        reply.turn = state.as_ref().and_then(|s| s.turn_id.clone());
        let disabled = !active
            || !busy
            || reply.pending.is_some()
            || reply.input.read(cx).value().trim().is_empty()
            || state.as_ref().is_none_or(|s| {
                matches!(s.status, Status::Disconnected | Status::Starting)
                    || s.compacting
                    || s.saved.pending_edit.is_some()
            });
        let input = reply.input.clone();
        let input_focus = input.focus_handle(cx);
        let trigger_focus = self.dashboard_timelines.reply_focus[&key].clone();
        reply.focus = trigger_focus;
        let close_focus = reply.close_focus.clone();
        let send_focus = reply.send_focus.clone();
        let error = reply.error.clone().or_else(|| {
            state
                .as_ref()
                .filter(|s| {
                    matches!(s.status, Status::Disconnected | Status::Starting)
                        || s.compacting
                        || s.saved.pending_edit.is_some()
                })
                .map(|_| {
                    "Open the session and finish connection or recovery before replying.".to_owned()
                })
        });
        let pending = reply.pending.is_some();
        let hints = "Enter to steer · Shift+Enter for a new line";
        let progress = reply.progress.clone();
        let from = progress.get();
        let generation = reply.generation;
        let close = super::super::components::header_control_frame(
            ("activity-reply-close", key.1.raw()),
            "Close reply",
            c::BG_HOVER(),
        )
        .track_focus(&close_focus)
        .tab_index(0)
        .tab_stop(true)
        .focus_visible(|s| s.bg(c::BG_HOVER()))
        .on_click(
            cx.listener(move |this, _, window, cx| this.close_activity_reply(key, window, cx)),
        )
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.close_activity_reply(key, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }),
        )
        .child(icon("close", ICON_SM, c::FG_DIM()));
        let send = super::super::codex_view::composer_icon(
            "activity-reply-submit",
            "Steer session",
            if pending {
                "status-working"
            } else {
                "arrow-up"
            },
            disabled,
            true,
        )
        .track_focus(&send_focus)
        .on_click(cx.listener(move |this, _, window, cx| {
            if !disabled {
                this.submit_activity_reply(key, false, window, cx);
            }
        }))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !disabled
                    && !event.is_held
                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                {
                    this.submit_activity_reply(key, false, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }),
        );
        let panel = div()
            .id(("activity-reply-overlay", key.1.raw()))
            .debug_selector(move || format!("activity-reply-overlay-{}", key.1.raw()))
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .p(rpx(SPACE_LG))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(
                cx.listener(move |this, _: &crate::keymap::ToggleZen, window, cx| {
                    let focused = this
                        .dashboard_timelines
                        .replies
                        .get(&key)
                        .is_some_and(|r| r.input.focus_handle(cx).is_focused(window));
                    if focused {
                        let steer = this
                            .dashboard_timelines
                            .replies
                            .get(&key)
                            .is_some_and(|r| r.mode != SubmissionMode::Send);
                        this.submit_activity_reply(key, steer, window, cx);
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_activity_reply(key, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                    if event.keystroke.key == "tab" {
                        let focuses = if pending {
                            vec![close_focus.clone()]
                        } else if disabled {
                            vec![close_focus.clone(), input_focus.clone()]
                        } else {
                            vec![close_focus.clone(), input_focus.clone(), send_focus.clone()]
                        };
                        let index = focuses
                            .iter()
                            .position(|f| f.is_focused(window))
                            .unwrap_or(0);
                        let next = if event.keystroke.modifiers.shift {
                            (index + focuses.len() - 1) % focuses.len()
                        } else {
                            (index + 1) % focuses.len()
                        };
                        focuses[next].focus(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("Steer session"),
                            )
                            .child(close),
                    )
                    .child(super::super::components::attached_validation(
                        ("activity-reply-error", key.1.raw()),
                        super::super::components::composer_surface()
                            .debug_selector(|| "activity-reply-well".into())
                            .py(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex()
                                    .items_end()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&input)
                                                .file_drop_overlay(
                                                    crate::views::components::file_drop_overlay(),
                                                )
                                                .aria_label("Activity reply")
                                                .text_size(rpx(TEXT_BODY))
                                                .line_height(rpx(TEXT_BODY + SPACE_SM))
                                                .appearance(false)
                                                .bordered(false)
                                                .focus_bordered(false)
                                                .disabled(pending || !active)
                                                .text_color(c::FG())
                                                .p_0(),
                                        ),
                                    )
                                    .child(send),
                            ),
                        error.as_deref(),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "activity-reply-hints".into())
                            .text_right()
                            .w_full()
                            .flex()
                            .justify_end()
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(TEXT_MICRO + SPACE_SM))
                            .text_color(c::FG_DIM())
                            .child(hints),
                    ),
            );
        let panel = panel.on_prepaint(move |bounds, _, cx| {
            if (measured_height.get() - bounds.size.height).abs() > gpui::px(0.5) {
                measured_height.set(bounds.size.height);
                let _ = owner.update(cx, |_, cx| cx.notify());
            }
        });
        let target_height = height.get().max(
            rpx(if error.is_some() {
                REPLY_ERROR_MIN_H
            } else {
                REPLY_MIN_H
            })
            .to_pixels(window.rem_size()),
        );
        let body = div().w_full().overflow_hidden().child(panel);
        if cx.reduce_motion() {
            progress.set(if active { 1. } else { 0. });
            frame.child(body).into_any_element()
        } else {
            frame
                .child(
                    body.with_animation(
                        format!("activity-reply-motion-{}-{generation}", key.1.raw()),
                        gpui::Animation::new(Duration::from_millis(REPLY_MOTION_MS))
                            .with_easing(motion::ease_standard),
                        move |body, t| {
                            let value = from + ((if active { 1. } else { 0. }) - from) * t;
                            progress.set(value);
                            body.max_h(target_height * value)
                                .opacity(0.35 + 0.65 * value)
                                .relative()
                                .top(rpx(-SPACE_MD * (1. - value)))
                        },
                    ),
                )
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{codex_session::CodexSession, workspace_state::SnapshotProject};
    use grove_core::codex::{Action as ProviderAction, Handle, Provider, SavedSession};
    use std::sync::mpsc::Receiver;

    struct ReplyProbe {
        _transport: Option<Receiver<ProviderAction>>,
        provider: Provider,
        sidebar: Entity<Sidebar>,
        id: SessionId,
    }
    impl Render for ReplyProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let row = self.sidebar.update(cx, |sidebar, cx| {
                let metadata = super::super::dashboard_timeline::RowMetadata {
                    project: "Grove".into(),
                    worktree: "Main checkout".into(),
                    provider: self.provider,
                    model: "GPT-6.1 Terra".into(),
                    effort: None,
                    context: "Reply fixture".into(),
                    status: Status::Working,
                    display_status: "Working",
                    review_completion: None,
                    age: "1m".into(),
                };
                sidebar.render_dashboard_progress(
                    self.id,
                    "Fix link icon and text alignment",
                    &metadata,
                    window,
                    cx,
                )
            });
            div()
                .size_full()
                .bg(c::BG())
                .font_family(crate::fonts::UI_FAMILY)
                .p(rpx(SPACE_3XL))
                .child(row)
        }
    }
    fn init(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.bind_keys(crate::keymap::shell_bindings());
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    }
    fn fixture(
        window: &mut Window,
        cx: &mut App,
        provider: Provider,
    ) -> (ReplyProbe, Entity<CodexSession>, Receiver<ProviderAction>) {
        let runtime = cx.new(Runtime::new);
        let saved: SavedSession = serde_json::from_value(serde_json::json!({"provider":provider,"key":"activity-reply-fixture","project":"Grove","cwd":"/tmp","label":"Fixture","thread_id":"fixture-thread","roots":[],"items":[],"draft":"main composer draft"})).unwrap();
        let (handle, receive) = Handle::channel();
        let session = cx.new(|cx| {
            let mut session = CodexSession::history(saved, cx);
            session.handle = handle.clone();
            session.state.status = Status::Working;
            session.state.turn_id = Some("fixture-turn".into());
            session
        });
        let registry = runtime.read(cx).registry.clone();
        let id = registry.update(cx, |r, _| {
            let id = r.insert_meta("Grove".into(), "/tmp".into(), provider.agent());
            r.attach_codex(id, session.clone(), handle);
            id
        });
        let sidebar = cx.new(|cx| {
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.active_workspace = 1;
            sidebar.dashboard_open = true;
            sidebar.snapshot.projects.push(SnapshotProject {
                sessions: vec![id],
                ..Default::default()
            });
            sidebar
        });
        (
            ReplyProbe {
                _transport: None,
                provider,
                sidebar,
                id,
            },
            session,
            receive,
        )
    }
    #[gpui::test]
    fn activity_done_requires_cleared_review_and_rejects_new_work(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        for provider in [Provider::Codex, Provider::Claude] {
            let mut session = None;
            let mut transport = None;
            let (probe, cx) = cx.add_window_view(|window, cx| {
                let (probe, saved, receive) = fixture(window, cx, provider);
                session = Some(saved);
                transport = Some(receive);
                probe
            });
            let session = session.unwrap();
            let (sidebar, id) = probe.read_with(cx, |probe, _| (probe.sidebar.clone(), probe.id));
            for status in [Status::Working, Status::Idle] {
                cx.update(|window, cx| {
                    session.update(cx, |session, _| {
                        session.state.status = status;
                        session.state.turn_id = None;
                        if status == Status::Idle {
                            session.state.apply(&serde_json::json!({"method":"turn/started","params":{"threadId":"fixture-thread","turn":{"id":"completion-test"}}}));
                            session.state.apply(&serde_json::json!({"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"completion-test","status":"completed"}}}));
                        }
                    });
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.act(Action::MarkActivityDone(id), window, cx)
                    });
                    assert!(sidebar
                        .read(cx)
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .codex(id)
                        .is_some());
                });
            }
            cx.update(|window, cx| {
                session.update(cx, |session, _| {
                    let receipt = session.state.review_completion().unwrap();
                    session.state.mark_reviewed(receipt).unwrap();
                    assert_eq!(session.state.session_status(), "Ready");
                });
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::MarkActivityDone(id), window, cx)
                });
                assert!(sidebar
                    .read(cx)
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .is_none());
            });
            drop(transport);
        }
    }
    #[gpui::test]
    fn activity_reply_native_keyboard_draft_and_acknowledgment(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        for provider in [Provider::Codex, Provider::Claude] {
            let mut session = None;
            let mut receive = None;
            let (probe, cx) = cx.add_window_view(|window, cx| {
                let (probe, s, r) = fixture(window, cx, provider);
                session = Some(s);
                receive = Some(r);
                probe
            });
            let session = session.unwrap();
            let receive = receive.unwrap();
            let key = cx.update(|_, cx| (1, probe.read(cx).id));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let respond = cx
                .debug_bounds(Box::leak(
                    format!("activity-respond-{}", key.1.raw()).into_boxed_str(),
                ))
                .unwrap();
            cx.simulate_click(respond.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.simulate_keystrokes("space");
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                assert_eq!(
                    sidebar.dashboard_timelines.replies[&key]
                        .input
                        .read(cx)
                        .value()
                        .as_ref(),
                    " "
                );
            });
            cx.update(|window, cx| {
                probe.read(cx).sidebar.read(cx).dashboard_timelines.replies[&key]
                    .input
                    .clone()
                    .update(cx, |input, cx| {
                        input.set_value("Check narrow layout", window, cx)
                    });
            });
            cx.simulate_keystrokes("tab tab tab");
            cx.update(|window, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                assert!(
                    sidebar.dashboard_timelines.replies[&key]
                        .input
                        .focus_handle(cx)
                        .is_focused(window),
                    "Tab cycles within the reply"
                );
            });
            for (width, rem) in [(620., crate::zoom::REM_BASE), (300., 20.)] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(500.)));
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let _ = window.draw(cx);
                });
                let overlay = cx
                    .debug_bounds(Box::leak(
                        format!("activity-reply-overlay-{}", key.1.raw()).into_boxed_str(),
                    ))
                    .unwrap();
                let hints = cx.debug_bounds("activity-reply-hints").unwrap();
                let submit = cx.debug_bounds("activity-reply-submit").unwrap();
                let well = cx.debug_bounds("activity-reply-well").unwrap();
                assert!(
                    hints.top() >= well.bottom(),
                    "hints sit below the input well"
                );
                assert!(hints.right() <= overlay.right());
                assert!(submit.right() <= overlay.right());
                assert!(
                    hints.bottom() <= overlay.bottom(),
                    "hints {hints:?} outside overlay {overlay:?} at width {width}, rem {rem}"
                );
            }
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            cx.update(|window, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                assert!(sidebar.dashboard_timelines.reply_open.is_none());
                assert!(sidebar.dashboard_timelines.replies[&key]
                    .focus
                    .is_focused(window));
            });
            cx.update(|window, cx| {
                probe.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
                    sidebar.open_activity_reply(key, window, cx)
                })
            });
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            let action = receive.try_recv().unwrap();
            let ProviderAction::ActivityPrompt {
                id,
                text,
                mode: SubmissionMode::Steer,
                ..
            } = action
            else {
                panic!("expected independent queue")
            };
            assert_eq!(text, "Check narrow layout");
            cx.simulate_keystrokes("enter");
            assert!(receive.try_recv().is_err(), "pending rejects duplicates");
            cx.update(|_, cx| {
                session.update(cx, |session, cx| {
                    session.state.accepted_submission = Some(id);
                    cx.notify();
                })
            });
            cx.run_until_parked();
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                assert!(sidebar.dashboard_timelines.reply_open.is_none());
                assert!(sidebar.dashboard_timelines.replies[&key]
                    .input
                    .read(cx)
                    .value()
                    .is_empty());
                assert_eq!(session.read(cx).state.saved.draft, "main composer draft");
                assert_eq!(
                    sidebar.dashboard_timelines.replies[&key].receipt,
                    Some("Session is being steered")
                );
            });
            cx.update(|_, cx| cx.set_reduce_motion(provider == Provider::Claude));
            cx.executor().advance_clock(Duration::from_millis(2999));
            cx.run_until_parked();
            cx.update(|_, cx| {
                assert!(
                    probe.read(cx).sidebar.read(cx).dashboard_timelines.replies[&key]
                        .receipt
                        .is_some()
                );
            });
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                let reply = &sidebar.dashboard_timelines.replies[&key];
                if provider == Provider::Claude {
                    assert!(
                        reply.receipt.is_none(),
                        "reduced motion removes immediately"
                    );
                    assert!(!reply.receipt_closing);
                } else {
                    assert!(reply.receipt.is_some(), "receipt survives through collapse");
                    assert!(reply.receipt_closing);
                }
            });
            cx.executor()
                .advance_clock(Duration::from_millis(REPLY_MOTION_MS));
            cx.run_until_parked();
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                assert!(sidebar.dashboard_timelines.replies[&key].receipt.is_none());
                assert!(!sidebar.dashboard_timelines.replies[&key].receipt_closing);
            });
            cx.update(|_, cx| cx.set_reduce_motion(true));
            cx.update(|window, cx| {
                probe.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
                    sidebar.open_activity_reply(key, window, cx)
                })
            });
            cx.update(|window, cx| {
                probe.read(cx).sidebar.read(cx).dashboard_timelines.replies[&key]
                    .input
                    .clone()
                    .update(cx, |input, cx| {
                        input.set_value("Steer narrow layout", window, cx)
                    })
            });
            cx.simulate_keystrokes("secondary-enter");
            cx.run_until_parked();
            let ProviderAction::ActivityPrompt {
                id,
                mode: SubmissionMode::Steer,
                expected_turn: Some(_),
                ..
            } = receive.try_recv().unwrap()
            else {
                panic!("expected turn-bound steer")
            };
            cx.update(|_, cx| {
                session.update(cx, |s, cx| {
                    s.state.rejected_submission = Some(id);
                    s.state.error = Some("Turn changed".into());
                    cx.notify();
                })
            });
            cx.run_until_parked();
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                let reply = &sidebar.dashboard_timelines.replies[&key];
                assert!(reply.pending.is_none());
                assert_eq!(reply.input.read(cx).value().as_ref(), "Steer narrow layout");
                assert!(reply.error.is_some());
            });
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            let ProviderAction::ActivityPrompt { id, .. } = receive.try_recv().unwrap() else {
                panic!("expected queue retry")
            };
            cx.update(|_, cx| {
                session.update(cx, |s, cx| {
                    s.state.queue_uncertain.push(id);
                    s.state.status = Status::Disconnected;
                    cx.notify();
                })
            });
            cx.run_until_parked();
            cx.update(|_, cx| {
                let sidebar = probe.read(cx).sidebar.read(cx);
                let reply = &sidebar.dashboard_timelines.replies[&key];
                assert_eq!(reply.pending, Some(id));
                assert!(reply.error.as_ref().unwrap().contains("unconfirmed"));
            });
            cx.simulate_keystrokes("enter");
            assert!(
                receive.try_recv().is_err(),
                "uncertain delivery is never replayed"
            );
        }
    }
    /// Isolated production rendering for desktop QA; no actor or installation.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens a disposable native QA window; stop the test after desktop inspection"]
    fn activity_reply_desktop_preview() {
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(|cx| {
                init(cx);
                crate::fonts::register(cx).unwrap();
                let theme =
                    std::env::var("GROVE_REPLY_QA_THEME").unwrap_or_else(|_| "grove-dark".into());
                grove_core::theme::set(grove_core::theme::by_name(&theme).unwrap());
                c::sync_component_theme(cx);
                let width: f32 = std::env::var("GROVE_REPLY_QA_WIDTH")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(760.);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(80.), gpui::px(80.)),
                            gpui::size(gpui::px(width), gpui::px(300.)),
                        ))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: Some("Grove Activity Reply QA".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        let rem: f32 = std::env::var("GROVE_REPLY_QA_REM")
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(crate::zoom::REM_BASE);
                        window.set_rem_size(gpui::px(rem));
                        let (mut probe, _, receiver) = fixture(window, cx, Provider::Codex);
                        // Keep a deterministic transport alive without starting any CLI.
                        probe._transport = Some(receiver);
                        let sidebar = probe.sidebar.clone();
                        let key = (1, probe.id);
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.open_activity_reply(key, window, cx);
                            sidebar.dashboard_timelines.replies[&key].input.update(
                                cx,
                                |input, cx| {
                                    input.set_value(
                                        "Also check the wrapped label at a narrow window width.",
                                        window,
                                        cx,
                                    )
                                },
                            );
                        });
                        let view = cx.new(|_| probe);
                        cx.new(|cx| gpui_component::Root::new(view, window, cx))
                    },
                )
                .unwrap();
                cx.activate(true);
            });
    }
}
