//! Project management in the main canvas, with path-stable decisions and retained progress.
mod scripts;

use crate::{
    entities::session_registry::SessionRegistry,
    icons::icon,
    project_service::{
        ProjectEvent, ProjectService, ProjectWorktreeReview, ReviewedWorktree, WorktreeRemovalStage,
    },
    settings::SettingsState,
    theme as c,
    views::{
        components::{project_field_well, project_form_field},
        rpx,
        tokens::*,
    },
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription,
    Window,
};
use gpui_component::{input::InputState, scroll::ScrollableElement, Disableable, ElementExt as _};
use grove_core::storage::{ProjectAction, ProjectActionKind, ProjectScripts};

mod appearance;

/// Compact reusable action register geometry.
const LIFECYCLE_ROW_MIN_H: f32 = 40.;
const LIFECYCLE_LABEL_W: f32 = 96.;
const SCRIPT_ROW_MIN_H: f32 = 54.;

#[derive(Clone, Debug, PartialEq)]
enum ScriptEditor {
    Setup,
    Teardown,
    Action(String),
}

struct ScriptRowDetails {
    title: String,
    preview: String,
    glyph: Option<&'static str>,
}

/// Bound focus traversal even if every decision action is disabled.
const FOCUS_SCAN_LIMIT: usize = 128;
/// Workspace destination choices are selection rows, not action buttons.
const DESTINATION_ROW_H: f32 = 44.;

#[derive(Clone)]
pub(in crate::views) enum ProjectPanelEvent {
    Closed,
    Selected(String),
    Decision(bool),
    Archived,
    Moved {
        name: String,
        workspace: String,
        path: String,
        open_destination: bool,
    },
}
#[derive(Clone, PartialEq)]
pub(in crate::views) enum Page {
    Edit(String),
    Remove(String),
    RemoveWorktree {
        project_path: String,
        path: String,
        name: String,
        branch: String,
    },
    Move(String),
    Archived,
}
#[derive(Clone, PartialEq)]
enum Decision {
    Archive,
    Delete(String),
}
pub(in crate::views) struct ProjectPanel {
    pub(super) workspace: u64,
    service: Entity<ProjectService>,
    registry: Entity<SessionRegistry>,
    page: Page,
    fields: Vec<Entity<InputState>>,
    script_entries: Vec<(String, bool, Entity<InputState>, Entity<InputState>)>,
    expanded_script: Option<ScriptEditor>,
    script_error: Option<(String, bool, String)>,
    script_focus: std::collections::HashMap<String, FocusHandle>,
    lifecycle_focus: [FocusHandle; 2],
    add_script_focus: FocusHandle,
    archive_overflow: Option<String>,
    draft_appearance: Option<grove_core::storage::identity::ProjectAppearance>,
    appearance_symbol_focus: Vec<FocusHandle>,
    appearance_color_focus: Vec<FocusHandle>,
    error: Option<String>,
    destination: Option<u64>,
    open_destination: bool,
    decision: Option<Decision>,
    delete_worktrees: bool,
    reviewed_worktrees: Option<ProjectWorktreeReview>,
    reviewed_worktree: Option<ReviewedWorktree>,
    discovery_done: bool,
    started: bool,
    focus: FocusHandle,

    return_focus: Option<FocusHandle>,
    settings_compact: Option<bool>,
    observers: Vec<Subscription>,
    scroll: gpui::ScrollHandle,
    scripts_anchor: gpui::ScrollAnchor,
    draft_anchor: gpui::ScrollAnchor,
    script_reveal: std::rc::Rc<std::cell::Cell<Option<bool>>>,
}
impl EventEmitter<ProjectPanelEvent> for ProjectPanel {}
impl ProjectPanel {
    pub(in crate::views) fn new(
        page: Page,
        service: Entity<ProjectService>,
        registry: Entity<SessionRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = match &page {
            Page::Edit(path) | Page::Remove(path) | Page::Move(path) => cx
                .global::<SettingsState>()
                .store
                .script_project(path)
                .into(),
            Page::RemoveWorktree { .. } | Page::Archived => None,
        };
        let draft_appearance = match &page {
            Page::Edit(path)
                if cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .iter()
                    .any(|p| p.path == *path) =>
            {
                Some(cx.global::<SettingsState>().store.project_appearance(path))
            }
            _ => None,
        };
        let values = project.as_ref().map_or_else(
            || vec![String::new(); 4],
            |p| {
                vec![
                    p.name.clone(),
                    p.scripts.setup.clone().unwrap_or_default(),
                    p.scripts.run.clone().unwrap_or_default(),
                    p.scripts.teardown.clone().unwrap_or_default(),
                ]
            },
        );
        let fields: Vec<_> = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx).multi_line(index > 0);
                    input.set_value(value, window, cx);
                    input
                })
            })
            .collect();
        let mut observers = vec![
            cx.subscribe(&service, |_, _, event, cx| {
                if matches!(
                    event,
                    ProjectEvent::ProjectRemovalChanged { .. }
                        | ProjectEvent::WorktreeRemovalChanged { .. }
                        | ProjectEvent::TreeInvalidated
                ) {
                    cx.notify();
                }
            }),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
        ];
        let script_entries: Vec<_> = project
            .as_ref()
            .map(|p| p.scripts.effective_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                let (prompt, content) = match entry.kind {
                    ProjectActionKind::Shell { command } => (false, command),
                    ProjectActionKind::Prompt { template } => (true, template),
                };
                let name = cx.new(|cx| InputState::new(window, cx).default_value(entry.name));
                let body = cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .default_value(content)
                });
                (entry.id, prompt, name, body)
            })
            .collect();
        for input in fields.iter().chain(
            script_entries
                .iter()
                .flat_map(|(_, _, name, body)| [name, body]),
        ) {
            observers.push(cx.observe(input, |_, _, cx| cx.notify()));
        }
        let script_focus = script_entries
            .iter()
            .map(|(id, _, _, _)| (id.clone(), cx.focus_handle()))
            .collect();
        let scroll = gpui::ScrollHandle::new();
        let panel = Self {
            scripts_anchor: gpui::ScrollAnchor::for_handle(scroll.clone()),
            draft_anchor: gpui::ScrollAnchor::for_handle(scroll.clone()),
            scroll,
            script_reveal: std::rc::Rc::default(),
            workspace: cx.global::<SettingsState>().store.workspaces.active,
            service,
            registry,
            page,
            fields,
            script_entries,
            expanded_script: None,
            script_error: None,
            script_focus,
            lifecycle_focus: [cx.focus_handle(), cx.focus_handle()],
            add_script_focus: cx.focus_handle(),
            archive_overflow: None,
            draft_appearance,
            appearance_symbol_focus: grove_core::storage::identity::SYMBOLS
                .iter()
                .map(|_| cx.focus_handle())
                .collect(),
            appearance_color_focus: (0..grove_core::storage::identity::COLOR_COUNT)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            error: None,
            destination: None,
            open_destination: false,
            decision: None,
            delete_worktrees: false,
            reviewed_worktrees: None,
            reviewed_worktree: None,
            discovery_done: false,
            started: false,
            focus: cx.focus_handle(),

            return_focus: None,
            settings_compact: None,
            observers,
        };
        if let Page::Remove(path) = &panel.page {
            let path = path.clone();
            cx.spawn(async move |this, cx| {
                let reviewed = cx
                    .background_executor()
                    .spawn(async move { ProjectService::review_project_worktrees(&path) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.discovery_done = true;
                    match reviewed {
                        Ok(worktrees) => this.reviewed_worktrees = Some(worktrees),
                        Err(error) => this.error = Some(format!(
                            "Could not discover worktrees: {error}. You can still remove the registration while keeping all files."
                        )),
                    }
                    cx.notify();
                });
            })
            .detach();
        } else if let Page::RemoveWorktree {
            project_path,
            path,
            branch,
            ..
        } = &panel.page
        {
            let (project_path, path, branch) = (project_path.clone(), path.clone(), branch.clone());
            cx.spawn(async move |this, cx| {
                let reviewed = cx
                    .background_executor()
                    .spawn(async move { ProjectService::review_worktree(&project_path, &path, &branch) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.discovery_done = true;
                    match reviewed {
                        Ok(worktree) => this.reviewed_worktree = Some(worktree),
                        Err(error) => {
                            this.error = Some(format!(
                                "Could not verify the selected worktree: {error} Return to Projects and reopen the worktree to review it again."
                            ));
                            this.service.update(cx, |_, cx| cx.emit(ProjectEvent::TreeInvalidated));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        panel.focus.focus(window, cx);
        panel
    }
    pub(super) fn is_decision(&self) -> bool {
        self.decision.is_some()
            || matches!(self.page, Page::Remove(_) | Page::RemoveWorktree { .. }) && !self.started
    }
    pub(in crate::views) fn is_pending(&self, cx: &App) -> bool {
        if !self.started {
            return false;
        }
        let service = self.service.read(cx);
        match &self.page {
            Page::Remove(path) => service
                .removal_status(path)
                .is_none_or(|status| !status.finished),
            Page::RemoveWorktree { path, .. } => service
                .worktree_removal_status(path)
                .is_none_or(|status| status.stage != WorktreeRemovalStage::Finished),
            _ => false,
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.is_pending(cx) {
            cx.emit(ProjectPanelEvent::Decision(false));
            cx.emit(ProjectPanelEvent::Closed);
        }
    }
    fn cancel_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.decision = None;
        self.error = None;
        cx.emit(ProjectPanelEvent::Decision(false));
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }
    fn decide(&mut self, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        self.return_focus = window.focused(cx);
        self.decision = Some(decision);
        self.focus.focus(window, cx);
        cx.emit(ProjectPanelEvent::Decision(true));
        cx.notify();
    }
    pub(in crate::views) fn in_settings(mut self, compact: bool) -> Self {
        self.settings_compact = Some(compact);
        self
    }

    pub(in crate::views) fn update_settings_layout(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings_compact
            .is_some_and(|previous| previous != compact)
        {
            self.settings_compact = Some(compact);
            cx.notify();
        }
    }

    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let Page::Edit(path) = &self.page else { return };
        let path = path.clone();
        let values: Vec<_> = self
            .fields
            .iter()
            .map(|f| f.read(cx).value().to_string())
            .collect();
        let script =
            |index: usize| (!values[index].trim().is_empty()).then(|| values[index].clone());
        let scripts = ProjectScripts {
            setup: script(1),
            run: None,
            entries: self
                .script_entries
                .iter()
                .map(|(id, prompt, name, body)| ProjectAction {
                    id: id.clone(),
                    name: name.read(cx).value().to_string(),
                    kind: if *prompt {
                        ProjectActionKind::Prompt {
                            template: body.read(cx).value().to_string(),
                        }
                    } else {
                        ProjectActionKind::Shell {
                            command: body.read(cx).value().to_string(),
                        }
                    },
                })
                .collect(),
            review_prompt_id: None,
            teardown: script(3),
        };
        self.script_error = None;
        if let Err(error) = scripts.validate() {
            let mut names = std::collections::HashSet::new();
            for (id, _, name, body) in &self.script_entries {
                let value = name.read(cx).value().to_string();
                let name_invalid =
                    value.trim().is_empty() || !names.insert(value.trim().to_lowercase());
                if name_invalid || body.read(cx).value().trim().is_empty() {
                    self.expanded_script = Some(ScriptEditor::Action(id.clone()));
                    self.script_error = Some((id.clone(), name_invalid, error.clone()));
                    break;
                }
            }
            self.error = Some(error);
            cx.notify();
            return;
        }
        if !cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .any(|p| p.path == path)
        {
            SettingsState::update(cx, |store| {
                store.directory_scripts.insert(path, scripts);
            });
            cx.emit(ProjectPanelEvent::Closed);
            return;
        }
        let appearance = self.draft_appearance;
        match self.service.update(cx, |service, cx| {
            service.update_project_with_appearance(
                &path,
                values[0].clone(),
                scripts,
                appearance,
                cx,
            )
        }) {
            Ok(()) => cx.emit(ProjectPanelEvent::Closed),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn move_project(&mut self, cx: &mut Context<Self>) {
        let Page::Move(path) = &self.page else { return };
        let path = path.clone();
        let Some(destination) = self.destination else {
            return;
        };
        let store = &cx.global::<SettingsState>().store;
        let name = store
            .projects
            .iter()
            .find(|project| project.path == path)
            .map_or_else(|| path.clone(), |project| project.name.clone());
        let workspace = store.workspaces.name(destination).to_owned();
        self.error = None;
        match self.service.update(cx, |service, cx| {
            service.move_project_to_workspace(&path, destination, self.open_destination, cx)
        }) {
            Ok(()) => cx.emit(ProjectPanelEvent::Moved {
                name,
                workspace,
                path,
                open_destination: self.open_destination,
            }),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn archive(&mut self, cx: &mut Context<Self>) {
        let Page::Edit(path) = &self.page else { return };
        let path = path.clone();
        self.error = None;
        match self
            .service
            .update(cx, |service, cx| service.archive_project_by_path(&path, cx))
        {
            Ok(()) => {
                self.page = Page::Archived;
                cx.emit(ProjectPanelEvent::Archived);
                self.decision = None;
                cx.emit(ProjectPanelEvent::Decision(false));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn remove(&mut self, cx: &mut Context<Self>) {
        let Page::Remove(path) = &self.page else {
            return;
        };
        if self.started || !self.discovery_done {
            return;
        }
        let path = path.clone();
        let reviewed = if self.delete_worktrees {
            let Some(reviewed) = self
                .reviewed_worktrees
                .as_ref()
                .map(|review| &review.verified)
                .filter(|paths| !paths.is_empty())
            else {
                return;
            };
            Some(reviewed.clone())
        } else {
            None
        };
        match self.service.update(cx, |service, cx| {
            if let Some(reviewed) = reviewed {
                service.remove_project_with_reviewed_worktrees(&path, reviewed, cx)
            } else {
                service.remove_project_by_path(&path, false, cx)
            }
        }) {
            Ok(()) => {
                self.started = true;
                self.error = None;
                cx.emit(ProjectPanelEvent::Decision(false));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn remove_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Page::RemoveWorktree { project_path, .. } = &self.page else {
            return;
        };
        if self.started {
            return;
        }
        let Some(reviewed) = self.reviewed_worktree.clone() else {
            return;
        };
        match self.service.update(cx, |service, cx| {
            service.remove_reviewed_worktree(project_path, reviewed, cx)
        }) {
            Ok(()) => {
                self.started = true;
                self.error = None;
                self.focus.focus(window, cx);
                cx.emit(ProjectPanelEvent::Decision(false));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn skip_worktree_teardown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Page::RemoveWorktree { path, .. } = &self.page else {
            return;
        };
        if self.started {
            self.error = self
                .service
                .update(cx, |service, cx| {
                    service.skip_worktree_teardown_for(path, cx)
                })
                .err();
            self.focus.focus(window, cx);
            cx.notify();
        }
    }
}
impl Focusable for ProjectPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
#[derive(Clone, Copy)]
enum ProjectActionStyle {
    Quiet,
    Primary,
    Danger,
    Mini,
}
fn project_action(
    id: gpui::ElementId,
    label: impl Into<gpui::SharedString>,
    glyph: Option<&'static str>,
    style: ProjectActionStyle,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    let (fill, fg, hover) = match style {
        ProjectActionStyle::Primary => (c::FG(), c::BG(), c::FG_DIM()),
        ProjectActionStyle::Danger => (c::RED(), c::BG(), c::RED()),
        ProjectActionStyle::Quiet => (c::BG(), c::FG_DIM(), c::BG_HOVER()),
        ProjectActionStyle::Mini => (c::FIELD_FILL(), c::FG(), c::BG_HOVER()),
    };
    let button = if matches!(style, ProjectActionStyle::Mini) {
        crate::views::components::dense_action_with_colors(id, label, fill, fg, hover, window, cx)
    } else {
        crate::views::components::form_action(
            id,
            label,
            matches!(style, ProjectActionStyle::Primary),
            window,
            cx,
        )
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(fg)
                .hover_foreground(if matches!(style, ProjectActionStyle::Quiet) {
                    c::FG()
                } else {
                    fg
                })
                .hover(hover)
                .active(hover)
                .shadow(false),
        )
    };
    if let Some(glyph) = glyph {
        button.icon(gpui_component::Icon::default().path(glyph))
    } else {
        button.icon(gpui_component::Icon::default().path("icons/close.svg"))
    }
}
fn panel_section(label: &'static str) -> impl IntoElement {
    div()
        .mt(rpx(SPACE_20))
        .mb(rpx(SPACE_LG))
        .text_size(rpx(TEXT_SMALL))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c::FG_DIM())
        .child(label)
}
fn readonly_path(label: &'static str, path: String) -> impl IntoElement {
    project_field_well(false, false)
        .h_auto()
        .min_h(rpx(FIELD_H))
        .py(rpx(SPACE_LG))
        .id(label)
        .debug_selector(move || label.into())
        .role(gpui::Role::Group)
        .aria_label(label)
        .aria_description(path.clone())
        .min_w_0()
        .w_full()
        .child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(label),
        )
        .child(crate::views::components::readonly_value(
            format!("readonly-{label}"),
            path,
        ))
}
fn script_preview(value: &str, empty: &str) -> String {
    value
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(|| empty.into(), str::to_owned)
}
fn register_heading(title: &'static str, detail: &'static str) -> gpui::Div {
    div()
        .pt(rpx(SPACE_2XL))
        .pb(rpx(SPACE_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XS))
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
        .child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(detail),
        )
}
fn script_field(
    label: &'static str,
    input: &Entity<InputState>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .debug_selector(move || label.into())
        .min_w_0()
        .w_full()
        .h(rpx(TEXTAREA_H))
        .px(rpx(FIELD_INSET_X))
        .py(rpx(SPACE_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .rounded(rpx(RADIUS_PANEL))
        .bg(if focused {
            c::BG_HOVER()
        } else {
            c::FIELD_FILL()
        })
        .border_1()
        .border_color(c::BORDER_SOFT())
        .child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(label),
        )
        .child(
            gpui_component::input::Input::new(input)
                .file_drop_overlay(crate::views::components::file_drop_overlay())
                .aria_label(label)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .h(rpx(SCRIPT_INPUT_H))
                .font_family(crate::fonts::UI_FAMILY)
                .text_size(rpx(TEXT_CODE))
                .text_color(c::FG())
                .p_0(),
        )
}
impl Render for ProjectPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::FontWeight;
        let _ = &self.observers;
        let page = self.page.clone();
        let title = match page {
            Page::Edit(_) => "Edit project",
            Page::Remove(_) => "Remove project",
            Page::RemoveWorktree { .. } => "Delete worktree",
            Page::Move(_) => "Move project",
            Page::Archived => "Archived projects",
        };
        let edit_page = matches!(page, Page::Edit(_));
        let settings_origin = self.settings_compact.is_some();
        let settings_removal =
            settings_origin && matches!(page, Page::Remove(_) | Page::RemoveWorktree { .. });
        let compact = self.settings_compact.unwrap_or_default();
        let heading_size = if settings_removal {
            crate::views::settings_panel::settings_page_title_size(compact)
        } else if edit_page {
            TEXT_TITLE
        } else {
            CONTROL_H
        };
        let mut content =
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(if settings_removal {
                    SPACE_LG
                } else {
                    SPACE_2XL
                }));
        let archived_page = matches!(page, Page::Archived) && self.decision.is_none();
        let mut footer = crate::views::components::form_actions()
            .debug_selector(|| "project-panel-actions".into())
            .when(!settings_removal, |footer| {
                footer.max_w(rpx(FORM_CONTENT_MAX_W))
            })
            .when(settings_removal && !compact, gpui::Styled::justify_end);
        let mut heading = title.to_owned();
        let mut description: String = match page {
            Page::Edit(_) => {
                "Manage this project’s name, icon and scripts.".into()
            }
            Page::Remove(_) => "Review what Grove will remove before continuing.".into(),
            Page::RemoveWorktree { .. } => String::new(),
            Page::Move(_) => {
                "The project folder and worktrees stay in place. Open sessions keep running.".into()
            }
            Page::Archived => format!("Archived projects in workspace {}. Restore a project to its sidebar or remove its registration.", cx.global::<SettingsState>().store.workspaces.name(cx.global::<SettingsState>().store.workspaces.active)),
        };
        if let Some(decision) = self.decision.clone() {
            match decision {
                Decision::Archive => {
                    let Page::Edit(path) = &page else {
                        unreachable!()
                    };
                    let project = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .iter()
                        .find(|project| &project.path == path)
                        .cloned();
                    let name = project
                        .as_ref()
                        .map_or(path.as_str(), |project| project.name.as_str())
                        .to_owned();
                    let blockers = self
                        .service
                        .update(cx, |service, cx| service.archive_blockers(path, cx))
                        .unwrap_or_default();
                    heading = format!("Archive {name}?");
                    description = "The project moves to Archived projects. Its folder, worktrees, and files stay on disk.".into();
                    footer = footer.child(
                        project_action(
                            "project-decision-cancel".into(),
                            "Cancel",
                            None,
                            ProjectActionStyle::Quiet,
                            window,
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_decision(window, cx)),
                        ),
                    );
                    if !blockers.is_empty() {
                        content = content.child(
                            div()
                                .p(rpx(SPACE_2XL))
                                .rounded(rpx(RADIUS_CONTROL))
                                .bg(c::FIELD_FILL())
                                .text_color(c::AMBER())
                                .child(format!(
                                    "{} sessions are still open. Close them before archiving.",
                                    blockers.len()
                                )),
                        );
                        for id in &blockers {
                            if let Some(meta) = self.registry.read(cx).meta(*id) {
                                content = content.child(
                                    div()
                                        .py(rpx(SPACE_LG))
                                        .border_b_1()
                                        .border_color(c::BORDER_SOFT())
                                        .child(meta.label.clone())
                                        .child(
                                            div()
                                                .font_family(crate::fonts::UI_FAMILY)
                                                .text_size(rpx(TEXT_SMALL))
                                                .text_color(c::FG_DIM())
                                                .child(meta.wt_path.clone()),
                                        ),
                                );
                            }
                        }
                        if let Some(project) = project {
                            footer = footer.child(
                                project_action(
                                    "archive-close-all".into(),
                                    "Close all sessions",
                                    Some("icons/close.svg"),
                                    ProjectActionStyle::Primary,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.error = this
                                            .service
                                            .update(cx, |service, cx| {
                                                service.kill_sessions_for_project_path(
                                                    &project.path,
                                                    cx,
                                                )
                                            })
                                            .err();
                                        cx.notify();
                                    },
                                )),
                            );
                        }
                    } else {
                        content = content.child(div().p(rpx(SPACE_2XL)).rounded(rpx(RADIUS_CONTROL))
                            .bg(c::FIELD_FILL()).text_color(c::FG_DIM())
                            .child("You can restore this project from Archived projects at any time."));
                        footer = footer.child(
                            project_action(
                                "archive-confirm".into(),
                                "Archive project",
                                Some("icons/archive.svg"),
                                ProjectActionStyle::Primary,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.archive(cx))),
                        );
                    }
                }
                Decision::Delete(path) => {
                    let name = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .iter()
                        .find(|project| project.path == path)
                        .map_or(path.as_str(), |project| project.name.as_str())
                        .to_owned();
                    heading = format!("Remove {name} from Grove?");
                    description = "This removes the saved project entry and scripts.".into();
                    content = content
                        .child(readonly_path("Project folder", path.clone()))
                        .child(
                            div()
                                .p(rpx(SPACE_2XL))
                                .rounded(rpx(RADIUS_CONTROL))
                                .bg(c::FIELD_FILL())
                                .text_color(c::FG_DIM())
                                .child("The project folder, worktrees, and files stay on disk."),
                        );
                    footer = footer
                        .child(
                            project_action(
                                "project-decision-cancel".into(),
                                "Cancel",
                                None,
                                ProjectActionStyle::Quiet,
                                window,
                                cx,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cancel_decision(window, cx)),
                            ),
                        )
                        .child(
                            project_action(
                                "archive-delete-confirm".into(),
                                "Remove registration",
                                Some("icons/trash.svg"),
                                ProjectActionStyle::Danger,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    match this.service.update(cx, |service, cx| {
                                        service.delete_archived_by_path(&path, cx)
                                    }) {
                                        Ok(()) => {
                                            this.decision = None;
                                            cx.emit(ProjectPanelEvent::Decision(false));
                                        }
                                        Err(error) => this.error = Some(error),
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                }
            }
        } else {
            match page {
                Page::Edit(path) => {
                    let project = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .iter()
                        .find(|p| p.path == path)
                        .cloned();
                    let invalid_name = !grove_core::git::valid_project_name(
                        self.fields[0].read(cx).value().as_ref(),
                    );
                    if project.is_some() {
                        content = content
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_2XL))
                                    .child(div().min_w_0().w_full().child(project_form_field(
                                        "project-name",
                                        "Project name",
                                        &self.fields[0],
                                        if invalid_name {
                                            self.error.as_deref()
                                        } else {
                                            None
                                        },
                                        window,
                                        cx,
                                    ))),
                            )
                            .child(self.render_appearance(window, cx));
                    } else {
                        heading = "Directory scripts".into();
                        description = "Reusable actions for this directory.".into();
                    }
                    content = content.child(self.render_scripts(window, cx));
                    if project.is_some() {
                        content = content.child(div().w_full().py(rpx(SPACE_3XL)).border_t_1()
                            .border_color(c::BORDER_SOFT()).flex().items_center().gap(rpx(SPACE_2XL))
                            .child(div().flex_1().min_w_0().flex().flex_col().gap(rpx(SPACE_SM))
                                .child(div().font_weight(FontWeight::MEDIUM).child("Archive project"))
                                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                                    .child("Hide it from project pickers without deleting its folder.")))
                            .child(project_action("project-archive".into(), "Archive",
                                Some("icons/archive.svg"), ProjectActionStyle::Mini, window, cx)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.decide(Decision::Archive, window, cx);
                                }))));
                    }
                    footer = footer
                        .child(
                            div()
                                .debug_selector(|| "project-panel-cancel-action".into())
                                .flex_none()
                                .child(
                                    project_action(
                                        "project-panel-cancel".into(),
                                        "Cancel",
                                        None,
                                        ProjectActionStyle::Quiet,
                                        window,
                                        cx,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                                ),
                        )
                        .child(
                            project_action(
                                "project-save".into(),
                                "Save changes",
                                Some("icons/check.svg"),
                                ProjectActionStyle::Primary,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        );
                }
                Page::Move(path) => {
                    let store = &cx.global::<SettingsState>().store;
                    let name = store
                        .projects
                        .iter()
                        .find(|project| project.path == path)
                        .map_or(path.as_str(), |project| project.name.as_str());
                    heading = format!("Move {name}");
                    let source = store.project_workspace_id(&path);
                    let current = store.workspaces.name(source).to_owned();
                    let destinations: Vec<_> = store
                        .workspaces
                        .rows
                        .iter()
                        .filter(|workspace| workspace.id != source)
                        .cloned()
                        .collect();
                    let valid_destination = self
                        .destination
                        .is_some_and(|id| destinations.iter().any(|workspace| workspace.id == id));
                    content = content
                        .child(readonly_path("Current workspace", current))
                        .child(panel_section("Destination workspace"));
                    if destinations.is_empty() {
                        content = content.child(
                            div()
                                .text_color(c::FG_DIM())
                                .child("Create another workspace to move this project."),
                        );
                    }
                    for workspace in destinations {
                        let id = workspace.id;
                        let selected = self.destination == Some(id);
                        let full_name = workspace.name.clone();
                        let tooltip_name = full_name.clone();
                        content = content.child(
                            div()
                                .id(("project-move-destination", id))
                                .debug_selector(move || format!("project-move-destination-{id}"))
                                .role(gpui::Role::Button)
                                .cursor_pointer()
                                .aria_label(full_name.clone())
                                .aria_selected(selected)
                                .tooltip(move |window, cx| {
                                    crate::views::components::tooltip(tooltip_name.clone(), window)
                                        .build(window, cx)
                                })
                                .tab_index(0)
                                .w_full()
                                .min_w_0()
                                .h(rpx(DESTINATION_ROW_H))
                                .px(rpx(SPACE_2XL))
                                .rounded(rpx(RADIUS_PANEL))
                                .border_1()
                                .border_color(c::BORDER_SOFT())
                                .bg(if selected { c::BG_HOVER() } else { c::BG() })
                                .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                                .focus_visible(|style| style.border_color(c::FG()))
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .child(
                                    div()
                                        .w(rpx(SPACE_3XL))
                                        .flex_shrink_0()
                                        .when(selected, |slot| {
                                            slot.child(icon("check", ICON_MD, c::FG()))
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(full_name),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.destination = Some(id);
                                    this.error = None;
                                    cx.notify();
                                }))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            this.destination = Some(id);
                                            this.error = None;
                                            cx.stop_propagation();
                                            cx.notify();
                                        }
                                    },
                                )),
                        );
                    }
                    content = content.child(
                        div()
                            .w_full()
                            .min_h(rpx(PREFERENCE_ROW_MIN_H))
                            .py(rpx(SPACE_2XL))
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_2XL))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child("Open destination workspace after moving"),
                            )
                            .child(crate::views::components::boolean_choices(
                                "project-move-open-destination",
                                "Open destination workspace after moving",
                                self.open_destination,
                                true,
                                cx,
                                |this, enabled, _, cx| {
                                    this.open_destination = enabled;
                                    cx.notify();
                                },
                            )),
                    );
                    footer = footer
                        .child(
                            project_action(
                                "project-panel-cancel".into(),
                                "Cancel",
                                None,
                                ProjectActionStyle::Quiet,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        this.close(cx);
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                        )
                        .child(
                            project_action(
                                "project-move-submit".into(),
                                "Move project",
                                Some("icons/check.svg"),
                                ProjectActionStyle::Primary,
                                window,
                                cx,
                            )
                            .disabled(!valid_destination)
                            .on_click(cx.listener(|this, _, _, cx| this.move_project(cx)))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if valid_destination
                                        && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                    {
                                        this.move_project(cx);
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                        );
                }
                Page::Remove(path) => {
                    let name = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .iter()
                        .find(|p| p.path == path)
                        .map_or(path.as_str(), |project| project.name.as_str())
                        .to_owned();
                    let status = self
                        .started
                        .then(|| self.service.read(cx).removal_status(&path))
                        .flatten();
                    (heading, description) = match status {
                        Some(status) if status.finished && status.unregistered => (
                            "Project removed".into(),
                            "The main project folder stays on disk.".into(),
                        ),
                        Some(status) if status.finished => (
                            "Could not remove project".into(),
                            "Review the errors before trying again.".into(),
                        ),
                        _ if self.started => (
                            format!("Removing {name}"),
                            "Grove is closing sessions and removing the project registration.".into(),
                        ),
                        _ => (
                            format!("Remove {name} from Grove?"),
                            "Open sessions in this project will close. Grove will remove the project registration.".into(),
                        ),
                    };
                    if !self.started {
                        content = content
                            .when(self.delete_worktrees, |content| {
                                content.child(div().text_color(c::RED()).child(
                                    "Deleting these folders can erase uncommitted changes and untracked files. The main project folder stays on disk.",
                                ))
                            })
                            .child(readonly_path(
                                "Main project folder · protected",
                                path.clone(),
                            ))
                            .when(!self.discovery_done, |content| {
                                content.child(crate::views::status::label(
                                    "project-worktree-discovery",
                                    crate::views::status::StatusKind::Working,
                                    "Discovering worktrees…",
                                    ICON_SM,
                                    cx,
                                ))
                            });
                        if let Some(worktrees) = self
                            .reviewed_worktrees
                            .as_ref()
                            .map(|review| &review.verified)
                            .filter(|paths| !paths.is_empty())
                        {
                            let count = worktrees.len();
                            let count_label = format!(
                                "{count} linked worktree{}",
                                if count == 1 { "" } else { "s" }
                            );
                            content = content
                                .child(
                                    div()
                                        .w_full()
                                        .min_h(rpx(PREFERENCE_ROW_MIN_H))
                                        .py(rpx(SPACE_2XL))
                                        .border_y_1()
                                        .border_color(c::BORDER_SOFT())
                                        .flex()
                                        .items_center()
                                        .gap(rpx(SPACE_2XL))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .gap(rpx(SPACE_SM))
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child("Delete linked worktrees from disk"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(rpx(TEXT_SMALL))
                                                        .text_color(c::FG_DIM())
                                                        .child(count_label),
                                                ),
                                        )
                                        .child(crate::views::components::boolean_choices(
                                            "remove-worktrees",
                                            "Delete linked worktrees from disk",
                                            self.delete_worktrees,
                                            true,
                                            cx,
                                            |this, enabled, _, cx| {
                                                this.delete_worktrees = enabled;
                                                cx.notify();
                                            },
                                        )),
                                )
                                .when(self.delete_worktrees, |content| {
                                    content.child(
                                        div()
                                            .id("project-removal-reviewed-targets")
                                            .role(gpui::Role::Group)
                                            .aria_label("Worktree folders to delete")
                                            .flex()
                                            .flex_col()
                                            .gap(rpx(SPACE_SM))
                                            .child(
                                                div()
                                                    .text_color(c::FG_DIM())
                                                    .child("Worktree folders to delete"),
                                            )
                                            .children(worktrees.iter().enumerate().map(
                                                |(index, target)| {
                                                    crate::views::components::readonly_value(
                                                        ("project-removal-reviewed-target", index),
                                                        target.path().to_owned(),
                                                    )
                                                },
                                            )),
                                    )
                                });
                        }
                        if let Some(skipped) = self
                            .reviewed_worktrees
                            .as_ref()
                            .map(|review| &review.skipped_paths)
                            .filter(|paths| !paths.is_empty())
                        {
                            content = content.child(
                                div()
                                    .id("project-removal-skipped-paths")
                                    .role(gpui::Role::Group)
                                    .aria_label("Skipped worktree paths")
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_SM))
                                    .child(crate::views::status::label(
                                        "project-removal-skipped-notice",
                                        crate::views::status::StatusKind::Warning,
                                        "Unverified paths excluded from deletion",
                                        ICON_SM,
                                        cx,
                                    ))
                                    .children(skipped.iter().enumerate().map(|(index, path)| {
                                        crate::views::components::readonly_value(
                                            ("project-removal-skipped-path", index),
                                            path.clone(),
                                        )
                                    })),
                            );
                        }
                        content = content.when(!self.delete_worktrees, |content| {
                            content.child(
                                div()
                                    .text_color(c::FG_DIM())
                                    .child("All folders stay on disk."),
                            )
                        });
                        footer = footer
                            .child(
                                project_action(
                                    "project-panel-cancel".into(),
                                    "Cancel",
                                    None,
                                    ProjectActionStyle::Quiet,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                            .child(
                                project_action(
                                    "project-remove-confirm".into(),
                                    if self.delete_worktrees {
                                        "Delete worktrees and remove project"
                                    } else {
                                        "Remove project"
                                    },
                                    Some("icons/trash.svg"),
                                    ProjectActionStyle::Danger,
                                    window,
                                    cx,
                                )
                                .disabled(!self.discovery_done)
                                .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                            );
                    } else if let Some(status) = status {
                        content = content
                            .child(
                                div()
                                    .id("project-removal-status")
                                    .role(gpui::Role::Status)
                                    .child(crate::views::status::label(
                                        "project-removal-progress",
                                        if status.finished {
                                            if status.errors.is_empty() {
                                                crate::views::status::StatusKind::Success
                                            } else {
                                                crate::views::status::StatusKind::Warning
                                            }
                                        } else {
                                            crate::views::status::StatusKind::Working
                                        },
                                        format!(
                                            "{} of {} worktrees processed",
                                            status.completed, status.total
                                        ),
                                        ICON_SM,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .max_w(rpx(REMOVAL_PROGRESS_W))
                                    .h(rpx(SPACE_SM))
                                    .rounded(rpx(RADIUS_FULL))
                                    .bg(c::FIELD_FILL())
                                    .child(
                                        div()
                                            .h_full()
                                            .w(rpx(REMOVAL_PROGRESS_W
                                                * if status.total == 0 {
                                                    1.
                                                } else {
                                                    status.completed as f32 / status.total as f32
                                                }))
                                            .rounded(rpx(RADIUS_FULL))
                                            .bg(c::FG_DIM()),
                                    ),
                            );
                        if let Some(target) = &status.current_target {
                            content =
                                content.child(readonly_path("Current worktree", target.clone()));
                        }
                        for error in &status.errors {
                            content = content.child(
                                div()
                                    .id("project-removal-error")
                                    .role(gpui::Role::Alert)
                                    .p(rpx(SPACE_2XL))
                                    .rounded(rpx(RADIUS_CONTROL))
                                    .bg(c::RED_WASH())
                                    .text_color(c::RED())
                                    .child(crate::views::status::label(
                                        "project-operation-error",
                                        crate::views::status::StatusKind::Failed,
                                        error.clone(),
                                        ICON_SM,
                                        cx,
                                    )),
                            );
                        }
                        if status.finished {
                            if status.unregistered && !status.errors.is_empty() {
                                content = content.child(crate::views::status::label(
                                    "project-removal-result",
                                    crate::views::status::StatusKind::Warning,
                                    "Some reviewed worktree folders could not be deleted.",
                                    ICON_SM,
                                    cx,
                                ));
                            }
                            footer = footer.child(
                                project_action(
                                    "project-panel-cancel".into(),
                                    "Done",
                                    Some("icons/check.svg"),
                                    ProjectActionStyle::Primary,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            );
                        } else {
                            footer = footer.child(
                                project_action(
                                    "project-removing".into(),
                                    "Removing…",
                                    Some("icons/trash.svg"),
                                    ProjectActionStyle::Primary,
                                    window,
                                    cx,
                                )
                                .disabled(true),
                            );
                        }
                    }
                }
                Page::RemoveWorktree {
                    project_path,
                    path,
                    name,
                    branch,
                } => {
                    use crate::views::status::{self, StatusKind};
                    let operation = self
                        .started
                        .then(|| {
                            self.service
                                .read(cx)
                                .worktree_removal_status(&path)
                                .cloned()
                        })
                        .flatten();
                    heading = format!("Delete {name}?");
                    description = "Sessions using this worktree will stop.".into();
                    content = content
                        .when(!self.started, |content| {
                            content.child(div().text_color(c::RED()).child(
                                "Deleting this folder can erase uncommitted changes and untracked files. The main project folder stays on disk.",
                            ))
                        })
                        .child(readonly_path("Worktree folder", path))
                        .child(readonly_path("Branch", branch))
                        .child(readonly_path(
                            "Main project folder · protected",
                            project_path,
                        ));
                    if self.started {
                        let stage = operation.as_ref().map(|operation| operation.stage);
                        let error = operation
                            .as_ref()
                            .and_then(|operation| operation.error.as_ref());
                        let (kind, label) = match stage {
                            None | Some(WorktreeRemovalStage::Checking) => {
                                (StatusKind::Working, "Checking worktree")
                            }
                            Some(WorktreeRemovalStage::Stopping) => {
                                (StatusKind::Working, "Stopping sessions")
                            }
                            Some(WorktreeRemovalStage::RunningScript) => {
                                (StatusKind::Working, "Running teardown script")
                            }
                            Some(WorktreeRemovalStage::Removing) => {
                                (StatusKind::Working, "Removing worktree folder")
                            }
                            Some(WorktreeRemovalStage::Finished) if error.is_some() => {
                                (StatusKind::Failed, "Could not remove worktree")
                            }
                            Some(WorktreeRemovalStage::Finished) => {
                                (StatusKind::Success, "Worktree removed")
                            }
                        };
                        heading = if stage == Some(WorktreeRemovalStage::Finished) {
                            label.into()
                        } else {
                            format!("Removing {name}")
                        };
                        description = match kind {
                            StatusKind::Success => "The worktree folder was removed. The main project folder stays on disk.",
                            StatusKind::Failed => "Grove could not confirm removal. Review the error below before trying again.",
                            _ => "The main project folder stays on disk.",
                        }.into();
                        content = content.child(status::label(
                            "settings-worktree-removal-progress",
                            kind,
                            label,
                            ICON_SM,
                            cx,
                        ));
                        if let Some(error) = error {
                            content = content.child(status::label(
                                "settings-worktree-removal-error",
                                StatusKind::Failed,
                                error.clone(),
                                ICON_SM,
                                cx,
                            ));
                        }
                        if stage == Some(WorktreeRemovalStage::Finished) {
                            footer = footer.child(
                                project_action(
                                    "settings-worktree-removal-done".into(),
                                    if error.is_some() { "Dismiss" } else { "Done" },
                                    Some(if error.is_some() {
                                        "icons/close.svg"
                                    } else {
                                        "icons/check.svg"
                                    }),
                                    ProjectActionStyle::Primary,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            );
                        } else if stage == Some(WorktreeRemovalStage::RunningScript) {
                            footer = footer.child(
                                project_action(
                                    "settings-worktree-removal-skip".into(),
                                    "Skip script",
                                    Some("icons/arrow-right.svg"),
                                    ProjectActionStyle::Quiet,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.skip_worktree_teardown(window, cx),
                                )),
                            );
                        }
                    } else {
                        content = content.when(!self.discovery_done, |content| {
                            content.child(status::label(
                                "settings-worktree-removal-discovery",
                                StatusKind::Working,
                                "Verifying worktree…",
                                ICON_SM,
                                cx,
                            ))
                        });
                        footer = footer
                            .child(
                                project_action(
                                    "project-panel-cancel".into(),
                                    "Cancel",
                                    None,
                                    ProjectActionStyle::Quiet,
                                    window,
                                    cx,
                                )
                                .debug_selector(|| "settings-worktree-removal-cancel".into())
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                            .child(
                                project_action(
                                    "settings-worktree-removal-confirm".into(),
                                    "Delete worktree",
                                    Some("icons/trash.svg"),
                                    ProjectActionStyle::Danger,
                                    window,
                                    cx,
                                )
                                .debug_selector(|| "settings-worktree-removal-confirm".into())
                                .disabled(self.reviewed_worktree.is_none())
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.remove_worktree(window, cx),
                                )),
                            );
                    }
                }
                Page::Archived => {
                    let store = &cx.global::<SettingsState>().store;
                    let rows: Vec<_> = store
                        .projects
                        .iter()
                        .filter(|project| {
                            project.archived
                                && store.project_workspace_id(&project.path)
                                    == store.workspaces.active
                        })
                        .cloned()
                        .collect();
                    if rows.is_empty() {
                        content = content.child(div().min_h(rpx(EMPTY_STATE_MIN_H)).w_full().flex().flex_col()
                            .items_center().justify_center().text_center().gap(rpx(SPACE_LG))
                            .child(icon("archive", ICON_LG, c::FG_DIM()))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("No archived projects"))
                            .child(div().max_w(rpx(EMPTY_STATE_COPY_MAX_W)).text_size(rpx(TEXT_BODY)).text_color(c::FG_DIM())
                                .child("Projects you archive appear here. Their files remain in their folders.")));
                    }
                    for (index, project) in rows.into_iter().enumerate() {
                        let path = project.path.clone();
                        let delete = path.clone();
                        let overflow = self.archive_overflow.as_deref() == Some(&path);
                        let row = div()
                            .w_full()
                            .min_w_0()
                            .py(rpx(SPACE_2XL))
                            .border_b_1()
                            .border_color(c::BORDER_SOFT())
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_2XL))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_SM))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(project.name),
                                    )
                                    .child(
                                        div()
                                            .font_family(crate::fonts::UI_FAMILY)
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .truncate()
                                            .child(path.clone()),
                                    ),
                            )
                            .child(
                                project_action(
                                    ("restore-project", index).into(),
                                    "Restore",
                                    Some("icons/restore.svg"),
                                    ProjectActionStyle::Mini,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        match this.service.update(cx, |service, cx| {
                                            service.restore_archived_by_path(&path, cx)
                                        }) {
                                            Ok(()) => {
                                                cx.emit(ProjectPanelEvent::Selected(path.clone()));
                                            }
                                            Err(error) => this.error = Some(error),
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                project_action(
                                    ("delete-archived-project", index).into(),
                                    "More",
                                    Some("icons/more.svg"),
                                    ProjectActionStyle::Mini,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.archive_overflow.as_deref() == Some(&delete) {
                                            this.archive_overflow = None;
                                        } else {
                                            this.archive_overflow = Some(delete.clone());
                                        }
                                        cx.notify();
                                    },
                                )),
                            );
                        let mut item = div().w_full().min_w_0().flex().flex_col().child(row);
                        if overflow {
                            let delete = project.path.clone();
                            item = item.child(
                                div().w_full().flex().justify_end().child(
                                    div()
                                        .p(rpx(SPACE_SM))
                                        .bg(c::FIELD_FILL())
                                        .rounded(rpx(RADIUS_CONTROL))
                                        .border_1()
                                        .border_color(c::BORDER_SOFT())
                                        .child(
                                            project_action(
                                                ("delete-archived-registration", index).into(),
                                                "Remove from Grove",
                                                Some("icons/trash.svg"),
                                                ProjectActionStyle::Mini,
                                                window,
                                                cx,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, window, cx| {
                                                    this.archive_overflow = None;
                                                    this.decide(
                                                        Decision::Delete(delete.clone()),
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        ),
                                ),
                            );
                        }
                        content = content.child(item);
                    }
                }
            }
        }
        if let Some(error) = &self.error {
            if self.script_error.is_none()
                && (!edit_page
                    || self.decision.is_some()
                    || grove_core::git::valid_project_name(
                        self.fields[0].read(cx).value().as_ref(),
                    ))
            {
                content = content.child(
                    div()
                        .id("project-panel-error")
                        .role(gpui::Role::Alert)
                        .p(rpx(SPACE_2XL))
                        .rounded(rpx(RADIUS_CONTROL))
                        .bg(c::RED_WASH())
                        .text_color(c::FORM_ERROR())
                        .child(crate::views::status::label(
                            "project-operation-error",
                            crate::views::status::StatusKind::Failed,
                            error.clone(),
                            ICON_SM,
                            cx,
                        )),
                );
            }
        }
        div()
            .id("project-panel")
            .role(gpui::Role::Dialog)
            .aria_label(title)
            .tab_group()
            .debug_selector(|| "project-panel".into())
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c::BG())
            .text_color(c::FG())
            .text_size(rpx(TEXT_BODY))
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        if this.decision.is_some() {
                            this.cancel_decision(window, cx);
                        } else {
                            this.close(cx);
                        }
                        cx.stop_propagation();
                    } else if event.keystroke.key == "tab" && !settings_origin {
                        window.prevent_default();
                        for _ in 0..FOCUS_SCAN_LIMIT {
                            if event.keystroke.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            if this.focus.contains_focused(window, cx) {
                                break;
                            }
                        }
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                crate::views::components::content_header()
                    .h(rpx(FORM_PAGE_PADDING_Y))
                    .px(rpx(SPACE_3XL))
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .when(settings_removal, |header| {
                        header.h_auto().px_0().border_0().pt(rpx(if compact {
                            SPACE_20
                        } else {
                            SPACE_20 + SPACE_LG
                        }))
                    })
                    .when(archived_page || settings_origin, |header| {
                        header.child(
                            crate::views::components::back_action(
                                "project-panel-back",
                                "Back to Projects",
                                window,
                                cx,
                            )
                            .debug_selector(|| "project-panel-back".into())
                            .disabled(self.is_pending(cx))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                    })
                    .when(!settings_removal, |header| {
                        header.child(crate::views::components::content_header_title(title))
                    }),
            )
            .child(
                div()
                    .id("project-panel-scroll")
                    .debug_selector(|| "project-panel-scroll".into())
                    .when(!settings_removal, gpui::Styled::flex_1)
                    .when(settings_removal, gpui::Styled::flex_initial)
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .vertical_scrollbar(&self.scroll)
                    .when(!settings_removal, |scroll| scroll.px(rpx(SPACE_3XL)))
                    .flex()
                    .items_start()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .h_auto()
                            .w_full()
                            .min_w_0()
                            .when(!settings_removal, |container| {
                                container
                                    .min_h_full()
                                    .max_w(rpx(FORM_CONTENT_MAX_W))
                                    .py(rpx(FORM_PAGE_PADDING_Y))
                            })
                            .when(settings_removal, |container| container.pt(rpx(SPACE_LG)))
                            .child(
                                div()
                                    .mb(rpx(if edit_page {
                                        SPACE_3XL
                                    } else {
                                        FORM_HEADING_MARGIN
                                    }))
                                    .child(
                                        div()
                                            .text_size(rpx(heading_size))
                                            .font_weight(if settings_removal {
                                                FontWeight::BOLD
                                            } else {
                                                FontWeight::SEMIBOLD
                                            })
                                            .when(settings_removal, |title| {
                                                title.line_height(rpx(heading_size + SPACE_SM))
                                            })
                                            .child(heading),
                                    )
                                    .child(
                                        div()
                                            .mt(rpx(if settings_removal {
                                                SPACE_SM
                                            } else {
                                                SPACE_LG
                                            }))
                                            .text_size(rpx(if settings_removal {
                                                TEXT_TITLE
                                            } else {
                                                TEXT_BODY
                                            }))
                                            .text_color(c::FG_DIM())
                                            .child(description),
                                    ),
                            )
                            .child(content),
                    ),
            )
            .when(!archived_page, |panel| {
                panel.child(
                    div()
                        .id("project-panel-footer")
                        .debug_selector(|| "project-panel-footer".into())
                        .flex_shrink_0()
                        .when(!settings_removal, |footer| {
                            footer
                                .border_t_1()
                                .border_color(c::BORDER_SOFT())
                                .px(rpx(SPACE_3XL))
                                .py(rpx(SPACE_3XL))
                        })
                        .when(settings_removal, |footer| {
                            footer.pt(rpx(SPACE_20)).pb(rpx(SPACE_3XL))
                        })
                        .flex()
                        .justify_center()
                        .child(footer),
                )
            })
    }
}

const SCRIPT_INPUT_H: f32 = 96.;
const PREFERENCE_ROW_MIN_H: f32 = 70.;
const REMOVAL_PROGRESS_W: f32 = 300.;
const EMPTY_STATE_MIN_H: f32 = 220.;
const EMPTY_STATE_COPY_MAX_W: f32 = 280.;

#[cfg(test)]
mod tests;
