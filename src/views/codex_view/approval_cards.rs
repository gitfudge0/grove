//! Approval drafts, policy controls and responses.

use super::{
    action_key, activity_request, c, codex, literal_notice_text, permission_code,
    permission_heading, project_field_well, question_choice, rpx, status, transcript_action,
    Action, ApprovalDraft, CodexView, Disableable, HashSet, Input, InputState, StatusKind,
};
use crate::views::tokens::{
    ICON_SM, OPACITY_DISABLED, RADIUS_CONTROL, RADIUS_PANEL, SPACE_2XL, SPACE_3XL, SPACE_LG,
    SPACE_SM, TEXT_SMALL, TEXT_TITLE,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::AppContext as _;
use gpui::{div, Anchor, Context, Focusable, IntoElement, SharedString, Window};
use gpui_component::{
    button::Button,
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
};
use grove_core::controls::{approval_choices, approval_command, ApprovalChoice};
use serde_json::Value;

const APPROVAL_DETAILS_MAX_H: f32 = 132.0;
const APPROVAL_DETAILS_VIEWPORT_SHARE: f32 = 0.14;
const ALTERNATIVE_APPROACH_FEEDBACK: &str = "Please try a different approach.";

struct ApprovalCard<'a> {
    request: &'a Value,
    index: usize,
    key: &'a str,
    draft: &'a ApprovalDraft,
}

impl ApprovalCard<'_> {
    fn valid_input(&self, cx: &gpui::App) -> bool {
        !self.draft.editing
            || serde_json::from_str::<Value>(&self.draft.input.read(cx).value())
                .is_ok_and(|value| value.is_object())
    }
}

fn approval_icon(name: &str) -> gpui_component::Icon {
    gpui_component::Icon::default()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(rpx(ICON_SM))
}

fn command_overflow_choice(choice: &ApprovalChoice, has_once: bool, has_decline: bool) -> bool {
    match choice.id.as_str() {
        "allow_once" | "deny" | "execpolicy" => false,
        "allow_session" => has_once,
        "cancel" => has_decline,
        _ => has_once || has_decline,
    }
}

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
                && (draft.activity_anchor.focus.contains_focused(window, cx)
                    || draft.menu.as_ref().is_some_and(|(menu, _)| {
                        menu.focus_handle(cx).contains_focused(window, cx)
                    })
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
                    inspect_focus: cx.focus_handle(),
                    details_scroll: gpui::ScrollHandle::new(),
                    menu_focus: cx.focus_handle(),
                    menu: None,
                    inspecting: false,
                    feedback_open: false,
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
        let command_request = approval_command(request).is_some();
        let feedback = (denying && (!command_request || draft.feedback_open))
            .then(|| draft.feedback.read(cx).value().trim().to_owned())
            .filter(|text| !text.is_empty())
            .or_else(|| {
                (choice == "deny" && request["claudeInput"].is_object() && command_request)
                    .then(|| ALTERNATIVE_APPROACH_FEEDBACK.to_owned())
            });
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
            let card = ApprovalCard {
                request: approval,
                index,
                key: &key,
                draft,
            };
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
                .text_color(c::FG());
            if let Some(command) = approval_command(approval) {
                panel = panel.child(group.child(self.command_approval(
                    &card,
                    command,
                    state.saved.provider,
                    window,
                    cx,
                )));
                continue;
            }
            group = group.child(permission_heading(
                grove_core::controls::approval_title(approval),
                state.saved.provider.label().into(),
            ));
            group = group.child(literal_notice_text(
                format!("approval-details-{index}"),
                &grove_core::controls::approval_details(approval),
            ));
            let editable = approval
                .get("claudeInput")
                .is_some_and(serde_json::Value::is_object);
            let valid_json = card.valid_input(cx);
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
                let is_policy = choice.id == "execpolicy";
                let button = self.approval_choice_button(&card, choice_index, choice, window, cx);
                if is_policy {
                    policy_button = Some(button);
                } else {
                    buttons = buttons.child(button);
                }
            }
            group = group.child(buttons);
            if let Some(button) = policy_button {
                group = group.child(self.approval_policy(&card, button, window, cx));
            }
            panel = panel.child(group);
        }
        panel
    }
    fn command_approval(
        &self,
        card: &ApprovalCard<'_>,
        command: &str,
        provider: codex::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ApprovalCard {
            request,
            index,
            key,
            draft,
        } = *card;
        let title = request["claudeInput"]["description"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(
                || grove_core::controls::approval_title(request),
                |title| {
                    request["groveAgent"]
                        .as_str()
                        .map_or_else(|| title.to_owned(), |agent| format!("{agent}: {title}"))
                },
            );
        let reason = request["claudeRequest"]["decision_reason"]
            .as_str()
            .or_else(|| request["params"]["reason"].as_str());
        let choices = approval_choices(request);

        let has_once = choices.iter().any(|choice| choice.id == "allow_once");
        let has_decline = choices.iter().any(|choice| choice.id == "deny");
        let in_overflow =
            |choice: &ApprovalChoice| command_overflow_choice(choice, has_once, has_decline);
        let editable = request["claudeInput"].is_object();
        let show_menu =
            (editable && (!draft.editing || has_decline)) || choices.iter().any(in_overflow);
        let mut header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(rpx(SPACE_LG))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(crate::icons::icon(
                        "lock",
                        ICON_SM,
                        StatusKind::NeedsInput.color(),
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(rpx(TEXT_TITLE))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(rpx(SPACE_SM))
                    .text_color(c::FG_DIM())
                    .child(crate::icons::icon(
                        match provider {
                            codex::Provider::Codex => "codex",
                            codex::Provider::Claude => "claude",
                        },
                        ICON_SM,
                        c::FG(),
                    ))
                    .child(provider.label()),
            );
        if show_menu {
            header = header.child(self.approval_menu(card, window, cx));
        }

        let open = draft.inspecting;
        let inspect_key = key.to_owned();
        let inspect = crate::views::components::focusable_inline_disclosure(
            SharedString::from(format!("approval-inspect-{index}")),
            if draft.editing {
                "Edited tool input"
            } else {
                "Inspect command"
            },
        )
        .w_auto()
        .justify_start()
        .gap(rpx(SPACE_SM))
        .track_focus(&draft.inspect_focus)
        .aria_expanded(open)
        .debug_selector(move || format!("approval-inspect-{index}"))
        .child(crate::icons::icon(
            if open { "chev-down" } else { "chev-right" },
            ICON_SM,
            c::FG_DIM(),
        ))
        .child(if draft.editing {
            "Edited tool input"
        } else {
            "Inspect command"
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_command_inspection(&inspect_key, cx);
        }))
        .on_key_down(cx.listener({
            let key = key.to_owned();
            move |this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    this.toggle_command_inspection(&key, cx);
                    cx.stop_propagation();
                }
            }
        }));

        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM)).child(header);
        if let Some(reason) = reason.filter(|reason| !reason.trim().is_empty()) {
            body = body.child(
                div()
                    .text_color(c::FG_DIM())
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(reason.to_owned()),
            );
        }
        body = body.child(div().flex().items_start().child(inspect));
        if open {
            body = body.child(self.command_approval_details(card, command, reason, window, cx));
        }

        let mut decisions = div().flex().flex_wrap().justify_end().gap(rpx(SPACE_SM));
        if draft.editing {
            let key = key.to_owned();
            decisions = decisions.child(
                transcript_action(
                    SharedString::from(format!("approval-original-{index}")),
                    "Use original",
                    false,
                    window,
                    cx,
                )
                .icon(approval_icon("arrow-left"))
                .disabled(draft.submitting)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_approval_edit(&key, cx);
                    if let Some(draft) = this.approval_drafts.get(&key) {
                        draft.inspect_focus.focus(window, cx);
                    }
                })),
            );
        }
        let mut primary = None;
        let mut policy = None;
        for (choice_index, choice) in choices.into_iter().enumerate() {
            if choice.id == "execpolicy" {
                policy = Some(self.approval_choice_button(card, choice_index, choice, window, cx));
            } else if !in_overflow(&choice) {
                let emphasized = choice.id == "allow_once";
                let button = self.approval_choice_button(card, choice_index, choice, window, cx);
                if emphasized {
                    primary = Some(button);
                } else {
                    decisions = decisions.child(button);
                }
            }
        }
        let valid_input = card.valid_input(cx);
        let mut footer = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .pt(rpx(SPACE_LG));
        if draft.editing {
            footer = footer.child(if valid_input {
                div()
                    .text_color(c::FG_DIM())
                    .child("Approving sends the edited input.")
                    .into_any_element()
            } else {
                status::label(
                    format!("approval-error-status-{index}"),
                    StatusKind::Failed,
                    "Enter a valid JSON object before approving.",
                    ICON_SM,
                    cx,
                )
                .into_any_element()
            });
        }
        if let Some(policy) = policy {
            footer = footer.child(self.approval_policy(card, policy, window, cx));
        }
        body.child(footer.child(decisions.when_some(primary, ParentElement::child)))
    }

    fn command_approval_details(
        &self,
        card: &ApprovalCard<'_>,
        command: &str,
        reason: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ApprovalCard {
            request,
            index,
            draft,
            ..
        } = *card;
        let mut details = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .p(rpx(SPACE_SM));
        if draft.feedback_open {
            details = details
                .child(
                    div()
                        .text_color(c::FG_DIM())
                        .child("Feedback if declined (optional)"),
                )
                .child(
                    project_field_well(draft.feedback.focus_handle(cx).is_focused(window), false)
                        .child(
                            Input::new(&draft.feedback)
                                .file_drop_overlay(crate::views::components::file_drop_overlay())
                                .aria_label("Feedback if declined")
                                .disabled(draft.submitting)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0(),
                        ),
                );
        }
        if let Some(cwd) = request["params"]["cwd"].as_str() {
            details = details.child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(c::FG_DIM())
                            .child("Working directory"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(literal_notice_text(format!("approval-cwd-{index}"), cwd)),
                    ),
            );
        }
        if draft.editing {
            details = details.child(
                project_field_well(
                    draft.input.focus_handle(cx).is_focused(window),
                    !card.valid_input(cx),
                )
                .h_auto()
                .py(rpx(SPACE_SM))
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
        } else {
            details = details.child(permission_code(
                format!("approval-command-{index}"),
                command,
            ));
        }
        if let Some(reason) = reason {
            details = details.child(literal_notice_text(
                format!("approval-full-reason-{index}"),
                reason,
            ));
        }
        let extra = grove_core::controls::approval_command_extra(request);
        if !extra.is_empty() {
            details = details.child(literal_notice_text(
                format!("approval-extra-{index}"),
                &extra,
            ));
        }
        let scroll = draft.details_scroll.clone();
        let editor_focus = draft.input.focus_handle(cx);
        let feedback_focus = draft.feedback.focus_handle(cx);
        div()
            .id(SharedString::from(format!(
                "approval-inspection-{}",
                card.key
            )))
            .debug_selector(move || format!("approval-inspection-{index}"))
            .role(gpui::Role::Group)
            .aria_label("Command details")
            .tab_index(0)
            .focus_visible(|style| style.border_color(c::BORDER_STRONG()))
            .rounded(rpx(RADIUS_CONTROL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .max_h(
                rpx(APPROVAL_DETAILS_MAX_H)
                    .to_pixels(window.rem_size())
                    .min(window.viewport_size().height * APPROVAL_DETAILS_VIEWPORT_SHARE),
            )
            .overflow_y_scroll()
            .track_scroll(&draft.details_scroll)
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                if editor_focus.is_focused(window) || feedback_focus.is_focused(window) {
                    return;
                }
                let line = rpx(SPACE_3XL).to_pixels(window.rem_size());
                let page = scroll.bounds().size.height;
                let mut offset = scroll.offset();
                offset.y += match event.keystroke.key.as_str() {
                    "up" => line,
                    "down" => -line,
                    "pageup" => page,
                    "pagedown" => -page,
                    _ => return,
                };
                scroll.set_offset(offset);
                window.refresh();
                cx.stop_propagation();
            })
            .child(details)
    }

    fn approval_choice_button(
        &self,
        card: &ApprovalCard<'_>,
        choice_index: usize,
        choice: ApprovalChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Button {
        let ApprovalCard {
            request,
            index,
            key,
            draft,
        } = *card;
        let key = key.to_owned();
        let id = request["id"].clone();
        let denying = matches!(choice.id.as_str(), "deny" | "cancel");
        let alternative = choice.id == "deny"
            && request["claudeInput"].is_object()
            && approval_command(request).is_some();
        let label = if alternative {
            "Ask for another approach".into()
        } else {
            choice.label
        };
        transcript_action(
            SharedString::from(format!("approval-{index}-{choice_index}")),
            label,
            choice.id == "allow_once",
            window,
            cx,
        )
        .icon(approval_icon(if alternative {
            "reply"
        } else if denying {
            "close"
        } else {
            "check"
        }))
        .debug_selector(move || format!("approval-{index}-{choice_index}"))
        .disabled(draft.submitting || (!denying && !card.valid_input(cx)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.respond_approval(&key, id.clone(), choice.id.clone(), cx);
        }))
    }

    fn approval_policy(
        &self,
        card: &ApprovalCard<'_>,
        button: Button,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ApprovalCard {
            request,
            index,
            key,
            draft,
        } = *card;
        let key = key.to_owned();
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(rpx(SPACE_SM))
            .child(
                transcript_action(
                    SharedString::from(format!("approval-policy-{index}")),
                    "Matching command prefix",
                    false,
                    window,
                    cx,
                )
                .icon(approval_icon(if draft.policy_expanded {
                    "chev-down"
                } else {
                    "chev-right"
                }))
                .aria_expanded(draft.policy_expanded)
                .debug_selector(move || format!("approval-policy-{index}"))
                .disabled(draft.submitting)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_approval_policy(&key, cx))),
            )
            .when(draft.policy_expanded, |panel| {
                panel
                    .child(
                        div()
                            .text_color(c::FG_DIM())
                            .child("Proposed execution policy amendment"),
                    )
                    .child(permission_code(
                        format!("approval-prefix-{index}"),
                        &serde_json::to_string_pretty(
                            &request["params"]["proposedExecpolicyAmendment"],
                        )
                        .unwrap_or_default(),
                    ))
                    .child(button)
            })
    }

    fn approval_menu(
        &self,
        card: &ApprovalCard<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ApprovalCard {
            index, key, draft, ..
        } = *card;
        let menu = draft.menu.as_ref().map(|(menu, _)| menu.clone());
        let keyboard_key = key.to_owned();
        let pointer_key = key.to_owned();
        let owner = cx.entity().downgrade();
        let trigger = transcript_action(
            SharedString::from(format!("approval-more-{index}")),
            "Other request actions",
            false,
            window,
            cx,
        )
        .icon(approval_icon("more"))
        .icon_only()
        .with_focus_handle(draft.menu_focus.clone())
        .debug_selector(move || format!("approval-more-{index}"))
        .disabled(draft.submitting)
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if action_key(event) {
                    this.toggle_approval_menu(&keyboard_key, window, cx);
                    cx.stop_propagation();
                }
            }),
        );
        Popover::new(SharedString::from(format!("approval-menu-{key}")))
            .anchor(Anchor::TopRight)
            .appearance(false)
            .overlay_closable(false)
            .open(menu.is_some())
            .on_open_change(move |open, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if *open
                        != view
                            .approval_drafts
                            .get(&pointer_key)
                            .is_some_and(|draft| draft.menu.is_some())
                    {
                        view.toggle_approval_menu(&pointer_key, window, cx);
                    }
                });
            })
            .trigger(trigger)
            .content(move |_, _, _| {
                menu.clone()
                    .map_or_else(|| div().into_any_element(), IntoElement::into_any_element)
            })
    }

    fn toggle_approval_menu(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.session.read(cx).state;
        let Some(request) = state
            .approvals
            .iter()
            .find(|request| Self::approval_key(request, state.interaction_epoch) == key)
        else {
            return;
        };
        let Some(draft) = self.approval_drafts.get_mut(key) else {
            return;
        };
        if draft.menu.take().is_some() {
            draft.menu_focus.focus(window, cx);
            cx.notify();
            return;
        }
        if draft.submitting {
            return;
        }
        let choices = approval_choices(request);
        let has_once = choices.iter().any(|choice| choice.id == "allow_once");
        let editable = request["claudeInput"].is_object();
        let has_decline = choices.iter().any(|choice| choice.id == "deny");
        let editing = draft.editing;
        let valid_input = !editing
            || serde_json::from_str::<Value>(&draft.input.read(cx).value())
                .is_ok_and(|value| value.is_object());
        let id = request["id"].clone();
        let focus = draft.menu_focus.clone();
        let owner = cx.entity().downgrade();
        let key = key.to_owned();
        let menu_key = key.clone();
        let dismiss_key = key.clone();
        let popup = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu =
                crate::views::components::dropdown_menu(menu, window).action_context(focus);
            if editable && !editing {
                let owner = owner.clone();
                let key = key.clone();
                menu = menu.item(
                    PopupMenuItem::new("Edit tool input")
                        .icon(approval_icon("edit"))
                        .on_click(move |_, window, cx| {
                            let owner = owner.clone();
                            let key = key.clone();
                            window.defer(cx, move |window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.toggle_approval_edit(&key, cx);
                                    if let Some(draft) = view.approval_drafts.get(&key) {
                                        draft.input.focus_handle(cx).focus(window, cx);
                                    }
                                });
                            });
                        }),
                );
            }
            if editable && has_decline {
                let owner = owner.clone();
                let key = key.clone();
                menu = menu.item(
                    PopupMenuItem::new("Add feedback")
                        .icon(approval_icon("reply"))
                        .on_click(move |_, window, cx| {
                            let owner = owner.clone();
                            let key = key.clone();
                            window.defer(cx, move |window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if let Some(draft) = view.approval_drafts.get_mut(&key) {
                                        if !draft.submitting {
                                            draft.feedback_open = true;
                                            draft.inspecting = true;
                                            draft.feedback.focus_handle(cx).focus(window, cx);
                                            cx.notify();
                                        }
                                    }
                                });
                            });
                        }),
                );
            }
            for choice in choices
                .into_iter()
                .filter(|choice| command_overflow_choice(choice, has_once, has_decline))
            {
                let owner = owner.clone();
                let key = key.clone();
                let id = id.clone();
                let cancel = choice.id == "cancel";
                menu = menu.item(
                    PopupMenuItem::new(choice.label)
                        .icon(approval_icon(if cancel { "close" } else { "check" }))
                        .disabled(!cancel && !valid_input)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.respond_approval(&key, id.clone(), choice.id.clone(), cx);
                            });
                        }),
                );
            }
            menu
        });
        let dismissed = cx.subscribe_in(
            &popup,
            window,
            move |view, _, _: &gpui::DismissEvent, _, cx| {
                if let Some(draft) = view.approval_drafts.get_mut(&dismiss_key) {
                    draft.menu = None;
                    cx.notify();
                }
            },
        );
        popup.focus_handle(cx).focus(window, cx);
        if let Some(draft) = self.approval_drafts.get_mut(&menu_key) {
            draft.menu = Some((popup, dismissed));
        }
        cx.notify();
    }

    pub(super) fn toggle_command_inspection(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.approval_drafts.get_mut(key) {
            draft.inspecting = !draft.inspecting;
            if !draft.inspecting {
                draft.feedback_open = false;
            }
            if draft.inspecting {
                draft
                    .details_scroll
                    .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
            }
            cx.notify();
        }
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
                draft.inspecting = true;
                draft
                    .details_scroll
                    .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                cx.notify();
            }
        }
    }
}
