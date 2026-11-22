//! Project script registration, editing and lifecycle controls.

use super::*;

impl ProjectPanel {
    pub(super) fn toggle_script(&mut self, editor: ScriptEditor, cx: &mut Context<Self>) {
        self.expanded_script = if self.expanded_script.as_ref() == Some(&editor) {
            None
        } else {
            Some(editor)
        };
        cx.notify();
    }

    pub(in crate::views::sidebar) fn open_scripts(
        &mut self,
        add: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if add {
            self.add_script(window, cx);
        } else {
            self.add_script_focus.focus(window, cx);
        }
        self.script_reveal.set(Some(add));
        cx.notify();
    }

    pub(super) fn add_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = format!(
            "script-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("New script"));
        let body = cx.new(|cx| InputState::new(window, cx).multi_line(true));
        self.script_focus.insert(id.clone(), cx.focus_handle());
        self.expanded_script = Some(ScriptEditor::Action(id.clone()));
        name.read(cx).focus_handle(cx).focus(window, cx);
        self.observers
            .push(cx.observe(&name, |_, _, cx| cx.notify()));
        self.observers
            .push(cx.observe(&body, |_, _, cx| cx.notify()));
        self.script_entries.push((id, false, name, body));
        self.script_reveal.set(Some(true));
        cx.notify();
    }

    pub(super) fn change_script(
        &mut self,
        id: &str,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.script_entries.iter().position(|entry| entry.0 == id) else {
            return;
        };
        match action {
            "remove" => {
                self.script_entries.remove(index);
                self.script_focus.remove(id);
                if self.expanded_script == Some(ScriptEditor::Action(id.into())) {
                    self.expanded_script = None;
                }
                let focus = self
                    .script_entries
                    .get(index)
                    .or_else(|| self.script_entries.last())
                    .and_then(|entry| self.script_focus.get(&entry.0))
                    .unwrap_or(&self.add_script_focus);
                focus.focus(window, cx);
            }
            "shell" | "prompt" => {
                self.script_entries[index].1 = action == "prompt";
            }
            _ => {}
        }
        cx.notify();
    }

    fn script_row(
        &self,
        key: String,
        editor: ScriptEditor,
        focus: &FocusHandle,
        details: ScriptRowDetails,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let ScriptRowDetails {
            title,
            preview,
            glyph,
        } = details;
        let expanded = self.expanded_script.as_ref() == Some(&editor);
        let keyboard_editor = editor.clone();
        let mut row = div()
            .id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || key.clone())
            .role(gpui::Role::Button)
            .aria_expanded(expanded)
            .aria_description(preview.clone())
            .aria_label(format!(
                "{title}, {}",
                if expanded {
                    "collapse editor"
                } else {
                    "expand editor"
                }
            ))
            .track_focus(focus)
            .tab_index(0)
            .cursor_pointer()
            .group("neutral-control")
            .w_full()
            .min_w_0()
            .min_h(rpx(if glyph.is_some() {
                SCRIPT_ROW_MIN_H
            } else {
                LIFECYCLE_ROW_MIN_H
            }))
            .py(rpx(SPACE_LG))
            .gap(rpx(SPACE_LG))
            .flex()
            .items_center()
            .text_color(if expanded { c::FG() } else { c::FG_DIM() })
            .when(expanded, |s| s.bg(c::BG_HOVER()))
            .hover(|s| s.bg(c::BG_HOVER()).text_color(c::FG()))
            .focus_visible(|s| s.bg(c::BG_HOVER()));
        if let Some(glyph) = glyph {
            row = row.child(icon(
                glyph,
                ICON_MD,
                if expanded { c::FG() } else { c::FG_DIM() },
            ));
        }
        let copy = div()
            .flex_1()
            .min_w_0()
            .flex()
            .when(glyph.is_some(), |s| s.flex_col().gap(rpx(SPACE_XS)))
            .when(glyph.is_none(), |s| s.items_center().gap(rpx(SPACE_LG)))
            .child(
                div()
                    .when(glyph.is_none(), |s| {
                        s.w(rpx(LIFECYCLE_LABEL_W)).flex_shrink_0()
                    })
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .truncate()
                    .child(title),
            )
            .when(!expanded, |s| {
                s.child(
                    div()
                        .when(glyph.is_none(), |s| s.flex_1().min_w_0())
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(if expanded { c::FG() } else { c::FG_DIM() })
                        .group_hover("neutral-control", |text| text.text_color(c::FG()))
                        .truncate()
                        .child(preview),
                )
            });
        row.child(copy)
            .child(icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                if expanded { c::FG() } else { c::FG_DIM() },
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_script(editor.clone(), cx)))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        window.prevent_default();
                        this.toggle_script(keyboard_editor.clone(), cx);
                        cx.stop_propagation();
                    }
                }),
            )
    }

    fn render_lifecycle(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let mut group = div().flex().flex_col();
        for (label, index, editor, focus_index) in [
            ("Setup", 1, ScriptEditor::Setup, 0),
            ("Teardown", 3, ScriptEditor::Teardown, 1),
        ] {
            let value = self.fields[index].read(cx).value().to_string();
            let mut item =
                div()
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .child(self.script_row(
                        format!("lifecycle-{label}"),
                        editor.clone(),
                        &self.lifecycle_focus[focus_index],
                        ScriptRowDetails {
                            title: label.into(),
                            preview: script_preview(&value, "Not configured"),
                            glyph: None,
                        },
                        cx,
                    ));
            if self.expanded_script.as_ref() == Some(&editor) {
                item = item.child(div().pb(rpx(SPACE_2XL)).child(script_field(
                    if index == 1 {
                        "Setup script"
                    } else {
                        "Teardown script"
                    },
                    &self.fields[index],
                    window,
                    cx,
                )));
            }
            group = group.child(item);
        }
        group
    }

    pub(super) fn render_scripts(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let add = project_action(
            "script-add".into(),
            "Add script",
            Some("icons/plus.svg"),
            ProjectActionStyle::Mini,
            window,
            cx,
        )
        .with_focus_handle(self.add_script_focus.clone())
        .on_click(cx.listener(|this, _, window, cx| this.add_script(window, cx)));
        let mut list = div()
            .id("project-scripts-register")
            .debug_selector(|| "project-scripts-register".into())
            .anchor_scroll(Some(self.scripts_anchor.clone()))
            .on_prepaint({
                let reveal = self.script_reveal.clone();
                let register = self.scripts_anchor.clone();
                let draft = self.draft_anchor.clone();
                move |_, window, cx| {
                    // The entry anchors must have completed their first layout before scrolling.
                    if let Some(add) = reveal.take() {
                        if add {
                            draft.scroll_to(window, cx);
                        } else {
                            register.scroll_to(window, cx);
                        }
                    }
                }
            })
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(div().flex_1().min_w_0().child(register_heading(
                        "Scripts",
                        "Setup, teardown, shell commands and agent prompts.",
                    )))
                    .child(add),
            );
        if self.draft_appearance.is_some() {
            list = list.child(self.render_lifecycle(window, cx));
        }
        for (id, prompt, name, body) in &self.script_entries {
            let editor = ScriptEditor::Action(id.clone());
            let expanded = self.expanded_script.as_ref() == Some(&editor);
            let title = if expanded {
                let saved = match &self.page {
                    Page::Edit(path) => cx
                        .global::<SettingsState>()
                        .store
                        .script_project(path)
                        .scripts
                        .effective_entries()
                        .iter()
                        .any(|entry| &entry.id == id),
                    _ => false,
                };
                if saved { "Edit script" } else { "New script" }.into()
            } else {
                format!(
                    "{} · {}",
                    name.read(cx).value(),
                    if *prompt { "Prompt" } else { "Shell" }
                )
            };
            let focus = self.script_focus.get(id).unwrap_or(&self.add_script_focus);
            let mut item = div()
                .id(gpui::SharedString::from(format!("script-item-{id}")))
                .when(expanded, |item| {
                    item.anchor_scroll(Some(self.draft_anchor.clone()))
                })
                .border_b_1()
                .border_color(c::BORDER_SOFT())
                .child(self.script_row(
                    format!("script-row-{id}"),
                    editor,
                    focus,
                    ScriptRowDetails {
                        title,
                        preview: script_preview(body.read(cx).value().as_ref(), "No content yet"),
                        glyph: Some(if *prompt { "reply" } else { "terminal" }),
                    },
                    cx,
                ));
            if expanded {
                let mut types = div().flex().items_center().gap(rpx(SPACE_SM));
                for (action, label) in [("shell", "Shell"), ("prompt", "Prompt")] {
                    let id = id.clone();
                    let key_id = id.clone();
                    types = types.child(
                        crate::views::components::settings_control(
                            gpui::SharedString::from(format!("script-{id}-{action}")),
                            label,
                            *prompt == (action == "prompt"),
                            true,
                        )
                        .child(crate::icons::icon(
                            if action == "shell" {
                                "terminal"
                            } else {
                                "reply"
                            },
                            ICON_SM,
                            c::FG_DIM(),
                        ))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.change_script(&id, action, window, cx);
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    window.prevent_default();
                                    this.change_script(&key_id, action, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    );
                }
                let validation = self
                    .script_error
                    .as_ref()
                    .filter(|(error_id, _, _)| error_id == id);
                let edit = div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .py(rpx(SPACE_2XL))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("Type"),
                            )
                            .child(types),
                    )
                    .child(
                        div()
                            .debug_selector(|| "script-execution-hint".into())
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if *prompt {
                                "Sends literal content to the current chat."
                            } else {
                                "Runs in a terminal for the current checkout."
                            }),
                    )
                    .child(project_form_field(
                        "script-entry-name",
                        "Name",
                        name,
                        validation
                            .filter(|(_, name, _)| *name)
                            .map(|(_, _, message)| message.as_str()),
                        window,
                        cx,
                    ))
                    .child(crate::views::components::attached_validation(
                        gpui::SharedString::from(format!("script-body-{id}")),
                        script_field(
                            if *prompt { "Prompt" } else { "Shell command" },
                            body,
                            window,
                            cx,
                        ),
                        validation
                            .filter(|(_, name, _)| !*name)
                            .map(|(_, _, message)| message.as_str()),
                    ));
                let remove_id = id.clone();
                let controls = div().flex().flex_wrap().gap(rpx(SPACE_SM)).child(
                    project_action(
                        format!("script-{id}-remove").into(),
                        "Remove script",
                        Some("icons/trash.svg"),
                        ProjectActionStyle::Quiet,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.change_script(&remove_id, "remove", window, cx);
                    })),
                );
                item = item.child(edit.child(controls));
            }
            list = list.child(item);
        }
        list
    }
}
