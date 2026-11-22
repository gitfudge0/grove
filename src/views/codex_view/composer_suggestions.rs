//! Composer completion and selected token tracking.

use super::*;

impl CodexView {
    pub(super) fn suggestion_choices(
        &self,
        state: &codex::Snapshot,
        cx: &App,
    ) -> Option<(Vec<(String, String)>, &'static str)> {
        let input = self.input.read(cx);
        let text = input.value();
        if self.menu.is_some()
            || self.dismissed_suggestions.as_deref() == Some(text.as_ref())
            || !input.selected_range().is_empty()
        {
            return None;
        }
        let (_, trigger, query) = completion_trigger(&text, input.cursor())?;
        Some(match trigger {
            '@' => (
                self.file_suggestions
                    .iter()
                    .filter(|_| self.file_query == query)
                    .map(|path| (format!("@{path}"), path.clone()))
                    .collect(),
                "Files and folders · relative to this worktree",
            ),
            '$' => (skill_choices(state, query), "Skills"),
            '/' => (slash_choices(state, query), "Commands"),
            _ => return None,
        })
    }

    pub(super) fn move_suggestion(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let state = self.session.read(cx).state.clone();
        let Some((choices, _)) = self.suggestion_choices(&state, cx) else {
            return false;
        };
        if choices.is_empty() {
            return false;
        }
        self.suggestion_index = if previous {
            (self.suggestion_index + choices.len() - 1) % choices.len()
        } else {
            (self.suggestion_index + 1) % choices.len()
        };
        self.suggestion_scroll.scroll_to_item(self.suggestion_index);
        cx.notify();
        cx.stop_propagation();
        true
    }

    pub(super) fn accept_first_suggestion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.submitted.is_some() {
            return false;
        }
        let state = self.session.read(cx).state.clone();
        let Some((choices, _)) = self.suggestion_choices(&state, cx) else {
            return false;
        };
        let Some((value, _)) =
            choices.get(self.suggestion_index.min(choices.len().saturating_sub(1)))
        else {
            return false;
        };
        self.accept_suggestion(value, false, window, cx);
        true
    }

    pub(super) fn validate_picked_tokens(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value();
        self.picked_tokens.retain(|token| {
            let ranges = token.decoration.get_ranges(cx);
            let valid = ranges.first().is_some_and(|range| {
                text.get(range.clone()) == Some(token.value.as_str())
                    && text[..range.start]
                        .chars()
                        .next_back()
                        .is_none_or(char::is_whitespace)
                    && text[range.end..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
            });
            if !valid {
                token.decoration.clear(cx);
            }
            valid
        });
    }

    pub(super) fn accept_suggestion(
        &mut self,
        value: &str,
        execute: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.read(cx);
        let text = input.value();
        let Some((range, _, _)) = completion_trigger(&text, input.cursor()) else {
            return;
        };
        let start = range.start;
        let following_space = text[range.end..]
            .chars()
            .next()
            .filter(|character| character.is_whitespace());
        let suffix =
            if following_space.is_some() || (value.starts_with('@') && value.ends_with('/')) {
                ""
            } else {
                " "
            };
        self.input.update(cx, |input, cx| {
            input.set_selected_range(range.clone(), cx);
            input.replace(format!("{value}{suffix}"), window, cx);
            if let Some(character) = following_space {
                let caret = start + value.len() + character.len_utf8();
                input.set_selected_range(caret..caret, cx);
            }
        });
        self.validate_picked_tokens(cx);
        {
            let decoration = self.input.update(cx, |input, cx| {
                input.create_decorations_collection(
                    vec![TextDecoration::new(
                        start..start + value.len(),
                        gpui::HighlightStyle {
                            underline: Some(gpui::UnderlineStyle {
                                thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                    .to_pixels(window.rem_size()),
                                color: Some(c::BORDER_STRONG()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )],
                    cx,
                )
            });
            self.picked_tokens.push(ComposerToken {
                value: value.into(),
                decoration,
            });
        }
        let replacement = self.input.read(cx).value().to_string();
        if self.queue_editor.is_none() {
            self.session.update(cx, |session, cx| {
                session.send(Action::Draft(replacement), cx);
            });
        }
        if execute && value.starts_with('/') {
            self.run_command(window, cx);
        } else if value.starts_with('@') {
            self.refresh_files(cx);
        }
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn composer_choices(
        &self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some((choices, title)) = self.suggestion_choices(state, cx) else {
            return div().into_any_element();
        };
        let stacked_rows = matches!(title, "Skills" | "Commands");
        let file_rows = title == "Files and folders · relative to this worktree";
        let choice_count = choices.len();
        let mut rows = div()
            .id("codex-composer-option-rows")
            .track_scroll(&self.suggestion_scroll)
            .max_h(rpx(QUEUED_PROMPTS_MAX_H))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if choices.is_empty() {
            rows = rows.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_MUTE())
                    .child("No available choices. Check the connection or select a model first."),
            );
        }
        for (index, (value, label)) in choices.into_iter().enumerate() {
            let keyboard_value = value.clone();
            let (name, description) = label.split_once(" · ").filter(|_| !file_rows).map_or_else(
                || (label.clone(), None),
                |(name, description)| (name.to_string(), Some(description.to_string())),
            );
            rows = rows.child(
                header_control_frame(
                    "codex-composer-option",
                    "Select composer option",
                    c::BG_HOVER(),
                )
                .id(("codex-composer-option", index))
                .aria_label(label.clone())
                .aria_description("Tab to insert this option")
                .when(index == self.suggestion_index, |row| {
                    row.bg(c::BG_HOVER()).text_color(c::FG())
                })
                .debug_selector(move || format!("codex-composer-option-{index}"))
                .w_full()
                .h_auto()
                .min_h(rpx(CONTROL_H))
                .justify_start()
                .gap(rpx(SPACE_SM))
                .when(stacked_rows, gpui::Styled::items_start)
                .px(rpx(SPACE_LG))
                .py(rpx(SPACE_SM))
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .when(file_rows, |row| {
                    let (glyph, color) = super::super::file_browser::file_icon(
                        std::path::Path::new(&label),
                        label.ends_with('/'),
                        false,
                    );
                    row.child(crate::icons::icon(glyph, ICON_SM, color))
                })
                .when(!file_rows && !stacked_rows, |row| {
                    row.child(crate::icons::icon("tool", ICON_SM, c::FG_DIM()))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .gap(rpx(SPACE_SM))
                        .when(stacked_rows, |text| text.flex_col().gap(rpx(SPACE_XS)))
                        .child(
                            div()
                                .min_w_0()
                                .max_w_full()
                                .when(!file_rows, gpui::Styled::flex_shrink_0)
                                .whitespace_normal()
                                .when(file_rows, |name| name.flex_1().truncate())
                                .text_size(rpx(if stacked_rows { TEXT_BODY } else { TEXT_SMALL }))
                                .when(stacked_rows, |name| name.w_full().truncate())
                                .text_color(if stacked_rows || index == self.suggestion_index {
                                    c::FG()
                                } else {
                                    c::FG_DIM()
                                })
                                .group_hover("neutral-control", |text| text.text_color(c::FG()))
                                .child(name),
                        )
                        .when_some(description, |row, description| {
                            row.child(
                                div()
                                    .when(!stacked_rows, gpui::Styled::flex_1)
                                    .when(stacked_rows, gpui::Styled::w_full)
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(if stacked_rows {
                                        c::FG_MUTE()
                                    } else if index == self.suggestion_index {
                                        c::FG()
                                    } else {
                                        c::FG_DIM()
                                    })
                                    .when(!stacked_rows, |text| {
                                        text.group_hover("neutral-control", |text| {
                                            text.text_color(c::FG())
                                        })
                                    })
                                    .child(description),
                            )
                        }),
                )
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                        {
                            this.choose_option(&keyboard_value, window, cx);
                            cx.stop_propagation();
                        } else if event.keystroke.key == "escape" {
                            this.menu = None;
                            this.input.focus_handle(cx).focus(window, cx);
                            cx.notify();
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.choose_option(&value, window, cx);
                })),
            );
        }
        composer_tray(window)
            .when(attached_above, gpui::Styled::rounded_none)
            .id("codex-composer-options")
            .debug_selector(|| "codex-composer-options".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .pb(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(choice_count.to_string()),
                    ),
            )
            .child(rows)
            .into_any_element()
    }

    pub(super) fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let Some((_, '@', query)) = completion_trigger(&text, self.input.read(cx).cursor()) else {
            self.file_suggestions.clear();
            self.file_query.clear();
            self.file_lookup = None;
            return;
        };
        if self.file_lookup.as_deref() == Some(query) {
            return;
        }
        // Keep the visible rows until the replacement search finishes. Clearing
        // here collapses the tray to its empty state on every keystroke.
        self.file_lookup = Some(query.to_owned());
        query.clone_into(&mut self.file_query);
        let query = query.to_owned();
        let cwd = self.session.read(cx).state.saved.cwd.clone();
        cx.spawn(async move |this, cx| {
            let lookup = query.clone();
            let files = cx
                .background_executor()
                .spawn(async move { file_choices(&cwd, &lookup) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.file_lookup.as_deref() == Some(query.as_str()) {
                    this.file_suggestions = files;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
