//! MCP elicitation drafts and cards.

use super::{
    activity_request, attached_validation, c, codex, form_action, literal_notice_text,
    permission_heading, project_field_well, question_choice, rpx, status, Action, CodexView,
    Disableable, ElicitationChoice, ElicitationDraft, ElicitationFieldDraft, ElicitationKind,
    ElicitationMode, ElicitationRequest, HashMap, HashSet, Input, InputState, StatusKind,
};
use crate::views::tokens::{
    ICON_SM, OPACITY_DISABLED, RADIUS_PANEL, SPACE_2XL, SPACE_3XL, SPACE_LG, SPACE_SM, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::AppContext as _;
use gpui::{div, App, Context, Focusable, Window};

impl CodexView {
    pub(super) fn elicitation_key(request: &ElicitationRequest, epoch: u64) -> String {
        format!("{epoch}:{request:?}")
    }
    pub(super) fn sync_elicitations(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: HashSet<_> = state
            .elicitations
            .iter()
            .map(|request| Self::elicitation_key(request, state.interaction_epoch))
            .collect();
        let restore_focus = self.elicitation_drafts.iter().any(|(key, draft)| {
            !keys.contains(key)
                && (draft.activity_anchor.focus.is_focused(window)
                    || draft.fields.iter().any(|field| {
                        field.input.focus_handle(cx).is_focused(window)
                            || field
                                .choice_focus
                                .values()
                                .any(|focus| focus.is_focused(window))
                    }))
        });
        self.elicitation_drafts.retain(|key, _| keys.contains(key));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for request in &state.elicitations {
            let key = Self::elicitation_key(request, state.interaction_epoch);
            if let Some(draft) = self.elicitation_drafts.get_mut(&key) {
                if state.error.is_some() {
                    draft.submitting = false;
                }
                continue;
            }
            let fields = match &request.mode {
                ElicitationMode::Form(form) => form
                    .fields
                    .iter()
                    .map(|field| {
                        let value = field.default.clone().unwrap_or(serde_json::Value::Null);
                        let initial = value.as_str().map_or_else(
                            || {
                                if value.is_number() {
                                    value.to_string()
                                } else {
                                    String::new()
                                }
                            },
                            str::to_owned,
                        );
                        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
                        let subscription = cx.observe(&input, |_, _, cx| cx.notify());
                        let selected = if let Some(values) = value.as_array() {
                            values
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        } else {
                            value.as_str().map(str::to_owned).into_iter().collect()
                        };
                        let choice_focus = match &field.kind {
                            ElicitationKind::Select { options }
                            | ElicitationKind::MultiSelect { options, .. } => options
                                .iter()
                                .map(|option| (option.value.clone(), cx.focus_handle()))
                                .collect(),
                            _ => HashMap::new(),
                        };
                        ElicitationFieldDraft {
                            input,
                            selected,
                            choice_focus,
                            included: field.required || field.default.is_some(),
                            boolean: value.as_bool().unwrap_or(false),
                            _subscription: subscription,
                        }
                    })
                    .collect(),
                ElicitationMode::Url { .. } => Vec::new(),
            };
            self.elicitation_drafts.insert(
                key,
                ElicitationDraft {
                    activity_anchor: activity_request::RequestAnchor::new(
                        &self.agent_controls_scroll,
                        cx,
                    ),
                    fields,
                    submitting: false,
                },
            );
        }
    }
    fn elicitation_field_value(
        field: &grove_core::elicitation::ElicitationField,
        draft: &ElicitationFieldDraft,
        cx: &App,
    ) -> Result<serde_json::Value, String> {
        let text = draft.input.read(cx).value().to_string();
        let value = match &field.kind {
            ElicitationKind::String { .. } => serde_json::Value::String(text),
            ElicitationKind::Number { integer, .. } => {
                let number = serde_json::from_str::<serde_json::Value>(text.trim())
                    .ok()
                    .filter(|v| {
                        v.is_number() && (!integer || v.as_i64().is_some() || v.as_u64().is_some())
                    });
                number.ok_or_else(|| {
                    format!(
                        "{}: enter {}",
                        field.title,
                        if *integer {
                            "a whole number"
                        } else {
                            "a number"
                        }
                    )
                })?
            }
            ElicitationKind::Boolean => serde_json::Value::Bool(draft.boolean),
            ElicitationKind::Select { options } => options
                .iter()
                .find(|option| draft.selected.contains(&option.value))
                .map(|option| serde_json::Value::String(option.value.clone()))
                .ok_or_else(|| format!("{}: choose an option", field.title))?,
            ElicitationKind::MultiSelect { options, .. } => serde_json::Value::Array(
                options
                    .iter()
                    .filter(|option| draft.selected.contains(&option.value))
                    .map(|option| serde_json::Value::String(option.value.clone()))
                    .collect(),
            ),
        };
        Ok(value)
    }
    fn elicitation_field_error(
        field: &grove_core::elicitation::ElicitationField,
        draft: &ElicitationFieldDraft,
        cx: &App,
    ) -> Option<String> {
        let mut content = serde_json::Map::new();
        if draft.included {
            match Self::elicitation_field_value(field, draft, cx) {
                Ok(value) => {
                    content.insert(field.name.clone(), value);
                }
                Err(error) => return Some(error),
            }
        }
        grove_core::elicitation::ElicitationForm {
            fields: vec![field.clone()],
        }
        .validate(&content)
        .err()
        .and_then(|errors| errors.into_iter().next())
        .map(|error| error.message)
    }
    pub(super) fn elicitation_content(
        &self,
        key: &str,
        request: &ElicitationRequest,
        cx: &App,
    ) -> Result<serde_json::Value, String> {
        let ElicitationMode::Form(form) = &request.mode else {
            return Ok(serde_json::Value::Null);
        };
        let draft = self.elicitation_drafts.get(key).ok_or("Request expired")?;
        let mut content = serde_json::Map::new();
        for (field, draft) in form.fields.iter().zip(&draft.fields) {
            if !draft.included {
                continue;
            }
            let value = Self::elicitation_field_value(field, draft, cx)?;
            content.insert(field.name.clone(), value);
        }
        form.validate(&content).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| format!("{}: {}", error.field, error.message))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
    pub(super) fn change_elicitation(
        &mut self,
        key: &str,
        index: usize,
        value: Option<String>,
        multiple: bool,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.elicitation_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        let field = &mut group.fields[index];
        if let Some(value) = value {
            if multiple {
                if !field.selected.remove(&value) {
                    field.selected.insert(value);
                }
            } else {
                field.selected.clear();
                field.selected.insert(value);
            }
        } else if include {
            field.included = !field.included;
        } else {
            field.boolean = !field.boolean;
        }
        cx.notify();
    }
    pub(super) fn respond_elicitation(
        &mut self,
        key: &str,
        id: serde_json::Value,
        action: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .session
            .read(cx)
            .state
            .elicitations
            .iter()
            .find(|request| {
                request.id == id
                    && Self::elicitation_key(request, self.session.read(cx).state.interaction_epoch)
                        == key
            })
        else {
            return;
        };
        let content = if action == "accept" && matches!(request.mode, ElicitationMode::Form(_)) {
            let Ok(content) = self.elicitation_content(key, request, cx) else {
                return;
            };
            Some(content)
        } else {
            None
        };
        let Some(draft) = self.elicitation_drafts.get_mut(key) else {
            return;
        };
        if draft.submitting {
            return;
        }
        draft.submitting = true;
        let epoch = self.session.read(cx).state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::RespondElicitation {
                        id,
                        action: action.to_owned(),
                        content,
                    }),
                },
                cx,
            );
        });
        cx.notify();
    }
    pub(super) fn elicitation_panel(
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
        for (request_index, request) in state.elicitations.iter().enumerate() {
            let key = Self::elicitation_key(request, state.interaction_epoch);
            let draft = &self.elicitation_drafts[&key];
            let source = state.interaction_agent(&request.id).map_or_else(
                || request.server_name.clone(),
                |agent| format!("{agent} · {}", request.server_name),
            );
            let mut group = div()
                .id(("agent-elicitation", request_index))
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
                .aria_label(format!("Input requested by {source}"))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(permission_heading(
                    format!("{} needs {}", if request.server_name == "cua_repl" { "Computer Use" } else { &request.server_name }, if matches!(&request.mode, ElicitationMode::Form(form) if form.fields.is_empty()) { "permission" } else { "your input" }),
                    format!("{} · {source}", state.saved.provider.label()),
                ))
                .child(literal_notice_text(format!("elicitation-message-{request_index}"), &request.message));
            let valid = self.elicitation_content(&key, request, cx);
            match &request.mode {
                ElicitationMode::Url { url, .. } => {
                    let open_url = url.clone();
                    group = group
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(url.clone()),
                        )
                        .child(
                            form_action(
                                gpui::SharedString::from(format!(
                                    "elicitation-open-{request_index}"
                                )),
                                "Open link",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/external-link.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .disabled(draft.submitting)
                            .on_click(move |_, _, cx| {
                                if grove_core::elicitation::validate_elicitation_url(&open_url)
                                    .is_ok()
                                {
                                    cx.open_url(&open_url);
                                }
                            }),
                        )
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(
                                "Finish the requested step in your browser, then confirm below.",
                            ),
                        );
                }
                ElicitationMode::Form(form) => {
                    for (index, field) in form.fields.iter().enumerate() {
                        let field_draft = &draft.fields[index];
                        let field_error = Self::elicitation_field_error(field, field_draft, cx);
                        let mut row = div()
                            .id(gpui::SharedString::from(format!(
                                "elicitation-field-{request_index}-{index}"
                            )))
                            .role(if matches!(field.kind, ElicitationKind::Select { .. }) {
                                gpui::Role::RadioGroup
                            } else {
                                gpui::Role::Group
                            })
                            .aria_label(field.title.clone())
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .pt(rpx(SPACE_LG))
                            .child(div().child(format!(
                                "{}{}",
                                field.title,
                                if field.required {
                                    " (required)"
                                } else {
                                    " (optional)"
                                }
                            )));
                        if let Some(description) = &field.description {
                            row = row.child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(description.clone()),
                            );
                        }
                        if !field.required {
                            row = row.child(self.elicitation_choice(
                                &key,
                                index,
                                request_index,
                                ElicitationChoice::Include,
                                format!("Provide {}", field.title),
                                field_draft.included,
                                draft.submitting,
                                cx,
                            ));
                        }
                        if field_draft.included {
                            let limit = match &field.kind {
                                ElicitationKind::MultiSelect {
                                    min_items,
                                    max_items,
                                    ..
                                } => match (min_items, max_items) {
                                    (Some(min), Some(max)) if min == max => {
                                        Some(format!("Select {min}"))
                                    }
                                    (Some(min), Some(max)) => Some(format!("Select {min}–{max}")),
                                    (Some(min), None) => Some(format!("Select at least {min}")),
                                    (None, Some(max)) => Some(format!("Select up to {max}")),
                                    (None, None) => Some("Select all that apply".to_owned()),
                                },
                                ElicitationKind::Number {
                                    minimum,
                                    maximum,
                                    integer,
                                } => {
                                    let kind = if *integer {
                                        "a whole number"
                                    } else {
                                        "a number"
                                    };
                                    match (minimum, maximum) {
                                        (Some(min), Some(max)) => {
                                            Some(format!("Enter {kind} from {min} to {max}"))
                                        }
                                        (Some(min), None) => {
                                            Some(format!("Enter {kind} of at least {min}"))
                                        }
                                        (None, Some(max)) => {
                                            Some(format!("Enter {kind} up to {max}"))
                                        }
                                        (None, None) => None,
                                    }
                                }
                                _ => None,
                            };
                            if let Some(limit) = limit {
                                row = row.child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(limit),
                                );
                            }
                            match &field.kind {
                                ElicitationKind::Boolean => {
                                    row = row.child(self.elicitation_choice(
                                        &key,
                                        index,
                                        request_index,
                                        ElicitationChoice::Boolean,
                                        "Yes".to_owned(),
                                        field_draft.boolean,
                                        draft.submitting,
                                        cx,
                                    ));
                                }
                                ElicitationKind::Select { options }
                                | ElicitationKind::MultiSelect { options, .. } => {
                                    let multiple =
                                        matches!(field.kind, ElicitationKind::MultiSelect { .. });
                                    for option in options {
                                        row = row.child(self.elicitation_choice(
                                            &key,
                                            index,
                                            request_index,
                                            ElicitationChoice::Option(
                                                option.value.clone(),
                                                multiple,
                                            ),
                                            option.title.clone(),
                                            field_draft.selected.contains(&option.value),
                                            draft.submitting,
                                            cx,
                                        ));
                                    }
                                }
                                ElicitationKind::String { .. } | ElicitationKind::Number { .. } => {
                                    row = row.child(
                                        project_field_well(
                                            field_draft.input.focus_handle(cx).is_focused(window),
                                            field_error.is_some(),
                                        )
                                        .child(
                                            Input::new(&field_draft.input)
                                                .file_drop_overlay(
                                                    crate::views::components::file_drop_overlay(),
                                                )
                                                .aria_label(field.title.clone())
                                                .disabled(draft.submitting)
                                                .appearance(false)
                                                .bordered(false)
                                                .focus_bordered(false)
                                                .p_0(),
                                        ),
                                    );
                                }
                            }
                        }
                        group = group.child(attached_validation(
                            gpui::SharedString::from(format!(
                                "elicitation-validation-field-{request_index}-{index}"
                            )),
                            row,
                            field_error.as_deref(),
                        ));
                    }
                    if let Err(error) = &valid {
                        group = group.child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "elicitation-validation-{request_index}"
                                )))
                                .role(gpui::Role::Alert)
                                .text_size(rpx(TEXT_SMALL))
                                .child(status::label(
                                    format!("elicitation-error-status-{request_index}"),
                                    StatusKind::Failed,
                                    error.clone(),
                                    ICON_SM,
                                    cx,
                                )),
                        );
                    }
                }
            }
            let mut secondary = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            let mut primary = None;
            for (action, label) in [
                ("cancel", "Cancel request"),
                ("decline", "Decline"),
                (
                    "accept",
                    if matches!(request.mode, ElicitationMode::Url { .. }) {
                        "I've completed this"
                    } else if matches!(&request.mode, ElicitationMode::Form(form) if form.fields.is_empty())
                    {
                        "Allow"
                    } else {
                        "Submit"
                    },
                ),
            ] {
                let click_key = key.clone();
                let id = request.id.clone();
                let button = form_action(
                    gpui::SharedString::from(format!("elicitation-{request_index}-{action}")),
                    label,
                    action == "accept",
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(gpui::SharedString::from(format!(
                            "icons/{}.svg",
                            if action == "accept" { "check" } else { "close" }
                        )))
                        .size(rpx(ICON_SM)),
                )
                .disabled(draft.submitting || (action == "accept" && valid.is_err()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.respond_elicitation(&click_key, id.clone(), action, cx);
                }));
                if action == "accept" {
                    primary = Some(button);
                } else {
                    secondary = secondary.child(button);
                }
            }
            panel = panel.child(
                group.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_between()
                        .items_center()
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .pt(rpx(SPACE_LG))
                        .gap(rpx(SPACE_LG))
                        .child(secondary)
                        .children(primary),
                ),
            );
        }
        panel
    }
    fn move_elicitation_choice(
        &mut self,
        key: &str,
        index: usize,
        current: &str,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .session
            .read(cx)
            .state
            .elicitations
            .iter()
            .find(|request| {
                Self::elicitation_key(request, self.session.read(cx).state.interaction_epoch) == key
            })
        else {
            return;
        };
        let ElicitationMode::Form(form) = &request.mode else {
            return;
        };
        let ElicitationKind::Select { options } = &form.fields[index].kind else {
            return;
        };
        let Some(position) = options.iter().position(|option| option.value == current) else {
            return;
        };
        let next = if forward {
            (position + 1) % options.len()
        } else {
            (position + options.len() - 1) % options.len()
        };
        let value = options[next].value.clone();
        if let Some(draft) = self.elicitation_drafts.get(key) {
            if draft.submitting {
                return;
            }
            if let Some(focus) = draft.fields[index].choice_focus.get(&value) {
                focus.focus(window, cx);
            }
        }
        self.change_elicitation(key, index, Some(value), false, false, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn elicitation_choice(
        &self,
        key: &str,
        index: usize,
        request_index: usize,
        choice: ElicitationChoice,
        label: String,
        selected: bool,
        submitting: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let click_key = key.to_owned();
        let keyboard_key = key.to_owned();
        let description = if matches!(&choice, ElicitationChoice::Boolean) {
            "Unchecked sends No.".to_owned()
        } else {
            String::new()
        };
        let (value, multiple, include) = match choice {
            ElicitationChoice::Include => (None, true, true),
            ElicitationChoice::Boolean => (None, true, false),
            ElicitationChoice::Option(value, multiple) => (Some(value), multiple, false),
        };
        let identity = format!(
            "elicitation-choice-{request_index}-{index}-{include}-{}",
            value.as_deref().unwrap_or("toggle")
        );
        let keyboard_value = value.clone();
        let focus = value
            .as_ref()
            .and_then(|value| {
                self.elicitation_drafts[key].fields[index]
                    .choice_focus
                    .get(value)
            })
            .cloned();
        question_choice(
            gpui::SharedString::from(identity),
            label,
            description,
            multiple,
            selected,
        )
        .when(!selected, |row| {
            row.bg(gpui::transparent_black())
                .border_color(gpui::transparent_black())
        })
        .when_some(focus, |row, focus| row.track_focus(&focus))
        .when(submitting, |row| {
            row.cursor_default().opacity(OPACITY_DISABLED)
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.change_elicitation(&click_key, index, value.clone(), multiple, include, cx);
        }))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held
                    && !multiple
                    && matches!(
                        event.keystroke.key.as_str(),
                        "up" | "left" | "down" | "right"
                    )
                {
                    if let Some(current) = &keyboard_value {
                        this.move_elicitation_choice(
                            &keyboard_key,
                            index,
                            current,
                            matches!(event.keystroke.key.as_str(), "down" | "right"),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }
                }
                if !event.is_held && matches!(event.keystroke.key.as_str(), "space" | "enter") {
                    this.change_elicitation(
                        &keyboard_key,
                        index,
                        keyboard_value.clone(),
                        multiple,
                        include,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }),
        )
    }
}
