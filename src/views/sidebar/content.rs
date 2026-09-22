//! Main canvas driven by the sidebar's stable selection.
use super::{display_task_title, Action, Selection, Sidebar, ViewMode};
use crate::{
    activity::ActivityState,
    icons::icon,
    settings::SettingsState,
    theme as c,
    views::{
        components::{form_action, form_field},
        rpx,
        terminal_view::TerminalView,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnyElement, Context, Focusable, FontWeight, Window};
use gpui_component::Disableable;
use grove_core::agent::Agent;

const FORM_ACTION_H: f32 = 44.;
const FORM_PRIMARY_MIN_W: f32 = 130.;
const FORM_SECONDARY_MIN_W: f32 = 100.;
const SESSION_HEADER_H: f32 = 36.;
const SESSION_ICON_SLOT: f32 = 24.;
const EMPTY_CARD_W: f32 = 430.;
const EMPTY_CARD_PAD: f32 = 24.;
const EMPTY_TITLE_SIZE: f32 = 18.;
const PROMPT_INSET: f32 = 40.;

impl Sidebar {
    pub(super) fn begin_worktree(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_return_focus.is_none() {
            self.worktree_return_focus = window.focused(cx);
        }
        self.pending_new_worktree = Some(idx);
        self.content_error = None;
        self.worktree_errors = [None, None, None];
        self.worktree_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.worktree_branch
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.worktree_base
            .update(cx, |input, cx| input.set_value("HEAD", window, cx));
        cx.notify();
    }
    pub(super) fn submit_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.pending_new_worktree else {
            return;
        };
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
        else {
            return;
        };
        let name = self.worktree_name.read(cx).value().trim().to_string();
        let branch = self.worktree_branch.read(cx).value().trim().to_string();
        let base = self.worktree_base.read(cx).value().trim().to_string();
        self.worktree_errors = validate_worktree_fields(&name, &branch, &base);
        if self.worktree_errors.iter().any(Option::is_some) {
            cx.notify();
            return;
        }
        let service = self.runtime.read(cx).projects.clone();
        match service.update(cx, |service, cx| {
            service.create_worktree_with_branch(&project, &name, &branch, Some(&base), cx)
        }) {
            Ok(path) => {
                self.pending_new_worktree = None;
                self.content_error = None;
                self.select(Selection::Worktree(idx, path), cx);
            }
            Err(error) => {
                let field = worktree_error_field(&error);
                self.worktree_errors[field] = Some(error);
            }
        }
        cx.notify();
    }
    pub(super) fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_new_worktree = None;
        self.project_return_path = None;
        self.project_return_focus = window.focused(cx);
        self.project_panel = None;
        self.project_decision = false;
        self.mode = ViewMode::Project;
        let workspace = cx.global::<SettingsState>().store.workspaces.active;
        let service = self.runtime.read(cx).projects.clone();
        let setup =
            cx.new(|cx| super::project_setup::ProjectSetup::new(window, cx, workspace, service));
        self.observers.push(
            cx.subscribe_in(&setup, window, |this, _, event, window, cx| {
                match event {
                    super::project_setup::ProjectSetupEvent::Completed { path, workspace } => {
                        SettingsState::update(cx, |store| {
                            store.workspaces.select(*workspace);
                        });
                        this.project_setup = None;
                        this.finish_project_selection(path, window, cx);
                    }
                    super::project_setup::ProjectSetupEvent::Cancelled => {
                        this.project_setup = None;
                        if let Some(f) = this.project_return_focus.take() {
                            f.focus(window, cx);
                        }
                    }
                }
                cx.notify();
            }),
        );
        setup.focus_handle(cx).focus(window, cx);
        self.project_setup = Some(setup);
        cx.notify();
    }
    pub(super) fn project_path_is_active(&self, path: &str, cx: &gpui::App) -> bool {
        let store = &cx.global::<SettingsState>().store;
        store.projects.iter().any(|p| p.path == path && !p.archived)
            && store.project_workspace_id(path) == store.workspaces.active
    }
    pub(super) fn finish_project_selection(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = cx
            .global::<SettingsState>()
            .store
            .project_workspace_id(path);
        SettingsState::update(cx, |store| {
            store.workspaces.select(workspace);
        });
        self.project_return_focus = None;
        self.project_return_path = None;
        self.sync(window, cx);
        self.select_project_path(path, cx);
        let index = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .position(|p| p.path == path);
        let focus = index.map_or_else(
            || self.focus.clone(),
            |idx| {
                self.project_menu_focus
                    .entry(idx)
                    .or_insert_with(|| cx.focus_handle())
                    .clone()
            },
        );
        focus.focus(window, cx);
    }
    pub(super) fn select_project_path(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(idx) = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .position(|p| p.path == path)
        {
            self.select(Selection::Project(idx), cx);
        }
    }
    pub(super) fn open_project_panel(
        &mut self,
        page: super::projects::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_new_worktree = None;
        self.project_return_path = match &page {
            super::projects::Page::Edit(path) | super::projects::Page::Remove(path) => {
                Some(path.clone())
            }
            super::projects::Page::Archived => None,
        };
        self.project_return_focus = self.menu_return_focus.take().or_else(|| window.focused(cx));
        self.menu = None;
        self.project_setup = None;
        self.mode = ViewMode::Project;
        let runtime = self.runtime.read(cx);
        let (service, registry) = (runtime.projects.clone(), runtime.registry.clone());
        let panel =
            cx.new(|cx| super::projects::ProjectPanel::new(page, service, registry, window, cx));
        self.project_decision = panel.read(cx).is_decision();
        self.observers.push(
            cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
                match event {
                    super::projects::ProjectPanelEvent::Closed => {
                        this.project_panel = None;
                        this.project_decision = false;
                        let source_exists = this.project_return_path.take().is_none_or(|path| {
                            cx.global::<SettingsState>()
                                .store
                                .projects
                                .iter()
                                .any(|p| p.path == path && !p.archived)
                        });
                        if source_exists {
                            if let Some(f) = this.project_return_focus.take() {
                                f.focus(window, cx);
                            }
                        } else {
                            this.project_return_focus = None;
                            this.focus.focus(window, cx);
                        }
                    }
                    super::projects::ProjectPanelEvent::Selected(path) => {
                        this.project_panel = None;
                        this.project_decision = false;
                        this.finish_project_selection(path, window, cx);
                    }
                    super::projects::ProjectPanelEvent::Archived => {
                        this.project_panel = None;
                        this.project_decision = false;
                        this.project_return_focus = None;
                        this.project_return_path = None;
                        this.focus.focus(window, cx);
                        this.selection = None;
                    }
                    super::projects::ProjectPanelEvent::Decision(value) => {
                        this.project_decision = *value;
                    }
                }
                cx.notify();
            }),
        );
        self.project_panel = Some(panel);
        cx.notify();
    }
    fn terminal_content(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        home: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let registry = self.runtime.read(cx).registry.read(cx);
        let (meta, session) = if home {
            let index = registry
                .home_terminals()
                .iter()
                .position(|meta| meta.id == id)?;
            (
                registry.home_terminals()[index].clone(),
                registry.home_terminal(index).cloned(),
            )
        } else {
            (registry.meta(id)?.clone(), registry.session(id).cloned())
        };
        let workspaces = &cx.global::<SettingsState>().store.workspaces;
        let workspace = workspaces.name(workspaces.active);
        let worktree = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|worktree| worktree.path == meta.wt_path);
        let branch = worktree
            .map(|worktree| worktree.branch.clone())
            .unwrap_or_default();
        let location =
            worktree.map_or_else(|| meta.wt_path.clone(), |worktree| worktree.name.clone());
        let context = if home {
            format!("{workspace} / Standalone terminal / ~")
        } else {
            format!("{workspace} / {} / {location}", meta.project)
        };
        let error = session
            .as_ref()
            .and_then(|session| session.read(cx).spawn_error().map(str::to_owned));
        let pending = session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_pending_attach());
        let exited = session
            .as_ref()
            .is_some_and(|session| session.read(cx).has_exited());
        let activity = if home {
            ActivityState::Idle
        } else {
            self.runtime.read(cx).activity.read(cx).state_of(id)
        };
        let state = canvas_state(
            meta.agent,
            activity,
            error.is_some(),
            pending,
            session.is_none(),
            exited,
        );
        let task = display_task_title(
            session
                .as_ref()
                .and_then(|session| session.read(cx).title()),
            &meta.label,
        );
        let agent = agent_name(meta.agent);
        let grid = self.mode == ViewMode::Grid;
        let accessible_label = format!("{agent} · {task} · {context} · {}", state.label());
        let mut view = None;
        if let Some(session) = session {
            if !self.canvas_observers.contains_key(&id) {
                let signature = session_signature(&session, cx);
                self.canvas_signatures.insert(id, signature);
                self.canvas_observers.insert(
                    id,
                    cx.observe(&session, move |this, session, cx| {
                        let signature = session_signature(&session, cx);
                        if this.canvas_signatures.get(&id) != Some(&signature) {
                            this.canvas_signatures.insert(id, signature);
                            cx.notify();
                        }
                    }),
                );
            }
            let views = if home {
                &mut self.home_terminal_views
            } else {
                &mut self.terminal_views
            };
            let newly_created = !views.contains_key(&id);
            let terminal = views
                .entry(id)
                .or_insert_with(|| cx.new(|cx| TerminalView::new(session, cx)))
                .clone();
            self.canvas_focus_observers.entry(id).or_insert_with(|| {
                cx.on_focus_in(&terminal.focus_handle(cx), window, move |this, _, cx| {
                    this.select(
                        if home {
                            Selection::Home(id)
                        } else {
                            Selection::Session(id)
                        },
                        cx,
                    );
                })
            });
            if newly_created
                && !grid
                && matches!(self.selection,Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id)
            {
                terminal.focus_handle(cx).focus(window, cx);
            }
            view = Some(terminal);
        }
        let bounds = self.canvas_bounds.entry(id).or_default().clone();
        let close_name = format!("Close {agent} session {task} in {context}");
        let close = self.canvas_close_button(id, home, close_name, cx);
        let title_tooltip = accessible_label.clone();
        let mut header = div()
            .id(gpui::SharedString::from(format!(
                "terminal-header-{}",
                id.raw()
            )))
            .debug_selector(move || format!("terminal-header-{}", id.raw()))
            .aria_label(accessible_label.clone())
            .relative()
            .child(gpui::canvas(move |rect,_,_|bounds.set(rect),|_,(),_,_|{}).absolute().inset_0())
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .px(rpx(SPACE_3XL))
            .bg(if grid && matches!(self.selection,Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id) { c::BG_HOVER() } else { c::BG_STRIP() })
            .when(!grid, |header| header.border_b_1().border_color(c::BORDER()))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(title_tooltip.clone())
                    .bg(c::BG_STRIP())
                    .text_color(c::FG())
                    .build(window, cx)
            })
;
        let agent_icon = div()
            .w(rpx(SESSION_ICON_SLOT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(meta.agent.icon_name(), ICON_LG, c::FG()));
        let status = div()
            .flex_shrink_0()
            .text_size(rpx(TEXT_MICRO))
            .text_color(state.color())
            .whitespace_nowrap()
            .child(state.label());
        if grid {
            header = header
                .child(agent_icon)
                .h(rpx(SESSION_HEADER_H))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(rpx(TEXT_BODY))
                        .child(task),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(context.clone()),
                )
                .child(status)
                .child(close);
        } else {
            header = header
                .h(rpx(APPBAR_H))
                .child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "terminal-header-left-{}",
                            id.raw()
                        )))
                        .debug_selector(move || format!("terminal-header-left-{}", id.raw()))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_MD))
                        .child(agent_icon)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(rpx(TEXT_BODY))
                                .child(task),
                        ),
                )
                .child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "terminal-header-center-{}",
                            id.raw()
                        )))
                        .debug_selector(move || format!("terminal-header-center-{}", id.raw()))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(context.clone()),
                        ),
                )
                .child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "terminal-header-right-{}",
                            id.raw()
                        )))
                        .debug_selector(move || format!("terminal-header-right-{}", id.raw()))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(rpx(SPACE_MD))
                        .when(!branch.is_empty(), |zone| {
                            zone.child(
                                div()
                                    .min_w_0()
                                    .max_w(rpx(MODAL_W_SM / 3.))
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(branch.clone()),
                            )
                        })
                        .child(status)
                        .child(close),
                );
        }
        let mut pane = div()
            .id(gpui::SharedString::from(format!(
                "terminal-pane-{}",
                id.raw()
            )))
            .role(gpui::Role::Group)
            .aria_label(accessible_label)
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select(
                        if home {
                            Selection::Home(id)
                        } else {
                            Selection::Session(id)
                        },
                        cx,
                    );
                }),
            )
            .child(header);
        if let Some(message) = state.message(error.as_deref()) {
            let mut banner = div()
                .id(gpui::SharedString::from(format!(
                    "session-state-{}",
                    id.raw()
                )))
                .role(if matches!(state, CanvasState::Failed) {
                    gpui::Role::Alert
                } else {
                    gpui::Role::Status
                })
                .flex_shrink_0()
                .min_w_0()
                .p(rpx(SPACE_LG))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(rpx(SPACE_LG))
                .bg(c::BG_STRIP())
                .border_b_1()
                .border_color(c::BORDER())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(state.color())
                        .child(message),
                );
            if state == CanvasState::NeedsYou {
                if let Some(terminal) = view.clone() {
                    banner = banner.child(
                        form_action(
                            "focus-session-terminal",
                            "Focus terminal",
                            false,
                            window,
                            cx,
                        )
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.select(
                                    if home {
                                        Selection::Home(id)
                                    } else {
                                        Selection::Session(id)
                                    },
                                    cx,
                                );
                                terminal.focus_handle(cx).focus(window, cx);
                            },
                        )),
                    );
                }
            } else if matches!(state, CanvasState::Failed | CanvasState::Exited) {
                if !home {
                    banner = banner.child(
                        self.control(
                            ("retry-canvas", id.raw()),
                            format!("Retry {agent} session"),
                            Action::Retry(id),
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child("Retry"),
                    );
                }
                banner = banner.child(
                    self.canvas_close_button(id, home, "Remove session".into(), cx)
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child("Remove"),
                );
            }
            pane = pane.child(banner);
        }
        pane = if let Some(terminal) = view {
            pane.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p(rpx(SPACE_3XL))
                    .overflow_hidden()
                    .child(terminal),
            )
        } else {
            pane.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FG_DIM())
                    .child("Preparing terminal…"),
            )
        };
        Some(
            pane.when_some(self.canvas_confirmation(id, window, cx), |pane, popup| {
                pane.child(popup)
            })
            .into_any_element(),
        )
    }
    fn canvas_close_button(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        home: bool,
        label: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let removal = label == "Remove session";
        let focus = self
            .canvas_close_focus
            .entry((id, removal))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        div()
            .id(gpui::SharedString::from(format!(
                "close-canvas-{}-{removal}",
                id.raw()
            )))
            .debug_selector(move || {
                format!(
                    "canvas-{}-{}",
                    if removal { "remove" } else { "close" },
                    id.raw()
                )
            })
            .track_focus(&focus)
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .tab_index(0)
            .size(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(rpx(RADIUS_CONTROL))
            .hover(|style| style.bg(c::BG_HOVER()))
            .focus(|style| style.bg(c::BG_HOVER()))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(label.clone())
                    .bg(c::BG_STRIP())
                    .text_color(c::FG())
                    .build(window, cx)
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.request_canvas_close(id, home, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        cx.stop_propagation();
                        this.request_canvas_close(id, home, window, cx);
                    }
                }),
            )
            .child(icon("close", ICON_SM, c::FG_DIM()))
    }
    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(setup) = &self.project_setup {
            return setup.clone().into_any_element();
        }
        if let Some(panel) = &self.project_panel {
            return panel.clone().into_any_element();
        }
        if self.pending_new_worktree.is_some() {
            let errors = validate_worktree_fields(
                self.worktree_name.read(cx).value().as_ref(),
                self.worktree_branch.read(cx).value().as_ref(),
                self.worktree_base.read(cx).value().as_ref(),
            );
            let enabled = errors.iter().all(Option::is_none);
            return div()
                .id("worktree-editor-scroll")
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .p(rpx(SPACE_3XL))
                .child(
                    div().w_full().min_w_0().flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(rpx(MODAL_W_SM))
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_2XL))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("New worktree"),
                            )
                            .child(
                                div()
                                    .text_color(c::FG_DIM())
                                    .child("Create a branch and isolated working directory."),
                            )
                            .child(form_field(
                                "worktree-name-field",
                                "Worktree name",
                                &self.worktree_name,
                                self.worktree_errors[0].as_deref(),
                                window,
                                cx,
                            ))
                            .child(form_field(
                                "worktree-branch-field",
                                "Branch name",
                                &self.worktree_branch,
                                self.worktree_errors[1].as_deref(),
                                window,
                                cx,
                            ))
                            .child(form_field(
                                "worktree-base-field",
                                "Base branch",
                                &self.worktree_base,
                                self.worktree_errors[2].as_deref(),
                                window,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        form_action(
                                            "create-worktree",
                                            "Create worktree",
                                            true,
                                            window,
                                            cx,
                                        )
                                        .h(rpx(FORM_ACTION_H))
                                        .flex_1()
                                        .min_w(rpx(FORM_PRIMARY_MIN_W))
                                        .disabled(!enabled)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.submit_worktree(cx)),
                                        ),
                                    )
                                    .child(
                                        form_action("cancel-worktree", "Cancel", false, window, cx)
                                            .h(rpx(FORM_ACTION_H))
                                            .flex_1()
                                            .min_w(rpx(FORM_SECONDARY_MIN_W))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.act(super::Action::Cancel, window, cx);
                                            })),
                                    ),
                            ),
                    ),
                )
                .into_any_element();
        }
        if self.mode == ViewMode::Grid {
            let ids = self.active_canvas_sessions(cx);
            if !ids.is_empty() {
                let tiles: Vec<_> = ids
                    .into_iter()
                    .filter_map(|(id, home)| self.terminal_content(id, home, window, cx))
                    .collect();
                return self.render_grid(tiles, window, cx);
            }
        } else {
            match self.selection.clone() {
                Some(Selection::Session(id)) => {
                    if let Some(el) = self.terminal_content(id, false, window, cx) {
                        return el;
                    }
                }
                Some(Selection::Home(id)) => {
                    if let Some(el) = self.terminal_content(id, true, window, cx) {
                        return el;
                    }
                }
                _ => {}
            }
        }
        let selected = match self.selection.clone() {
            _ if self.mode == ViewMode::Grid => None,
            Some(Selection::Project(idx) | Selection::Worktree(idx, _)) => Some(idx),
            _ => None,
        };
        let project = selected
            .and_then(|idx| self.snapshot.projects.iter().find(|p| p.idx == idx))
            .cloned();
        let workspace = &cx.global::<SettingsState>().store.workspaces;
        let workspace_name = workspace.name(workspace.active).to_string();
        let empty_grid = self.mode == ViewMode::Grid;
        let no_projects = self.snapshot.projects.is_empty() && !empty_grid;
        let mut section = div()
            .id("canvas-overview")
            .debug_selector(|| "canvas-overview".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col();
        if let Some(project) = project {
            let selected_worktree = match &self.selection {
                Some(Selection::Worktree(_, path)) => project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.path == *path),
                _ => None,
            };
            let title = selected_worktree
                .map_or_else(|| project.name.clone(), |worktree| worktree.name.clone());
            let details = selected_worktree.map_or_else(
                || {
                    format!(
                        "{workspace_name} / {} · {} worktrees · {} sessions",
                        project.name,
                        project.worktrees.len(),
                        project.sessions.len()
                    )
                },
                |worktree| {
                    format!(
                        "{workspace_name} / {} / {} · {} · {} sessions",
                        project.name,
                        worktree.name,
                        worktree.branch,
                        worktree.sessions.len()
                    )
                },
            );
            section=section.child(canvas_section_header(title)).child(div().id("worktree-prompt-scroll").flex_1().min_h_0().overflow_y_scroll().p(rpx(SPACE_3XL))
                .child(div().w_full().min_w_0().max_w(rpx(MODAL_W_XL)).mx_auto().mt(rpx(PROMPT_INSET-SPACE_3XL)).p(rpx(EMPTY_CARD_PAD)).border_1().border_dashed().border_color(c::BORDER_STRONG()).rounded(rpx(RADIUS_CHROME)).flex().flex_col().gap(rpx(SPACE_LG)).text_size(rpx(TEXT_BODY)).text_color(c::FG_DIM())
                    .child(details)
                    .child("Choose a session in the sidebar, or hover or focus a worktree to reveal Codex, Claude Code, and Terminal launch actions.")));
        } else {
            let (title, description) = if no_projects {
                (
                    "No projects",
                    format!("Add a local project to {workspace_name}."),
                )
            } else if empty_grid {
                (
                    "No active sessions",
                    format!(
                        "{workspace_name} has no sessions. Switch to Project view and use a worktree’s launch actions to begin."
                    ),
                )
            } else {
                ("Select a session", "Choose a session in the sidebar, or focus a worktree to reveal its launch actions.".into())
            };
            let card = div()
                .id("canvas-empty-card")
                .debug_selector(|| "canvas-empty-card".into())
                .w_full()
                .max_w(rpx(EMPTY_CARD_W))
                .min_w_0()
                .p(rpx(EMPTY_CARD_PAD))
                .rounded(rpx(RADIUS_PANEL))
                .flex()
                .flex_col()
                .items_center()
                .gap(rpx(SPACE_LG))
                .text_center()
                .child(
                    div()
                        .text_size(rpx(EMPTY_TITLE_SIZE))
                        .font_weight(FontWeight::BOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FG_DIM())
                        .child(description),
                )
                .when(no_projects, |card| {
                    card.child(
                        form_action("add-project", "Add project", true, window, cx)
                            .icon(gpui_component::Icon::default().path("icons/plus.svg"))
                            .h(rpx(FORM_ACTION_H))
                            .mt(rpx(SPACE_LG))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                            ),
                    )
                });
            section = section.child(canvas_section_header(title.into())).child(
                div()
                    .id("canvas-empty-scroll")
                    .debug_selector(|| "canvas-empty-scroll".into())
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .min_h_full()
                            .p(rpx(SPACE_3XL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(card),
                    ),
            );
        }
        section
            .when_some(self.content_error.clone(), |section, error| {
                section.child(
                    div()
                        .flex_shrink_0()
                        .p(rpx(SPACE_LG))
                        .text_color(c::FORM_ERROR())
                        .child(error),
                )
            })
            .into_any_element()
    }
}

fn canvas_section_header(title: String) -> impl gpui::IntoElement {
    div()
        .id("canvas-section-header")
        .debug_selector(|| "canvas-section-header".into())
        .h(rpx(APPBAR_H))
        .flex_shrink_0()
        .min_w_0()
        .px(rpx(SPACE_3XL))
        .flex()
        .items_center()
        .bg(c::BG_STRIP())
        .border_b_1()
        .border_color(c::BORDER())
        .text_size(rpx(TEXT_BODY))
        .font_weight(FontWeight::MEDIUM)
        .child(div().min_w_0().truncate().child(title))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CanvasState {
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
    fn label(self) -> &'static str {
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
    fn color(self) -> gpui::Hsla {
        match self {
            Self::Running | Self::Working => c::GREEN(),
            Self::NeedsYou => c::YELLOW(),
            Self::Failed => c::FORM_ERROR(),
            _ => c::FG_DIM(),
        }
    }
    fn message(self, error: Option<&str>) -> Option<String> {
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
fn canvas_state(
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
fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude Code",
        Agent::OpenCode => "OpenCode",
        Agent::Terminal => "Terminal",
    }
}
fn session_signature(
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

fn validate_worktree_fields(name: &str, branch: &str, base: &str) -> [Option<String>; 3] {
    let name = name.trim();
    let branch = branch.trim();
    let base = base.trim();
    [
        if name.is_empty() {
            Some("Enter a worktree name.".into())
        } else if !grove_core::git::valid_worktree_name(name) {
            Some(
                "Use letters, numbers, dots, hyphens, or underscores for the worktree name.".into(),
            )
        } else {
            None
        },
        if branch.is_empty() {
            Some("Enter a branch name.".into())
        } else if !valid_branch_name(branch) {
            Some("Enter a valid branch name, such as feat/billing-retry.".into())
        } else {
            None
        },
        if base.is_empty() {
            Some("Enter a base branch or revision.".into())
        } else if base.starts_with('-') || base.contains(char::is_whitespace) {
            Some("Enter a valid base branch or revision.".into())
        } else {
            None
        },
    ]
}
fn valid_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "@"
        && !branch.starts_with('-')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        && branch
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
fn worktree_error_field(error: &str) -> usize {
    let error = error.to_lowercase();
    if error.contains("rev-parse")
        || error.contains("base")
        || error.contains("invalid reference")
        || error.contains("path")
    {
        2
    } else if error.contains("worktree name") {
        0
    } else {
        usize::from(error.contains("branch"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvas_status_uses_real_lifecycle_before_activity() {
        assert_eq!(
            canvas_state(
                Agent::Codex,
                ActivityState::Working,
                true,
                false,
                false,
                false
            ),
            CanvasState::Failed
        );
        assert_eq!(
            canvas_state(
                Agent::Codex,
                ActivityState::Exited,
                false,
                true,
                false,
                true
            ),
            CanvasState::Starting
        );
        assert_eq!(
            canvas_state(
                Agent::Codex,
                ActivityState::Working,
                false,
                false,
                true,
                false
            ),
            CanvasState::Starting
        );
        assert_eq!(
            canvas_state(
                Agent::Terminal,
                ActivityState::Idle,
                false,
                false,
                false,
                false
            ),
            CanvasState::Running
        );
        assert_eq!(
            canvas_state(
                Agent::Terminal,
                ActivityState::Idle,
                false,
                false,
                false,
                true
            ),
            CanvasState::Exited
        );
        assert_eq!(
            canvas_state(
                Agent::Claude,
                ActivityState::WaitingForInput,
                false,
                false,
                false,
                false
            ),
            CanvasState::NeedsYou
        );
        assert!(CanvasState::NeedsYou
            .message(None)
            .unwrap()
            .contains("Review the terminal"));
        assert!(CanvasState::Failed
            .message(Some("executable was not found"))
            .unwrap()
            .contains("executable was not found"));
        assert!(!CanvasState::Exited.message(None).unwrap().contains("code"));
    }
    #[gpui::test]
    fn metadata_without_attached_terminal_renders_loading_header(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: "/grove-canvas-fixture".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/grove-canvas-fixture".into(), Agent::Codex)
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar
        });
        draw(cx);
        assert!(cx.debug_bounds("terminal-header-1").is_some());
        cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(200.)));
        draw(cx);
        let header = cx.debug_bounds("terminal-header-1").unwrap();
        let close = cx.debug_bounds("canvas-close-1").unwrap();
        assert!(
            close.left() >= header.left() && close.right() <= header.right(),
            "{header:?} {close:?}"
        );
        assert_eq!(f32::from(header.size.height), APPBAR_H);
        let center = cx.debug_bounds("terminal-header-center-1").unwrap();
        assert!(f32::from(center.center().x - header.center().x).abs() < 1.);
        for zone in [
            "terminal-header-left-1",
            "terminal-header-center-1",
            "terminal-header-right-1",
        ] {
            let bounds = cx.debug_bounds(zone).unwrap();
            assert!(f32::from(bounds.center().y - header.center().y).abs() < 1.);
        }

        cx.update(|window, cx| {
            let missing = sidebar.update(cx, |sidebar, cx| {
                sidebar
                    .terminal_content(
                        crate::entities::session_registry::SessionId::from_raw(u64::MAX),
                        false,
                        window,
                        cx,
                    )
                    .is_none()
            });
            assert!(missing);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.mode = ViewMode::Grid;
                cx.notify();
            });
        });
        draw(cx);
        let grid_header = cx.debug_bounds("terminal-header-1").unwrap();
        assert_eq!(f32::from(grid_header.size.height), SESSION_HEADER_H);
        let close = cx.debug_bounds("canvas-close-1").unwrap();
        assert!(f32::from(close.center().y - grid_header.center().y).abs() < 1.);
    }
    #[gpui::test]
    fn empty_workspace_card_is_centered_and_bounded(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        draw(cx);
        let canvas = cx.debug_bounds("canvas-overview").unwrap();
        let header = cx.debug_bounds("canvas-section-header").unwrap();
        let scroll = cx.debug_bounds("canvas-empty-scroll").unwrap();
        let card = cx.debug_bounds("canvas-empty-card").unwrap();
        assert_eq!(f32::from(header.size.height), APPBAR_H);
        assert!(header.bottom() <= scroll.top());
        assert!(card.left() >= canvas.left() && card.right() <= canvas.right());
        assert!((f32::from(card.center().x - canvas.center().x)).abs() < 1.);
        assert!(card.top() >= canvas.top() && card.bottom() <= canvas.bottom());
    }
    #[test]
    fn session_header_uses_readable_label_for_uuid_only_titles() {
        assert_eq!(
            display_task_title(
                Some(" 3e9f01f9-54cd-4f09-8617-137231faccc9 ".into()),
                "Claude 1"
            ),
            "Claude 1"
        );
        assert_eq!(
            display_task_title(
                Some("Fix 3e9f01f9-54cd-4f09-8617-137231faccc9".into()),
                "Claude 1"
            ),
            "Fix 3e9f01f9-54cd-4f09-8617-137231faccc9"
        );
        assert_eq!(
            display_task_title(Some(" ".into()), "Terminal 1"),
            "Terminal 1"
        );
    }

    #[test]
    fn missing_values_report_the_corresponding_field() {
        let errors = validate_worktree_fields("", "", "");
        assert_eq!(errors[0].as_deref(), Some("Enter a worktree name."));
        assert_eq!(errors[1].as_deref(), Some("Enter a branch name."));
        assert_eq!(
            errors[2].as_deref(),
            Some("Enter a base branch or revision.")
        );
        assert!(
            validate_worktree_fields("billing-retry", "feat/billing-retry", "main")
                .iter()
                .all(Option::is_none)
        );
    }
    #[test]
    fn invalid_branch_does_not_enable_submit() {
        for branch in [
            "feat:billing",
            "feat//billing",
            ".hidden",
            "feat/billing.lock",
            "feat/bill ing",
            "feat@{billing",
            "-",
        ] {
            assert!(
                validate_worktree_fields("billing", branch, "main")[1].is_some(),
                "{branch}"
            );
        }
    }
    #[test]
    fn server_errors_attach_to_their_source_field() {
        assert_eq!(
            worktree_error_field("Branch feat/billing already exists."),
            1
        );
        assert_eq!(worktree_error_field("Worktree path /x already exists."), 2);
        assert_eq!(worktree_error_field("git rev-parse --verify failed"), 2);
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn worktree_form_fields_stay_within_narrow_and_desktop_canvas(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.begin_worktree(0, window, cx));
        });
        for (width, height) in [(1280., 800.), (320., 200.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            draw(cx);
            for field in [
                "worktree-name-field",
                "worktree-branch-field",
                "worktree-base-field",
            ] {
                let bounds = cx.debug_bounds(field).unwrap();
                assert!(f32::from(bounds.size.width) > 100., "{field}: {bounds:?}");
                assert!(f32::from(bounds.right()) <= width, "{field}: {bounds:?}");
                assert!(f32::from(bounds.left()) >= 0., "{field}: {bounds:?}");
            }
        }
    }
}
