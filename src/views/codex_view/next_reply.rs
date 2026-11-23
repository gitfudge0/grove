//! Optional suggested-reply generation, keyboard navigation, and draft insertion.

use super::{c, codex, header_control, Action, CodexView};
use crate::views::tokens::ICON_SM;
use gpui::prelude::{
    InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::{App, Context, Focusable, Window};

impl CodexView {
    fn next_reply_mode_eligible(&self, state: &codex::Snapshot) -> bool {
        !self.transcript_only()
            && !self.dashboard_pending
            && self.dashboard_tray.is_none()
            && self.plan_answer.is_none()
            && self.queue_editor.is_none()
            && self.submitted.is_none()
            && !self.edit_request_pending
            && self.edit_confirmation.is_none()
            && state.saved.pending_edit.is_none()
            && state.questions.is_empty()
            && state.approvals.is_empty()
            && state.elicitations.is_empty()
            && state.queued_prompts.is_empty()
            && !state.status.busy()
            && self.images.is_empty()
            && self.reference.is_none()
            && self.pending_image_pastes == 0
            && !self.pending_attachment_picker
            && self.description_scope.is_none()
            && self.review_scope.is_none()
    }

    pub(super) fn dismiss_next_reply(&mut self, cx: &mut Context<Self>) {
        self.next_reply.dismiss();
        self.next_reply_request = None;
        self.next_reply_control = None;
        self.next_reply_task = None;
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(None, c::FG_DIM(), cx);
        });
        cx.notify();
    }

    pub(super) fn next_reply_text(&self, window: &Window, cx: &App) -> Option<String> {
        let state = &self.session.read(cx).state;
        let input = self.input.read(cx);
        if self.next_reply_request.as_deref()
            != grove_core::next_reply::Request::capture(state)
                .as_ref()
                .map(|r| r.key.as_str())
            || !self.next_reply_mode_eligible(state)
            || self.menu.is_some()
            || self.popup.is_some()
            || self.composer_collapsed
            || !(input.focus_handle(cx).is_focused(window)
                || self
                    .next_reply_focus
                    .iter()
                    .any(|focus| focus.is_focused(window)))
            || !input.reply_preview_eligible()
            || self.suggestion_choices(state, cx).is_some()
        {
            return None;
        }
        self.next_reply.visible(&input.value()).map(str::to_owned)
    }

    pub(super) fn accept_next_reply(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(text) = self.next_reply_text(window, cx) else {
            return false;
        };
        if !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(None, c::FG_DIM(), cx);
            input.accept_reply_preview(text.into(), window, cx);
        });
        // Explicit replacements use silent input events; persist the actual accepted draft.
        let draft = self.input.read(cx).value().to_string();
        self.session
            .update(cx, |session, cx| session.send(Action::Draft(draft), cx));
        true
    }

    pub(super) fn move_next_reply(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.next_reply_text(window, cx).is_none() {
            return false;
        }
        self.next_reply.cycle(previous);
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn next_reply_navigation(
        &self,
        previous: bool,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let (id, label, glyph, index) = if previous {
            (
                "next-reply-previous",
                "Previous suggested reply",
                "chev-left",
                0,
            )
        } else {
            ("next-reply-next", "Next suggested reply", "chev-right", 1)
        };
        if self
            .next_reply
            .position()
            .is_none_or(|(_, count)| count < 2)
        {
            return super::super::components::disabled_header_control(id, label)
                .debug_selector(move || id.into())
                .child(crate::icons::icon(glyph, ICON_SM, c::FG_DIM()))
                .into_any_element();
        }
        header_control(id, label)
            .debug_selector(move || id.into())
            .track_focus(&self.next_reply_focus[index])
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.move_next_reply(previous, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space")
                        && !event.keystroke.modifiers.modified()
                        && !event.is_held
                        && this.move_next_reply(previous, window, cx)
                    {
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .child(crate::icons::icon(glyph, ICON_SM, c::FG_DIM()))
            .into_any_element()
    }

    pub(super) fn sync_next_reply(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = grove_core::next_reply::Request::capture(state);
        if self
            .next_reply_request
            .as_ref()
            .is_some_and(|key| request.as_ref().is_none_or(|r| &r.key != key))
            || (self.next_reply_request.is_some() && !self.next_reply_mode_eligible(state))
        {
            self.dismiss_next_reply(cx);
        }
        if let Some(request) = request.filter(|r| self.next_reply_seen.as_ref() != Some(&r.key)) {
            self.next_reply_seen = Some(request.key.clone());
            if self.next_reply_mode_eligible(state) {
                self.next_reply = grove_core::next_reply::Candidate::pending();
                self.next_reply_request = Some(request.key.clone());
                let control = grove_core::next_reply::JobControl::default();
                let cancel = control.0.clone();
                self.next_reply_control = Some(control);
                let generate = self.next_reply_generate;
                self.next_reply_task = Some(cx.spawn(async move |this, cx| {
                    let key = request.key.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { generate(&request, &cancel) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.next_reply_request.as_ref() == Some(&key) {
                            this.next_reply_control = None;
                            let result = match result {
                                Ok(text) => text,
                                Err(error) => {
                                    tracing::debug!(%error, "Next reply generation unavailable");
                                    Vec::new()
                                }
                            };
                            this.next_reply.resolve(result);
                            cx.notify();
                        }
                    });
                }));
            }
        }
        let preview = self
            .next_reply_text(window, cx)
            .map(gpui::SharedString::from);
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(preview, c::FG_DIM(), cx);
        });
    }
}
