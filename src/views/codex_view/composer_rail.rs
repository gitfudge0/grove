//! Composer panels, draft editor, attachments, submission actions, and layout.

use super::{
    action_key, active_turn_redirect_available, attachment_preview, c, codex,
    collapsed_draft_preview, composer_icon, composer_placeholder, composer_tray,
    composer_tray_inset, context_meter, form_action, rpx, transcript_action, Action, Cell,
    CodexView, ComposerMenu, ComposerPlacement, Disableable, Input, Rc, StatusKind, TaskList,
    APPROVAL_TRAY_VIEWPORT_SHARE, CHAT_MAX_W, COMPOSER_MIN_H, DASHBOARD_COMPOSER_MIN_H,
    NEXT_REPLY_POSITION_W, QUESTION_PANEL_MAX_H, STEER_DESCRIPTION,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, ICON_MD, ICON_SM, MOTION_BASE_MS, OPACITY_DISABLED, RADIUS_GROUP,
    RADIUS_PANEL, SPACE_2XL, SPACE_3XL, SPACE_LG, SPACE_MD, SPACE_SM, TEXT_MICRO, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AnimationExt as _;
use gpui::{div, Context, Window};

const SINGLE_COMMAND_APPROVAL_VIEWPORT_SHARE: f32 = 0.60;

pub(super) struct ComposerFrame {
    pub(super) placement: ComposerPlacement,
    pub(super) input_locked: bool,
    pub(super) glow_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
}

impl CodexView {
    pub(super) fn composer_input_locked(&self, state: &codex::Snapshot) -> bool {
        self.dashboard_pending
            || self.submitted.is_some()
            || self
                .plan_answer
                .as_ref()
                .is_some_and(|answer| answer.saving.is_some())
            || self.edit_request_pending
            || self.edit_confirmation.is_some()
            || state.saved.pending_edit.is_some()
            || self
                .queue_editor
                .as_ref()
                .is_some_and(|editor| !editor.loaded || editor.saving)
    }

    pub(super) fn render_composer_rail(
        &mut self,
        state: &codex::Snapshot,
        task_list: Option<&TaskList>,
        frame: &ComposerFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let ComposerPlacement {
            content_inset,
            has_minimap,
            focused_launch,
        } = frame.placement;
        let composer_disabled = frame.input_locked;
        let measured_glow_bounds = frame.glow_bounds.clone();
        let provider = state.saved.provider.label();
        let busy = state.status.busy();
        let stopping = busy && self.interrupt_requested_for_turn.is_some();
        self.sync_status_cards(state);
        let session_notice = self.session_notice(false, state, window, cx);
        let (notice_above, notice_below) = if self.dashboard_tray.is_some() {
            (None, session_notice)
        } else {
            (session_notice, None)
        };
        let controls_attached = notice_above.is_some();
        let status_attached =
            controls_attached || !state.approvals.is_empty() || !state.elicitations.is_empty();
        let session_review = if state.plan_ready() {
            None
        } else {
            self.session_review(status_attached, state, window, cx)
        };
        let has_review = session_review.is_some();
        let tasks_attached = status_attached || has_review;
        let lower_attached = tasks_attached || task_list.is_some();
        let command_approvals = !state.approvals.is_empty()
            && state
                .approvals
                .iter()
                .all(|request| grove_core::controls::approval_command(request).is_some());
        let controls_viewport_share =
            if command_approvals && state.approvals.len() == 1 && state.elicitations.is_empty() {
                SINGLE_COMMAND_APPROVAL_VIEWPORT_SHARE
            } else {
                APPROVAL_TRAY_VIEWPORT_SHARE
            };
        let agent_controls = if !state.approvals.is_empty() || !state.elicitations.is_empty() {
            Some(
                div()
                    .id("agent-controls")
                    .debug_selector(|| "agent-controls".into())
                    .min_w_0()
                    .min_h_0()
                    .max_h(
                        rpx(QUESTION_PANEL_MAX_H)
                            .to_pixels(window.rem_size())
                            .min(window.viewport_size().height * controls_viewport_share),
                    )
                    .overflow_y_scroll()
                    .track_scroll(&self.agent_controls_scroll)
                    .anchor_scroll(Some(self.agent_controls_anchor.clone()))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .mx(rpx(composer_tray_inset(window)))
                    .when(command_approvals, |tray| {
                        tray.child(self.approval_panel(state, window, cx))
                    })
                    .when(!state.approvals.is_empty() && !command_approvals, |tray| {
                        tray.child(
                            composer_tray(window)
                                .rounded_none()
                                .mx_0()
                                .mb_0()
                                .pb(rpx(SPACE_LG))
                                .when(!controls_attached, |panel| {
                                    panel.rounded_t(rpx(RADIUS_PANEL))
                                })
                                .child(self.status_card_header(
                                    "approval-status-disclosure",
                                    "Approval needed",
                                    StatusKind::NeedsInput,
                                    cx,
                                ))
                                .when(
                                    !self.expanded.contains("approval-status-disclosure"),
                                    |panel| panel.child(self.approval_panel(state, window, cx)),
                                ),
                        )
                    })
                    .when(!state.elicitations.is_empty(), |tray| {
                        tray.child(
                            composer_tray(window)
                                .rounded_none()
                                .mx_0()
                                .mb_0()
                                .pb(rpx(SPACE_LG))
                                .rounded_none()
                                .when(!controls_attached && state.approvals.is_empty(), |panel| {
                                    panel.rounded_t(rpx(RADIUS_PANEL))
                                })
                                .child(self.status_card_header(
                                    "elicitation-status-disclosure",
                                    "Input needed",
                                    StatusKind::NeedsInput,
                                    cx,
                                ))
                                .when(
                                    !self.expanded.contains("elicitation-status-disclosure"),
                                    |panel| panel.child(self.elicitation_panel(state, window, cx)),
                                ),
                        )
                    }),
            )
        } else {
            None
        };
        self.previews.retain(|url, _| self.images.contains(url));
        let queued = self.queue_tray(
            lower_attached
                || !state.saved.rewind_drafts.is_empty()
                || (!self.composer_collapsed && self.suggestion_choices(state, cx).is_some()),
            state,
            window,
            cx,
        );
        let task_strip = task_list.map(|list| {
            let jumps = list.tasks.iter().map(|task| task.started_at).collect();
            self.task_strip(tasks_attached, list, state.status, jumps, window, cx)
        });
        let mut attachments = div()
            .id("codex-attachments")
            .debug_selector(|| "codex-attachments".into())
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_LG));
        for (index, url) in self.images.iter().enumerate() {
            if let Some(image) = self.previews.get(url).cloned() {
                attachments = attachments.child(
                    attachment_preview(image)
                        .id(("codex-image", index))
                        .debug_selector(move || format!("codex-image-{index}"))
                        .group("codex-image")
                        .child(
                            (if self.dashboard_pending {
                                super::super::components::disabled_header_control(
                                    "codex-remove-image",
                                    "Remove attached image",
                                )
                            } else {
                                super::super::components::header_control(
                                    "codex-remove-image",
                                    "Remove attached image",
                                )
                            })
                            .tab_index(if self.dashboard_pending { -1 } else { 0 })
                            .tab_stop(!self.dashboard_pending)
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        && this.submitted.is_none()
                                        && !event.is_held
                                    {
                                        this.remove_image(index, window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .absolute()
                            .top_0()
                            .right_0()
                            .size(rpx(CHROME_CONTROL_H))
                            .p_0()
                            .bg(c::SURFACE_RAISED())
                            .opacity(if self.dashboard_pending {
                                OPACITY_DISABLED
                            } else {
                                0.0
                            })
                            .when(!self.dashboard_pending, |control| {
                                control
                                    .group_hover("codex-image", |style| style.opacity(1.0))
                                    .focus_visible(|style| style.opacity(1.0))
                            })
                            .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.submitted.is_some() {
                                        return;
                                    }
                                    this.remove_image(index, window, cx);
                                },
                            )),
                        ),
                );
            }
        }
        if !state.status.busy()
            || self.interrupt_requested_for_turn != state.turn_id
            || state.error.is_some()
        {
            self.interrupt_requested_for_turn = None;
        }
        let native_invocation =
            grove_core::describe_skill::is_invocation(self.input.read(cx).value().as_ref())
                || grove_core::review_skill::is_invocation(self.input.read(cx).value().as_ref());
        let dashboard_catalog_blocked = self.dashboard_tray.is_some() && !native_invocation && {
            let session = self.session.read(cx);
            session.catalog_loading || session.catalog_error.is_some()
        };
        let draft_disabled = dashboard_catalog_blocked
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || state.saved.pending_edit.is_some()
            || state.status == codex::Status::Disconnected
            || (state.status == codex::Status::Starting
                && state.saved.provider != codex::Provider::Claude)
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
            || stopping
            || (self.input.read(cx).value().trim().is_empty()
                && self.images.is_empty()
                && self.reference.is_none());
        let steer_available = active_turn_redirect_available(state);
        let editing = self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded);
        let primary_disabled = if let Some(answer) = &self.plan_answer {
            answer.saving.is_some() || self.input.read(cx).value().trim().is_empty()
        } else if editing {
            self.queue_editor
                .as_ref()
                .is_some_and(|editor| editor.saving)
                || (self.input.read(cx).value().trim().is_empty()
                    && self.images.is_empty()
                    && self
                        .queue_editor
                        .as_ref()
                        .is_none_or(|editor| editor.reference.is_none()))
        } else {
            draft_disabled
        };
        let primary_label = if self.plan_answer.is_some() {
            "Save answer · Enter · Does not start implementation"
        } else if editing {
            "Save queued message · Enter · Shift+Enter for a new line"
        } else if busy {
            "Queue · Enter · Send after the current turn finishes · Shift+Enter for a new line"
        } else {
            "Send · Enter · Start a new turn · Shift+Enter for a new line"
        };
        // An absent menu must not leave an empty flex child and an extra rail gap.
        let rewind_drafts = (!state.saved.rewind_drafts.is_empty()).then(|| {
            div()
                .id("codex-saved-rewind-drafts")
                .debug_selector(|| "codex-saved-rewind-drafts".into())
                .w_full()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_MD))
                .children(state.saved.rewind_drafts.iter().map(|draft| {
                    let id = draft.id.clone();
                    let disabled = self.edit_request_pending
                        || self.pending_image_pastes > 0
                        || self.pending_attachment_picker
                        || state.saved.pending_edit.is_some()
                        || state.status.busy()
                        || self.queue_editor.is_some()
                        || self.submitted.is_some();
                    div()
                        .id(gpui::SharedString::from(format!(
                            "saved-rewind-{}",
                            draft.id
                        )))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .p(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_GROUP))
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .bg(c::BG_RAIL())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_size(rpx(TEXT_SMALL)).child("Saved draft"))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .truncate()
                                        .child(if draft.text.is_empty() {
                                            "Attachments or referenced selection".into()
                                        } else {
                                            draft.text.clone()
                                        }),
                                ),
                        )
                        .child(
                            form_action(
                                format!("restore-rewind-{}", draft.id),
                                "Restore draft",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/restore.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .disabled(disabled)
                            .debug_selector(|| "codex-restore-rewind-draft".into())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if disabled
                                        || this.edit_request_pending
                                        || this.pending_image_pastes > 0
                                        || this.pending_attachment_picker
                                    {
                                        return;
                                    }
                                    this.dismiss_next_reply(cx);
                                    this.sync_composer_before_rewind(cx);
                                    this.edit_request_pending = true;
                                    this.rewind_attempt =
                                        Some(this.session.read(cx).state.rewind_attempts);
                                    this.session.update(cx, |session, cx| {
                                        session.state.clear_error();
                                        session.send(
                                            Action::RestoreRewindDraft { id: id.clone() },
                                            cx,
                                        );
                                    });
                                },
                            )),
                        )
                }))
        });
        let choices = (!self.composer_collapsed && self.suggestion_choices(state, cx).is_some())
            .then(|| {
                self.composer_choices(
                    lower_attached || !state.saved.rewind_drafts.is_empty(),
                    state,
                    window,
                    cx,
                )
            });
        let collapsed = self.composer_collapsed;
        let composer_min_h = if self.dashboard_tray.is_some() {
            DASHBOARD_COMPOSER_MIN_H
        } else {
            COMPOSER_MIN_H
        };
        let target = self.composer_body_target.clone();
        let expanded_height = self.composer_expanded_height.clone();
        if self.composer_transition_from.is_some() {
            target.set(if collapsed {
                rpx(CHROME_CONTROL_H).to_pixels(window.rem_size())
            } else {
                expanded_height
                    .get()
                    .max(rpx(composer_min_h).to_pixels(window.rem_size()))
            });
        }
        let owner = cx.entity().downgrade();
        let body_height = self.composer_body_height.clone();
        let height_owner = cx.entity().downgrade();
        let body = div()
            .relative()
            .w_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let changed = body_height.replace(bounds.size.height) != bounds.size.height;
                        let _ = height_owner.update(cx, |this, cx| {
                            if changed && this.composer_pin_bottom {
                                this.smooth_follow = true;
                                cx.notify();
                            }
                            if this.composer_motion_finished.get() {
                                this.composer_pin_bottom = false;
                            }
                        });
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let changed =
                                    target.replace(bounds.size.height) != bounds.size.height;
                                if !collapsed {
                                    expanded_height.set(bounds.size.height);
                                }
                                if changed {
                                    let _ = owner.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(!collapsed && !self.images.is_empty(), |body| {
                        body.child(attachments.mb(rpx(SPACE_LG)))
                    })
                    .child(
                        div()
                            .id("codex-composer-input")
                            .debug_selector(|| "codex-composer-input".into())
                            .relative()
                            .min_h(rpx(if self.composer_collapsed {
                                CHROME_CONTROL_H
                            } else {
                                composer_min_h
                            }))
                            .capture_any_mouse_down(cx.listener(
                                |this, _: &gpui::MouseDownEvent, window, cx| {
                                    crate::platform::inline_html::NativeDocument::focus_host(
                                        window,
                                    );
                                    this.set_composer_collapsed(false, cx);
                                },
                            ))
                            .child(
                                Input::new(&self.input)
                                    .min_h(rpx(if self.composer_collapsed {
                                        CHROME_CONTROL_H
                                    } else {
                                        composer_min_h
                                    }))
                                    .when(!self.pending_attachment_picker, |input| {
                                        input.file_drop_overlay(
                                            crate::views::components::file_drop_overlay(),
                                        )
                                    })
                                    .disabled(composer_disabled)
                                    .aria_label(self.plan_answer.as_ref().map_or_else(
                                        || format!("Message {provider}"),
                                        |answer| format!("Your answer: {}", answer.prompt),
                                    ))
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .text_color(c::FG())
                                    .p_0(),
                            )
                            .when(self.next_reply_text(window, cx).is_some(), |field| {
                                let (position, count) =
                                    self.next_reply.position().unwrap_or((0, 0));
                                field.child(
                                    div()
                                        .id("next-reply-hint")
                                        .debug_selector(|| "next-reply-hint".into())
                                        .flex()
                                        .items_center()
                                        .flex_wrap()
                                        .gap(rpx(SPACE_LG))
                                        .text_size(rpx(TEXT_MICRO))
                                        .text_color(c::FG_DIM())
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(rpx(SPACE_SM))
                                                .child(self.next_reply_navigation(true, cx))
                                                .child(
                                                    div()
                                                        .id("next-reply-position")
                                                        .role(gpui::Role::Status)
                                                        .w(rpx(NEXT_REPLY_POSITION_W))
                                                        .text_center()
                                                        .child(format!("{position} / {count}")),
                                                )
                                                .child(self.next_reply_navigation(false, cx)),
                                        )
                                        .child("Tab to use · ← / → to switch · Esc to dismiss"),
                                )
                            })
                            .when(self.composer_collapsed, |field| {
                                field.child(
                                    div()
                                        .id("codex-composer-preview")
                                        .debug_selector(|| "codex-composer-preview".into())
                                        .role(gpui::Role::Button)
                                        .cursor_pointer()
                                        .aria_label("Expand message input")
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .bg(c::BG_RAIL())
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(
                                                    if self.input.read(cx).value().is_empty() {
                                                        c::FG_DIM()
                                                    } else {
                                                        c::FG()
                                                    },
                                                )
                                                .child(if self.input.read(cx).value().is_empty() {
                                                    composer_placeholder(state.saved.provider)
                                                        .to_owned()
                                                } else {
                                                    collapsed_draft_preview(
                                                        &self.input.read(cx).value(),
                                                    )
                                                }),
                                        ),
                                )
                            }),
                    ),
            );
        let body = if let Some(from) = self
            .composer_transition_from
            .filter(|_| !cx.reduce_motion())
        {
            let target = self.composer_body_target.clone();
            let finished = self.composer_motion_finished.clone();
            body.with_animation(
                ("codex-composer-height", self.composer_transition_generation),
                gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .with_easing(super::super::motion::ease_standard),
                move |body, progress| {
                    finished.set(progress >= 1.0);
                    body.h(from + (target.get() - from) * progress)
                },
            )
            .into_any_element()
        } else {
            body.into_any_element()
        };
        let composer_attached = lower_attached
            || rewind_drafts.is_some()
            || choices.is_some()
            || !self.queue_motion_rows.is_empty();
        let mut options = self.option_triggers(state, window, cx).into_iter();
        let access_control = options.next();
        let model_control = options.next();
        let add_bounds = self.option_bounds[3].clone();
        let add_disabled = self.dashboard_pending
            || self.plan_answer.is_some()
            || self.submitted.is_some()
            || self.pending_attachment_picker
            || self.edit_request_pending
            || state.saved.pending_edit.is_some();
        let dashboard_status = self.render_dashboard_status(state, window, cx);
        div()
                    .id("codex-composer-rail")
                    .debug_selector(|| "codex-composer-rail".into())
                    // Short panes scroll attached controls instead of clipping them.
                    .min_h_0()
                    .max_h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.composer_rail_scroll)
                    .w_full()
                    .when(focused_launch && self.dashboard_tray.is_none(), |rail| rail.pt(rpx(SPACE_3XL)))
                    .px(rpx(content_inset))
                    // Match the transcript's reserved navigation gutter so both
                    // centered columns retain the same edges at every pane width.
                    .when(has_minimap, |rail| {
                        rail.pl(rpx(content_inset + CHROME_CONTROL_H))
                    })
                    .pb(rpx(SPACE_3XL))
                    .flex()
                    .items_start()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_MD))
                            .when(has_review, gpui::Styled::gap_0)

                            .when_some(notice_above, gpui::ParentElement::child)
                            .when_some(agent_controls, gpui::ParentElement::child)
                            .when_some(session_review, gpui::ParentElement::child)
                            .when_some(task_strip, gpui::ParentElement::child)
                            .when_some(rewind_drafts, gpui::ParentElement::child)
                            .when_some(choices, gpui::ParentElement::child)
                            .when(!self.queue_motion_rows.is_empty(), |rail| rail.child(queued))
                            .when_some(dashboard_status, gpui::ParentElement::child)
                            .child(div().w_full().flex().flex_col().flex_shrink_0()
                            .children(self.render_reference_preview(composer_attached, window, cx))
                            .child(
                        super::super::components::composer_surface()
                            .relative()
                            .child(gpui::canvas(move |bounds, _, _| measured_glow_bounds.set(bounds), |_, (), _, _| {}).absolute().size_full())
                            .id("codex-composer")
                            .debug_selector(|| "codex-composer".into())
                            .max_w(rpx(CHAT_MAX_W))
                            .children(self.render_plan_answer_context(window, cx))
                            .child(body)
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .when_some(model_control, gpui::ParentElement::child)
                                    .when_some(access_control, gpui::ParentElement::child)
                                    .child(div().flex_1())
                                    .child(
                                        div().flex().items_center().gap(rpx(SPACE_LG)).flex_shrink_0().ml_auto()
                                    .when_some(state.saved.context.filter(|usage| usage.window > 0), |row, usage| row.child(context_meter(usage)))
                                    .child(
                                        composer_icon("codex-add", "Add context", "paperclip", add_disabled, false)
                                            .track_focus(&self.option_focus[3])
                                            .relative()
                                            .child(gpui::canvas(move |rect, _, _| add_bounds.set(rect), |_, (), _, _| {}).absolute().size_full())
                                            .when(self.menu == Some(ComposerMenu::Add), |button| button.text_color(c::FG()))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if !add_disabled { if this.menu == Some(ComposerMenu::Add) { this.close_menu(cx); } else { this.open_menu(ComposerMenu::Add, window, cx); } }
                                            }))
                                            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                if !add_disabled && action_key(event) { this.open_menu(ComposerMenu::Add, window, cx); cx.stop_propagation(); }
                                            }))
                                    )

                                            .when(self.plan_answer.is_some(), |row| row.child(
                                                transcript_action("save-plan-answer", "Save answer", true, window, cx)
                                                    .debug_selector(|| "save-plan-answer".into())
                                                    .icon(gpui_component::Icon::default().path("icons/check.svg").size(rpx(ICON_SM)))
                                                    .disabled(primary_disabled)
                                                    .on_click(cx.listener(|this, _, window, cx| this.save_plan_answer(window, cx)))
                                            ))
                                            .when(busy && self.plan_answer.is_none(), |row| row.child(
                                                composer_icon("codex-stop", if stopping { "Stopping the current turn" } else { "Stop · Interrupt the current turn" }, "stop", stopping || state.turn_id.is_none(), true)
                                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                                        if action_key(event) { this.interrupt(cx); cx.stop_propagation(); }
                                                    }))
                                                    .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
                                            ))
                                            .when(!busy && self.plan_answer.is_none(), |row| row.child(
                                                composer_icon("codex-send", primary_label, "arrow-up", primary_disabled, true)
                                                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                        if action_key(event) {
                                                            if !primary_disabled { this.submit(window, cx); }
                                                            cx.stop_propagation();
                                                        }
                                                    }))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        if !primary_disabled { this.submit(window, cx); }
                                                    }))
                                            ))
                                    ),
                            )
                            )
                            .children(self.dashboard_tray.clone()))
                            .child(
                                div()
                                    .id("codex-composer-hints")
                                    .debug_selector(|| "codex-composer-hints".into())
                                    .aria_description(STEER_DESCRIPTION)
                                    .w_full()
                                    .px(rpx(SPACE_2XL))
                                    .flex()
                                    .flex_wrap()
                                    .justify_end()
                                    .text_right()
                                    .whitespace_normal()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(gpui::phi())
                                    .text_color(c::FG_DIM())
                                    .when(!stopping && self.dashboard_tray.is_none() && (self.plan_answer.is_some() || self.submitted.is_some() || editing || busy), |hints| hints.child(super::super::components::shortcut_hint(if self.plan_answer.as_ref().is_some_and(|answer| answer.saving.is_some()) {
                                        "Saving answer…"
                                    } else if self.plan_answer.is_some() {
                                        "Enter to save answer"
                                    } else if self.submitted.is_some() {
                                        "Sending…"
                                    } else if editing {
                                        "Enter to save"
                                    } else if busy && steer_available {
                                        if cfg!(target_os = "macos") { "Enter to queue · Cmd+Enter to steer" } else { "Enter to queue · Ctrl+Shift+Enter to steer" }
                                    } else {
                                        "Enter to queue"
                                    })))
                            )
                            .when_some(notice_below, gpui::ParentElement::child)
                            ,
                    )
    }
}
