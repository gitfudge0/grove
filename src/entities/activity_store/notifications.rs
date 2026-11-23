//! Converts live session state into desktop alerts and retires stale click targets.

use std::collections::HashMap;
use std::time::Instant;

use futures::channel::mpsc;
use grove_core::codex::{SessionNoticeKind, Snapshot, Status};

use crate::entities::session_registry::{SessionId, SessionMeta};
use crate::platform::desktop_notifications::DesktopNotifications;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Alert {
    Approval(u64, String),
    Question(u64),
    Elicitation(u64, String),
    Plan(u64, usize),
    Review(u64, usize),
    TerminalPrompt,
    TerminalReview(Instant),
    Disconnected,
    UsageLimit,
    Stopped,
    QueuedMessage(u64),
    UncertainDelivery(u64),
}

impl Alert {
    fn title(&self) -> &'static str {
        match self {
            Self::Approval(..) => "Approval needed",
            Self::Question(..) | Self::Elicitation(..) => "Answer needed",
            Self::Plan(..) => "Plan ready",
            Self::Review(..) | Self::TerminalReview(_) => "Result ready to review",
            Self::TerminalPrompt => "Terminal needs input",
            Self::Disconnected => "Reconnect conversation",
            Self::UsageLimit => "Usage limit reached",
            Self::Stopped => "Request stopped",
            Self::QueuedMessage(_) => "Queued message needs attention",
            Self::UncertainDelivery(_) => "Message delivery unconfirmed",
        }
    }

    pub(super) fn structured(state: &Snapshot) -> Vec<Self> {
        if state.reconnecting {
            return Vec::new();
        }
        let epoch = state.interaction_epoch;
        let mut alerts = Vec::new();
        alerts.extend(state.approvals.iter().filter_map(|request| {
            let id = &request["id"];
            ((id.is_string() || id.is_number())
                && !grove_core::controls::approval_choices(request).is_empty())
            .then(|| Self::Approval(epoch, id.to_string()))
        }));
        alerts.extend(
            state
                .questions
                .iter()
                .map(|request| Self::Question(request.instance())),
        );
        alerts.extend(
            state
                .elicitations
                .iter()
                .map(|request| Self::Elicitation(epoch, request.id.to_string())),
        );

        if !alerts.is_empty() || state.provider_work_active() {
            return alerts;
        }

        // A confirmed connection or request failure takes precedence over
        // queue recovery. Neither a retry in progress nor an idle queue is a failure.
        let notice = state.effective_notice().map(|notice| notice.kind);
        let recovery = match state.status {
            Status::Disconnected => Some(Self::Disconnected),
            Status::Failed | Status::Interrupted | Status::Done
                if notice == Some(SessionNoticeKind::UsageLimit) =>
            {
                Some(Self::UsageLimit)
            }
            Status::Failed
                if notice != Some(SessionNoticeKind::Retrying) && state.error.is_some() =>
            {
                Some(Self::Stopped)
            }
            _ => None,
        };
        if let Some(recovery) = recovery {
            alerts.push(recovery);
            return alerts;
        }

        if let Some(&id) = state.queue_uncertain.first() {
            alerts.push(Self::UncertainDelivery(id));
        } else if state.status == Status::Interrupted && state.queue_edit.is_none() {
            if let Some(next) = state.queued_prompts.first() {
                alerts.push(Self::QueuedMessage(next.id));
            }
        }

        if let Some(review) = state.live_review_completion() {
            let conversation = state.cleared_conversations;
            alerts.push(if state.plan_ready() {
                Self::Plan(conversation, review)
            } else {
                Self::Review(conversation, review)
            });
        }
        alerts
    }
}

#[derive(Default)]
struct PendingAlerts {
    next_id: u64,
    sessions: HashMap<SessionId, Vec<TrackedAlert>>,
}

struct TrackedAlert {
    alert: Alert,
    /// Present only for outstanding banners. A visible or previously opened
    /// alert stays tracked until it resolves so it cannot notify again.
    banner: Option<u64>,
}

enum Delivery {
    Show(u64, &'static str),
    Close(u64),
}

impl PendingAlerts {
    fn observe(
        &mut self,
        session: SessionId,
        alerts: Vec<Alert>,
        visible: bool,
        mut deliver: impl FnMut(Delivery),
    ) {
        if alerts.is_empty() {
            if let Some(old) = self.sessions.remove(&session) {
                close_alerts(old, &mut deliver);
            }
            return;
        }

        let tracked = self.sessions.entry(session).or_default();
        tracked.retain(|old| {
            if alerts.contains(&old.alert) {
                return true;
            }
            if let Some(id) = old.banner {
                deliver(Delivery::Close(id));
            }
            false
        });
        for alert in alerts {
            if !tracked.iter().any(|old| old.alert == alert) {
                let banner = (!visible).then(|| {
                    self.next_id += 1;
                    deliver(Delivery::Show(self.next_id, alert.title()));
                    self.next_id
                });
                tracked.push(TrackedAlert { alert, banner });
            }
        }
    }

    fn take_target(&mut self, id: u64) -> Option<SessionId> {
        self.sessions.iter_mut().find_map(|(session, alerts)| {
            let alert = alerts.iter_mut().find(|alert| alert.banner == Some(id))?;
            alert.banner = None;
            Some(*session)
        })
    }
}

fn close_alerts(
    alerts: impl IntoIterator<Item = TrackedAlert>,
    deliver: &mut impl FnMut(Delivery),
) {
    for alert in alerts {
        if let Some(id) = alert.banner {
            deliver(Delivery::Close(id));
        }
    }
}

pub(super) struct SessionNotifications {
    pending: PendingAlerts,
    desktop: DesktopNotifications,
}

impl SessionNotifications {
    pub(super) fn new(activated: mpsc::UnboundedSender<u64>) -> Self {
        Self {
            pending: PendingAlerts::default(),
            desktop: DesktopNotifications::new(activated),
        }
    }

    pub(super) fn observe(&mut self, meta: &SessionMeta, alerts: Vec<Alert>, visible: bool) {
        self.pending
            .observe(meta.id, alerts, visible, |event| match event {
                Delivery::Show(id, title) => {
                    let agent = match meta.agent {
                        grove_core::agent::Agent::Codex => "Codex",
                        grove_core::agent::Agent::Claude => "Claude",
                        grove_core::agent::Agent::OpenCode => "OpenCode",
                        grove_core::agent::Agent::Terminal => "Terminal",
                    };
                    let project: String = meta
                        .project
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(80)
                        .collect();
                    let body = if project.is_empty() {
                        agent.to_owned()
                    } else {
                        format!("{agent} · {project}")
                    };
                    self.desktop.show(id, title, body);
                }
                Delivery::Close(id) => self.desktop.close(id),
            });
    }

    pub(super) fn retain(&mut self, is_live: impl Fn(SessionId) -> bool) {
        self.pending.sessions.retain(|session, alerts| {
            if is_live(*session) {
                return true;
            }
            close_alerts(alerts.drain(..), &mut |event| {
                if let Delivery::Close(id) = event {
                    self.desktop.close(id);
                }
            });
            false
        });
    }

    pub(super) fn take_target(&mut self, id: u64) -> Option<SessionId> {
        let target = self.pending.take_target(id)?;
        self.desktop.close(id);
        Some(target)
    }
}

#[cfg(test)]
mod tests;
