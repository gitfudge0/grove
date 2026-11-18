//! Dashboard shortcuts into the existing provider request controls.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivityRequest {
    pub cue: String,
    pub target: ActivityRequestTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivityRequestTarget {
    epoch: u64,
    pending: Vec<RequestIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RequestIdentity {
    Approval(String),
    Question { key: String, instance: u64 },
    Elicitation(String),
}

impl ActivityRequestTarget {
    fn capture(state: &codex::Snapshot) -> Option<Self> {
        if !matches!(state.session_status(), "Approval needed" | "Answer needed") {
            return None;
        }
        let epoch = state.interaction_epoch;
        let approvals = state
            .approvals
            .iter()
            .filter(|request| actionable_approval(request))
            .map(|request| RequestIdentity::Approval(CodexView::approval_key(request, epoch)));
        let elicitations = state.elicitations.iter().map(|request| {
            RequestIdentity::Elicitation(CodexView::elicitation_key(request, epoch))
        });
        let questions = state
            .questions
            .iter()
            .map(|request| RequestIdentity::Question {
                key: question_key(state, request),
                instance: request.instance(),
            });
        let pending: Vec<_> = approvals.chain(elicitations).chain(questions).collect();
        (!pending.is_empty()).then_some(Self { epoch, pending })
    }

    pub(crate) fn is_current(&self, state: &codex::Snapshot) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }
}

fn actionable_approval(request: &serde_json::Value) -> bool {
    (request["id"].is_string() || request["id"].is_number())
        && !grove_core::controls::approval_choices(request).is_empty()
}

fn request_headline(text: &str) -> Option<String> {
    const LIMIT: usize = 128;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= LIMIT {
        return Some(text);
    }
    let prefix: String = text.chars().take(LIMIT - 1).collect();
    let end = prefix.rfind(' ').unwrap_or(prefix.len());
    Some(format!("{}…", &prefix[..end]))
}

pub(crate) fn activity_request(state: &codex::Snapshot) -> Option<ActivityRequest> {
    let target = ActivityRequestTarget::capture(state)?;
    let cue = if target.pending.len() > 1 {
        format!("{} pending requests", target.pending.len())
    } else {
        match &target.pending[0] {
            RequestIdentity::Approval(_) => {
                let request = state
                    .approvals
                    .iter()
                    .find(|request| actionable_approval(request))?;
                request["params"]["reason"]
                    .as_str()
                    .and_then(request_headline)
                    .or_else(|| request_headline(&grove_core::controls::approval_title(request)))
                    .unwrap_or_else(|| "Approval needed".into())
            }
            RequestIdentity::Question { .. } => {
                let request = state.questions.first()?;
                request
                    .title
                    .as_deref()
                    .and_then(request_headline)
                    .or_else(|| {
                        let first = request.questions.first()?;
                        request_headline(&first.header).or_else(|| request_headline(&first.text))
                    })
                    .unwrap_or_else(|| "Answer needed".into())
            }
            RequestIdentity::Elicitation(_) => {
                request_headline(&state.elicitations.first()?.message)
                    .unwrap_or_else(|| "Input needed".into())
            }
        }
    };
    Some(ActivityRequest { cue, target })
}

#[derive(Clone)]
pub(super) struct RequestAnchor {
    pub(super) focus: FocusHandle,
    pub(super) scroll: gpui::ScrollAnchor,
}

pub(super) struct PendingRequestFocus {
    target: ActivityRequestTarget,
    origin: Option<FocusHandle>,
}

impl RequestAnchor {
    pub(super) fn new(scroll: &gpui::ScrollHandle, cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            scroll: gpui::ScrollAnchor::for_handle(scroll.clone()),
        }
    }
}

impl CodexView {
    pub(crate) fn focus_activity_request(
        &mut self,
        target: ActivityRequestTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript_only() && target.is_current(&self.session.read(cx).state) {
            self.activity_request_focus = Some(PendingRequestFocus {
                target,
                origin: window.focused(cx),
            });
            self.set_composer_collapsed(false, cx);
            cx.notify();
        }
    }

    // The shortcut may materialize a conversation for the first time. Its
    // exact controls and scroll anchors are available only after rendering.
    pub(super) fn reveal_activity_request(
        &mut self,
        history_start: usize,
        state: &codex::Snapshot,
        transcript: &TranscriptRenderCache,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(PendingRequestFocus { target, origin }) = self.activity_request_focus.take()
        else {
            return history_start;
        };
        if window.focused(cx) != origin || !target.is_current(state) {
            return history_start;
        }
        let (anchor, scroll_tray) = match &target.pending[0] {
            RequestIdentity::Approval(key) => {
                let Some(draft) = self.approval_drafts.get(key) else {
                    return history_start;
                };
                self.expanded.remove("approval-status-disclosure");
                (&draft.activity_anchor, true)
            }
            RequestIdentity::Elicitation(key) => {
                let Some(draft) = self.elicitation_drafts.get(key) else {
                    return history_start;
                };
                self.expanded.remove("elicitation-status-disclosure");
                (&draft.activity_anchor, true)
            }
            RequestIdentity::Question { key, .. } => {
                let Some(draft) = self.question_drafts.get(key) else {
                    return history_start;
                };
                self.turn_history.reveal_through(
                    draft.anchor,
                    &state.saved.items,
                    &transcript.minimap_entries,
                );
                self.smooth_follow = false;
                self.follow_after_submit = false;
                self.composer_pin_bottom = false;
                (&draft.activity_anchor, false)
            }
        };
        let anchor = anchor.clone();
        let owner = cx.entity().downgrade();
        let tray_anchor = scroll_tray.then(|| self.agent_controls_anchor.clone());
        window.on_next_frame(move |window, cx| {
            if window.focused(cx) != origin {
                return;
            }
            let _ = owner.update(cx, |this, cx| {
                if target.is_current(&this.session.read(cx).state) {
                    if let Some(tray) = tray_anchor {
                        tray.scroll_to(window, cx);
                    }
                    anchor.scroll.scroll_to(window, cx);
                    // Both anchors apply their offsets at the next frame boundary.
                    window.on_next_frame(|window, _| window.refresh());
                    anchor.focus.focus(window, cx);
                    cx.notify();
                }
            });
        });
        self.turn_history.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::codex_view::tests::{draw, fixture, snapshot};
    use serde_json::json;

    fn approval(reason: &str) -> serde_json::Value {
        json!({
            "id": "approval",
            "method": "item/commandExecution/requestApproval",
            "params": {
                "reason": reason,
                "command": "pwd",
                "cwd": "/tmp"
            }
        })
    }

    fn question() -> codex::QuestionRequest {
        codex::QuestionRequest::from_codex(
            json!("question"),
            &json!({
                "title": "Choose a layout",
                "questions": [{"id": "layout", "question": "Which layout?"}]
            }),
        )
        .unwrap()
    }

    #[test]
    fn activity_request_checks_kind_payload_epoch_and_question_instance() {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            assert!(activity_request(&state).is_none());
            state
                .approvals
                .push(approval("Read the checkout directory."));
            let captured = activity_request(&state).unwrap();
            assert_eq!(captured.cue, "Read the checkout directory.");
            assert!(captured.target.is_current(&state));
            state.approvals[0] = approval("Read the build directory.");
            assert!(!captured.target.is_current(&state));
            state.approvals[0] = approval("Read the checkout directory.");
            state.interaction_epoch += 1;
            assert!(!captured.target.is_current(&state));

            state.questions.push(question());
            let multiple = activity_request(&state).unwrap();
            assert_eq!(multiple.cue, "2 pending requests");
            state.approvals.clear();
            assert!(!multiple.target.is_current(&state));
            let question_shortcut = activity_request(&state).unwrap();
            assert_eq!(question_shortcut.cue, "Choose a layout");
            state.questions[0] = question();
            assert!(!question_shortcut.target.is_current(&state));
        }
    }

    #[gpui::test]
    fn activity_request_focuses_the_exact_expanded_control_and_restores_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let focus = cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.approvals = vec![approval("Read the checkout directory.")];
                        cx.notify();
                    });
                    view.expanded.insert("approval-status-disclosure".into());
                    let state = &view.session.read(cx).state;
                    let target = activity_request(state).unwrap().target;
                    view.input.focus_handle(cx).focus(window, cx);
                    view.focus_activity_request(target, window, cx);
                    view.input.focus_handle(cx)
                })
            });
            draw(cx);
            cx.update(gpui::Window::simulate_next_frame);
            draw(cx);
            cx.update(|window, cx| {
                let group = {
                    let view = view.read(cx);
                    assert!(!view.expanded.contains("approval-status-disclosure"));
                    view.approval_drafts
                        .values()
                        .next()
                        .unwrap()
                        .activity_anchor
                        .focus
                        .clone()
                };
                assert!(group.is_focused(window));
                window.focus_next(cx);
                assert!(group.contains_focused(window, cx) && !group.is_focused(window));
                group.focus(window, cx);
            });
            assert!(actions.try_iter().all(|action| !matches!(
                action,
                Action::Interaction { action, .. } if matches!(*action, Action::RespondApproval { .. })
            )));
            cx.update(|_, cx| {
                let session = view.read(cx).session.clone();
                session.update(cx, |session, cx| {
                    session.state.approvals.clear();
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.update(|window, _| focus.is_focused(window)));
        }
    }
}
