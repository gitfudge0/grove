//! Terminal canvas status and session observation.

use super::{ActivityState, Agent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CanvasState {
    Starting,
    Running,
    Working,
    NeedsYou,
    Done,
    Idle,
    Failed,
    Exited,
}
impl CanvasState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Working => "Working",
            Self::NeedsYou => "Needs you",
            Self::Done => "Done",
            Self::Idle => "Idle",
            Self::Failed => "Failed",
            Self::Exited => "Exited",
        }
    }
    pub(super) fn color(self) -> gpui::Hsla {
        crate::views::status::session_kind(self.label()).color()
    }
    pub(super) fn message(self, error: Option<&str>) -> Option<String> {
        match self {
            Self::Starting => Some("Starting session. Preparing the terminal…".into()),
            Self::NeedsYou => {
                Some("This session needs your input. Review the terminal to continue.".into())
            }
            Self::Failed => Some(error.map_or_else(
                || "The session could not start.".into(),
                |error| format!("Could not start the session: {error}"),
            )),
            Self::Exited => Some("The session process has exited. Files remain on disk.".into()),
            _ => None,
        }
    }
}
pub(super) fn canvas_state(
    agent: Agent,
    activity: ActivityState,
    failed: bool,
    pending: bool,
    missing: bool,
    exited: bool,
) -> CanvasState {
    if failed {
        CanvasState::Failed
    } else if pending || missing {
        CanvasState::Starting
    } else if exited || activity == ActivityState::Exited {
        CanvasState::Exited
    } else if agent == Agent::Terminal {
        CanvasState::Running
    } else {
        match activity {
            ActivityState::Working => CanvasState::Working,
            ActivityState::WaitingForInput => CanvasState::NeedsYou,
            ActivityState::Done => CanvasState::Done,
            ActivityState::Idle => CanvasState::Idle,
            ActivityState::Exited => CanvasState::Exited,
        }
    }
}
pub(super) fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude Code",
        Agent::OpenCode => "OpenCode",
        Agent::Terminal => "Terminal",
    }
}
pub(super) fn session_signature(
    session: &gpui::Entity<crate::entities::terminal_session::TerminalSession>,
    cx: &gpui::App,
) -> (Option<String>, Option<String>, bool, bool) {
    let session = session.read(cx);
    (
        session.title(),
        session.spawn_error().map(str::to_owned),
        session.is_pending_attach(),
        session.has_exited(),
    )
}
