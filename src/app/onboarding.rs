use super::{cycle, list_dirs, needs_tmux_choice, shellexpand_tilde, App, Modal};
use anyhow::Result;
use fs_err as fs;
use grove_core::agent::Agent;

/// The one-time modal (if any) to show on launch, in priority order: the
/// first-run onboarding wizard takes precedence over the tmux/native choice
/// (the wizard's environment step already surfaces tmux), which in turn only
/// appears once. Pure so the precedence is unit-testable without iced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstRunModal {
    Onboarding,
    TmuxChoice,
    None,
}

pub fn first_run_modal(
    onboarded: bool,
    tmux_available: bool,
    tmux_enabled: Option<bool>,
) -> FirstRunModal {
    if !onboarded {
        FirstRunModal::Onboarding
    } else if needs_tmux_choice(tmux_available, tmux_enabled) {
        FirstRunModal::TmuxChoice
    } else {
        FirstRunModal::None
    }
}

/// The ordered steps of the first-run onboarding wizard. `Welcome` orients the
/// user, `Environment` reports detected tools, `Project` registers the first
/// project, and `Session` launches the first agent. The flow no longer varies
/// with tmux availability — new sessions default to tmux when it's present,
/// native otherwise (set on completion in [`App::onboard_finish`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardStep {
    Welcome,
    Environment,
    Project,
    Session,
}

impl OnboardStep {
    pub const ALL: [OnboardStep; 4] = [
        OnboardStep::Welcome,
        OnboardStep::Environment,
        OnboardStep::Project,
        OnboardStep::Session,
    ];

    /// The wizard's fixed step sequence.
    pub fn flow() -> &'static [OnboardStep] {
        &Self::ALL
    }

    pub fn index_in(self) -> usize {
        Self::flow().iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<OnboardStep> {
        Self::flow().get(self.index_in() + 1).copied()
    }

    pub fn prev(self) -> Option<OnboardStep> {
        self.index_in().checked_sub(1).map(|i| Self::flow()[i])
    }

    /// Short label shown in the progress rail.
    pub fn label(self) -> &'static str {
        match self {
            OnboardStep::Welcome => "welcome",
            OnboardStep::Environment => "environment",
            OnboardStep::Project => "project",
            OnboardStep::Session => "session",
        }
    }
}

/// Build the initial onboarding modal.
pub fn onboarding_modal() -> Modal {
    Modal::Onboarding {
        step: OnboardStep::Welcome,
        path: String::new(),
        dir_sel: 0,
        name: None,
        note: None,
        added_proj: None,
        agent_sel: 0,
        perms_skip: false,
        name_focused: false,
    }
}

impl App {
    // ── onboarding wizard ──────────────────────────────────────────────────

    /// Move the wizard to `step` (no-op if onboarding isn't open).
    fn onboard_goto(&mut self, step: OnboardStep) {
        if let Modal::Onboarding { step: s, .. } = &mut self.modal {
            *s = step;
        }
    }

    /// Advance the wizard one step. The project step validates and registers
    /// the project before advancing. The session step is terminal —
    /// [`onboard_finish`](Self::onboard_finish) handles it.
    pub(crate) fn onboard_next(&mut self) {
        let Modal::Onboarding { step, .. } = &self.modal else {
            return;
        };
        let step = *step;
        match step {
            // Plain forward steps: walk to the next one.
            OnboardStep::Welcome | OnboardStep::Environment => {
                if let Some(next) = step.next() {
                    self.onboard_goto(next);
                }
            }
            OnboardStep::Project => self.onboard_submit_project(),
            OnboardStep::Session => {}
        }
    }

    /// Step back. Never un-registers a project added on the way forward; the
    /// project step recognizes it's already added and skips re-adding.
    pub(crate) fn onboard_back(&mut self) {
        let prev = match &self.modal {
            Modal::Onboarding { step, .. } => step.prev(),
            _ => None,
        };
        if let Some(prev) = prev {
            self.onboard_goto(prev);
        }
    }

    /// Register the project from the path field, then advance to the session
    /// step. On validation failure the inline note is set and the step stays
    /// put. A project already added (e.g. after stepping back and forward)
    /// just advances. Unlike the normal add-project flow this is quiet — no
    /// git probe or init-git choice.
    fn onboard_submit_project(&mut self) {
        let (already, path, name) = match &self.modal {
            Modal::Onboarding {
                added_proj,
                path,
                name,
                ..
            } => (added_proj.is_some(), path.clone(), name.clone()),
            _ => return,
        };
        if already {
            self.onboard_goto(OnboardStep::Session);
            return;
        }
        match self.onboard_add_project(&path, name) {
            Ok(idx) => {
                if let Modal::Onboarding {
                    added_proj, note, ..
                } = &mut self.modal
                {
                    *added_proj = Some(idx);
                    *note = None;
                }
                self.onboard_goto(OnboardStep::Session);
            }
            Err(e) => {
                if let Modal::Onboarding { note, .. } = &mut self.modal {
                    *note = Some(e);
                }
            }
        }
    }

    /// Register a project quietly (no follow-up modals), returning its index.
    /// Mirrors the validation in [`submit_input`](Self::submit_input)'s
    /// add-project branch.
    fn onboard_add_project(&mut self, path: &str, name: Option<String>) -> Result<usize, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err("enter a path, or skip setup".into());
        }
        let pb = std::path::PathBuf::from(shellexpand_tilde(trimmed));
        if !pb.is_dir() {
            return Err("not a directory".into());
        }
        let project_name = name
            .unwrap_or_else(|| {
                pb.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string()
            })
            .trim()
            .to_string();
        if project_name.is_empty() {
            return Err("name required".into());
        }
        if self.store.projects.iter().any(|p| p.name == project_name) {
            return Err(format!("project '{project_name}' already exists"));
        }
        let abs = fs::canonicalize(&pb)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        self.register_project(project_name, abs)
            .map_err(|e| e.to_string())
    }

    /// Live-update the project-step path field, mirroring the add-project input:
    /// clears the inline note, resets the directory cursor, and once the path
    /// resolves to a real directory, prefills the name from its basename.
    pub(crate) fn onboard_set_path(&mut self, value: String) {
        let resolved = std::path::PathBuf::from(shellexpand_tilde(value.trim()));
        let base = resolved
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string);
        let is_dir = resolved.is_dir();
        if let Modal::Onboarding {
            path,
            dir_sel,
            note,
            name,
            ..
        } = &mut self.modal
        {
            *path = value;
            *dir_sel = 0;
            *note = None;
            if is_dir {
                if name.is_none() {
                    *name = Some(base.unwrap_or_else(|| "project".into()));
                }
            } else {
                *name = None;
            }
        }
    }

    pub(crate) fn onboard_set_name(&mut self, value: String) {
        if let Modal::Onboarding { name, .. } = &mut self.modal {
            *name = Some(value);
        }
    }

    /// Fill the path field from a clicked directory match (trailing slash so the
    /// next keystroke descends).
    pub(crate) fn onboard_pick_dir(&mut self, dir: String) {
        self.onboard_set_path(format!("{dir}/"));
    }

    /// Move the directory-match cursor in the project step.
    pub(crate) fn onboard_dir_move(&mut self, delta: i32) {
        let entries = match &self.modal {
            Modal::Onboarding { path, .. } => list_dirs(path).len(),
            _ => return,
        };
        if entries == 0 {
            return;
        }
        if let Modal::Onboarding { dir_sel, .. } = &mut self.modal {
            *dir_sel = cycle(*dir_sel, delta, entries);
        }
    }

    /// Reset Tab's toggle target to the path field. Called whenever the
    /// project step is (re-)entered, so a stale toggle from a previous visit
    /// doesn't leave the first Tab press landing on the name field.
    pub(crate) fn onboard_reset_project_focus(&mut self) {
        if let Modal::Onboarding { name_focused, .. } = &mut self.modal {
            *name_focused = false;
        }
    }

    /// Tab in the project step: alternate focus between the path and name
    /// fields. Returns `true` if focus moved to the name field (the caller
    /// then skips path-completion); `false` if it's on the path field (where
    /// the caller runs the existing directory completion). No name field
    /// (path not yet a valid directory) means there's nothing to alternate
    /// to, so this always reports the path field.
    pub(crate) fn onboard_toggle_project_focus(&mut self) -> bool {
        if let Modal::Onboarding {
            name, name_focused, ..
        } = &mut self.modal
        {
            if name.is_none() {
                *name_focused = false;
                return false;
            }
            *name_focused = !*name_focused;
            return *name_focused;
        }
        false
    }

    /// Complete the path from the selected directory match (Tab in the project
    /// step).
    pub(crate) fn onboard_dir_pick(&mut self) {
        let pick = match &self.modal {
            Modal::Onboarding { path, dir_sel, .. } => list_dirs(path).into_iter().nth(*dir_sel),
            _ => None,
        };
        if let Some(dir) = pick {
            self.onboard_set_path(format!("{dir}/"));
        }
    }

    pub(crate) fn onboard_agent_select(&mut self, i: usize) {
        if let Modal::Onboarding { agent_sel, .. } = &mut self.modal {
            *agent_sel = i;
        }
    }

    pub(crate) fn onboard_set_perms(&mut self, skip: bool) {
        if let Modal::Onboarding { perms_skip, .. } = &mut self.modal {
            *perms_skip = skip;
        }
    }

    /// Skip the wizard: mark onboarded, persist, and close. The first-run
    /// gate won't show it again.
    pub(crate) fn onboard_skip(&mut self) -> Result<()> {
        self.store.onboarded = true;
        grove_core::storage::save(&self.store)?;
        self.modal = Modal::None;
        Ok(())
    }

    /// Finish the wizard: mark onboarded, default new sessions to tmux when
    /// available (native otherwise), close, and return the
    /// `(project index, agent)` to launch a first session into — or `None` if
    /// no project was added or no agent is available.
    pub(crate) fn onboard_finish(&mut self) -> Result<Option<(usize, Agent)>> {
        let (added_proj, agent_sel, perms_skip) = match &self.modal {
            Modal::Onboarding {
                added_proj,
                agent_sel,
                perms_skip,
                ..
            } => (*added_proj, *agent_sel, *perms_skip),
            _ => (None, 0, false),
        };
        let agent = self.available_agents.get(agent_sel).copied();
        self.store.onboarded = true;
        // New sessions default to the tmux backend when it's available, native
        // otherwise; changeable later via the palette's Settings pane.
        self.store.tmux_enabled = Some(self.tmux_available);
        self.store.dangerously_skip_permissions_enabled = Some(perms_skip);
        grove_core::storage::save(&self.store)?;
        self.modal = Modal::None;
        Ok(match (added_proj, agent) {
            (Some(p), Some(a)) => Some((p, a)),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{first_run_modal, onboarding_modal, FirstRunModal, OnboardStep};
    use crate::app::{test_app, Modal};

    #[test]
    fn onboarding_takes_precedence_until_completed() {
        // Fresh install: onboarding wins even when a tmux choice is also pending.
        assert_eq!(
            first_run_modal(false, true, None),
            FirstRunModal::Onboarding
        );
        assert_eq!(
            first_run_modal(false, false, None),
            FirstRunModal::Onboarding
        );
        // Once onboarded, the tmux choice falls through (only when pending).
        assert_eq!(first_run_modal(true, true, None), FirstRunModal::TmuxChoice);
        assert_eq!(first_run_modal(true, true, Some(true)), FirstRunModal::None);
        assert_eq!(first_run_modal(true, false, None), FirstRunModal::None);
    }

    #[test]
    fn onboard_step_navigation_is_bounded() {
        assert_eq!(OnboardStep::Welcome.prev(), None);
        assert_eq!(OnboardStep::Environment.next(), Some(OnboardStep::Project));
        assert_eq!(OnboardStep::Project.prev(), Some(OnboardStep::Environment));
        assert_eq!(OnboardStep::Project.next(), Some(OnboardStep::Session));
        assert_eq!(OnboardStep::Session.next(), None);
        // index round-trips through the flow in order.
        for (i, s) in OnboardStep::flow().iter().enumerate() {
            assert_eq!(s.index_in(), i);
        }
    }

    #[test]
    fn onboard_tab_toggles_project_focus_only_when_name_field_exists() {
        let mut app = test_app(vec![]);
        app.modal = onboarding_modal();
        // Path not yet resolved to a directory: no name field to toggle to.
        assert!(!app.onboard_toggle_project_focus());
        assert!(!app.onboard_toggle_project_focus());
        // Once a name is inferred, Tab alternates path <-> name.
        if let Modal::Onboarding { name, .. } = &mut app.modal {
            *name = Some("repo".into());
        }
        assert!(app.onboard_toggle_project_focus());
        assert!(!app.onboard_toggle_project_focus());
        assert!(app.onboard_toggle_project_focus());
        // Losing the name field again snaps back to the path field.
        if let Modal::Onboarding { name, .. } = &mut app.modal {
            *name = None;
        }
        assert!(!app.onboard_toggle_project_focus());
    }

    #[test]
    fn onboard_reset_project_focus_clears_stale_toggle() {
        let mut app = test_app(vec![]);
        app.modal = onboarding_modal();
        if let Modal::Onboarding {
            name, name_focused, ..
        } = &mut app.modal
        {
            *name = Some("repo".into());
            *name_focused = true;
        }
        app.onboard_reset_project_focus();
        let Modal::Onboarding { name_focused, .. } = &app.modal else {
            unreachable!()
        };
        assert!(!name_focused);
    }
}
