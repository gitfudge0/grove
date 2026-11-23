//! Composer commands that start native Grove workflows.

use super::{host_command, Action, CodexSession, CodexView, CodexViewEvent, ComposerMenu};
use gpui::{Context, Focusable, Window};

impl CodexView {
    pub(super) fn run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let native_command = self.input.read(cx).value().to_string();
        if grove_core::review_skill::is_invocation(&native_command) {
            if self.reference.is_some() || !self.images.is_empty() || self.pending_image_pastes > 0
            {
                self.session.update(cx, |session, cx| { session.state.error = Some("Review changes uses the checkout diff. Remove attachments or the quoted reference before invoking it; your draft is preserved.".into()); cx.notify(); });
                return true;
            }
            let Some(preference) = grove_core::review_skill::invocation(&native_command) else {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(grove_core::review_skill::USAGE.into());
                    cx.notify();
                });
                return true;
            };
            self.session.update(cx, |session, cx| {
                session.state.clear_error();
                cx.notify();
            });
            cx.emit(CodexViewEvent::ReviewChanges(
                preference,
                self.input.focus_handle(cx),
            ));
            // Keep this command draft intact: native actions do not send or replace a provider prompt.
            return true;
        }
        if grove_core::describe_skill::is_invocation(&native_command) {
            if self.reference.is_some() || !self.images.is_empty() || self.pending_image_pastes > 0
            {
                self.session.update(cx, |session, cx| { session.state.error = Some("Describe changes uses the checkout diff. Remove attachments or the quoted reference before invoking it; your draft is preserved.".into()); cx.notify(); });
                return true;
            }
            let Some(preference) = grove_core::describe_skill::invocation(&native_command) else {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(grove_core::describe_skill::USAGE.into());
                    cx.notify();
                });
                return true;
            };
            self.session.update(cx, |session, cx| {
                session.state.clear_error();
                cx.notify();
            });
            cx.emit(CodexViewEvent::DescribeChanges(
                preference,
                self.input.focus_handle(cx),
            ));
            // Keep this command draft intact: native actions do not send or replace a provider prompt.
            return true;
        }
        if self.reference.is_some() {
            return false;
        }
        let text = self.input.read(cx).value().to_string();
        let mut parts = text.split_whitespace();
        let Some(command) = parts.next() else {
            return false;
        };
        if !command.strip_prefix('/').is_some_and(host_command) {
            return false;
        }
        let result = if parts.next().is_some() {
            Err(format!(
                "{command} does not accept arguments in Grove. Run {command} on its own."
            ))
        } else if self.pending_image_pastes > 0 {
            Err("Wait for attached images to finish loading before running a command.".into())
        } else {
            match command {
                "/model" | "/effort" | "/access" => {
                    self.open_menu(
                        match command {
                            "/model" => ComposerMenu::Model,
                            "/effort" => ComposerMenu::Effort,
                            _ => ComposerMenu::Access,
                        },
                        window,
                        cx,
                    );
                    Ok(())
                }
                "/stop" => {
                    let state = &self.session.read(cx).state;
                    if !state.status.busy() || state.turn_id.is_none() {
                        Err("There is no active turn to interrupt.".into())
                    } else if self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref() {
                        Err("The current turn is already stopping.".into())
                    } else {
                        self.interrupt(cx);
                        self.session
                            .read(cx)
                            .state
                            .error
                            .clone()
                            .map_or(Ok(()), Err)
                    }
                }
                "/queue" => self.session.update(cx, CodexSession::resume_queue),
                "/compact" => self.session.update(cx, CodexSession::compact),
                "/clear" => self.session.update(cx, CodexSession::clear_conversation),
                _ => unreachable!(),
            }
        };
        if let Err(error) = result {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
            return true;
        }
        if command == "/clear" {
            self.pending_clear = Some((
                self.session.read(cx).state.cleared_conversations,
                text,
                self.images.clone(),
            ));
            cx.notify();
            return true;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            session.send(Action::Draft(String::new()), cx);
        });
        cx.notify();
        true
    }
}
