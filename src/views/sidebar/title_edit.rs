//! Header editing stays bound to the session that opened it, independently of chat drafts.
use super::*;
use crate::views::components::{
    compact_field_well, disabled_header_control, header_control_with_label_tooltip,
};
use gpui_component::input::{Input, InputEvent};

pub(super) struct TitleEdit {
    pub(super) session: SessionId,
    pub(super) input: Entity<InputState>,
    actions: [FocusHandle; 3],
    _subscription: gpui::Subscription,
}

impl Sidebar {
    pub(super) fn begin_title_edit(
        &mut self,
        id: SessionId,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.runtime.read(cx).registry.read(cx).codex(id).is_none() {
            return;
        }
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Session title");
            input.set_value(title, window, cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, move |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.finish_title_edit(id, true, window, cx);
            }
        });
        input.focus_handle(cx).focus(window, cx);
        self.title_edit = Some(TitleEdit {
            session: id,
            input,
            actions: std::array::from_fn(|_| cx.focus_handle()),
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn finish_title_edit(
        &mut self,
        id: SessionId,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.title_edit.as_ref().filter(|edit| edit.session == id) else {
            return;
        };
        if save {
            let title = edit
                .input
                .read(cx)
                .value()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if title.is_empty() || title.chars().count() > 80 {
                return;
            }
            let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
            if let Some(session) = session {
                session.update(cx, |session, cx| {
                    session.send(grove_core::codex::Action::SetTitle(title), cx);
                });
            }
        }
        self.title_edit = None;
        if let Some(focus) = self.title_edit_focus.get(&id) {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn activate_title_edit_action(
        &mut self,
        id: SessionId,
        action: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action == 2 {
            if self
                .title_edit
                .as_ref()
                .is_none_or(|edit| edit.session != id)
            {
                return;
            }
            let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
            if let Some(session) = session {
                session.update(cx, |session, cx| {
                    session.send(grove_core::codex::Action::RegenerateTitle, cx);
                });
            }
            self.finish_title_edit(id, false, window, cx);
        } else {
            self.finish_title_edit(id, action == 0, window, cx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn title_edit_action(
        &self,
        id: SessionId,
        action: usize,
        name: &'static str,
        label: &'static str,
        glyph: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(edit) = self.title_edit.as_ref() else {
            return div().into_any_element();
        };
        let focus = edit.actions[action].clone();
        if !enabled {
            return disabled_header_control((name, id.raw()), label)
                .debug_selector(move || format!("{name}-{}", id.raw()))
                .aria_description("Unavailable: title must contain 1–80 characters; regeneration requires conversation input and no pending request")
                .child(icon(glyph, ICON_LG, c::FG_DIM())).into_any_element();
        }
        header_control_with_label_tooltip(name, label)
            .id((name, id.raw()))
            .debug_selector(move || format!("{name}-{}", id.raw()))
            .track_focus(&focus)
            .tab_index(if enabled { 0 } else { -1 })
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(icon(glyph, ICON_LG, c::FG_DIM()))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if enabled {
                    this.activate_title_edit_action(id, action, window, cx);
                }
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if enabled
                        && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        && !event.is_held
                    {
                        this.activate_title_edit_action(id, action, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .into_any_element()
    }

    pub(super) fn session_header_title(
        &mut self,
        id: SessionId,
        title: &str,
        editable: bool,
        status: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = div()
            .debug_selector(move || format!("session-header-title-{}", id.raw()))
            .min_w_0()
            .truncate()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c::FG())
            .child(self.session_title_label(id, title, "session-header-title-shimmer", cx));
        if !editable {
            return div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(label)
                .child(status)
                .into_any_element();
        }
        let focus = self
            .title_edit_focus
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let row = div()
            .id(("session-title-row", id.raw()))
            .group("session-title")
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM));
        if let Some(edit) = self.title_edit.as_ref().filter(|edit| edit.session == id) {
            let input = edit.input.clone();
            let focused = input.focus_handle(cx).is_focused(window);
            let value = input
                .read(cx)
                .value()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let valid = !value.is_empty() && value.chars().count() <= 80;
            return row
                .flex_1()
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.finish_title_edit(id, false, window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }),
                )
                .child(
                    compact_field_well(focused).id(("session-title-input-well", id.raw())).on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).flex_1().child(
                        Input::new(&input).file_drop_overlay(crate::views::components::file_drop_overlay())
                            .aria_label("Session title")
                            .aria_description(
                                "Enter a title of 1–80 characters. Enter saves; Escape cancels.",
                            )
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .p_0()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG()),
                    ),
                )
                .child(self.title_edit_action(
                    id,
                    0,
                    "save-session-title",
                    "Save title",
                    "check",
                    valid,
                    cx,
                ))
                .child(self.title_edit_action(
                    id,
                    1,
                    "cancel-session-title",
                    "Cancel title edit",
                    "close",
                    true,
                    cx,
                ))
                .child(
                    self.title_edit_action(
                        id,
                        2,
                        "regenerate-session-title",
                        "Regenerate title",
                        "refresh",
                        self.runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .codex(id)
                            .is_some_and(|session| {
                                !session.read(cx).state.title_generation_pending
                                    && session
                                        .read(cx)
                                        .state
                                        .saved
                                        .items
                                        .iter()
                                        .any(|item| item["type"] == "userMessage")
                            }),
                        cx,
                    ),
                )
                .child(status)
                .into_any_element();
        }
        let title = title.to_owned();
        let keyboard_title = title.clone();
        row.child(label)
            .child(status)
            .child(
                header_control_with_label_tooltip("edit-session-title", "Edit session title")
                    .id(("edit-session-title", id.raw()))
                    .debug_selector(move || format!("edit-session-title-{}", id.raw()))
                    .track_focus(&focus)
                    .tab_index(0)
                    .opacity(0.)
                    .group_hover("session-header", |style| style.opacity(1.))
                    .focus(|style| style.opacity(1.))
                    .focus_visible(|style| style.opacity(1.).bg(c::BG_HOVER()))
                    .child(icon("edit", ICON_LG, c::FG_DIM()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.begin_title_edit(id, &title, window, cx);
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                this.begin_title_edit(id, &keyboard_title, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        },
                    )),
            )
            .into_any_element()
    }
}
