//! Approval drafts, policy controls and responses.

use super::{
    activity_request, c, codex, literal_notice_text, permission_code, permission_heading,
    project_field_well, question_choice, rpx, status, transcript_action, Action, ApprovalDraft,
    CodexView, Disableable, HashSet, Input, InputState, StatusKind,
};
use crate::views::tokens::{
    ICON_SM, OPACITY_DISABLED, RADIUS_CONTROL, RADIUS_PANEL, SPACE_2XL, SPACE_3XL, SPACE_LG,
    TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::AppContext as _;
use gpui::{div, Context, Focusable, Window};

impl CodexView {
    pub(super) fn sync_approvals(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: HashSet<_> = state
            .approvals
            .iter()
            .map(|approval| Self::approval_key(approval, state.interaction_epoch))
            .collect();
        let restore_focus = self.approval_drafts.iter().any(|(key, draft)| {
            !keys.contains(key)
                && (draft.activity_anchor.focus.is_focused(window)
                    || draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window))
        });
        self.approval_drafts.retain(|key, _| keys.contains(key));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for approval in &state.approvals {
            let key = Self::approval_key(approval, state.interaction_epoch);
            if let Some(draft) = self.approval_drafts.get_mut(&key) {
                if state.error.is_some() {
                    draft.submitting = false;
                }
                continue;
            }
            let input = cx.new(|cx| {
                InputState::new(window, cx).auto_grow(3, 10).default_value(
                    serde_json::to_string_pretty(&approval["claudeInput"]).unwrap_or_default(),
                )
            });
            let feedback = cx.new(|cx| {
                InputState::new(window, cx).placeholder("e.g. Please use the project folder only")
            });
            let subscriptions = vec![
                cx.observe(&input, |_, _, cx| cx.notify()),
                cx.observe(&feedback, |_, _, cx| cx.notify()),
            ];
            self.approval_drafts.insert(
                key,
                ApprovalDraft {
                    activity_anchor: activity_request::RequestAnchor::new(
                        &self.agent_controls_scroll,
                        cx,
                    ),
                    input,
                    feedback,
                    editing: false,
                    policy_expanded: false,
                    submitting: false,
                    _subscriptions: subscriptions,
                },
            );
        }
    }
    pub(super) fn approval_key(approval: &serde_json::Value, epoch: u64) -> String {
        format!("{epoch}:{approval}")
    }
    pub(super) fn respond_approval(
        &mut self,
        key: &str,
        id: serde_json::Value,
        choice: String,
        cx: &mut Context<Self>,
    ) {
        let state = &self.session.read(cx).state;
        let Some(request) = state.approvals.iter().find(|request| {
            Self::approval_key(request, state.interaction_epoch) == key && request["id"] == id
        }) else {
            return;
        };
        if !grove_core::controls::approval_choices(request)
            .iter()
            .any(|option| option.id == choice)
        {
            return;
        }
        let Some(draft) = self.approval_drafts.get_mut(key) else {
            return;
        };
        if draft.submitting {
            return;
        }
        let denying = matches!(choice.as_str(), "deny" | "cancel");
        let input = if draft.editing && !denying {
            match serde_json::from_str::<serde_json::Value>(&draft.input.read(cx).value()) {
                Ok(value) if value.is_object() => Some(value),
                _ => return,
            }
        } else {
            None
        };
        let feedback = denying
            .then(|| draft.feedback.read(cx).value().trim().to_owned())
            .filter(|text| !text.is_empty());
        draft.submitting = true;
        let epoch = state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::RespondApproval {
                        id,
                        choice,
                        input,
                        feedback,
                    }),
                },
                cx,
            );
        });
        cx.notify();
    }
    pub(super) fn approval_panel(
        &self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut panel = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL));
        for (index, approval) in state.approvals.iter().enumerate() {
            let key = Self::approval_key(approval, state.interaction_epoch);
            let draft = &self.approval_drafts[&key];
            let mut group = div()
                .id(("codex-approval", index))
                .track_focus(&draft.activity_anchor.focus)
                .tab_group()
                .anchor_scroll(Some(draft.activity_anchor.scroll.clone()))
                .focus_visible(|style| style.border_color(c::BORDER_STRONG()))
                .p(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .role(gpui::Role::Group)
                .aria_label(grove_core::controls::approval_title(approval))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .font_family(crate::fonts::UI_FAMILY)
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG())
                .child(permission_heading(
                    grove_core::controls::approval_title(approval),
                    state.saved.provider.label().into(),
                ));
            if let Some(command) = grove_core::controls::approval_command(approval) {
                if let Some(reason) = approval["params"]["reason"]
                    .as_str()
                    .or_else(|| approval["claudeInput"]["description"].as_str())
                {
                    group = group.child(literal_notice_text(
                        format!("approval-reason-{index}"),
                        reason,
                    ));
                }
                group = group.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .p(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_CONTROL))
                        .bg(c::BG())
                        .child(permission_code(
                            format!("approval-command-{index}"),
                            command,
                        )),
                );
                if let Some(cwd) = approval["params"]["cwd"].as_str() {
                    group = group
                        .child(div().text_color(c::FG_DIM()).child("Working directory"))
                        .child(literal_notice_text(format!("approval-cwd-{index}"), cwd));
                }
                let extra = grove_core::controls::approval_command_extra(approval);
                if !extra.is_empty() {
                    group = group.child(literal_notice_text(
                        format!("approval-extra-{index}"),
                        &extra,
                    ));
                }
            } else {
                group = group.child(literal_notice_text(
                    format!("approval-details-{index}"),
                    &grove_core::controls::approval_details(approval),
                ));
            }
            let editable = approval
                .get("claudeInput")
                .is_some_and(serde_json::Value::is_object);
            let valid_json = !draft.editing
                || serde_json::from_str::<serde_json::Value>(&draft.input.read(cx).value())
                    .is_ok_and(|value| value.is_object());
            if editable {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                group = group.child(
                    question_choice(
                        gpui::SharedString::from(format!("approval-edit-{index}")),
                        "Edit tool input".into(),
                        "Applies only when you approve this request".into(),
                        true,
                        draft.editing,
                    )
                    .when(draft.submitting, |row| {
                        row.cursor_default().opacity(OPACITY_DISABLED)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_approval_edit(&click_key, cx);
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "space" | "enter")
                            {
                                this.toggle_approval_edit(&keyboard_key, cx);
                                cx.stop_propagation();
                            }
                        },
                    )),
                );
                if draft.editing {
                    group = group.child(
                        project_field_well(
                            draft.input.focus_handle(cx).is_focused(window),
                            !valid_json,
                        )
                        .child(
                            Input::new(&draft.input)
                                .file_drop_overlay(crate::views::components::file_drop_overlay())
                                .aria_label("Tool input JSON")
                                .disabled(draft.submitting)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0(),
                        ),
                    );
                    if !valid_json {
                        group = group.child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "approval-validation-{index}"
                                )))
                                .role(gpui::Role::Alert)
                                .child(status::label(
                                    format!("approval-error-status-{index}"),
                                    StatusKind::Failed,
                                    "Enter a valid JSON object before approving.",
                                    ICON_SM,
                                    cx,
                                )),
                        );
                    }
                }
                group = group
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Feedback if denied (optional)"),
                    )
                    .child(
                        project_field_well(
                            draft.feedback.focus_handle(cx).is_focused(window),
                            false,
                        )
                        .child(
                            Input::new(&draft.feedback)
                                .file_drop_overlay(crate::views::components::file_drop_overlay())
                                .aria_label("Feedback if denied")
                                .disabled(draft.submitting)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0(),
                        ),
                    );
            }
            let mut buttons = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            let mut policy_button = None;
            for (choice_index, choice) in grove_core::controls::approval_choices(approval)
                .into_iter()
                .enumerate()
            {
                let click_key = key.clone();
                let id = approval["id"].clone();
                let denying = matches!(choice.id.as_str(), "deny" | "cancel");
                let is_policy = choice.id == "execpolicy";
                let button = transcript_action(
                    gpui::SharedString::from(format!("approval-{index}-{choice_index}")),
                    choice.label,
                    choice.id == "allow_once",
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(gpui::SharedString::from(format!(
                            "icons/{}.svg",
                            if denying { "close" } else { "check" }
                        )))
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(move || format!("approval-{index}-{choice_index}"))
                .disabled(draft.submitting || (!denying && !valid_json))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.respond_approval(&click_key, id.clone(), choice.id.clone(), cx);
                }));
                if is_policy {
                    policy_button = Some(button);
                } else {
                    buttons = buttons.child(button);
                }
            }
            group = group.child(buttons);
            if let Some(button) = policy_button {
                let click_key = key.clone();
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .pt(rpx(SPACE_LG))
                        .child(
                            transcript_action(
                                gpui::SharedString::from(format!("approval-policy-{index}")),
                                "Matching command prefix",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path(if draft.policy_expanded {
                                        "icons/chev-down.svg"
                                    } else {
                                        "icons/chev-right.svg"
                                    })
                                    .size(rpx(ICON_SM)),
                            )
                            .aria_expanded(draft.policy_expanded)
                            .debug_selector(move || format!("approval-policy-{index}"))
                            .disabled(draft.submitting)
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.toggle_approval_policy(&click_key, cx),
                            )),
                        ),
                );
                if draft.policy_expanded {
                    group = group
                        .child(
                            div()
                                .text_color(c::FG_DIM())
                                .child("Proposed execution policy amendment"),
                        )
                        .child(permission_code(
                            format!("approval-prefix-{index}"),
                            &serde_json::to_string_pretty(
                                &approval["params"]["proposedExecpolicyAmendment"],
                            )
                            .unwrap_or_default(),
                        ))
                        .child(button);
                }
            }
            panel = panel.child(group);
        }
        panel
    }
    fn toggle_approval_policy(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.approval_drafts.get_mut(key) {
            if !draft.submitting {
                draft.policy_expanded = !draft.policy_expanded;
                cx.notify();
            }
        }
    }
    pub(super) fn toggle_approval_edit(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.approval_drafts.get_mut(key) {
            if !draft.submitting {
                draft.editing = !draft.editing;
                cx.notify();
            }
        }
    }
}
