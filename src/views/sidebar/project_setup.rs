//! Retained two-step project setup, scoped to the workspace that opened it.
use crate::{
    add_project::{self, ChooseOutcome, GitProbe, SubmitOutcome},
    icons::icon,
    modal::{AddProjectState, AddProjectStep},
    project_service::ProjectService,
    settings::SettingsState,
    theme as c,
    views::{
        components::{form_action, project_form_field},
        rpx,
        tokens::*,
    },
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription,
    Window,
};
use gpui_component::{
    input::{InputEvent, InputState},
    scroll::ScrollableElement,
    Disableable,
};

const HEADER_H: f32 = 36.;
const ACTION_H: f32 = 44.;
const SUGGESTION_H: f32 = 38.;
const MAX_SUGGESTIONS: usize = 8;
const SWITCH_W: f32 = 48.;
const SWITCH_H: f32 = 28.;
const SWITCH_KNOB: f32 = 22.;

pub(super) enum ProjectSetupEvent {
    Completed { path: String, workspace: u64 },
    Cancelled,
}
pub(super) struct ProjectSetup {
    workspace: u64,
    service: Entity<ProjectService>,
    state: AddProjectState,
    probe: GitProbe,
    path_input: Entity<InputState>,
    name_input: Entity<InputState>,
    suggestions: Vec<String>,
    suggestions_dismissed: bool,
    focus: FocusHandle,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<ProjectSetupEvent> for ProjectSetup {}
impl Focusable for ProjectSetup {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.state.step == AddProjectStep::PickSource {
            self.path_input.focus_handle(cx)
        } else {
            self.name_input.focus_handle(cx)
        }
    }
}
impl ProjectSetup {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        workspace: u64,
        service: Entity<ProjectService>,
    ) -> Self {
        let mut state = add_project::opened();
        state.path.clear();
        state.init_git = false;
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("~/Projects/my-project"));
        path_input.update(cx, |input, cx| {
            input.set_value(state.path.clone(), window, cx);
        });
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("my-project"));
        let subscriptions = vec![
            cx.subscribe_in(
                &path_input,
                window,
                |this: &mut Self, _, event, window, cx| match event {
                    InputEvent::Change if !this.busy => {
                        this.state.path = this.path_input.read(cx).value().to_string();
                        this.state.note = None;
                        this.state.dir_sel = 0;
                        this.refresh_suggestions();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } if !this.busy => {
                        let path = this.path_input.read(cx).value().to_string();
                        if std::path::Path::new(&add_project::shellexpand_tilde(&path)).is_dir() {
                            this.choose(window, cx);
                        } else if !this.suggestions_dismissed && !this.suggestions.is_empty() {
                            this.pick_suggestion(this.state.dir_sel, window, cx);
                        } else {
                            this.choose(window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &name_input,
                window,
                |this: &mut Self, _, event, window, cx| match event {
                    InputEvent::Change if !this.busy => {
                        this.state.name = this.name_input.read(cx).value().to_string();
                        this.state.note = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } if !this.busy => this.submit(window, cx),
                    _ => {}
                },
            ),
        ];
        path_input.update(cx, |input, cx| input.focus(window, cx));
        let mut result = Self {
            workspace,
            service,
            state,
            probe: GitProbe::NotRepo,
            path_input,
            name_input,
            suggestions: Vec::new(),
            suggestions_dismissed: false,
            focus: cx.focus_handle(),
            busy: false,
            _subscriptions: subscriptions,
        };
        result.refresh_suggestions();
        result
    }
    fn refresh_suggestions(&mut self) {
        self.suggestions = if self.state.path.trim().is_empty() {
            Vec::new()
        } else {
            add_project::list_dirs(&self.state.path)
                .into_iter()
                .take(MAX_SUGGESTIONS)
                .collect()
        };
        self.suggestions_dismissed = false;
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.state.step != AddProjectStep::PickSource {
            return;
        }
        self.state.path = self.path_input.read(cx).value().to_string();
        match add_project::choose_typed(&mut self.state) {
            ChooseOutcome::Advanced(probe) => {
                self.probe = probe;
                self.state.init_git = false;
                if self.state.name.trim().is_empty() {
                    self.state.name = add_project::path_basename(&self.state.path);
                }
                self.name_input.update(cx, |input, cx| {
                    input.set_value(self.state.name.clone(), window, cx);
                    input.focus(window, cx);
                });
            }
            ChooseOutcome::Rejected => {}
        }
        cx.notify();
    }
    fn pick_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.suggestions.get(index).cloned() else {
            return;
        };
        self.path_input.update(cx, |input, cx| {
            input.set_value(format!("{path}/"), window, cx);
            input.focus(window, cx);
        });
        self.state.path = format!("{path}/");
        self.state.dir_sel = 0;
        self.refresh_suggestions();
        cx.notify();
    }
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            let path = path.to_string_lossy().to_string();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.busy || this.state.step != AddProjectStep::PickSource {
                    return;
                }
                this.path_input
                    .update(cx, |input, cx| input.set_value(path, window, cx));
                this.choose(window, cx);
            });
        })
        .detach();
    }
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.state.name = self.name_input.read(cx).value().to_string();
        add_project::change_source(&mut self.state);
        self.path_input.update(cx, |input, cx| {
            input.set_value(self.state.path.clone(), window, cx);
            input.focus(window, cx);
        });
        self.refresh_suggestions();
        cx.notify();
    }
    fn submit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.state.step != AddProjectStep::Details {
            return;
        }
        self.state.name = self.name_input.read(cx).value().to_string();
        let store = &cx.global::<SettingsState>().store;
        if !store
            .workspaces
            .rows
            .iter()
            .any(|workspace| workspace.id == self.workspace)
        {
            self.state.note = Some(
                "The destination workspace no longer exists. Cancel and choose a workspace.".into(),
            );
            cx.notify();
            return;
        }
        if !std::path::Path::new(&self.state.path).is_dir() {
            self.state.note = Some(
                "The selected folder no longer exists. Go back and choose another folder.".into(),
            );
            cx.notify();
            return;
        }
        let existing = store
            .projects
            .iter()
            .map(|project| {
                (
                    project.name.clone(),
                    fs_err::canonicalize(&project.path).map_or_else(
                        |_| project.path.clone(),
                        |path| path.to_string_lossy().into_owned(),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let SubmitOutcome::Register {
            name,
            path,
            init_git,
        } = add_project::validate_submit(&mut self.state, &self.probe, &existing)
        else {
            cx.notify();
            return;
        };
        if !grove_core::git::valid_project_name(&name) {
            self.state.note =
                Some("Enter a valid project name without slashes or consecutive dots.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.state.note = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let init_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if init_git {
                        grove_core::git::init_if_needed(&init_path)
                            .map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = result {
                    this.state.note = Some(format!("Could not initialize Git: {error}"));
                    cx.notify();
                    return;
                }
                let result = this.service.update(cx, |service, cx| {
                    service.register_project_in_workspace(name, path.clone(), this.workspace, cx)
                });
                match result {
                    Ok(_) => cx.emit(ProjectSetupEvent::Completed {
                        path,
                        workspace: this.workspace,
                    }),
                    Err(error) => this.state.note = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for ProjectSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::FontWeight;
        use gpui_component::input::Input;
        let source = self.state.step == AddProjectStep::PickSource;
        let error = self.state.note.clone();
        let body_heading = if source {
            "Add project"
        } else {
            "Project details"
        };
        let body_description = if source {
            "Choose the folder Grove should manage."
        } else {
            "Confirm the name and repository state before adding it."
        };
        let mut fields = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        if source {
            let path_focused = self.path_input.read(cx).focus_handle(cx).is_focused(window);
            let path_fill = if path_focused {
                c::BG_HOVER()
            } else {
                c::FIELD_FILL()
            };
            let path_field = div()
                .id("setup-path")
                .debug_selector(|| "setup-path".into())
                .role(gpui::Role::Group)
                .aria_label("Project folder")
                .when_some(error.as_deref(), |row, message| {
                    row.aria_description(message.to_string())
                })
                .min_w_0()
                .w_full()
                .h(rpx(60.))
                .px(rpx(14.))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(if error.is_some() {
                    c::FORM_ERROR()
                } else if path_focused {
                    c::BORDER_STRONG()
                } else {
                    c::BORDER_SOFT()
                })
                .bg(path_fill)
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("Project folder"),
                        )
                        .child(
                            Input::new(&self.path_input)
                                .aria_label("Project folder")
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .text_size(rpx(14.))
                                .text_color(c::FG())
                                .p_0(),
                        ),
                )
                .child(
                    form_action("project-browse", "Browse", false, window, cx)
                        .icon(gpui_component::Icon::default().path("icons/folder.svg"))
                        .h(rpx(30.))
                        .on_click(cx.listener(Self::browse_click)),
                );
            fields = fields.child(path_field);
            if let Some(message) = error {
                fields = fields.child(
                    div()
                        .id("setup-path-error")
                        .role(gpui::Role::Alert)
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FORM_ERROR())
                        .child(message),
                );
            } else {
                fields = fields.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Grove uses this folder as the project root. Nothing is moved."),
                );
            }
            if !self.suggestions_dismissed && !self.suggestions.is_empty() {
                let mut suggestions = div()
                    .id("project-folder-suggestions")
                    .role(gpui::Role::ListBox)
                    .aria_label("Matching folders")
                    .w_full()
                    .max_h(rpx(SUGGESTION_H * MAX_SUGGESTIONS as f32))
                    .p(rpx(SPACE_SM))
                    .rounded(rpx(RADIUS_PANEL))
                    .border_1()
                    .border_color(c::BORDER_SOFT())
                    .bg(c::FIELD_FILL())
                    .overflow_y_scrollbar();
                for (index, path) in self.suggestions.iter().enumerate() {
                    let path = path.clone();
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .map_or_else(|| path.clone(), |part| part.to_string_lossy().into_owned());
                    suggestions = suggestions.child(
                        div()
                            .id(("project-folder", index))
                            .role(gpui::Role::ListBoxOption)
                            .aria_label(path.clone())
                            .aria_selected(index == self.state.dir_sel)
                            .tab_index(if index == self.state.dir_sel { 0 } else { -1 })
                            .h(rpx(SUGGESTION_H))
                            .px(rpx(SPACE_LG))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .rounded(rpx(RADIUS_CONTROL))
                            .when(index == self.state.dir_sel, |row| row.bg(c::BG_HOVER()))
                            .hover(|row| row.bg(c::BG_HOVER()))
                            .child(icon("folder", ICON_SM, c::FG_DIM()))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(rpx(TEXT_BODY))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(rpx(210.))
                                    .truncate()
                                    .font_family(crate::fonts::MONO_FAMILY)
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(path),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_suggestion(index, window, cx);
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        && !event.is_held
                                    {
                                        this.pick_suggestion(index, window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                    );
                }
                fields = fields.child(suggestions).child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_MUTE())
                        .child("↑ ↓ choose · Tab accept · Esc dismiss"),
                );
            }
        } else {
            fields = fields.child(project_form_field(
                "setup-name",
                "Project name",
                &self.name_input,
                error.as_deref(),
                window,
                cx,
            ));
            fields = fields.child(
                div()
                    .w_full()
                    .min_w_0()
                    .h(rpx(60.))
                    .px(rpx(14.))
                    .rounded(rpx(RADIUS_PANEL))
                    .bg(c::FIELD_FILL())
                    .border_1()
                    .border_color(c::BORDER_SOFT())
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Project folder"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(crate::fonts::MONO_FAMILY)
                            .text_size(rpx(TEXT_CODE))
                            .child(self.state.path.clone()),
                    ),
            );
            match &self.probe {
                GitProbe::Repo { branch } => {
                    fields = fields.child(
                        div()
                            .min_w_0()
                            .py(rpx(SPACE_2XL))
                            .border_b_1()
                            .border_color(c::BORDER_SOFT())
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_2XL))
                            .child(icon("git-branch", ICON_SM, c::FG_DIM()))
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
                                            .child("Git repository detected"),
                                    )
                                    .child(
                                        div()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(format!(
                                                "{branch} · working tree at the selected folder"
                                            )),
                                    ),
                            )
                            .child(icon("check", ICON_SM, c::GREEN())),
                    );
                }
                GitProbe::NotRepo => {
                    fields = fields.child(div().id("initialize-project-git")
                        .role(gpui::Role::Switch).aria_label("Initialize Git repository")
                        .aria_toggled(if self.state.init_git { gpui::Toggled::True } else { gpui::Toggled::False })
                        .tab_index(if self.busy { -1 } else { 0 })
                        .w_full().min_h(rpx(60.)).py(rpx(SPACE_2XL))
                        .border_b_1().border_color(c::BORDER_SOFT())
                        .flex().items_center().gap(rpx(SPACE_2XL))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(rpx(SPACE_SM))
                            .child(div().font_weight(FontWeight::MEDIUM).child("Initialize Git repository"))
                            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                                .child("Create a repository in this folder when the project is added.")))
                        .child(div().w(rpx(SWITCH_W)).h(rpx(SWITCH_H)).flex_shrink_0()
                            .p(rpx(3.)).rounded(rpx(RADIUS_FULL)).flex().items_center()
                            .bg(if self.state.init_git { c::GREEN() } else { c::BORDER_STRONG() })
                            .when(self.state.init_git, gpui::Styled::justify_end)
                            .child(div().size(rpx(SWITCH_KNOB)).rounded(rpx(RADIUS_FULL))
                                .bg(gpui::hsla(0., 0., 1., 1.))))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.busy { this.state.init_git = !this.state.init_git; cx.notify(); }
                        }))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if !this.busy && matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                                this.state.init_git = !this.state.init_git;
                                cx.notify();
                                cx.stop_propagation();
                            }
                        })))
                    .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_MUTE())
                        .child("Leave this off to manage the folder without Git worktrees."));
                }
            }
            if self.busy {
                fields = fields.child(
                    div()
                        .id("setup-name-pending")
                        .role(gpui::Role::Status)
                        .aria_label("Adding project")
                        .text_color(c::FG_DIM())
                        .child("Checking the folder and saving to Grove…"),
                );
            }
        }
        let secondary = if source {
            form_action("project-cancel", "Cancel", false, window, cx)
                .h(rpx(ACTION_H))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectSetupEvent::Cancelled)))
                .into_any_element()
        } else {
            form_action("project-back", "Back", false, window, cx)
                .icon(gpui_component::Icon::default().path("icons/arrow-left.svg"))
                .h(rpx(ACTION_H))
                .disabled(self.busy)
                .on_click(cx.listener(|this, _, window, cx| this.back(window, cx)))
                .into_any_element()
        };
        let primary = if source {
            form_action("project-choose", "Continue", true, window, cx)
                .icon(gpui_component::Icon::default().path("icons/arrow-right.svg"))
                .h(rpx(ACTION_H))
                .disabled(self.path_input.read(cx).value().trim().is_empty())
                .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx)))
                .into_any_element()
        } else {
            form_action(
                "project-submit",
                if self.busy {
                    "Adding project…"
                } else {
                    "Add project"
                },
                true,
                window,
                cx,
            )
            .icon(gpui_component::Icon::default().path("icons/plus.svg"))
            .h(rpx(ACTION_H))
            .disabled(self.busy)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
            .into_any_element()
        };
        div()
            .id("project-setup")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if this.busy {
                    return;
                }
                let key = event.keystroke.key.as_str();
                if key == "escape" {
                    if this.state.step == AddProjectStep::Details {
                        this.back(window, cx);
                    } else if !this.suggestions_dismissed && !this.suggestions.is_empty() {
                        this.suggestions_dismissed = true;
                        cx.notify();
                    } else {
                        cx.emit(ProjectSetupEvent::Cancelled);
                    }
                    cx.stop_propagation();
                } else if this.state.step == AddProjectStep::PickSource
                    && !this.suggestions.is_empty()
                    && !this.suggestions_dismissed
                    && (this.path_input.focus_handle(cx).is_focused(window)
                        || matches!(key, "up" | "down"))
                {
                    match key {
                        "up" => this.state.dir_sel = this.state.dir_sel.saturating_sub(1),
                        "down" => {
                            this.state.dir_sel =
                                (this.state.dir_sel + 1).min(this.suggestions.len() - 1);
                        }
                        "tab" if !event.keystroke.modifiers.shift => {
                            this.pick_suggestion(this.state.dir_sel, window, cx);
                        }
                        _ => return,
                    }
                    this.path_input
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .h(rpx(HEADER_H))
                    .flex_shrink_0()
                    .px(rpx(SPACE_3XL))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .text_size(rpx(TEXT_BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .child(body_heading),
            )
            .child(
                div()
                    .id("project-setup-scroll")
                    .debug_selector(|| "project-setup-scroll".into())
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
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .mb(rpx(26.))
                                    .child(
                                        div()
                                            .text_size(rpx(24.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(body_heading),
                                    )
                                    .child(
                                        div()
                                            .mt(rpx(SPACE_LG))
                                            .text_size(rpx(13.))
                                            .text_color(c::FG_DIM())
                                            .child(body_description),
                                    ),
                            )
                            .child(fields),
                    ),
            )
            .child(
                div()
                    .id("project-setup-footer")
                    .debug_selector(|| "project-setup-footer".into())
                    .w_full()
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .px(rpx(SPACE_3XL))
                    .py(rpx(SPACE_3XL))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(rpx(620.))
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(rpx(SPACE_2XL))
                            .child(secondary)
                            .child(primary),
                    ),
            )
    }
}
impl ProjectSetup {
    fn browse_click(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.browse(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "grove-project-setup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs_err::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }
    #[gpui::test]
    fn submit_registers_in_captured_workspace(cx: &mut gpui::TestAppContext) {
        const MARKER: &str = "GROVE_PROJECT_SETUP_TEST_CHILD";
        if std::env::var_os(MARKER).is_none() {
            let config = TestDirectory::new();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "views::sidebar::project_setup::tests::submit_registers_in_captured_workspace",
                    "--test-threads=1",
                ])
                .env(MARKER, "1")
                .env("GROVE_CONFIG_DIR", config.path())
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let directory = TestDirectory::new();
        let canonical = fs_err::canonicalize(directory.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (setup, cx) = fixture(cx);
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>()
                .store
                .workspaces
                .create("Other")
                .unwrap();
            setup.update(cx, |setup, cx| {
                setup.path_input.update(cx, |input, cx| {
                    input.set_value(format!("{canonical}/."), window, cx);
                });
                setup.choose(window, cx);
                setup.name_input.update(cx, |input, cx| {
                    input.set_value("Captured project", window, cx);
                });
                setup.submit(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let store = &cx.global::<SettingsState>().store;
            assert_ne!(store.workspaces.active, 1);
            assert_eq!(store.projects.len(), 1);
            assert_eq!(store.projects[0].path, canonical);
            assert_eq!(store.project_workspace_id(&canonical), 1);
            assert!(!setup.read(cx).busy);
            assert!(setup.read(cx).state.note.is_none());
        });
        assert!(!directory.path().join(".git").exists());
    }
    fn fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<ProjectSetup>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            ProjectSetup::new(window, cx, 1, service)
        })
    }
    #[gpui::test]
    fn choosing_folder_is_read_only_and_back_preserves_name(cx: &mut gpui::TestAppContext) {
        let directory = TestDirectory::new();
        let path = directory.path().to_string_lossy().to_string();
        let (setup, cx) = fixture(cx);
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                setup
                    .path_input
                    .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                setup.choose(window, cx);
                assert_eq!(setup.state.step, AddProjectStep::Details);
                assert_eq!(setup.probe, GitProbe::NotRepo);
                assert!(!setup.state.init_git);
                setup
                    .name_input
                    .update(cx, |input, cx| input.set_value("retained-name", window, cx));
                setup.back(window, cx);
                assert_eq!(setup.state.step, AddProjectStep::PickSource);
                assert_eq!(setup.state.name, "retained-name");
                setup.choose(window, cx);
                assert_eq!(setup.name_input.read(cx).value().as_ref(), "retained-name");
            });
        });
        assert!(!directory.path().join(".git").exists());
    }
    #[gpui::test]
    fn keyboard_suggestions_and_details_escape_preserve_draft(cx: &mut gpui::TestAppContext) {
        let directory = TestDirectory::new();
        for name in ["alpha", "beta"] {
            fs_err::create_dir(directory.path().join(name)).unwrap();
        }
        let (setup, cx) = fixture(cx);
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                setup.state.path = format!("{}/", directory.path().display());
                setup.path_input.update(cx, |input, cx| {
                    input.set_value(setup.state.path.clone(), window, cx);
                });
                setup.refresh_suggestions();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("down tab");
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                assert!(setup.state.path.trim_end_matches('/').ends_with("beta"));
                setup.choose(window, cx);
                setup
                    .name_input
                    .update(cx, |input, cx| input.set_value("bad/name", window, cx));
                setup.submit(window, cx);
                assert!(setup.state.note.is_some());
                assert_eq!(setup.name_input.read(cx).value().as_ref(), "bad/name");
                assert!(!setup.busy);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert_eq!(setup.read(cx).state.step, AddProjectStep::PickSource);
            assert_eq!(setup.read(cx).state.name, "bad/name");
        });
    }
    #[gpui::test]
    fn missing_folder_keeps_source_step_and_inline_error(cx: &mut gpui::TestAppContext) {
        let directory = TestDirectory::new();
        let missing = directory
            .path()
            .join("missing")
            .to_string_lossy()
            .to_string();
        let (setup, cx) = fixture(cx);
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                setup
                    .path_input
                    .update(cx, |input, cx| input.set_value(missing, window, cx));
                setup.choose(window, cx);
                assert_eq!(setup.state.step, AddProjectStep::PickSource);
                assert!(setup.state.note.is_some());
                assert!(!setup.busy);
            });
        });
    }
    #[gpui::test]
    fn source_and_details_keep_fields_above_pinned_actions_at_narrow_width(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = TestDirectory::new();
        let (setup, cx) = fixture(cx);
        for width in [768., 1280.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(620.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let field = cx.debug_bounds("setup-path").unwrap();
            let footer = cx.debug_bounds("project-setup-footer").unwrap();
            assert!(f32::from(field.left()) >= 0.);
            assert!(f32::from(field.right()) <= width);
            assert!(f32::from(field.bottom()) < f32::from(footer.top()));
        }
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                setup.path_input.update(cx, |input, cx| {
                    input.set_value(directory.path().to_string_lossy().to_string(), window, cx);
                });
                setup.choose(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let field = cx.debug_bounds("setup-name").unwrap();
        let footer = cx.debug_bounds("project-setup-footer").unwrap();
        assert!(f32::from(field.bottom()) < f32::from(footer.top()));
    }
}
