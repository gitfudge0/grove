use super::focus;
use crate::app::Modal;
use crate::gui::state::{Grove, Msg, OnboardingMsg};
use iced::Task;

impl Grove {
    /// First-run onboarding wizard family dispatch (`Msg::Onboarding`).
    pub(super) fn on_onboarding(&mut self, msg: OnboardingMsg) -> Task<Msg> {
        match msg {
            OnboardingMsg::Next => return self.onboard_advance(),
            OnboardingMsg::Back => {
                self.app.onboard_back();
                self.restart_onb_anim();
            }
            OnboardingMsg::Skip => self.onboard_skip(),
            OnboardingMsg::PathChanged(s) => self.app.onboard_set_path(s),
            OnboardingMsg::NameChanged(s) => self.app.onboard_set_name(s),
            OnboardingMsg::PickDir(p) => {
                self.app.onboard_pick_dir(p);
                return super::move_cursor_to_end(crate::gui::view::modal_input_id());
            }
            OnboardingMsg::AgentSelect(i) => self.app.onboard_agent_select(i),
            OnboardingMsg::PermsSelect(skip) => self.app.onboard_set_perms(skip),
        }
        Task::none()
    }

    /// On the session step, advance == finish (launch). On any other step, move
    /// forward; if the project step just registered a project, refresh the
    /// worktree cache so the rest of the app sees it.
    pub(super) fn onboard_advance(&mut self) -> Task<Msg> {
        let on_session = matches!(
            self.app.modal,
            Modal::Onboarding {
                step: crate::app::OnboardStep::Session,
                ..
            }
        );
        if on_session {
            return self.onboard_finish();
        }
        self.app.onboard_next();
        self.restart_onb_anim();
        self.rebuild_wt_cache();
        // Keep the project-step path input focused after rendering.
        if matches!(
            self.app.modal,
            Modal::Onboarding {
                step: crate::app::OnboardStep::Project,
                ..
            }
        ) {
            self.app.onboard_reset_project_focus();
            return focus(crate::gui::view::modal_input_id());
        }
        Task::none()
    }

    pub(super) fn onboard_skip(&mut self) {
        if let Err(e) = self.app.onboard_skip() {
            self.set_modal(Modal::Message(format!("Setup failed: {e}")));
            return;
        }
        self.after_onboarding();
    }

    pub(super) fn onboard_finish(&mut self) -> Task<Msg> {
        match self.app.onboard_finish() {
            Ok(Some((proj, agent))) => {
                let before = self.session_keys();
                self.spawn(proj, 0, agent);
                self.resize_new_sessions(&before);
                // If the grid is open, append the new session index so it appears.
                if self.grid_view && self.app.sessions.len() > before.len() {
                    self.tile_order.push(self.app.sessions.len() - 1);
                    self.persist_grid_order();
                    self.refresh_pty_viewport();
                }
                self.rebuild_wt_cache();
            }
            Ok(None) => {}
            Err(e) => {
                self.set_modal(Modal::Message(format!("Setup failed: {e}")));
                return Task::none();
            }
        }
        self.after_onboarding();
        Task::none()
    }

    /// After the wizard closes, surface the one-time tmux/native choice if it's
    /// still pending and nothing else grabbed the modal slot.
    pub(super) fn after_onboarding(&mut self) {
        if matches!(self.app.modal, Modal::None)
            && crate::app::needs_tmux_choice(self.app.tmux_available, self.app.store.tmux_enabled)
        {
            self.set_modal(Modal::TmuxChoice);
        }
    }
}
