//! Question cards, review and answer delivery.

use super::{
    activity_request, attached_validation, c, codex, project_field_well, question_answer_summary,
    question_choice, question_has_answer, question_key, rpx, status, transcript_action, Action,
    CodexView, Disableable, HashMap, HashSet, Input, InputEvent, InputState, QuestionDraft,
    QuestionGroupDraft, QuestionReceipt, StatusKind, CHAT_MAX_W,
};
use crate::views::tokens::{
    ICON_SM, OPACITY_DISABLED, RADIUS_GROUP, SPACE_2XL, SPACE_3XL, SPACE_LG, SPACE_SM, SPACE_XS,
    TEXT_BODY, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AppContext as _;
use gpui::{div, App, Context, Focusable, Window};
use std::fmt::Write as _;

impl CodexView {
    pub(super) fn sync_questions(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.question_generation != state.cleared_conversations
            || (self.stream_thread.is_some() && self.stream_thread != state.saved.thread_id)
        {
            self.question_receipts.clear();
            self.question_drafts.clear();
            self.question_generation = state.cleared_conversations;
        }
        let instances: HashMap<_, _> = state
            .questions
            .iter()
            .map(|r| (question_key(state, r), r.instance()))
            .collect();
        let current = |key: &String, draft: &QuestionGroupDraft| {
            instances.get(key) == Some(&draft.request_instance)
        };
        let restore_focus = self.question_drafts.iter().any(|(key, group)| {
            !current(key, group)
                && (group.activity_anchor.focus.is_focused(window)
                    || group.row_focus.iter().any(|focus| focus.is_focused(window))
                    || group.questions.iter().any(|question| {
                        question.input.focus_handle(cx).is_focused(window)
                            || question
                                .choice_focus
                                .iter()
                                .any(|focus| focus.is_focused(window))
                    }))
        });
        for (key, group) in &mut self.question_drafts {
            if group.pending_receipt.is_some()
                && state.question_answer_acks.iter().any(|ack| {
                    ack.sequence > group.ack_after
                        && format!(
                            "{}:{}:{}:{}",
                            state.interaction_epoch,
                            ack.thread_id.as_deref().unwrap_or_default(),
                            if group.turn_bound {
                                ack.turn_id.as_deref().unwrap_or_default()
                            } else {
                                ""
                            },
                            ack.id
                        ) == *key
                })
            {
                if let Some(receipt) = group.pending_receipt.take() {
                    self.question_receipts.push(receipt);
                }
            }
        }
        // Async answers become ordinary user messages. Keep the acknowledgement
        // only until that message arrives; tool answers still need their receipt.
        self.question_receipts.retain(|receipt| {
            !receipt.echo_text.as_ref().is_some_and(|echo| {
                state
                    .saved
                    .items
                    .iter()
                    .skip(receipt.echo_after)
                    .any(|item| item["type"] == "userMessage" && codex::item_text(item) == *echo)
            })
        });
        self.question_drafts
            .retain(|key, draft| current(key, draft));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for request in &state.questions {
            let key = question_key(state, request);
            if let Some(draft) = self.question_drafts.get_mut(&key) {
                if state.error.is_some() && draft.submitting {
                    draft.submitting = false;
                    draft.reviewing = false;
                    draft.pending_receipt = None;
                }
                continue;
            }
            let questions = request
                .questions
                .iter()
                .map(|question| {
                    let input = cx.new(|cx| {
                        let placeholder = question.placeholder.clone().unwrap_or_else(|| {
                            if let codex::QuestionKind::Number(number) = &question.kind {
                                format!("e.g. {}", number.min)
                            } else if question.options.is_empty() {
                                "Type your answer…".into()
                            } else {
                                "Type a different answer…".into()
                            }
                        });
                        let mut input = InputState::new(window, cx).placeholder(placeholder);
                        if let codex::QuestionKind::Number(number) = &question.kind {
                            if let Some(value) = number.default_value {
                                input = input.default_value(value.to_string());
                            }
                        }
                        if question.secret {
                            input.masked(true)
                        } else if matches!(question.kind, codex::QuestionKind::Number(_)) {
                            input
                        } else {
                            input.auto_grow(1, 4)
                        }
                    });
                    let input_key = key.clone();
                    let subscription =
                        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change) {
                                if let Some(draft) = this.question_drafts.get_mut(&input_key) {
                                    if !draft.submitting {
                                        draft.reviewing = false;
                                    }
                                }
                            }
                            cx.notify();
                        });
                    QuestionDraft {
                        selected: HashSet::new(),
                        choice_focus: (0..question.options.len()
                            + usize::from(question.allow_custom))
                            .map(|_| cx.focus_handle())
                            .collect(),
                        custom: question.options.is_empty(),
                        input,
                        _subscription: subscription,
                    }
                })
                .collect();
            self.question_drafts.insert(
                key,
                QuestionGroupDraft {
                    activity_anchor: activity_request::RequestAnchor::new(&self.scroll, cx),
                    request_instance: request.instance(),
                    reviewing: false,
                    row_anchors: request
                        .questions
                        .iter()
                        .map(|_| gpui::ScrollAnchor::for_handle(self.scroll.clone()))
                        .collect(),
                    questions,
                    row_focus: request
                        .questions
                        .iter()
                        .map(|_| cx.focus_handle())
                        .collect(),
                    anchor: state.saved.items.len(),
                    ack_after: state
                        .question_answer_acks
                        .last()
                        .map_or(0, |ack| ack.sequence),
                    turn_bound: request.turn_bound(),
                    pending_receipt: None,
                    submitting: false,
                },
            );
        }
    }
    fn question_answers(
        &self,
        key: &str,
        request: &codex::QuestionRequest,
        cx: &App,
    ) -> codex::QuestionAnswers {
        let Some(group) = self.question_drafts.get(key) else {
            return HashMap::new();
        };
        request
            .questions
            .iter()
            .zip(&group.questions)
            .map(|(question, draft)| {
                let mut values: Vec<_> = question
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| draft.selected.contains(i))
                    .map(|(_, option)| option.label.clone())
                    .collect();
                if draft.custom {
                    let value = draft.input.read(cx).value();
                    values.push(if question.secret {
                        value.to_string()
                    } else {
                        value.trim().to_owned()
                    });
                }
                (question.id.clone(), values)
            })
            .collect()
    }
    pub(super) fn select_question_choice(
        &mut self,
        key: &str,
        question_index: usize,
        choice: Option<usize>,
        multiple: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        group.reviewing = false;
        let draft = &mut group.questions[question_index];
        let was_custom = draft.custom;
        if !multiple {
            draft.selected.clear();
            draft.custom = false;
        }
        if let Some(index) = choice {
            draft.choice_focus[index].focus(window, cx);
            if !draft.selected.insert(index) && multiple {
                draft.selected.remove(&index);
            }
        } else {
            draft.custom = !was_custom;
            if draft.custom {
                draft.input.focus_handle(cx).focus(window, cx);
            } else if let Some(focus) = draft.choice_focus.last() {
                focus.focus(window, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn answer_questions(
        &mut self,
        key: &str,
        id: serde_json::Value,
        cancel: bool,
        cx: &mut Context<Self>,
    ) {
        let state = &self.session.read(cx).state;
        let Some(request) = state
            .questions
            .iter()
            .find(|r| r.id == id && question_key(state, r) == key)
        else {
            return;
        };
        let answers = if cancel {
            None
        } else {
            let answers = self.question_answers(key, request, cx);
            if request.validate(&answers).is_err() {
                return;
            }
            Some(answers)
        };
        let receipt = answers.as_ref().map(|answers| QuestionReceipt {
            anchor: self.question_drafts[key].anchor,
            echo_text: request.answer_echo_text(answers),
            echo_after: state.saved.items.len(),
            title: request.title.clone(),
            answers: request
                .questions
                .iter()
                .map(|question| {
                    (
                        question.text.clone(),
                        question_answer_summary(question, answers),
                    )
                })
                .collect(),
        });
        let ack_after = state
            .question_answer_acks
            .last()
            .map_or(0, |ack| ack.sequence);
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        group.pending_receipt = receipt;
        group.ack_after = ack_after;
        group.submitting = true;
        let epoch = state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::AnswerQuestions(id, answers)),
                },
                cx,
            );
        });
        cx.notify();
    }
    pub(super) fn toggle_question(
        &mut self,
        key: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        group.row_anchors[index].scroll_to(window, cx);
        group.row_focus[index].focus(window, cx);
        cx.notify();
    }

    pub(super) fn has_question_anchor(&self, anchor: usize) -> bool {
        self.question_drafts
            .values()
            .any(|group| group.anchor == anchor)
            || self
                .question_receipts
                .iter()
                .any(|receipt| receipt.anchor == anchor)
    }

    pub(super) fn question_blocks_at(
        &mut self,
        anchor: usize,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut blocks = Vec::new();
        for (index, receipt) in self
            .question_receipts
            .iter()
            .enumerate()
            .filter(|(_, receipt)| receipt.anchor == anchor)
        {
            let mut group = div()
                .id(("question-receipt", index))
                .debug_selector(move || format!("question-receipt-{index}"))
                .role(gpui::Role::Group)
                .aria_label("Submitted answers")
                .flex_shrink_0()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_GROUP))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::BG_RAIL())
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Answers sent"),
                );
            if let Some(title) = &receipt.title {
                group = group.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.clone()),
                );
            }
            for (answer_index, (question, answer)) in receipt.answers.iter().enumerate() {
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_XS))
                        .child(
                            div()
                                .id(("question-receipt-question", answer_index))
                                .debug_selector(move || {
                                    format!("question-receipt-{index}-question-{answer_index}")
                                })
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(question.clone()),
                        )
                        .child(
                            div()
                                .id(("question-receipt-answer", answer_index))
                                .debug_selector(move || {
                                    format!("question-receipt-{index}-answer-{answer_index}")
                                })
                                .child(answer.clone()),
                        ),
                );
            }
            // Resolve the transcript width before measuring wrapped receipt text.
            // A max-width on the flex card itself leaves its intrinsic height
            // based on the wider transcript, clipping the final answer.
            blocks.push(
                div()
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex_shrink_0()
                    .child(group)
                    .into_any_element(),
            );
        }
        for (index, request) in state.questions.iter().enumerate() {
            if self
                .question_drafts
                .get(&question_key(state, request))
                .is_some_and(|group| group.anchor == anchor)
            {
                blocks.push(
                    div()
                        .id(("inline-questions", index))
                        .debug_selector(|| "agent-questions".into())
                        .flex_shrink_0()
                        .w_full()
                        .max_w(rpx(CHAT_MAX_W))
                        .min_w_0()
                        .child(self.question_group(state, index, window, cx))
                        .into_any_element(),
                );
            }
        }
        blocks
    }

    fn question_group(
        &mut self,
        state: &codex::Snapshot,
        request_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let request = &state.questions[request_index];
        let key = question_key(state, request);
        let draft = &self.question_drafts[&key];
        let submitting = draft.submitting;
        let answers = self.question_answers(&key, request, cx);
        let answer_checks: Vec<_> = request
            .questions
            .iter()
            .map(|question| question.validate_answer(answers.get(&question.id).map(Vec::as_slice)))
            .collect();
        let ready_count = answer_checks.iter().filter(|check| check.is_ok()).count();
        let validation = request.validate(&answers);
        let ready = !submitting && validation.is_ok();
        let reviewing = draft.reviewing;
        let next_index = answer_checks.iter().position(Result::is_err);
        let next_key = key.clone();
        let mut group = div()
            .id(("agent-question-group", request_index))
            .track_focus(&draft.activity_anchor.focus)
            .tab_group()
            .anchor_scroll(Some(draft.activity_anchor.scroll.clone()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .role(gpui::Role::Group)
            .aria_label(format!(
                "Questions from {}",
                state
                    .interaction_agent(&request.id)
                    .unwrap_or(state.saved.provider.label())
            ))
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(status::label(
                        format!("question-status-{request_index}"),
                        StatusKind::NeedsInput,
                        "Needs your input",
                        ICON_SM,
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(
                                state
                                    .interaction_agent(&request.id)
                                    .unwrap_or(state.saved.provider.label())
                                    .to_owned(),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(format!(
                                "{ready_count} of {} ready · {}",
                                request.questions.len(),
                                if reviewing { "review" } else { "draft" }
                            )),
                    )
                    .child(
                        transcript_action(
                            gpui::SharedString::from(format!("questions-next-{request_index}")),
                            "Next incomplete",
                            false,
                            window,
                            cx,
                        )
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/chev-down.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .debug_selector(move || format!("questions-next-{request_index}"))
                        .disabled(submitting || reviewing || next_index.is_none())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if let Some(index) = next_index {
                                    this.toggle_question(&next_key, index, window, cx);
                                }
                            },
                        )),
                    ),
            );
        if let Some(title) = &request.title {
            group = group.child(
                div()
                    .pb(rpx(SPACE_LG))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title.clone()),
            );
        }
        for (question_index, question) in request.questions.iter().enumerate() {
            let question_draft = &draft.questions[question_index];
            if reviewing {
                let mut summary = div().flex().flex_col().gap(rpx(SPACE_XS));
                if question.multiple && !question.secret {
                    if let Some(values) = answers.get(&question.id) {
                        for (index, value) in values.iter().enumerate() {
                            if question_draft.custom && index + 1 == values.len() {
                                summary = summary.child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child("Custom answer"),
                                );
                            }
                            summary = summary.child(div().child(value.clone()));
                        }
                    }
                } else {
                    summary = summary.child(question_answer_summary(question, &answers));
                }
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_SM))
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .py(rpx(SPACE_LG))
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(question.text.clone()),
                        )
                        .child(summary),
                );
                continue;
            }
            let mut field = div()
                .id(gpui::SharedString::from(format!(
                    "question-{request_index}-{question_index}"
                )))
                .role(gpui::Role::Group)
                .aria_label(question.text.clone())
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .py(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(format!("{:02}", question_index + 1)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(rpx(TEXT_BODY))
                                .child(question.text.clone()),
                        ),
                )
                .anchor_scroll(Some(draft.row_anchors[question_index].clone()))
                .track_focus(&draft.row_focus[question_index])
                .tab_index(0)
                .debug_selector(move || format!("question-row-{request_index}-{question_index}"))
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .when(
                    !question.header.is_empty() || !question.options.is_empty(),
                    |field| {
                        let instruction = if question.options.is_empty() {
                            ""
                        } else if question.multiple {
                            "Select all that apply"
                        } else {
                            "Select one"
                        };
                        let supporting = if question.header.is_empty() {
                            instruction.to_owned()
                        } else if instruction.is_empty() {
                            question.header.clone()
                        } else {
                            format!("{} · {instruction}", question.header)
                        };
                        field.child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(supporting),
                        )
                    },
                );
            if let Some(description) = &question.description {
                field = field.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(description.clone()),
                );
            }
            if let codex::QuestionKind::Number(number) = &question.kind {
                let mut hint = format!("{}–{}", number.min, number.max);
                if let Some(unit) = &number.unit {
                    let _ = write!(hint, " {unit}");
                }
                if let Some(step) = number.step {
                    let _ = write!(hint, " · increment {step}");
                }
                field = field.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(hint),
                );
            }
            for (option_index, option) in question.options.iter().enumerate() {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                let multiple = question.multiple;
                let choice_count = question.options.len();
                let option_count = choice_count + usize::from(question.allow_custom);
                field = field.child(
                    question_choice(
                        gpui::SharedString::from(format!(
                            "question-option-{request_index}-{question_index}-{option_index}"
                        )),
                        option.label.clone(),
                        match &option.preview {
                            Some(preview) if !option.description.is_empty() => {
                                format!("{}\n{preview}", option.description)
                            }
                            Some(preview) => preview.clone(),
                            None => option.description.clone(),
                        },
                        multiple,
                        question_draft.selected.contains(&option_index),
                    )
                    .when(!question_draft.selected.contains(&option_index), |row| {
                        row.bg(gpui::transparent_black())
                            .border_color(gpui::transparent_black())
                    })
                    .track_focus(&question_draft.choice_focus[option_index])
                    .debug_selector(move || {
                        format!("question-option-{request_index}-{question_index}-{option_index}")
                    })
                    .when(submitting, |row| {
                        row.cursor_default().opacity(OPACITY_DISABLED)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_question_choice(
                            &click_key,
                            question_index,
                            Some(option_index),
                            multiple,
                            window,
                            cx,
                        );
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if event.is_held {
                                return;
                            }
                            let next = match event.keystroke.key.as_str() {
                                "enter" | "space" => Some(option_index),
                                "down" | "right" if !multiple => {
                                    Some((option_index + 1) % option_count)
                                }
                                "up" | "left" if !multiple => {
                                    Some((option_index + option_count - 1) % option_count)
                                }
                                _ => None,
                            };
                            if let Some(next) = next {
                                this.select_question_choice(
                                    &keyboard_key,
                                    question_index,
                                    (next < choice_count).then_some(next),
                                    multiple,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        },
                    )),
                );
            }
            if question.allow_custom && !question.options.is_empty() {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                let multiple = question.multiple;
                let choice_count = question.options.len();
                field = field.child(
                    div().flex().pl(rpx(SPACE_3XL + SPACE_LG)).child(
                        transcript_action(
                            gpui::SharedString::from(format!(
                                "question-other-{request_index}-{question_index}"
                            )),
                            match (question_draft.custom, question.multiple) {
                                (true, true) => "Remove custom answer",
                                (true, false) => "Return to suggested answers",
                                (false, true) => "Add a custom answer",
                                (false, false) => "Use a custom answer instead",
                            },
                            false,
                            window,
                            cx,
                        )
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/edit.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .disabled(submitting)
                        .with_focus_handle(
                            question_draft.choice_focus[question.options.len()].clone(),
                        )
                        .when(submitting, |row| {
                            row.cursor_default().opacity(OPACITY_DISABLED)
                        })
                        .debug_selector(move || {
                            format!("question-other-{request_index}-{question_index}")
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_question_choice(
                                &click_key,
                                question_index,
                                None,
                                multiple,
                                window,
                                cx,
                            );
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if event.is_held {
                                    return;
                                }
                                let choice = match event.keystroke.key.as_str() {
                                    "enter" | "space" => Some(None),
                                    "up" | "left" if !multiple => Some(Some(choice_count - 1)),
                                    "down" | "right" if !multiple => Some(Some(0)),
                                    _ => None,
                                };
                                if let Some(choice) = choice {
                                    this.select_question_choice(
                                        &keyboard_key,
                                        question_index,
                                        choice,
                                        multiple,
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
                );
            }
            if question_draft.custom {
                let field_error = question_has_answer(question, &answers)
                    .then(|| answer_checks[question_index].as_ref().err())
                    .flatten();
                let focused = question_draft.input.focus_handle(cx).is_focused(window);
                field = field.child(attached_validation(
                    gpui::SharedString::from(format!(
                        "question-validation-field-{request_index}-{question_index}"
                    )),
                    project_field_well(focused, field_error.is_some())
                        .debug_selector(move || {
                            format!("question-input-{request_index}-{question_index}")
                        })
                        .child(
                            Input::new(&question_draft.input)
                                .file_drop_overlay(crate::views::components::file_drop_overlay())
                                .disabled(submitting)
                                .aria_label(format!("Answer: {}", question.text))
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .text_color(c::FG())
                                .p_0(),
                        ),
                    field_error.map(String::as_str),
                ));
            }
            group = group.child(field);
        }
        if let Err(error) = validation {
            group = group.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "questions-validation-{request_index}"
                    )))
                    .debug_selector(move || format!("questions-validation-{request_index}"))
                    .role(gpui::Role::Alert)
                    .px(rpx(SPACE_3XL))
                    .pt(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .child(status::label(
                        format!("question-error-status-{request_index}"),
                        StatusKind::Failed,
                        error,
                        ICON_SM,
                        cx,
                    )),
            );
        }
        let cancel_key = key.clone();
        let cancel_id = request.id.clone();
        let submit_key = key.clone();
        let submit_id = request.id.clone();
        group = group.child(
            div()
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_2XL))
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .flex()
                .flex_wrap()
                .justify_between()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(
                    transcript_action(
                        gpui::SharedString::from(format!("questions-cancel-{request_index}")),
                        if reviewing {
                            "Edit answers"
                        } else {
                            "Cancel questions"
                        },
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path(if reviewing {
                                "icons/edit.svg"
                            } else {
                                "icons/close.svg"
                            })
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(move || format!("questions-cancel-{request_index}"))
                    .disabled(submitting)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(group) = this.question_drafts.get_mut(&cancel_key) {
                            if group.reviewing {
                                group.reviewing = false;
                                cx.notify();
                                return;
                            }
                        }
                        this.answer_questions(&cancel_key, cancel_id.clone(), true, cx);
                    })),
                )
                .child(
                    transcript_action(
                        gpui::SharedString::from(format!("questions-submit-{request_index}")),
                        if submitting {
                            "Sending…"
                        } else if reviewing {
                            "Send answers"
                        } else {
                            "Review response"
                        },
                        true,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/check.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(move || format!("questions-submit-{request_index}"))
                    .disabled(!ready)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(group) = this.question_drafts.get_mut(&submit_key) {
                            if !group.reviewing {
                                group.reviewing = true;
                                cx.notify();
                                return;
                            }
                        }
                        this.answer_questions(&submit_key, submit_id.clone(), false, cx);
                    })),
                ),
        );
        group
    }
}
