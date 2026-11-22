//! Conversation focus and keyboard routing, empty state, and turn status.

use super::*;

impl CodexView {
    pub(super) fn render_conversation(
        &self,
        state: &codex::Snapshot,
        body: InteractiveTranscript,
        composer: &ComposerFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let InteractiveTranscript {
            transcript,
            minimap,
            placement,
        } = body;
        let ComposerPlacement {
            content_inset,
            has_minimap,
            focused_launch,
        } = placement;
        let writing_focused = self.input.focus_handle(cx).is_focused(window)
            && !self.composer_collapsed
            && !composer.input_locked;
        let mut surface = div()
            .id("codex-conversation")
            .track_focus(&self.conversation_focus)
            .relative()
            .w_full()
            .when(self.dashboard_tray.is_none(), gpui::Styled::h_full)
            .min_h_0()
            .flex()
            .flex_col()
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .child(super::super::components::composer_focus_glow(
                composer.glow_bounds.clone(),
                writing_focused,
                !self.composer_collapsed && !composer.input_locked,
                ("composer-focus-pulse", self.input.entity_id()),
                window,
                cx,
            ))
            .when(!focused_launch, |surface| {
                surface
                    // Anchor navigation to the pane, independent of the transcript's
                    // changing height above tasks, attachments, and the composer.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(rpx(CHROME_CONTROL_H))
                            .flex()
                            .child(minimap),
                    )
                    .child(transcript)
            })
            .when(
                focused_launch && self.dashboard_tray.is_none() && !self.transcript_only(),
                |surface| {
                    let project = grove_core::session_meta::multi_project_identity(
                        &state.saved.project,
                        &state.saved.roots,
                    )
                    .unwrap_or_else(|| state.saved.project.clone());
                    let worktree = self.checkout_context.as_ref().map_or_else(
                        || {
                            std::path::Path::new(&state.saved.cwd)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        },
                        |(name, _)| name.clone(),
                    );
                    let identity = [project, worktree]
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    surface.child(
                        div()
                            .id("codex-empty-intro")
                            .debug_selector(|| "codex-empty-intro".into())
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .px(rpx(content_inset))
                            .pt(rpx(SPACE_3XL))
                            .gap(rpx(SPACE_LG))
                            .text_center()
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .child("Ship fast. Break faster."),
                            )
                            .child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(identity),
                            ),
                    )
                },
            )
            .when(
                focused_launch && self.dashboard_tray.is_none(),
                gpui::StatefulInteractiveElement::overflow_y_scroll,
            )
            // Cmd+Enter resolves to the shell's global Zen action before Input's
            // secondary-enter binding. Own it only while the main draft has focus.
            .on_action(
                cx.listener(|this, _: &crate::keymap::ToggleZen, window, cx| {
                    if this.input.focus_handle(cx).is_focused(window) {
                        this.submit_modified_enter(cfg!(target_os = "macos"), window, cx);
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.input.focus_handle(cx).is_focused(window)
                    && (this.accept_first_suggestion(window, cx)
                        || this.accept_next_reply(window, cx))
                {
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveLeft, window, cx| {
                if !this.input.focus_handle(cx).is_focused(window)
                    || !this.move_next_reply(true, window, cx)
                {
                    cx.propagate();
                } else {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveRight, window, cx| {
                if !this.input.focus_handle(cx).is_focused(window)
                    || !this.move_next_reply(false, window, cx)
                {
                    cx.propagate();
                } else {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.move_suggestion(false, window, cx)
                    && !this.move_queue_selection(false, window, cx)
                {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.move_suggestion(true, window, cx)
                    && !this.move_queue_selection(true, window, cx)
                {
                    cx.propagate();
                }
            }))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape"
                    || event.keystroke.modifiers.modified()
                    || this.menu.is_some()
                    || this.model_from_card
                {
                    this.clear_stop_confirmation(cx);
                }
                if event.keystroke.key == "escape" && this.model_from_card {
                    this.model_from_card = false;
                    this.open_menu(ComposerMenu::Settings, window, cx);
                    this.card_model_focus.focus(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && this.menu.is_some() {
                    let index = if this.menu == Some(ComposerMenu::Add) {
                        3
                    } else if this.menu == Some(ComposerMenu::Access) {
                        2
                    } else {
                        0
                    };
                    this.close_menu(cx);
                    this.option_focus[index].focus(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.menu == Some(ComposerMenu::Settings) {
                    this.close_menu(cx);
                    this.option_focus[0].focus(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.plan_answer.is_some() {
                    if this
                        .plan_answer
                        .as_ref()
                        .is_some_and(|a| a.saving.is_none())
                    {
                        this.cancel_plan_answer(window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.queue_editor.is_some() {
                    this.cancel_queue_edit(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.queue_selected.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.popup.is_none()
                    && trailing_trigger(&this.input.read(cx).value()).is_some()
                    && this.dismissed_suggestions.as_deref()
                        != Some(this.input.read(cx).value().as_ref())
                {
                    this.menu = None;
                    this.clear_stop_confirmation(cx);
                    this.dismissed_suggestions = Some(this.input.read(cx).value().to_string());
                    this.input.focus_handle(cx).focus(window, cx);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.next_reply_text(window, cx).is_some() {
                    this.dismiss_next_reply(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.stop_with_escape(event, cx) {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }));
        if let Some(activity) = live_activity(state)
            .filter(|_| state.approvals.is_empty() && state.elicitations.is_empty())
        {
            let running = compacting(state)
                || (state.status.busy()
                    && state.questions.is_empty()
                    && state.elicitations.is_empty()
                    && state.approvals.is_empty());
            surface = surface.child(
                div()
                    .id("codex-live-activity")
                    .role(gpui::Role::Status)
                    .debug_selector(|| "codex-live-activity".into())
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .when(has_minimap, |row| row.pl(rpx(CHROME_CONTROL_H)))
                    .py(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .px(rpx(content_inset))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(rpx(CHAT_MAX_W))
                                    .truncate()
                                    .id("codex-live-activity-text")
                                    .debug_selector(|| "codex-live-activity-text".into())
                                    .child(shimmer_label(
                                        "codex-activity-shimmer".into(),
                                        activity,
                                        running,
                                        cx,
                                    )),
                            ),
                    ),
            );
        }
        let busy = state.status.busy();
        let stopping = busy && self.interrupt_requested_for_turn.is_some();
        // Keep immediate stop feedback above every attached panel, alongside live activity.
        if self.stop_confirmation.is_some() || stopping {
            surface = surface.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .when(has_minimap, |row| row.pl(rpx(CHROME_CONTROL_H)))
                    .px(rpx(content_inset))
                    .child(
                        div()
                            .id("codex-stop-confirmation")
                            .debug_selector(|| "codex-stop-confirmation".into())
                            .role(gpui::Role::Status)
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .py(rpx(SPACE_MD))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG())
                            .child(if stopping {
                                super::super::status::mark(
                                    "escape-stopping",
                                    StatusKind::Working,
                                    ICON_SM,
                                    cx,
                                )
                            } else {
                                crate::icons::icon("stop", ICON_SM, c::FG_DIM()).into_any_element()
                            })
                            .child(div().flex_1().min_w_0().child(if stopping {
                                "Stopping agent…"
                            } else {
                                "Press Esc again to stop the agent"
                            })),
                    ),
            );
        }
        surface
    }
}
