//! Draft-only selection of the exact checkouts shared by one conversation.
use crate::{
    icons::icon,
    theme as c,
    views::{components, motion, rpx, tokens::*},
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    ScrollHandle, SharedString, Window,
};
use gpui_component::{
    input::{InputEvent, InputState},
    Disableable,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeTarget {
    pub project: usize,
    pub project_path: String,
    pub project_name: String,
    pub path: String,
    pub branch: String,
}

pub(super) enum ScopePickerEvent {
    Applied(Vec<ScopeTarget>, String),
    Cancelled,
}

pub(super) struct ScopePicker {
    projects: Vec<(usize, String, String)>,
    targets: Vec<ScopeTarget>,
    selected: Vec<String>,
    cwd: String,
    input: Entity<InputState>,
    _input_events: gpui::Subscription,
    focus: FocusHandle,
    row_focus: HashMap<String, FocusHandle>,
    cancel_focus: FocusHandle,
    confirm_focus: FocusHandle,
    retry_focus: FocusHandle,
    loading: bool,
    closing: bool,
    close_task: Option<gpui::Task<()>>,
    generation: u64,
    error: Option<String>,
    query: String,
    scroll: ScrollHandle,
    presence: motion::Presence,
}
impl EventEmitter<ScopePickerEvent> for ScopePicker {}

impl ScopePicker {
    pub(super) fn new(
        projects: Vec<(usize, String, String)>,
        selected_paths: Vec<String>,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Project, branch, or path"));
        let input_events = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = this.input.read(cx).value().to_lowercase();
                cx.notify();
            }
        });
        let mut seen = HashSet::new();
        let selected = selected_paths
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect();
        let mut this = Self {
            projects,
            targets: Vec::new(),
            selected,
            cwd,
            input,
            _input_events: input_events,
            focus: cx.focus_handle(),
            row_focus: HashMap::new(),
            cancel_focus: cx.focus_handle(),
            confirm_focus: cx.focus_handle(),
            retry_focus: cx.focus_handle(),
            loading: false,
            closing: false,
            close_task: None,
            generation: 0,
            error: None,
            query: String::new(),
            scroll: ScrollHandle::new(),
            presence: motion::Presence::fast(),
        };
        this.presence.set(true, cx);
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let projects = self.projects.clone();
        cx.spawn(async move |this, cx| {
            let (targets, errors) = cx
                .background_executor()
                .spawn(async move {
                    let mut targets = Vec::new();
                    let mut errors = Vec::new();
                    let mut paths = HashSet::new();
                    for (project, name, project_path) in projects {
                        match grove_core::git::list_worktrees_checked(&project_path) {
                            Ok(rows) => {
                                for row in rows {
                                    if paths.insert(row.path.clone()) {
                                        targets.push(ScopeTarget {
                                            project,
                                            project_path: project_path.clone(),
                                            project_name: name.clone(),
                                            path: row.path,
                                            branch: row.branch,
                                        });
                                    }
                                }
                            }
                            Err(error) => errors.push(format!("{name}: {error}")),
                        }
                    }
                    (targets, errors)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.targets = targets;
                this.loading = false;
                this.error = (!errors.is_empty()).then(|| errors.join("\n"));
                for path in this
                    .selected
                    .iter()
                    .chain(this.targets.iter().map(|t| &t.path))
                {
                    this.row_focus
                        .entry(path.clone())
                        .or_insert_with(|| cx.focus_handle());
                }
                this.reconcile_cwd();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reconcile_cwd(&mut self) {
        if !self.selected.contains(&self.cwd) {
            self.cwd = self.selected.first().cloned().unwrap_or_default();
        }
    }
    fn dismiss(&mut self, event: ScopePickerEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.generation = self.generation.wrapping_add(1);
        self.presence.set(false, cx);
        if cx.reduce_motion() {
            cx.emit(event);
        } else {
            let duration = self.presence.transition_duration();
            self.close_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |_, cx| cx.emit(event));
            }));
        }
        cx.notify();
    }
    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if self.selected.iter().any(|p| p == path) {
            self.selected.retain(|p| p != path);
        } else {
            self.selected.push(path.to_owned());
        }
        self.reconcile_cwd();
        cx.notify();
    }
    fn available(&self, path: &str) -> bool {
        self.targets.iter().any(|target| target.path == path) && std::path::Path::new(path).is_dir()
    }
    fn can_apply(&self) -> bool {
        !self.loading
            && !self.closing
            && !self.selected.is_empty()
            && self.selected.contains(&self.cwd)
            && self.selected.iter().all(|path| self.available(path))
    }
    fn apply(&mut self, cx: &mut Context<Self>) {
        if !self.can_apply() {
            return;
        }
        let targets = self
            .selected
            .iter()
            .filter_map(|path| self.targets.iter().find(|t| &t.path == path).cloned())
            .collect();
        self.dismiss(ScopePickerEvent::Applied(targets, self.cwd.clone()), cx);
    }
    fn visible(&self, target: &ScopeTarget) -> bool {
        self.query.is_empty()
            || format!("{} {} {}", target.project_name, target.branch, target.path)
                .to_lowercase()
                .contains(&self.query)
    }
    fn missing_paths(&self) -> Vec<String> {
        self.selected
            .iter()
            .filter(|path| !self.targets.iter().any(|t| &t.path == *path))
            .cloned()
            .collect()
    }
    fn focus_order(&self, cx: &App) -> Vec<FocusHandle> {
        let mut handles = vec![self.input.focus_handle(cx)];
        for target in self.targets.iter().filter(|target| self.visible(target)) {
            if let Some(handle) = self.row_focus.get(&target.path) {
                handles.push(handle.clone());
            }
        }
        for path in self.missing_paths() {
            if let Some(handle) = self.row_focus.get(&path) {
                handles.push(handle.clone());
            }
        }
        if (self.error.is_some() || self.selected.iter().any(|path| !self.available(path)))
            && !self.loading
        {
            handles.push(self.retry_focus.clone());
        }
        handles.push(self.cancel_focus.clone());
        if self.can_apply() {
            handles.push(self.confirm_focus.clone());
        }
        handles
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        let key = &event.keystroke;
        // Preserve native text-editing commands, but keep shell navigation and
        // launch shortcuts outside this source-attached modal from firing.
        if (key.modifiers.control || key.modifiers.platform)
            && !matches!(
                key.key.as_str(),
                "a" | "c"
                    | "v"
                    | "x"
                    | "z"
                    | "y"
                    | "left"
                    | "right"
                    | "up"
                    | "down"
                    | "home"
                    | "end"
                    | "backspace"
                    | "delete"
            )
        {
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        match key.key.as_str() {
            "escape" => {
                self.dismiss(ScopePickerEvent::Cancelled, cx);
            }
            "tab" => {
                let handles = self.focus_order(cx);
                let current = handles.iter().position(|h| h.is_focused(window));
                let index = if key.modifiers.shift {
                    current.map_or(handles.len() - 1, |i| {
                        (i + handles.len() - 1) % handles.len()
                    })
                } else {
                    current.map_or(0, |i| (i + 1) % handles.len())
                };
                handles[index].focus(window, cx);
            }
            "space" | "enter" if !self.input.focus_handle(cx).is_focused(window) => {
                if event.is_held {
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if self.cancel_focus.is_focused(window) {
                    self.dismiss(ScopePickerEvent::Cancelled, cx);
                } else if self.confirm_focus.is_focused(window) {
                    self.apply(cx);
                } else if self.retry_focus.is_focused(window) {
                    self.load(cx);
                } else if let Some(path) = self
                    .row_focus
                    .iter()
                    .find(|(_, h)| h.is_focused(window))
                    .map(|(p, _)| p.clone())
                {
                    self.toggle(&path, cx);
                }
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn row(&self, target: &ScopeTarget, unavailable: bool, cx: &Context<Self>) -> impl IntoElement {
        let checked = self.selected.contains(&target.path);
        let path = target.path.clone();
        let focus = self
            .row_focus
            .get(&path)
            .cloned()
            .unwrap_or_else(|| self.focus.clone());
        div()
            .id(SharedString::from(format!("scope-worktree-{path}")))
            .debug_selector({
                let path = path.clone();
                move || format!("scope-worktree-{path}")
            })
            .role(gpui::Role::CheckBox)
            .aria_label(format!("{} / {}", target.project_name, target.branch))
            .aria_toggled(if checked {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .track_focus(&focus)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_LG))
            .rounded(rpx(RADIUS_CONTROL))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(|row| row.bg(c::BG_HOVER()))
            .focus_visible(|row| row.border_color(c::SEL_RING()).bg(c::BG_HOVER()))
            .on_click(cx.listener(move |this, _, window, cx| {
                focus.focus(window, cx);
                this.toggle(&path, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(if target.branch.is_empty() {
                                        "Detached checkout".to_owned()
                                    } else {
                                        target.branch.clone()
                                    }),
                            ),
                    )
                    .when(unavailable, |row| {
                        row.child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::STATUS_WARNING())
                                .child("Unavailable — deselect or retry"),
                        )
                    }),
            )
            .child(
                div()
                    .size(rpx(ICON_MD))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(rpx(RADIUS_CONTROL))
                    .border_1()
                    .border_color(c::FG_MUTE())
                    .when(checked, |mark| mark.child(icon("check", ICON_SM, c::FG()))),
            )
    }
}

impl Focusable for ScopePicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}
impl Render for ScopePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let viewport = window.viewport_size();
        let width = MODAL_W_MD.min((f32::from(viewport.width) / scale - SPACE_LG * 2.).max(0.));
        let height = (f32::from(viewport.height) / scale - SPACE_LG * 2.).max(0.);
        let mut rows = div()
            .id("scope-worktree-list")
            .debug_selector(|| "scope-worktree-list".into())
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(rpx(SPACE_2XL));
        if self.loading {
            rows = rows.child(
                div()
                    .py(rpx(SPACE_LG))
                    .text_color(c::FG_DIM())
                    .child("Loading worktrees…"),
            );
        }
        for (project, name, project_path) in &self.projects {
            let targets: Vec<_> = self
                .targets
                .iter()
                .filter(|t| t.project == *project && self.visible(t))
                .collect();
            if targets.is_empty() {
                continue;
            }
            rows = rows.child(
                div()
                    .pt(rpx(SPACE_LG))
                    .pb(rpx(SPACE_SM))
                    .px(rpx(SPACE_LG))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .text_color(c::FG_DIM())
                    .child(components::project_symbol_for_path(
                        project_path,
                        ICON_MD,
                        cx,
                    ))
                    .child(name.clone()),
            );
            for target in targets {
                rows = rows.child(self.row(target, !self.available(&target.path), cx));
            }
        }
        let missing = self.missing_paths();
        if !self.loading && !missing.is_empty() {
            rows = rows.child(
                div()
                    .pt(rpx(SPACE_LG))
                    .text_color(c::STATUS_WARNING())
                    .child("Unavailable worktrees"),
            );
            for path in missing {
                rows = rows.child(self.row(
                    &ScopeTarget {
                        project: usize::MAX,
                        project_path: String::new(),
                        project_name: "Unavailable".into(),
                        path,
                        branch: "Unavailable worktree".into(),
                    },
                    true,
                    cx,
                ));
            }
        }
        if !self.loading
            && !self.targets.iter().any(|t| self.visible(t))
            && self.missing_paths().is_empty()
        {
            rows = rows.child(div().py(rpx(SPACE_LG)).text_color(c::FG_DIM()).child(
                if self.query.is_empty() {
                    "No worktrees are available in this workspace."
                } else {
                    "No matching worktrees."
                },
            ));
        }
        if let Some(error) = &self.error {
            rows = rows.child(
                div()
                    .py(rpx(SPACE_LG))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_color(c::STATUS_WARNING())
                            .text_size(rpx(TEXT_SMALL))
                            .child(error.clone()),
                    )
                    .child(
                        components::form_action(
                            "scope-retry",
                            "Retry discovery",
                            false,
                            window,
                            cx,
                        )
                        .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                        .with_focus_handle(self.retry_focus.clone())
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
            );
        }
        if self.error.is_none()
            && !self.loading
            && self.selected.iter().any(|path| !self.available(path))
        {
            rows = rows.child(
                components::form_action(
                    "scope-retry-unavailable",
                    "Retry discovery",
                    false,
                    window,
                    cx,
                )
                .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                .with_focus_handle(self.retry_focus.clone())
                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            );
        }
        let invalid = !self.loading && self.selected.iter().any(|path| !self.available(path));
        let footer = div()
            .flex_shrink_0()
            .px(rpx(SPACE_3XL))
            .py(rpx(SPACE_2XL))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .when(invalid, |footer| {
                footer.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::STATUS_WARNING())
                        .child("Deselect unavailable worktrees or retry discovery."),
                )
            })
            .child(
                components::form_actions()
                    .child(
                        div()
                            .flex_1()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(format!("{} selected", self.selected.len())),
                    )
                    .child(
                        components::form_action("scope-cancel", "Cancel", false, window, cx)
                            .icon(gpui_component::Icon::default().path("icons/close.svg"))
                            .with_focus_handle(self.cancel_focus.clone())
                            .debug_selector(|| "scope-cancel".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dismiss(ScopePickerEvent::Cancelled, cx);
                            })),
                    )
                    .child(
                        components::form_action("scope-continue", "Continue", true, window, cx)
                            .icon(gpui_component::Icon::default().path("icons/check.svg"))
                            .with_focus_handle(self.confirm_focus.clone())
                            .disabled(!self.can_apply())
                            .debug_selector(|| "scope-continue".into())
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    ),
            );
        let panel = div()
            .id("worktree-scope-panel")
            .debug_selector(|| "worktree-scope-panel".into())
            .role(gpui::Role::Dialog)
            .aria_label("Select worktrees")
            .track_focus(&self.focus)
            .w(rpx(width))
            .h(rpx(
                height.min(MODAL_SCROLL_MAX_H + FIELD_H + APPBAR_H * 5.0)
            ))
            .max_h(rpx(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(c::BORDER())
            .rounded(rpx(RADIUS_PANEL))
            .bg(c::SURFACE_RAISED())
            .text_color(c::FG())
            .text_size(rpx(TEXT_BODY))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_shrink_0()
                    .p(rpx(SPACE_3XL))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(div().text_size(rpx(TEXT_TITLE)).child("Select worktrees"))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Choose one or more worktrees for this session."),
                    )
                    .child(components::form_field(
                        "scope-search",
                        "Find worktrees",
                        &self.input,
                        None,
                        window,
                        cx,
                    )),
            )
            .child(rows)
            .child(footer);
        let overlay = div()
            .id("worktree-scope-overlay")
            .debug_selector(|| "worktree-scope-overlay".into())
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .bg(c::SCRIM())
            .flex()
            .items_center()
            .justify_center()
            .capture_key_down(cx.listener(Self::key))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss(ScopePickerEvent::Cancelled, cx)),
            )
            .child(panel)
            .when(self.closing, |overlay| {
                overlay.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
                )
            });
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .anchor(gpui::Anchor::TopLeft)
            .position(gpui::point(gpui::px(0.), gpui::px(0.)))
            .child(self.presence.render(overlay, "worktree-scope-presence", cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    }

    #[gpui::test]
    fn pending_worktree_scope_keeps_missing_selection_until_deselected(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let dir = tempfile::tempdir().expect("worktree");
        let path = dir.path().to_string_lossy().into_owned();
        let missing = dir.path().join("removed").to_string_lossy().into_owned();
        let (picker, cx) = cx.add_window_view(|window, cx| {
            ScopePicker::new(
                Vec::new(),
                vec![path.clone(), missing.clone(), path.clone()],
                missing.clone(),
                window,
                cx,
            )
        });
        cx.update(|_, cx| {
            picker.update(cx, |picker, cx| {
                // Invalidate the empty catalog task and install an exact available row.
                picker.generation = picker.generation.wrapping_add(1);
                picker.loading = false;
                picker.targets = vec![ScopeTarget {
                    project: 0,
                    project_path: path.clone(),
                    project_name: "Project".into(),
                    path: path.clone(),
                    branch: "main".into(),
                }];
                assert_eq!(picker.selected, vec![path.clone(), missing.clone()]);
                assert_eq!(picker.missing_paths(), vec![missing.clone()]);
                assert!(!picker.can_apply());
                picker.toggle(&missing, cx);
                assert_eq!(picker.cwd, path);
                assert!(picker.can_apply());
                picker.toggle(&path, cx);
                assert!(picker.selected.is_empty());
                assert_eq!(picker.cwd, "");
                assert!(!picker.can_apply());
            });
        });
    }

    #[gpui::test]
    fn worktree_scope_close_emits_once_after_passive_exit(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (picker, cx) = cx.add_window_view(|window, cx| {
            ScopePicker::new(Vec::new(), Vec::new(), String::new(), window, cx)
        });
        let events = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = events.clone();
        let _subscription =
            cx.update(|_, cx| cx.subscribe(&picker, move |_, _, _| count.set(count.get() + 1)));
        cx.update(|_, cx| {
            cx.set_reduce_motion(false);
            picker.update(cx, |picker, cx| {
                picker.dismiss(ScopePickerEvent::Cancelled, cx);
                picker.dismiss(ScopePickerEvent::Cancelled, cx);
                assert!(picker.closing);
                assert!(!picker.can_apply());
            });
        });
        assert_eq!(events.get(), 0);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_FAST_MS) * 2);
        cx.run_until_parked();
        assert_eq!(events.get(), 1);
    }
}
