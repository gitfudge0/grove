//! Checkout Scripts section and its row interactions.

use super::*;

const SCRIPT_VISIBLE_ROWS: usize = 5;
const SCRIPT_DETAILS_MAX_H: f32 = 160.;

/// Preserve the native button's focus and hit target while using Grove's inverse tooltip.
fn script_control_tooltip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    action: gpui_component::button::Button,
) -> gpui::Stateful<gpui::Div> {
    let text = text.into();
    div()
        .id(id)
        .flex_shrink_0()
        .tooltip(move |window, cx| script_tooltip_content(text.clone(), window).build(window, cx))
        .child(action)
}

impl Sidebar {
    pub(super) fn render_checkout_scripts(
        &mut self,
        project: &grove_core::storage::Project,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        use grove_core::storage::ProjectActionKind;
        let entries: Vec<_> = project.scripts.effective_entries().into_iter().collect();
        let show_all = self.checkout_panel.scripts_show_all.contains(&project.path);
        let add_path = project.path.clone();
        let manage_path = project.path.clone();
        let mut section = div()
            .id("checkout-scripts")
            .debug_selector(|| "checkout-scripts".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .px(rpx(SPACE_2XL))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .text_size(rpx(TEXT_SMALL))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .debug_selector(|| "checkout-scripts-heading".into())
                                    .child("Scripts"),
                            ),
                    )
                    .child(script_control_tooltip(
                        "checkout-script-add-tooltip",
                        "Add script",
                        checkout_action("checkout-script-add", "Add script", false, window, cx)
                            .debug_selector(|| "checkout-script-add".into())
                            .w(rpx(CHROME_CONTROL_H))
                            .h(rpx(CHROME_CONTROL_H))
                            .icon_only()
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/plus.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.act(
                                    Action::EditProjectScripts(add_path.clone(), true),
                                    window,
                                    cx,
                                );
                            })),
                    ))
                    .child(
                        checkout_action(
                            "checkout-script-manage",
                            "Manage project scripts",
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "checkout-script-manage".into())
                        .w(rpx(CHROME_CONTROL_H))
                        .h(rpx(CHROME_CONTROL_H))
                        .icon_only()
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/more.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.act(
                                    Action::EditProjectScripts(manage_path.clone(), false),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    ),
            );
        let mut hidden_count = 0;
        for (index, entry) in entries.iter().enumerate() {
            let key = (path.to_owned(), entry.id.clone());
            let script_feedback = self
                .checkout_panel
                .feedback
                .get(path, &format!("script-{}", entry.id));
            let error = self.checkout_panel.script_errors.get(&key).cloned();
            let runs = self
                .runtime
                .read(cx)
                .script_runs
                .iter()
                .filter(|run| {
                    run.project_path == project.path
                        && run.checkout_path == path
                        && run.script_id == entry.id
                })
                .cloned()
                .collect::<Vec<_>>();
            let retain_run = runs.iter().any(|run| {
                let terminal = run.terminal.read(cx);
                !terminal.has_exited()
                    || terminal.exit_code() != Some(0)
                    || self
                        .checkout_panel
                        .script_peek
                        .as_ref()
                        .is_some_and(|peek| peek.id == run.id)
                    || self
                        .checkout_panel
                        .feedback
                        .get(path, &format!("run-{}", run.id))
                        .is_some()
            });
            let has_focus = self
                .checkout_panel
                .script_row_focus
                .get(&key)
                .is_some_and(|(focus, _)| focus.contains_focused(window, cx));
            let open = self.checkout_panel.open_script_preview.as_ref() == Some(&key);
            if index >= SCRIPT_VISIBLE_ROWS
                && !retain_run
                && script_feedback.is_none()
                && error.is_none()
                && !has_focus
                && !open
            {
                hidden_count += 1;
                if !show_all {
                    continue;
                }
            }
            let (command, prompt) = match &entry.kind {
                ProjectActionKind::Shell { command } => (command, false),
                ProjectActionKind::Prompt { template } => (template, true),
            };
            let (row_focus, _) = self
                .checkout_panel
                .script_row_focus
                .entry(key.clone())
                .or_insert_with(|| {
                    let focus = cx.focus_handle();
                    let row_key = key.clone();
                    let observer = cx.on_focus_out(&focus, window, move |this, _, _, cx| {
                        if this.checkout_panel.open_script_preview.as_ref() == Some(&row_key) {
                            this.checkout_panel.open_script_preview = None;
                        }
                        cx.notify();
                    });
                    (focus, observer)
                });
            let row_focus = row_focus.clone();
            let details_focus = self
                .checkout_panel
                .script_details_focus
                .entry(key.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let details_scroll = self
                .checkout_panel
                .script_details_scroll
                .entry(key.clone())
                .or_default()
                .clone();
            let keyboard_scroll = details_scroll.clone();
            let run_focus = self
                .checkout_panel
                .script_run_focus
                .entry(key.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let keyboard_run = run_focus.clone();
            let run_enabled = script_feedback.is_none();
            let name_bounds = self
                .checkout_panel
                .script_name_bounds
                .entry(key.clone())
                .or_default()
                .clone();
            let anchor = name_bounds.get().origin;
            let preview_key = key.clone();
            let preview_focus = details_focus.clone();
            let escape_focus = details_focus.clone();
            let show_preview =
                move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                    this.checkout_panel.open_script_preview = Some(preview_key.clone());
                    preview_focus.focus(window, cx);
                    cx.notify();
                };
            let click_preview = show_preview.clone();
            let project_path = project.path.clone();
            let target = path.to_owned();
            let entry_id = entry.id.clone();
            let dispatch = move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                this.act(
                    Action::RunNamedScript(project_path.clone(), target.clone(), entry_id.clone()),
                    window,
                    cx,
                );
            };
            let click_run = dispatch.clone();
            let dismiss_key = key.clone();
            let selector = format!("checkout-script-{}", entry.id);
            let mut item = div()
                .id(SharedString::from(format!(
                    "checkout-script-item-{}",
                    entry.id
                )))
                .track_focus(&row_focus)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape"
                                if this.checkout_panel.open_script_preview.as_ref()
                                    == Some(&key) =>
                            {
                                this.checkout_panel.open_script_preview = None;
                                escape_focus.focus(window, cx);
                                cx.notify();
                            }
                            "enter" | "space"
                                if !event.is_held && escape_focus.is_focused(window) =>
                            {
                                show_preview(this, window, cx);
                            }
                            "enter" | "space"
                                if !event.is_held
                                    && run_enabled
                                    && keyboard_run.is_focused(window) =>
                            {
                                dispatch(this, window, cx);
                            }
                            "pageup" | "pagedown" if open && escape_focus.is_focused(window) => {
                                let direction = if event.keystroke.key == "pageup" {
                                    1.
                                } else {
                                    -1.
                                };
                                let mut offset = keyboard_scroll.offset();
                                offset.y += keyboard_scroll.bounds().size.height * 0.8 * direction;
                                keyboard_scroll.set_offset(offset);
                                window.refresh();
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                    }),
                )
                .child(
                    crate::views::components::checkout_register_frame()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .child(
                            crate::views::components::focusable_inline_disclosure(
                                SharedString::from(format!("checkout-script-preview-{}", entry.id)),
                                format!(
                                    "Preview {}: {}",
                                    if prompt { "prompt" } else { "command" },
                                    entry.name
                                ),
                            )
                            .flex_1()
                            .min_w_0()
                            .w_auto()
                            .h(rpx(CONTROL_H))
                            .justify_start()
                            .gap(rpx(SPACE_SM))
                            .relative()
                            .track_focus(&details_focus)
                            .aria_expanded(open)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(
                                gpui::canvas(
                                    move |bounds, _, _| name_bounds.set(bounds),
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(crate::views::components::checkout_register_leading().child(
                                icon(
                                    if prompt { "message-circle" } else { "terminal" },
                                    ICON_SM,
                                    c::FG_DIM(),
                                ),
                            ))
                            .child(
                                crate::views::components::checkout_register_label()
                                    .id(SharedString::from(format!(
                                        "checkout-script-name-{}",
                                        entry.id
                                    )))
                                    .debug_selector({
                                        let id = entry.id.clone();
                                        move || format!("checkout-script-name-{id}")
                                    })
                                    .child(entry.name.clone()),
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    click_preview(this, window, cx);
                                },
                            )),
                        )
                        .child(script_control_tooltip(
                            SharedString::from(format!("checkout-script-run-tooltip-{}", entry.id)),
                            if prompt {
                                "Send to the current chat"
                            } else {
                                "Run in this checkout"
                            },
                            checkout_action(
                                SharedString::from(format!("checkout-script-run-{}", entry.id)),
                                format!(
                                    "{} {}",
                                    if prompt { "Send prompt" } else { "Run script" },
                                    entry.name
                                ),
                                false,
                                window,
                                cx,
                            )
                            .debug_selector({
                                let id = entry.id.clone();
                                move || format!("checkout-script-run-{id}")
                            })
                            .icon_only()
                            .w(rpx(CONTROL_H))
                            .h(rpx(CONTROL_H))
                            .rounded(rpx(RADIUS_CONTROL).to_pixels(window.rem_size()))
                            .flex_shrink_0()
                            .with_focus_handle(run_focus)
                            .disabled(!run_enabled)
                            .child(if let Some(feedback) = &script_feedback {
                                crate::views::sidebar::action_feedback::mark(
                                    format!("script-{}", entry.id),
                                    feedback,
                                    ICON_SM,
                                    cx,
                                )
                            } else {
                                icon(if prompt { "send" } else { "play" }, ICON_MD, c::FG_DIM())
                                    .into_any_element()
                            })
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    click_run(this, window, cx);
                                },
                            )),
                        )),
                );
            if open {
                let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
                let width = rpx(CARD_WIDTH)
                    .to_pixels(window.rem_size())
                    .min(window.viewport_size().width - margin * 2.)
                    .max(gpui::px(1.));
                let max_height = rpx(SCRIPT_DETAILS_MAX_H)
                    .to_pixels(window.rem_size())
                    .min(window.viewport_size().height - margin * 2.)
                    .max(gpui::px(1.));
                item = item.child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .anchor(gpui::Anchor::TopRight)
                        .position(gpui::point(anchor.x - margin, anchor.y))
                        .snap_to_window_with_margin(margin)
                        .child(
                            project_field_well(false, false)
                                .id("checkout-script-preview")
                                .debug_selector(|| "checkout-script-details-text".into())
                                .w(width)
                                .h_auto()
                                .px_0()
                                .max_h(max_height)
                                .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                    if this.checkout_panel.open_script_preview.as_ref()
                                        == Some(&dismiss_key)
                                    {
                                        this.checkout_panel.open_script_preview = None;
                                        cx.notify();
                                    }
                                }))
                                .child(
                                    div()
                                        .id("scroll")
                                        .overflow_y_scroll()
                                        .track_scroll(&details_scroll)
                                        .px(rpx(SPACE_SM))
                                        .py(rpx(SPACE_SM))
                                        .text_size(rpx(TEXT_SMALL))
                                        .child(
                                            div()
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .child(entry.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .whitespace_normal()
                                                .when(!prompt, |text| {
                                                    text.font_family(crate::fonts::MONO_FAMILY)
                                                })
                                                .child(command.clone()),
                                        ),
                                ),
                        ),
                ));
            }
            if let Some(feedback) = script_feedback.filter(|_| prompt) {
                item = item.child(
                    div()
                        .pl(rpx(CHECKOUT_LABEL_INSET))
                        .pr(rpx(SPACE_2XL))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(if feedback.completed.is_some() {
                            c::STATUS_SUCCESS()
                        } else {
                            c::STATUS_INFO()
                        })
                        .child(feedback.label),
                );
            }
            if let Some(error) = error {
                item = item.child(
                    div()
                        .px(rpx(SPACE_2XL))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::STATUS_ERROR())
                        .whitespace_normal()
                        .child(error),
                );
            }
            for run in runs {
                item = item.child(self.script_run_row(run, cx));
            }
            section = section.child(item);
        }
        if entries.is_empty() {
            section = section.child(
                div()
                    .px(rpx(SPACE_2XL))
                    .py(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("No scripts yet"),
            );
        } else if show_all || hidden_count > 0 {
            let project_path = project.path.clone();
            let label = if show_all {
                "Show fewer".to_owned()
            } else if hidden_count == 1 {
                "Show 1 more script".to_owned()
            } else {
                format!("Show {hidden_count} more scripts")
            };
            section = section.child(
                checkout_action(
                    "checkout-scripts-disclosure",
                    label.clone(),
                    false,
                    window,
                    cx,
                )
                .debug_selector(|| "checkout-scripts-disclosure".into())
                .aria_expanded(show_all)
                .icon_only()
                .w_full()
                .px(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .gap(rpx(SPACE_SM))
                        .child(div().w(rpx(ICON_LG)).flex_shrink_0().child(
                            icon("chev-down", ICON_SM, c::FG_DIM()).when(show_all, |glyph| {
                                glyph.with_transformation(gpui::Transformation::rotate(
                                    gpui::percentage(0.5),
                                ))
                            }),
                        ))
                        .child(
                            div()
                                .debug_selector(|| "checkout-scripts-disclosure-label".into())
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(label),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.checkout_panel.scripts_show_all.remove(&project_path) {
                        this.checkout_panel
                            .scripts_show_all
                            .insert(project_path.clone());
                    }
                    cx.notify();
                })),
            );
        }
        section
    }
}
