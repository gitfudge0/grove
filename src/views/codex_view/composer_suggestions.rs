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

/// Rank the whole catalog before applying the popup limit, so an early scattered
/// match cannot hide a later exact or prefix match. Stable sorting preserves the
/// catalog order for ties and for the empty query.
fn fuzzy_completion_choices(
    choices: impl IntoIterator<Item = (String, String)>,
    query: &str,
) -> Vec<(String, String)> {
    let mut matches = choices
        .into_iter()
        .filter_map(|choice| {
            let name = choice.0.trim_start_matches(['/', '$']);
            crate::launcher::fuzzy_score(query, name, "", "").map(|score| (score, choice))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a, _), (b, _)| b.cmp(a));
    matches.into_iter().map(|(_, choice)| choice).collect()
}

pub(super) fn skill_choices(state: &codex::Snapshot, query: &str) -> Vec<(String, String)> {
    fuzzy_completion_choices(
        grove_core::describe_skill::completion('$', "")
            .into_iter()
            .chain(grove_core::review_skill::completion('$', ""))
            .chain(state.skills.iter().filter_map(|skill| {
                let name = skill["name"].as_str()?;
                (name != grove_core::describe_skill::NAME
                    && name != grove_core::review_skill::NAME
                    && (state.saved.provider != codex::Provider::Claude || !host_command(name))
                    && skill["enabled"].as_bool() != Some(false))
                .then(|| {
                    (
                        format!(
                            "{}{name}",
                            if state.saved.provider == codex::Provider::Claude {
                                "/"
                            } else {
                                "$"
                            }
                        ),
                        format!(
                            "{name} · {}",
                            skill["description"].as_str().unwrap_or("Skill")
                        ),
                    )
                })
            })),
        query,
    )
    .into_iter()
    .take(8)
    .collect()
}

/// Host commands first, then (for Claude) the CLI's own commands Grove does not host.
pub(super) fn slash_choices(state: &codex::Snapshot, query: &str) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = [
        (
            "/describe-changes",
            "Describe the selected checkout or attached PR",
        ),
        ("/review", "Review changes for actionable defects"),
        ("/model", "Choose model"),
        ("/effort", "Choose reasoning effort"),
        ("/access", "Choose access level"),
        ("/stop", "Interrupt the current turn"),
        ("/compact", "Compact conversation context"),
        ("/queue", "Send the next queued prompt"),
        ("/clear", "Start a new conversation"),
    ]
    .into_iter()
    .map(|(command, label)| (command.into(), format!("{command} · {label}")))
    .collect();
    if state.saved.provider == codex::Provider::Claude {
        choices.extend(state.skills.iter().filter_map(|skill| {
            let name = skill["name"].as_str()?;
            (!host_command(name) && skill["enabled"].as_bool() != Some(false)).then(|| {
                (
                    format!("/{name}"),
                    format!(
                        "/{name} · {}",
                        skill["description"].as_str().unwrap_or("Claude command")
                    ),
                )
            })
        }));
    }
    fuzzy_completion_choices(choices, query)
}

pub(super) fn host_command(name: &str) -> bool {
    matches!(
        name,
        "review"
            | "describe-changes"
            | "model"
            | "effort"
            | "access"
            | "stop"
            | "queue"
            | "clear"
            | "compact"
    )
}

/// Complete the token around a collapsed caret without replacing later draft text.
pub(super) fn completion_trigger(
    text: &str,
    cursor: usize,
) -> Option<(std::ops::Range<usize>, char, &str)> {
    let prefix = text.get(..cursor)?;
    let start = prefix
        .char_indices()
        .rfind(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    let token = &prefix[start..];
    let trigger = token.chars().next()?;
    if !(matches!(trigger, '@' | '$') || trigger == '/' && start == 0) {
        return None;
    }
    let end = text[cursor..]
        .find(char::is_whitespace)
        .map_or(text.len(), |at| cursor + at);
    Some((start..end, trigger, &token[trigger.len_utf8()..]))
}

pub(super) fn trailing_trigger(text: &str) -> Option<(char, &str)> {
    let token = text.rsplit(char::is_whitespace).next()?;
    let trigger = token.chars().next()?;
    if matches!(trigger, '@' | '$') || (trigger == '/' && token == text) {
        Some((trigger, &token[trigger.len_utf8()..]))
    } else {
        None
    }
}
/// Search only the active checkout, including untracked but non-ignored files.
pub(super) fn file_choices(cwd: &str, query: &str) -> Vec<String> {
    let root = std::path::Path::new(cwd);
    let output = std::process::Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ])
        .current_dir(root)
        .output();
    let mut paths = std::collections::BTreeSet::new();
    if let Some(output) = output.ok().filter(|output| output.status.success()) {
        for path in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8_lossy(path).into_owned();
            if root.join(&path).is_file() {
                paths.insert(path);
            }
        }
    } else {
        // Ordinary directories also support completion. Do not follow directory
        // symlinks, which may escape the checkout or form cycles.
        let mut directories = vec![std::path::PathBuf::new()];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = fs_err::read_dir(root.join(&directory)) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let relative = directory.join(name);
                if kind.is_dir() {
                    directories.push(relative);
                } else if kind.is_file() {
                    paths.insert(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    // Keep directory completion available alongside recursive filename lookup.
    for path in paths.clone() {
        let mut parent = std::path::Path::new(&path).parent();
        while let Some(directory) = parent.filter(|p| !p.as_os_str().is_empty()) {
            paths.insert(format!("{}/", directory.to_string_lossy()));
            parent = directory.parent();
        }
    }
    let mut matches = paths
        .into_iter()
        .filter_map(|path| {
            let name = path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&path);
            let score = crate::launcher::fuzzy_score(query, name, "", "")
                .map(|score| score + 200)
                .or_else(|| crate::launcher::fuzzy_score(query, &path, "", ""))?;
            Some((score, path))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a_score, a), (b_score, b)| b_score.cmp(a_score).then_with(|| a.cmp(b)));
    matches.into_iter().take(8).map(|(_, path)| path).collect()
}
