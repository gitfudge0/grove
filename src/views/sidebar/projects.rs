//! Project management in the main canvas, with path-stable decisions and retained progress.
use crate::{
    entities::session_registry::SessionRegistry,
    icons::icon,
    project_service::{ProjectEvent, ProjectService},
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
use gpui_component::{input::InputState, scroll::ScrollableElement, Disableable};
use grove_core::storage::ProjectScripts;

/// Bound focus traversal even if every decision action is disabled.
const FOCUS_SCAN_LIMIT: usize = 128;

#[derive(Clone)]
pub(super) enum ProjectPanelEvent {
    Closed,
    Selected(String),
    Decision(bool),
    Archived,
}
#[derive(Clone, PartialEq)]
pub(super) enum Page {
    Edit(String),
    Remove(String),
    Archived,
}
#[derive(Clone, PartialEq)]
enum Decision {
    Archive,
    Delete(String),
}
pub(super) struct ProjectPanel {
    pub(super) workspace: u64,
    service: Entity<ProjectService>,
    registry: Entity<SessionRegistry>,
    page: Page,
    fields: Vec<Entity<InputState>>,
    archive_overflow: Option<String>,
    error: Option<String>,
    decision: Option<Decision>,
    delete_worktrees: bool,
    worktree_count: Option<usize>,
    discovery_done: bool,
    started: bool,
    focus: FocusHandle,

    return_focus: Option<FocusHandle>,
    observers: Vec<Subscription>,
}
impl EventEmitter<ProjectPanelEvent> for ProjectPanel {}
impl ProjectPanel {
    pub(super) fn new(
        page: Page,
        service: Entity<ProjectService>,
        registry: Entity<SessionRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = match &page {
            Page::Edit(path) | Page::Remove(path) => cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .find(|p| &p.path == path)
                .cloned(),
            Page::Archived => None,
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
        let fields = values
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
        let observers = vec![
            cx.subscribe(&service, |_, _, event, cx| {
                if matches!(
                    event,
                    ProjectEvent::ProjectRemovalChanged { .. } | ProjectEvent::TreeInvalidated
                ) {
                    cx.notify();
                }
            }),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
        ];
        let panel = Self {
            workspace: cx.global::<SettingsState>().store.workspaces.active,
            service,
            registry,
            page,
            fields,
            archive_overflow: None,
            error: None,
            decision: None,
            delete_worktrees: false,
            worktree_count: None,
            discovery_done: false,
            started: false,
            focus: cx.focus_handle(),

            return_focus: None,
            observers,
        };
        if let Page::Remove(path) = &panel.page {
            let path = path.clone();
            cx.spawn(async move |this, cx| {
                let count = cx
                    .background_executor()
                    .spawn(async move {
                        if !grove_core::git::is_repo(&path) {return Ok(0)}
                        grove_core::git::list_worktrees_checked(&path).map(|worktrees|worktrees.iter().filter(|w| !w.is_main && w.path!=path).count()).map_err(|error|error.to_string())
                    }).await;
                let _ = this.update(cx, |this,cx| {
                    this.discovery_done=true;
                    match count {Ok(count)=>this.worktree_count=Some(count),Err(error)=>this.error=Some(format!("Could not discover worktrees: {error}. You can still remove the registration while keeping all files."))}
                    cx.notify();
                });
            })
            .detach();
            panel.focus.focus(window, cx);
        } else {
            panel.focus.focus(window, cx);
        }
        panel
    }
    pub(super) fn is_decision(&self) -> bool {
        self.decision.is_some() || matches!(self.page, Page::Remove(_)) && !self.started
    }
    fn pending(&self, cx: &App) -> bool {
        if let Page::Remove(path) = &self.page {
            self.started
                && self
                    .service
                    .read(cx)
                    .removal_status(path)
                    .is_some_and(|s| !s.finished)
        } else {
            false
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.pending(cx) {
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
    fn save(&mut self, cx: &mut Context<Self>) {
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
            run: script(2),
            teardown: script(3),
        };
        match self.service.update(cx, |service, cx| {
            service.update_project(&path, values[0].clone(), scripts, cx)
        }) {
            Ok(()) => cx.emit(ProjectPanelEvent::Selected(path)),
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
        match self.service.update(cx, |service, cx| {
            service.remove_project_by_path(&path, self.delete_worktrees, cx)
        }) {
            Ok(()) => {
                self.started = true;
                cx.emit(ProjectPanelEvent::Decision(false));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
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
    use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
    let (fill, fg, hover) = match style {
        ProjectActionStyle::Primary => (c::FG(), c::BG(), c::FG_DIM()),
        ProjectActionStyle::Danger => (c::RED(), c::BG(), c::RED()),
        ProjectActionStyle::Quiet => (c::BG(), c::FG_DIM(), c::BG_HOVER()),
        ProjectActionStyle::Mini => (c::FIELD_FILL(), c::FG(), c::BG_HOVER()),
    };
    let button = Button::new(id)
        .label(label)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(fg)
                .hover(hover)
                .active(hover)
                .shadow(false),
        )
        .h(rpx(if matches!(style, ProjectActionStyle::Mini) {
            30.
        } else {
            44.
        }))
        .rounded(gpui::px(
            RADIUS_PANEL * f32::from(window.rem_size()) / crate::zoom::REM_BASE,
        ));
    if let Some(glyph) = glyph {
        button.icon(gpui_component::Icon::default().path(glyph))
    } else {
        button
    }
}
fn panel_section(label: &'static str) -> impl IntoElement {
    div()
        .mt(rpx(SPACE_3XL))
        .mb(rpx(SPACE_LG))
        .text_size(rpx(TEXT_SMALL))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c::FG_DIM())
        .child(label)
}
fn readonly_path(label: &'static str, path: String) -> impl IntoElement {
    project_field_well(false, false)
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
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(crate::fonts::UI_FAMILY)
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(rpx(14.))
                .text_color(c::FG())
                .child(path),
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
        .min_w_0()
        .w_full()
        .h(rpx(130.))
        .px(rpx(14.))
        .py(rpx(SPACE_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .rounded(rpx(RADIUS_PANEL))
        .bg(c::FIELD_FILL())
        .border_1()
        .border_color(if focused {
            c::BORDER_STRONG()
        } else {
            c::BORDER_SOFT()
        })
        .child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(label),
        )
        .child(
            gpui_component::input::Input::new(input)
                .aria_label(label)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .h(rpx(96.))
                .font_family(crate::fonts::MONO_FAMILY)
                .text_size(rpx(TEXT_BODY))
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
            Page::Archived => "Archived projects",
        };
        let edit_page = matches!(page, Page::Edit(_));
        let mut content = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        let mut footer = div()
            .w_full()
            .max_w(rpx(620.))
            .min_w_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(rpx(SPACE_2XL));
        let mut heading = title.to_owned();
        let mut description: String = match page {
            Page::Edit(_) => "Update the project name and lifecycle scripts.".into(),
            Page::Remove(_) => "Review what Grove will remove before continuing.".into(),
            Page::Archived => "Restore a project to the sidebar or remove its registration.".into(),
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
                                                .font_family(crate::fonts::MONO_FAMILY)
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
                    content = content
                        .child(panel_section("GENERAL"))
                        .child(project_form_field(
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
                        ))
                        .child(readonly_path("Project folder", path.clone()))
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_MUTE())
                                .child("The folder stays in place when you rename this project."),
                        );
                    content = content
                        .child(panel_section("AUTOMATION"))
                        .child(script_field("Setup script", &self.fields[1], window, cx))
                        .child(script_field("Run script", &self.fields[2], window, cx))
                        .child(script_field("Teardown script", &self.fields[3], window, cx))
                        .child(panel_section("PROJECT MANAGEMENT"));
                    if project.is_some() {
                        content = content.child(div().w_full().py(rpx(SPACE_3XL)).border_t_1()
                            .border_color(c::BORDER_SOFT()).flex().items_center().gap(rpx(SPACE_2XL))
                            .child(div().flex_1().min_w_0().flex().flex_col().gap(rpx(SPACE_SM))
                                .child(div().font_weight(FontWeight::MEDIUM).child("Archive project"))
                                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                                    .child("Hide it from Projects without deleting its folder.")))
                            .child(project_action("project-archive".into(), "Archive",
                                Some("icons/archive.svg"), ProjectActionStyle::Mini, window, cx)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.decide(Decision::Archive, window, cx);
                                }))));
                    }
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
                Page::Remove(path) => {
                    let name = cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .iter()
                        .find(|p| p.path == path)
                        .map_or(path.as_str(), |project| project.name.as_str())
                        .to_owned();
                    heading = if self.started {
                        format!("Removing {name}")
                    } else {
                        format!("Remove {name} from Grove?")
                    };
                    description = if self.started {
                        "Grove is closing sessions and removing the project registration.".into()
                    } else {
                        "Open sessions in this project will close. Grove will forget the project registration.".into()
                    };
                    if !self.started {
                        let count = self.worktree_count.map_or_else(
                            || {
                                if self.discovery_done {
                                    "Worktree count unavailable".into()
                                } else {
                                    "Discovering worktrees…".into()
                                }
                            },
                            |n| format!("{n} additional worktree folders"),
                        );
                        let switch = div()
                            .id("remove-worktrees")
                            .role(gpui::Role::Switch)
                            .aria_label("Delete non-main worktrees from disk")
                            .aria_toggled(if self.delete_worktrees {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .tab_index(if self.worktree_count.is_some() { 0 } else { -1 })
                            .w_full()
                            .min_h(rpx(70.))
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
                                            .child("Delete non-main worktrees from disk"),
                                    )
                                    .child(
                                        div()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(count),
                                    ),
                            )
                            .child(
                                div()
                                    .w(rpx(48.))
                                    .h(rpx(28.))
                                    .flex_shrink_0()
                                    .p(rpx(3.))
                                    .rounded(rpx(RADIUS_FULL))
                                    .flex()
                                    .items_center()
                                    .bg(if self.delete_worktrees {
                                        c::GREEN()
                                    } else {
                                        c::BORDER_STRONG()
                                    })
                                    .when(self.delete_worktrees, gpui::Styled::justify_end)
                                    .child(
                                        div()
                                            .size(rpx(22.))
                                            .rounded(rpx(RADIUS_FULL))
                                            .bg(gpui::hsla(0., 0., 1., 1.)),
                                    ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.worktree_count.is_some() {
                                    this.delete_worktrees = !this.delete_worktrees;
                                    cx.notify();
                                }
                            }))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if this.worktree_count.is_some()
                                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                {
                                    this.delete_worktrees = !this.delete_worktrees;
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            }));
                        content = content.child(switch)
                            .child(readonly_path("Main project folder · protected", path.clone()))
                            .child(div().p(rpx(SPACE_2XL)).rounded(rpx(RADIUS_CONTROL))
                                .bg(if self.delete_worktrees { c::RED_WASH() } else { c::FIELD_FILL() })
                                .text_color(if self.delete_worktrees { c::RED() } else { c::FG_DIM() })
                                .child(if self.delete_worktrees {
                                    "Additional worktree folders will be deleted. This cannot be undone from Grove. The main project folder stays on disk."
                                } else {
                                    "The main project folder and all worktree files stay on disk."
                                }));
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
                                        "Delete worktrees and remove"
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
                    } else if let Some(status) = self.service.read(cx).removal_status(&path) {
                        content = content
                            .child(
                                div()
                                    .id("project-removal-status")
                                    .role(gpui::Role::Status)
                                    .child(format!(
                                        "{} of {} worktrees processed",
                                        status.completed, status.total
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .max_w(rpx(300.))
                                    .h(rpx(4.))
                                    .rounded(rpx(RADIUS_FULL))
                                    .bg(c::FIELD_FILL())
                                    .child(
                                        div()
                                            .h_full()
                                            .w(rpx(300.
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
                                    .child(error.clone()),
                            );
                        }
                        if status.finished {
                            content = content.child(div().text_color(if status.errors.is_empty() { c::FG_DIM() } else { c::RED() })
                                .child(if status.unregistered { "Project registration removed. The main project folder remains on disk." }
                                    else { "Project could not be unregistered. Review the errors above." }));
                            footer = footer.child(div()).child(
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
                            footer = footer.child(div()).child(
                                project_action(
                                    "project-removing".into(),
                                    "Removing…",
                                    None,
                                    ProjectActionStyle::Primary,
                                    window,
                                    cx,
                                )
                                .disabled(true),
                            );
                        }
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
                        content = content.child(div().min_h(rpx(220.)).w_full().flex().flex_col()
                            .items_center().justify_center().text_center().gap(rpx(SPACE_LG))
                            .child(icon("archive", ICON_LG, c::FG_DIM()))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("No archived projects"))
                            .child(div().max_w(rpx(280.)).text_size(rpx(TEXT_BODY)).text_color(c::FG_DIM())
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
                                            .font_family(crate::fonts::MONO_FAMILY)
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
                    footer = footer
                        .child(
                            project_action(
                                "project-panel-cancel".into(),
                                "Back to Projects",
                                Some("icons/arrow-left.svg"),
                                ProjectActionStyle::Quiet,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                        .child(div());
                }
            }
        }
        if let Some(error) = &self.error {
            if !edit_page
                || self.decision.is_some()
                || grove_core::git::valid_project_name(self.fields[0].read(cx).value().as_ref())
            {
                content = content.child(
                    div()
                        .id("project-panel-error")
                        .role(gpui::Role::Alert)
                        .p(rpx(SPACE_2XL))
                        .rounded(rpx(RADIUS_CONTROL))
                        .bg(c::RED_WASH())
                        .text_color(c::FORM_ERROR())
                        .child(error.clone()),
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
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.decision.is_some() {
                        this.cancel_decision(window, cx);
                    } else {
                        this.close(cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" && this.is_decision() {
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
            }))
            .child(
                div()
                    .h(rpx(36.))
                    .flex_shrink_0()
                    .px(rpx(SPACE_3XL))
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .id("project-panel-scroll")
                    .debug_selector(|| "project-panel-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px(rpx(SPACE_3XL))
                    .py(rpx(36.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(rpx(620.))
                            .min_w_0()
                            .child(
                                div()
                                    .mb(rpx(26.))
                                    .child(
                                        div()
                                            .text_size(rpx(24.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(heading),
                                    )
                                    .child(
                                        div()
                                            .mt(rpx(SPACE_LG))
                                            .text_size(rpx(13.))
                                            .text_color(c::FG_DIM())
                                            .child(description),
                                    ),
                            )
                            .child(content),
                    ),
            )
            .child(
                div()
                    .id("project-panel-footer")
                    .debug_selector(|| "project-panel-footer".into())
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .px(rpx(SPACE_3XL))
                    .py(rpx(SPACE_3XL))
                    .flex()
                    .justify_center()
                    .child(footer),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn project_editor_archive_restore_and_removal_keep_path_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        // Run persistence against a child-process-only config: no test changes the parent environment.
        if std::env::var_os("GROVE_PROJECT_PANEL_TEST_CHILD").is_none() {
            let root = std::env::temp_dir().join(format!(
                "grove-project-panel-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs_err::create_dir_all(&root).unwrap();
            let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","views::sidebar::projects::tests::project_editor_archive_restore_and_removal_keep_path_identity","--nocapture"]).env("GROVE_PROJECT_PANEL_TEST_CHILD","1").env("GROVE_CONFIG_DIR",&root).output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let path = std::env::var("GROVE_CONFIG_DIR").unwrap() + "/repository";
        fs_err::create_dir_all(&path).unwrap();
        let path = fs_err::canonicalize(&path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "old".into(),
                    path: path.clone(),
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..grove_core::storage::Store::default()
            }));
        });
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|cx| {
                crate::entities::workspace_state::WorkspaceState::new(
                    &cx.global::<SettingsState>().store,
                    MODAL_W_LG,
                )
            });
            let service = cx.new(|_| ProjectService::new(registry.clone(), state));
            ProjectPanel::new(Page::Edit(path.clone()), service, registry, window, cx)
        });
        draw(cx);
        for (width, height) in [(768., 560.), (1280., 800.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            draw(cx);
            let field = cx.debug_bounds("project-name").unwrap();
            let folder = cx.debug_bounds("Project folder").unwrap();
            let scroll = cx.debug_bounds("project-panel-scroll").unwrap();
            let footer = cx.debug_bounds("project-panel-footer").unwrap();
            assert!(f32::from(field.left()) >= 0.);
            assert!(f32::from(field.right()) <= width);
            assert_eq!(field.left(), folder.left());
            assert_eq!(field.right(), folder.right());
            assert_eq!(field.bottom() - field.top(), folder.bottom() - folder.top());
            assert!(f32::from(scroll.top()) < f32::from(footer.top()));
            assert!(f32::from(footer.bottom()) <= height);
        }
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                for (field, value) in
                    panel
                        .fields
                        .iter()
                        .zip(["renamed", "echo setup", "echo run", "echo teardown"])
                {
                    field.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                panel.fields[0].update(cx, |input, cx| input.set_value("", window, cx));
                panel.save(cx);
                assert!(panel.error.is_some());
                assert_eq!(panel.fields[1].read(cx).value().as_ref(), "echo setup");
                let mut collision = cx.global::<SettingsState>().store.projects[0].clone();
                collision.name = "taken".into();
                collision.path = format!("{path}/other");
                cx.global_mut::<SettingsState>()
                    .store
                    .projects
                    .push(collision);
                panel.fields[0].update(cx, |input, cx| input.set_value("taken", window, cx));
                panel.save(cx);
                assert!(panel.error.is_some());
                assert_eq!(panel.fields[1].read(cx).value().as_ref(), "echo setup");
                cx.global_mut::<SettingsState>().store.projects.pop();
                panel.fields[0].update(cx, |input, cx| input.set_value("renamed", window, cx));
                panel.save(cx);
                assert!(panel.error.is_none());
                let project = &cx.global::<SettingsState>().store.projects[0];
                assert_eq!(project.name, "renamed");
                assert_eq!(project.scripts.setup.as_deref(), Some("echo setup"));
                assert_eq!(project.scripts.run.as_deref(), Some("echo run"));
                assert_eq!(project.scripts.teardown.as_deref(), Some("echo teardown"));
                panel.registry.update(cx, |registry, _| {
                    registry.insert_meta(
                        "renamed".into(),
                        path.clone(),
                        grove_core::agent::Agent::Codex,
                    );
                });
                panel.decide(Decision::Archive, window, cx);
                panel.archive(cx);
                assert!(panel.error.is_some());
                assert!(!cx.global::<SettingsState>().store.projects[0].archived);
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.kill_sessions_for_project_path(&path, cx)
                    })
                    .unwrap();
                panel.error = None;
                panel.archive(cx);
                assert!(panel.error.is_none());
                assert!(cx.global::<SettingsState>().store.projects[0].archived);
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.restore_archived_by_path(&path, cx)
                    })
                    .unwrap();
                assert!(!cx.global::<SettingsState>().store.projects[0].archived);
                assert_eq!(
                    cx.global::<SettingsState>().store.projects[0]
                        .scripts
                        .run
                        .as_deref(),
                    Some("echo run")
                );
                panel
                    .service
                    .update(cx, |service, cx| service.archive_project_by_path(&path, cx))
                    .unwrap();
                panel.decide(Decision::Delete(path.clone()), window, cx);
                panel.cancel_decision(window, cx);
                assert_eq!(cx.global::<SettingsState>().store.projects.len(), 1);
                panel
                    .service
                    .update(cx, |service, cx| service.delete_archived_by_path(&path, cx))
                    .unwrap();
                assert!(cx.global::<SettingsState>().store.projects.is_empty());
                assert!(std::path::Path::new(&path).exists());
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.register_project_in_workspace(
                            "remove-me".into(),
                            path.clone(),
                            1,
                            cx,
                        )
                    })
                    .unwrap();
                panel.page = Page::Remove(path.clone());
                panel.worktree_count = Some(0);
                panel.discovery_done = true;
                panel.delete_worktrees = false;
                panel.remove(cx);
                assert!(panel.started);
                let status = panel.service.read(cx).removal_status(&path).unwrap();
                assert!(status.finished && status.unregistered);
                assert!(status.errors.is_empty());
                assert!(std::path::Path::new(&path).exists());
                panel.remove(cx);
                assert!(
                    panel
                        .service
                        .read(cx)
                        .removal_status(&path)
                        .unwrap()
                        .finished
                );
            });
        });
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.register_project_in_workspace(
                            "error-test".into(),
                            path.clone(),
                            1,
                            cx,
                        )
                    })
                    .unwrap();
                panel.started = false;
                panel.delete_worktrees = true;
                panel.remove(cx);
                assert!(panel.started);
            });
        });
        draw(cx);
        panel.read_with(cx, |panel, cx| {
            let status = panel.service.read(cx).removal_status(&path).unwrap();
            assert!(status.finished);
            assert!(!status.errors.is_empty());
            assert!(status.unregistered);
        });
        assert!(cx.debug_bounds("project-panel").is_some());
    }
}
