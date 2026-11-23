//! Worktree creation hosted by Settings → Projects.
use super::{resolved_source, suggested_branch};
use crate::{
    project_service::{worktree_prerequisite, ProjectService},
    settings::SettingsState,
    theme as c,
    views::{
        components::{form_action, form_actions, project_form_field},
        rpx,
        sidebar::{content::validate_worktree_fields, projects::readonly_path},
        status::{self, StatusKind},
        tokens::*,
    },
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Task,
    Window,
};
use gpui_component::{
    input::{InputEvent, InputState},
    scroll::ScrollableElement,
    Disableable, Icon,
};

pub(in crate::views) enum WorktreeSetupEvent {
    Completed { root: String, path: String },
    Cancelled,
}

enum Source {
    Checking,
    Blocked(&'static str),
    Resolving,
    Invalid,
    Ready(String),
}

pub(in crate::views) struct WorktreeSetup {
    project: String,
    workspace: u64,
    service: Entity<ProjectService>,
    focus: FocusHandle,
    fields: [Entity<InputState>; 3],
    errors: [Option<String>; 3],
    branch_edited: bool,
    source: Source,
    lookup: Option<Task<()>>,
    creating: bool,
    error: Option<String>,
    compact: bool,
    _inputs: Vec<gpui::Subscription>,
}

impl EventEmitter<WorktreeSetupEvent> for WorktreeSetup {}

impl Focusable for WorktreeSetup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl WorktreeSetup {
    pub(in crate::views) fn new(
        project: String,
        service: Entity<ProjectService>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));
        fields[2].update(cx, |input, cx| input.set_value("HEAD", window, cx));
        let inputs = fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                cx.subscribe_in(
                    field,
                    window,
                    move |this: &mut Self, _, event, window, cx| match event {
                        InputEvent::Change => this.change(index, window, cx),
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        _ => {}
                    },
                )
            })
            .collect();
        let mut form = Self {
            project,
            workspace: cx.global::<SettingsState>().store.workspaces.active,
            service,
            focus: cx.focus_handle(),
            fields,
            errors: Default::default(),
            branch_edited: false,
            source: Source::Checking,
            lookup: None,
            creating: false,
            error: None,
            compact,
            _inputs: inputs,
        };
        form.resolve(true, window, cx);
        form
    }

    pub(in crate::views) fn is_pending(&self) -> bool {
        self.creating
    }

    pub(in crate::views) fn update_settings_layout(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }

    fn project<'a>(&self, cx: &'a App) -> Option<&'a grove_core::storage::Project> {
        let store = &cx.global::<SettingsState>().store;
        (self.workspace == store.workspaces.active)
            .then(|| {
                store
                    .workspace_projects(self.workspace)
                    .find(|(_, project)| project.path == self.project)
                    .map(|(_, project)| project)
            })
            .flatten()
    }

    fn values(&self, cx: &App) -> [String; 3] {
        self.fields
            .each_ref()
            .map(|field| field.read(cx).value().trim().to_owned())
    }

    fn change(&mut self, field: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let [name, _, _] = self.values(cx);
        if field == 0 && !self.branch_edited {
            self.fields[1].update(cx, |input, cx| {
                input.set_value(suggested_branch(&name), window, cx);
            });
            self.errors[1] = None;
        }
        if field == 1 {
            self.branch_edited = true;
        }
        let [name, branch, base] = self.values(cx);
        let errors = validate_worktree_fields(&name, &branch, &base);
        self.errors[field].clone_from(&errors[field]);
        self.error = None;
        if field == 2 {
            self.resolve(false, window, cx);
        }
        cx.notify();
    }

    fn resolve(&mut self, check_prerequisite: bool, window: &Window, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let base = self.fields[2].read(cx).value().trim().to_owned();
        self.lookup = None;
        if base.is_empty() || base.starts_with('-') || base.contains(char::is_whitespace) {
            self.source = Source::Invalid;
            return;
        }
        self.source = if check_prerequisite {
            Source::Checking
        } else {
            Source::Resolving
        };
        let lookup = cx.background_spawn(async move {
            if check_prerequisite {
                if let Err(reason) = worktree_prerequisite(&project) {
                    return Source::Blocked(reason);
                }
            }
            resolved_source(&project, &base).map_or(Source::Invalid, Source::Ready)
        });
        self.lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let source = lookup.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.source = source;
                if check_prerequisite
                    && matches!(this.source, Source::Ready(_))
                    && this.focus.is_focused(window)
                {
                    this.fields[0].focus_handle(cx).focus(window, cx);
                }
                cx.notify();
            });
        }));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating || !matches!(self.source, Source::Ready(_)) {
            return;
        }
        let Some(project) = self.project(cx).cloned() else {
            return;
        };
        let [name, branch, base] = self.values(cx);
        self.errors = validate_worktree_fields(&name, &branch, &base);
        if self.errors.iter().any(Option::is_some) {
            cx.notify();
            return;
        }
        self.creating = true;
        self.error = None;
        self.focus.focus(window, cx);
        let creation = self.service.update(cx, |service, cx| {
            service.create_worktree_with_branch(&project, &name, &branch, Some(&base), cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = creation.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating = false;
                match result {
                    Ok(path) => cx.emit(WorktreeSetupEvent::Completed {
                        root: project.path,
                        path,
                    }),
                    Err(error) => {
                        this.error = Some(error);
                        this.fields[0].focus_handle(cx).focus(window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl gpui::Render for WorktreeSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.project(cx).cloned();
        let [name, branch, base] = self.values(cx);
        let valid = validate_worktree_fields(&name, &branch, &base)
            .iter()
            .all(Option::is_none);
        let title = project.as_ref().map_or_else(
            || "New worktree".to_owned(),
            |project| format!("New worktree for {}", project.name),
        );
        let store = &cx.global::<SettingsState>().store;
        let context = project.as_ref().map(|project| {
            format!(
                "{} · {}",
                project.name,
                store.workspaces.name(self.workspace)
            )
        });
        let base_error = self.errors[2].as_deref().or_else(|| {
            matches!(self.source, Source::Invalid)
                .then_some("Could not resolve source. Enter an existing branch or revision.")
        });
        let labels = ["Worktree name", "Branch name", "Base branch or revision"];
        let selectors = [
            "worktree-name-field",
            "worktree-branch-field",
            "worktree-base-field",
        ];
        let errors = [
            self.errors[0].as_deref(),
            self.errors[1].as_deref(),
            base_error,
        ];
        let values = [name.clone(), branch, base];
        let fields = std::array::from_fn::<_, 3, _>(|index| {
            div().flex_1().min_w_0().child(if self.creating {
                readonly_path(labels[index], values[index].clone()).into_any_element()
            } else {
                project_form_field(
                    selectors[index],
                    labels[index],
                    &self.fields[index],
                    errors[index],
                    window,
                    cx,
                )
                .into_any_element()
            })
        });
        let [worktree, branch, base] = fields;
        let destination = project.as_ref().and_then(|project| {
            (!name.is_empty())
                .then(|| {
                    grove_core::git::worktrees_root().ok().map(|root| {
                        root.join(project.worktree_dir())
                            .join(&name)
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .flatten()
        });
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .min_w_0()
            .child(
                div()
                    .text_size(rpx(crate::views::settings_panel::settings_page_title_size(
                        self.compact,
                    )))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .when_some(context, |body, context| {
                body.child(div().text_color(c::FG_DIM()).child(context))
            });
        match (&self.source, project.as_ref()) {
            (_, None) => {
                body = body.child(status::label(
                    "worktree-stale",
                    StatusKind::Failed,
                    "The selected project is no longer available in this workspace.",
                    ICON_SM,
                    cx,
                ));
            }
            (Source::Checking, _) => {
                body = body.child(status::label(
                    "worktree-prerequisite",
                    StatusKind::Working,
                    "Checking the project folder…",
                    ICON_SM,
                    cx,
                ));
            }
            (Source::Blocked(reason), _) => {
                body = body.child(status::label(
                    "worktree-prerequisite",
                    StatusKind::Warning,
                    *reason,
                    ICON_SM,
                    cx,
                ));
            }
            (source, _) => {
                body = body
                    .child(
                        div()
                            .flex()
                            .gap(rpx(SPACE_LG))
                            .min_w_0()
                            .when(self.compact, gpui::Styled::flex_col)
                            .child(worktree)
                            .child(branch),
                    )
                    .child(base);
                body = match source {
                    Source::Ready(source) => {
                        body.child(readonly_path("Resolved base", source.clone()))
                    }
                    Source::Resolving => body.child(status::label(
                        "worktree-resolution",
                        StatusKind::Working,
                        "Resolving source…",
                        ICON_SM,
                        cx,
                    )),
                    _ => body,
                };
                body = body.when_some(destination, |body, path| {
                    body.child(readonly_path("Worktree folder", path))
                });
            }
        }
        if self.creating {
            body = body.child(status::label(
                "worktree-creating",
                StatusKind::Working,
                "Creating worktree…",
                ICON_SM,
                cx,
            ));
        }
        body = body.when_some(self.error.clone(), |body, error| {
            body.child(status::label(
                "worktree-create-error",
                StatusKind::Failed,
                error,
                ICON_SM,
                cx,
            ))
        });
        let enabled = project.is_some() && matches!(self.source, Source::Ready(_)) && valid;
        div()
            .id("settings-worktree-setup")
            .debug_selector(|| "settings-worktree-setup".into())
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .id("settings-worktree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .py(rpx(SPACE_3XL))
                    .child(body),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .py(rpx(SPACE_3XL))
                    .child(
                        form_actions()
                            .when(self.compact, gpui::Styled::justify_start)
                            .child(
                                form_action("cancel-worktree", "Cancel", false, window, cx)
                                    .icon(Icon::default().path("icons/close.svg"))
                                    .disabled(self.creating)
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(WorktreeSetupEvent::Cancelled);
                                    })),
                            )
                            .child(
                                form_action("create-worktree", "Create worktree", true, window, cx)
                                    .icon(Icon::default().path("icons/plus.svg"))
                                    .loading(self.creating)
                                    .disabled(!enabled || self.creating)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.submit(window, cx)),
                                    ),
                            ),
                    ),
            )
    }
}
