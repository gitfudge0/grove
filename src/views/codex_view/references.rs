//! Transcript selection, detailed draft previews, and canonical reference prompts.

use super::{
    action_key, c, codex, header_control_with_label_tooltip, rpx, transcript_action, Action,
    CodexView, TextSelectionSnapshot, REFERENCE_EXPANDED_H, REFERENCE_PREVIEW_CHARS,
    REFERENCE_PREVIEW_H,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, ICON_MD, ICON_SM, RADIUS_PANEL, SPACE_2XL, SPACE_LG, SPACE_SM, TEXT_BODY,
    TEXT_MICRO,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{div, Context, Focusable, Window};
use gpui_component::WindowExt as _;

pub(super) const COMMAND_OUTPUT_REFERENCE_ROLE: &str = "Command output";

pub(super) fn new_reference_id() -> String {
    static NEXT_REFERENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_REFERENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Decode only complete canonical reference prompts. All other user text stays literal.
pub(super) fn reference_prompt(text: &str) -> Option<(codex::ReferenceDraft, &str)> {
    let body = text.strip_prefix("Referenced transcript selection (quoted data):\n")?;
    let (json, reply) = body.split_once("\n\nUser reply:\n")?;
    let reference: codex::ReferenceDraft = serde_json::from_str(json).ok()?;
    if reference.id.is_empty() || reference.sources.is_empty() || reference.serialize(reply) != text
    {
        return None;
    }
    Some((reference, reply))
}

pub(super) fn reference_prompt_copy(text: &str) -> String {
    let Some((reference, reply)) = reference_prompt(text) else {
        return text.to_owned();
    };
    let roles = reference
        .sources
        .iter()
        .map(|source| source.role.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "Referenced selection · {roles}\n{}\n\n{}",
        reference.text, reply
    )
}

pub(super) fn reference_prompt_markdown(text: &str) -> String {
    let Some((reference, reply)) = reference_prompt(text) else {
        return text.to_owned();
    };
    // A fence longer than any run in the literal passage prevents selected
    // Markdown/code from becoming authored transcript chrome.
    let longest = reference
        .text
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let roles = reference
        .sources
        .iter()
        .map(|source| source.role.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "Referenced selection · {roles}\n\n{fence}text\n{}\n{fence}\n\n{}",
        reference.text, reply
    )
}

impl CodexView {
    pub(super) fn remove_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dashboard_pending {
            return;
        }
        if let Some(editor) = self.queue_editor.as_mut() {
            editor.reference = None;
        } else {
            self.set_reference(None, cx);
        }
        self.reference_expanded = false;
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn set_reference(
        &mut self,
        reference: Option<codex::ReferenceDraft>,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending {
            return;
        }
        self.reference.clone_from(&reference);
        self.reference_expanded = false;
        self.session.update(cx, |session, cx| {
            session.state.saved.draft_reference.clone_from(&reference);
            session.send(Action::DraftReference(reference), cx);
        });
        cx.notify();
    }

    pub(super) fn selection_reference(
        &self,
        snapshot: &TextSelectionSnapshot,
        state: &codex::Snapshot,
    ) -> Option<codex::ReferenceDraft> {
        if snapshot.is_selecting
            || snapshot.text.trim().is_empty()
            || snapshot.spans.is_empty()
            || self.queue_editor.is_some()
            || self.plan_answer.is_some()
            || self.edit_confirmation.is_some()
            || self.menu.is_some()
            || self.popup.is_some()
        {
            return None;
        }
        let mut sources = Vec::new();
        for span in &snapshot.spans {
            let source = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                let role = match item["type"].as_str()? {
                    "userMessage" => "User",
                    "agentMessage" => "Assistant",
                    _ => return None,
                };
                let key = format!("{}:{}", state.saved.thread_id.as_deref().unwrap_or(""), id);
                let owner = self.selection_views.get(&key)?;
                (owner.entity_id() == span.owner).then(|| codex::ReferenceSource {
                    message_id: id.to_owned(),
                    role: role.to_owned(),
                })
            })?;
            if sources.last() != Some(&source) {
                sources.push(source);
            }
        }
        Some(codex::ReferenceDraft {
            id: String::new(),
            thread_id: state.saved.thread_id.clone(),
            sources,
            text: snapshot.text.clone(),
        })
    }

    pub(super) fn attach_selection(
        &mut self,
        expected: &TextSelectionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = window.text_selection_snapshot(cx) else {
            return;
        };
        if &current != expected {
            return;
        }
        let state = self.session.read(cx).state.shared();
        let Some(mut reference) = self.selection_reference(&current, &state) else {
            return;
        };
        reference.id = new_reference_id();
        self.set_reference(Some(reference), cx);
        window.clear_text_selection(cx);
        self.selection_dismissed = None;
        self.set_composer_collapsed(false, cx);
        self.input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn render_selection_actions(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        window.set_text_selection_action_bounds(cx.entity_id(), None, cx);
        let Some(snapshot) = window.text_selection_snapshot(cx) else {
            self.selection_dismissed = None;
            return None;
        };
        if snapshot.is_selecting {
            self.selection_dismissed = None;
            return None;
        }
        if self.selection_dismissed.as_ref() == Some(&snapshot) {
            return None;
        }
        self.selection_reference(&snapshot, state)?;
        let pane = self.transcript_bounds.get();
        if !pane.contains(&snapshot.endpoint) {
            return None;
        }
        let control = rpx(CHROME_CONTROL_H).to_pixels(window.rem_size());
        let gap = rpx(SPACE_SM).to_pixels(window.rem_size());
        let width = control + gap + control;
        let mut left = snapshot.endpoint.x + gap;
        if left + width > pane.right() {
            left = snapshot.endpoint.x - gap - width;
        }
        left = left
            .max(pane.left())
            .min((pane.right() - width).max(pane.left()));
        let top = snapshot
            .endpoint
            .y
            .min(pane.bottom() - control)
            .max(pane.top());
        let keyboard = snapshot.clone();
        let action_owner = cx.entity_id();
        let copy_markdown = |window: &mut Window, cx: &mut gpui::App| {
            window.dispatch_action(Box::new(gpui_component::text::CopyAsMarkdown), cx);
        };
        let copy_focus = self.ensure_control_focus("transcript-selection:markdown", window, cx);
        let reply =
            header_control_with_label_tooltip("codex-selection-reply", "Reply to selection")
                .debug_selector(|| "codex-selection-reply".into())
                .bg(c::SURFACE_RAISED())
                .border_1()
                .border_color(c::BORDER_SOFT())
                .track_focus(
                    &self
                        .selection_reply_focus
                        .clone()
                        .tab_index(0)
                        .tab_stop(true),
                )
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(crate::icons::icon("reply", ICON_MD, c::FG()))
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.attach_selection(&keyboard, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.attach_selection(&snapshot, window, cx);
                    cx.stop_propagation();
                }));
        let copy =
            header_control_with_label_tooltip("codex-selection-copy-markdown", "Copy as Markdown")
                .debug_selector(|| "codex-selection-copy-markdown".into())
                .bg(c::SURFACE_RAISED())
                .border_1()
                .border_color(c::BORDER_SOFT())
                .track_focus(&copy_focus)
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(crate::icons::icon("copy", ICON_MD, c::FG()))
                .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                    if action_key(event) {
                        copy_markdown(window, cx);
                        cx.stop_propagation();
                    }
                })
                .on_click(move |_, window, cx| {
                    copy_markdown(window, cx);
                    cx.stop_propagation();
                });
        Some(
            div()
                .absolute()
                .left(left - pane.left())
                .top(top - pane.top())
                .flex()
                .gap(rpx(SPACE_SM))
                .child(
                    gpui::canvas(
                        move |bounds, window, cx| {
                            window.set_text_selection_action_bounds(action_owner, Some(bounds), cx);
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(reply)
                .child(copy)
                .into_any_element(),
        )
    }

    pub(super) fn render_reference_preview(
        &self,
        attached_above: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let quote = if let Some(editor) = self.queue_editor.as_ref() {
            editor.reference.as_ref()?
        } else {
            self.reference.as_ref()?
        };
        let full = quote.text.clone();
        let needs_disclosure =
            full.lines().count() > 3 || full.chars().count() > REFERENCE_PREVIEW_CHARS;
        let preview = if self.reference_expanded {
            full.clone()
        } else {
            let lines: Vec<_> = full.lines().take(3).collect();
            let mut preview: String = lines
                .join("\n")
                .chars()
                .take(REFERENCE_PREVIEW_CHARS)
                .collect();
            if needs_disclosure {
                preview.push('…');
            }
            preview
        };
        let provider = self.session.read(cx).state.saved.provider;
        let mut roles = Vec::new();
        for source in &quote.sources {
            let role = match source.role.as_str() {
                "Assistant" => provider.label(),
                COMMAND_OUTPUT_REFERENCE_ROLE => COMMAND_OUTPUT_REFERENCE_ROLE,
                _ => "You",
            };
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
        let label = if roles == [COMMAND_OUTPUT_REFERENCE_ROLE] {
            "Referenced command output".to_owned()
        } else {
            format!("Replying to {}", roles.join(" · "))
        };
        Some(
            div()
                .id("codex-reference-preview")
                .debug_selector(|| "codex-reference-preview".into())
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px(rpx(SPACE_2XL))
                .pt(rpx(SPACE_LG))
                .pb(rpx(SPACE_LG))
                .when(!attached_above, |preview| {
                    preview.rounded_t(rpx(RADIUS_PANEL))
                })
                .border_1()
                .border_b_0()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(crate::icons::icon("reply", ICON_MD, c::FG_DIM()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(label),
                        )
                        .child(
                            (if self.dashboard_pending {
                                super::super::components::disabled_header_control(
                                    "codex-remove-reference",
                                    "Remove reference",
                                )
                            } else {
                                super::super::components::header_control(
                                    "codex-remove-reference",
                                    "Remove reference",
                                )
                            })
                            .debug_selector(|| "codex-remove-reference".into())
                            .track_focus(
                                &self
                                    .reference_remove_focus
                                    .clone()
                                    .tab_index(if self.dashboard_pending { -1 } else { 0 })
                                    .tab_stop(!self.dashboard_pending)
                                    .tab_stop(!self.dashboard_pending),
                            )
                            .tab_index(if self.dashboard_pending { -1 } else { 0 })
                            .tab_stop(!self.dashboard_pending)
                            .focus_visible(|style| style.bg(c::SURFACE_RAISED()))
                            .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if action_key(event) {
                                        this.remove_reference(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.remove_reference(window, cx);
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .id("codex-reference-text")
                        .debug_selector(|| "codex-reference-text".into())
                        .min_w_0()
                        .max_h(rpx(if self.reference_expanded {
                            REFERENCE_EXPANDED_H
                        } else {
                            REFERENCE_PREVIEW_H
                        }))
                        .overflow_y_scroll()
                        .border_l_2()
                        .border_color(c::BORDER())
                        .pl(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FG())
                        .children(
                            preview
                                .lines()
                                .map(|line| div().child(line.to_owned()))
                                .collect::<Vec<_>>(),
                        ),
                )
                .when(needs_disclosure, |preview| {
                    preview.child(
                        transcript_action(
                            "codex-reference-disclosure",
                            if self.reference_expanded {
                                "Show less"
                            } else {
                                "Show full quote"
                            },
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "codex-reference-disclosure".into())
                        .with_focus_handle(self.reference_disclosure_focus.clone())
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) {
                                this.reference_expanded = !this.reference_expanded;
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .icon(
                            gpui_component::Icon::default()
                                .path(if self.reference_expanded {
                                    "icons/arrow-up.svg"
                                } else {
                                    "icons/chev-down.svg"
                                })
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reference_expanded = !this.reference_expanded;
                            cx.notify();
                        })),
                    )
                })
                .into_any_element(),
        )
    }
}
