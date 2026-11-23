use super::{Category, Page, SettingsPanel, SettingsPanelEvent};
use crate::{
    settings::SettingsState,
    views::sidebar::{
        project_setup::{ProjectSetup, ProjectSetupEvent},
        worktree_form::{WorktreeSetup, WorktreeSetupEvent},
    },
};
use gpui::{App, AppContext, Context, Entity, Focusable, Window};

pub(super) enum CreationPanel {
    Project(Entity<ProjectSetup>),
    Worktree(Entity<WorktreeSetup>),
}

impl CreationPanel {
    pub(super) fn is_pending(&self, cx: &App) -> bool {
        match self {
            Self::Project(panel) => panel.read(cx).is_pending(),
            Self::Worktree(panel) => panel.read(cx).is_pending(),
        }
    }

    pub(super) fn update_layout(&self, compact: bool, cx: &mut App) {
        match self {
            Self::Project(panel) => {
                panel.update(cx, |panel, cx| panel.update_settings_layout(compact, cx));
            }
            Self::Worktree(panel) => {
                panel.update(cx, |panel, cx| panel.update_settings_layout(compact, cx));
            }
        }
    }
}

impl SettingsPanel {
    pub(crate) fn open_project_creation_shortcut(
        &mut self,
        root: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_operation_pending(cx) {
            return;
        }
        self.open_projects(None, window, cx);
        self.start_project_creation(root, true, window, cx);
    }

    pub(super) fn start_project_creation(
        &mut self,
        root: Option<&str>,
        from_shortcut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let compact = self.compact_layout(window);
        let service = self.runtime.read(cx).projects.clone();
        self.nested_return_focus = window.focused(cx);
        let panel = if let Some(root) = root {
            let panel =
                cx.new(|cx| WorktreeSetup::new(root.to_owned(), service, compact, window, cx));
            self.creation_subscription =
                Some(
                    cx.subscribe_in(&panel, window, |this, _, event, window, cx| match event {
                        WorktreeSetupEvent::Completed { root, path } => {
                            this.finish_creation(root.clone(), path.clone(), window, cx);
                        }
                        WorktreeSetupEvent::Cancelled => this.cancel_creation(window, cx),
                    }),
                );
            self.creation_observer = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
            panel.focus_handle(cx).focus(window, cx);
            CreationPanel::Worktree(panel)
        } else {
            let workspace = cx.global::<SettingsState>().store.workspaces.active;
            let panel =
                cx.new(|cx| ProjectSetup::new(window, cx, workspace, service).in_settings(compact));
            self.creation_subscription =
                Some(
                    cx.subscribe_in(&panel, window, |this, _, event, window, cx| match event {
                        ProjectSetupEvent::Completed { path, workspace } => {
                            if cx.global::<SettingsState>().store.workspaces.active == *workspace {
                                this.finish_creation(path.clone(), path.clone(), window, cx);
                            } else {
                                this.close(window, cx);
                            }
                        }
                        ProjectSetupEvent::Cancelled => this.cancel_creation(window, cx),
                    }),
                );
            self.creation_observer = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
            panel.focus_handle(cx).focus(window, cx);
            CreationPanel::Project(panel)
        };
        self.creation_panel = Some(panel);
        self.creation_from_shortcut = from_shortcut;
        self.category = Category::Projects;
        self.show(Page::ProjectCreation, cx);
    }

    pub(super) fn cancel_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creation_from_shortcut {
            self.close(window, cx);
        } else {
            self.back(window, cx);
        }
    }

    fn finish_creation(
        &mut self,
        root: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
        let (label, display) = if root == path {
            (
                "Added",
                cx.global::<SettingsState>()
                    .store
                    .projects
                    .iter()
                    .find(|project| project.path == root)
                    .map_or_else(|| path.clone(), |project| project.name.clone()),
            )
        } else {
            (
                "Created",
                std::path::Path::new(&path)
                    .file_name()
                    .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
            )
        };
        self.project_receipt(format!("{label} {display}"), cx);
        cx.emit(SettingsPanelEvent::UseCheckout { root, path });
    }

    pub(super) fn clear_creation(&mut self) {
        self.creation_panel = None;
        self.creation_subscription = None;
        self.creation_observer = None;
        self.creation_from_shortcut = false;
    }
}
