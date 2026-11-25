//! Main canvas driven by the sidebar's stable selection.

mod session_canvas;
mod start_panel;

mod terminal_status;
use terminal_status::{agent_name, canvas_state, session_signature, CanvasState};

#[cfg(test)]
use super::display_task_title;
use super::{
    session_display_title, sidebar_worktree_name, Action, Selection, Sidebar, ViewMode,
    WORKTREE_LAUNCH_AGENTS,
};
use crate::{
    activity::ActivityState,
    icons::icon,
    project_service::{WorktreeReadiness, WorktreeRemovalStage},
    settings::SettingsState,
    theme as c,
    views::{
        components::{
            dense_action_with_colors, form_action, header_control_with_label_tooltip,
            project_field_well,
        },
        motion, rpx,
        terminal_view::TerminalView,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnyElement, Context, FontWeight, Window};
use gpui_component::Disableable;
use grove_core::agent::Agent;

const SESSION_HEADER_H: f32 = 36.;
const SESSION_ICON_SLOT: f32 = 24.;
const HEADER_BREADCRUMB_MAX_W: f32 = 264.;
// Keep identity/title and Close usable when both navigation columns are visible.
const HEADER_STATUS_LABEL_MIN_W: f32 = 320.0;
const EMPTY_CARD_W: f32 = 430.;
const EMPTY_CARD_PAD: f32 = 24.;
const EMPTY_TITLE_SIZE: f32 = 18.;
const PROJECT_START_ROW_H: f32 = 50.;
const PROJECT_START_ICON_SIZE: f32 = 24.;

impl Sidebar {
    pub(super) fn render_worktree_removal(
        &self,
        removal: &super::PendingWorktreeRemoval,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = removal
            .started
            .then(|| {
                self.runtime
                    .read(cx)
                    .projects
                    .read(cx)
                    .worktree_removal_status(&removal.path)
                    .cloned()
            })
            .flatten();
        let stage = status.as_ref().map(|status| status.stage);
        let error = removal
            .error
            .clone()
            .or_else(|| status.as_ref().and_then(|status| status.error.clone()));
        let finished = removal.started
            && (removal.error.is_some() || stage == Some(WorktreeRemovalStage::Finished));
        let success = finished && error.is_none();
        let title = if !removal.started {
            format!("Delete {}?", removal.name)
        } else if success {
            "Worktree removed".into()
        } else if finished {
            "Could not remove worktree".into()
        } else {
            format!("Removing {}", removal.name)
        };
        let description = if !removal.started {
            "Registered sessions using this worktree will stop. Deleting its folder can erase uncommitted changes and untracked files. The main project folder stays on disk."
        } else if success {
            "The worktree folder was removed. The main project folder stays on disk."
        } else if finished {
            "Grove could not confirm removal. Review the error below before trying again."
        } else {
            "Registered sessions have stopped. Keep this view open while removal finishes."
        };
        let mut actions = div()
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_LG))
            .mt(rpx(SPACE_2XL));
        if !removal.started {
            actions = actions
                .child(
                    self.control("cancel-worktree-removal", "Cancel", Action::Cancel, cx)
                        .debug_selector(|| "cancel-worktree-removal".into())
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_SMALL))
                        .rounded(rpx(RADIUS_CONTROL))
                        .bg(c::FIELD_FILL())
                        .child(icon("close", ICON_SM, c::FG_DIM()))
                        .child("Cancel"),
                )
                .child(
                    self.control(
                        "confirm-worktree-removal",
                        "Delete worktree",
                        Action::ConfirmWorktreeRemoval,
                        cx,
                    )
                    .debug_selector(|| "confirm-worktree-removal".into())
                    .track_focus(&self.confirm_focus)
                    .w_auto()
                    .px(rpx(SPACE_LG))
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .rounded(rpx(RADIUS_CONTROL))
                    .bg(c::RED())
                    .text_color(c::BG())
                    .child(crate::icons::action_icon("trash", ICON_SM))
                    .child("Delete worktree"),
                );
        } else if finished {
            actions = actions.child(
                self.control(
                    "dismiss-worktree-removal",
                    if success { "Done" } else { "Dismiss" },
                    Action::DismissWorktreeRemoval,
                    cx,
                )
                .debug_selector(|| "dismiss-worktree-removal".into())
                .track_focus(&self.confirm_focus)
                .w_auto()
                .px(rpx(SPACE_LG))
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .rounded(rpx(RADIUS_CONTROL))
                .bg(c::FG())
                .text_color(c::BG())
                .child(icon(
                    if success { "check" } else { "close" },
                    ICON_SM,
                    c::BG(),
                ))
                .child(if success { "Done" } else { "Dismiss" }),
            );
        } else if stage == Some(WorktreeRemovalStage::RunningScript) {
            actions = actions.child(
                self.control(
                    "skip-worktree-script",
                    "Skip script",
                    Action::SkipWorktreeTeardown,
                    cx,
                )
                .debug_selector(|| "skip-worktree-script".into())
                .track_focus(&self.confirm_focus)
                .w_auto()
                .px(rpx(SPACE_LG))
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .rounded(rpx(RADIUS_CONTROL))
                .bg(c::FIELD_FILL())
                .child(icon("arrow-right", ICON_SM, c::FG_DIM()))
                .child("Skip script"),
            );
        }
        let stage_label = match stage {
            Some(WorktreeRemovalStage::Checking) => Some("Checking worktree"),
            Some(WorktreeRemovalStage::Stopping) => Some("Stopping sessions"),
            Some(WorktreeRemovalStage::RunningScript) => Some("Running teardown script"),
            Some(WorktreeRemovalStage::Removing) => Some("Removing worktree folder"),
            Some(WorktreeRemovalStage::Finished) => Some(if success {
                "Finished"
            } else {
                "Removal failed"
            }),
            None if removal.started && !finished => Some("Preparing removal"),
            _ => None,
        };
        let panel = div()
            .id("worktree-removal-scroll")
            .debug_selector(|| "worktree-removal-scroll".into())
            .w(rpx(MODAL_W_SM))
            .max_w_full()
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .child(
                        div()
                            .id("worktree-removal-decision")
                            .debug_selector(|| "worktree-removal-decision".into())
                            .role(gpui::Role::Dialog)
                            .aria_label(title.clone())
                            .tab_index(0)
                            .track_focus(&self.worktree_removal_focus)
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                                if !this.worktree_removal_focus.is_focused(window) { return; }
                                if event.keystroke.key == "enter" {
                                    if this.pending_worktree_removal.as_ref().is_some_and(|r| !r.started) {
                                        this.act(Action::ConfirmWorktreeRemoval, window, cx);
                                    } else if this.pending_worktree_removal.as_ref().is_some_and(|r| {
                                        r.error.is_some() || this.runtime.read(cx).projects.read(cx)
                                            .worktree_removal_status(&r.path)
                                            .is_some_and(|status| status.stage == WorktreeRemovalStage::Finished)
                                    }) {
                                        this.act(Action::DismissWorktreeRemoval, window, cx);
                                    }
                                    cx.stop_propagation();
                                }
                            }))
                            .w_full()
                            .max_w(rpx(MODAL_W_LG))
                            .min_w_0()
                            .p(rpx(SPACE_3XL))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_2XL))
                            .rounded(rpx(RADIUS_PANEL))
                            .bg(c::SURFACE_RAISED())
                            .border_1()
                            .border_color(c::BORDER_SOFT())
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().text_color(c::FG_DIM()).child(description))
                            .child(
                                project_field_well(false, false)
                                    .id("worktree-removal-path")
                                    .debug_selector(|| "worktree-removal-path".into())
                                    .w_full()
                                    .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Worktree folder"))
                                    .child(div().id("worktree-removal-path-value").debug_selector(|| "worktree-removal-path-value".into()).min_w_0().truncate().child(removal.path.replace('/', "/\u{200b}"))),
                            )
                            .when(!removal.started, |panel| {
                                panel.child(
                                    div()
                                        .p(rpx(SPACE_2XL))
                                        .rounded(rpx(RADIUS_CONTROL))
                                        .bg(c::ERROR_WASH())
                                        .text_color(c::FORM_ERROR())
                                        .child("This deletes the additional worktree folder. The main project folder is protected."),
                                )
                            })
                            .when_some(stage_label, |panel, label| {
                                panel.child(
                                    div()
                                        .id("worktree-removal-status")
                                        .debug_selector(|| "worktree-removal-status".into())
                                        .role(gpui::Role::Status)
                                        .text_color(c::FG_DIM())
                                        .flex()
                                        .flex_col()
                                        .gap(rpx(SPACE_LG))
                                        .child(motion::fast(
                                            super::super::status::label("worktree-removal-stage", if !finished { super::super::status::StatusKind::Working } else if success { super::super::status::StatusKind::Success } else { super::super::status::StatusKind::Failed }, label, ICON_SM, cx),
                                            format!("worktree-removal-stage-{label}"),
                                            cx,
                                        ))
                                        .when(!finished, |status| {
                                            status.child(
                                                div()
                                                    .w_full()
                                                    .h(rpx(SPACE_SM))
                                                    .rounded(rpx(RADIUS_FULL))
                                                    .bg(c::FIELD_FILL())
                                                    .child(
                                                        div()
                                                            .h_full()
                                                            .when(
                                                                stage == Some(WorktreeRemovalStage::RunningScript),
                                                                gpui::Styled::w_1_2,
                                                            )
                                                            .when(
                                                                stage != Some(WorktreeRemovalStage::RunningScript),
                                                                gpui::Styled::w_full,
                                                            )
                                                            .rounded(rpx(RADIUS_FULL))
                                                            .bg(c::FG_DIM()),
                                                    ),
                                            )
                                        }),
                                )
                            })
                            .when_some(error, |panel, error| {
                                panel.child(
                                    div()
                                        .id("worktree-removal-error")
                                        .debug_selector(|| "worktree-removal-error".into())
                                        .role(gpui::Role::Alert)
                                        .p(rpx(SPACE_2XL))
                                        .rounded(rpx(RADIUS_CONTROL))
                                        .bg(c::ERROR_WASH())
                                        .text_color(c::FORM_ERROR())
                                        .child(motion::fast(
                                            super::super::status::label("worktree-removal-failure", super::super::status::StatusKind::Failed, error.clone(), ICON_SM, cx),
                                            format!("worktree-removal-error-{error}"),
                                            cx,
                                        )),
                                )
                            })
                            .child(actions),
                    ),
            )
            ;
        motion::base(panel, "worktree-removal-enter", cx)
    }
    pub(super) fn begin_worktree(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        else {
            return;
        };
        if let Some(settings) = &self.settings_panel {
            settings.update(cx, |settings, cx| {
                settings.open_project_creation_shortcut(Some(&path), window, cx);
            });
        }
    }

    pub(super) fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = &self.settings_panel {
            settings.update(cx, |settings, cx| {
                settings.open_project_creation_shortcut(None, window, cx);
            });
        }
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
        self.project_return_navigation = None;
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
        self.project_return_navigation = Some((self.mode, self.selection.clone()));
        self.project_return_path = match &page {
            super::projects::Page::Edit(path)
            | super::projects::Page::Remove(path)
            | super::projects::Page::RemoveWorktree {
                project_path: path, ..
            }
            | super::projects::Page::Move(path) => Some(path.clone()),
            super::projects::Page::Archived => None,
        };
        self.project_return_focus = self.menu_return_focus.take().or_else(|| window.focused(cx));
        self.menu = None;
        self.mode = ViewMode::Project;
        let runtime = self.runtime.read(cx);
        let (service, registry) = (runtime.projects.clone(), runtime.registry.clone());
        let panel =
            cx.new(|cx| super::projects::ProjectPanel::new(page, service, registry, window, cx));
        self.project_decision = panel.read(cx).is_decision();
        self.observers.push(
            cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
                match event {
                    super::projects::ProjectPanelEvent::Closed
                    | super::projects::ProjectPanelEvent::Saved(_) => {
                        this.project_panel = None;
                        this.project_decision = false;
                        if let Some((mode, selection)) = this.project_return_navigation.take() {
                            this.mode = mode;
                            this.selection = selection;
                        }
                        let source_exists = this.project_return_path.take().is_none_or(|path| {
                            this.checkout_targets(cx)
                                .iter()
                                .any(|(_, target)| target == &path)
                                || cx
                                    .global::<SettingsState>()
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
                    super::projects::ProjectPanelEvent::Moved {
                        name,
                        workspace,
                        path,
                        open_destination,
                    } => {
                        this.project_panel = None;
                        this.project_decision = false;
                        this.project_return_focus = None;
                        this.project_return_path = None;
                        if *open_destination {
                            this.finish_project_selection(path, window, cx);
                        } else {
                            this.sync(window, cx);
                            this.focus.focus(window, cx);
                        }
                        this.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
                            toast.set_toast(format!("Moved {name} to {workspace}."), cx);
                        });
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
    pub(super) fn side_container_controls(
        &mut self,
        side_chat_available: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let panel_toggle = if self.checkout_panel.toggle_available {
            let hidden = (self.checkout_panel.side_chat_owner.is_some()
                && self.checkout_panel.side_chat_owner == self.selection)
                || (self.files_active && self.files_owner == self.selection)
                || self.checkout_panel.hidden
                || (Self::logical_window_width(window) < super::checkout_panel::WIDE_VIEWPORT
                    && !self.checkout_panel.expanded);
            let label = if hidden {
                "Show right panel"
            } else {
                "Hide right panel"
            };
            let focus = self
                .checkout_panel
                .toggle_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let mouse_focus = focus.clone();
            Some(
                header_control_with_label_tooltip("session-panel-toggle", label)
                    .debug_selector(|| "session-panel-toggle".into())
                    .track_focus(&focus)
                    .tab_index(0)
                    .aria_expanded(!hidden)
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .when(!hidden, |style| style.bg(c::BG_HOVER()))
                    .child(icon("panel-right", ICON_LG, c::FG_DIM()))
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                        mouse_focus.focus(window, cx);
                    })
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_session_panel(window, cx)),
                    )
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                        {
                            this.toggle_session_panel(window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    })),
            )
        } else {
            None
        };
        let side_chat_toggle = if self.checkout_panel.toggle_available && side_chat_available {
            let active = self.checkout_panel.side_chat_owner.is_some()
                && self.checkout_panel.side_chat_owner == self.selection;
            let focus = self
                .checkout_panel
                .side_chat_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let mouse_focus = focus.clone();
            Some(
                header_control_with_label_tooltip(
                    "side-chat-toggle",
                    if active {
                        "Hide side chat"
                    } else {
                        "Show side chat"
                    },
                )
                .debug_selector(|| "side-chat-toggle".into())
                .track_focus(&focus)
                .tab_index(0)
                .aria_expanded(active)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .when(active, |style| style.bg(c::BG_HOVER()))
                .child(icon("message-circle", ICON_LG, c::FG_DIM()))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    mouse_focus.focus(window, cx);
                })
                .on_click(cx.listener(|this, _, window, cx| this.toggle_side_chat(window, cx)))
                .on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                        {
                            this.toggle_side_chat(window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    },
                )),
            )
        } else if self.checkout_panel.toggle_available {
            Some(
                crate::views::components::disabled_header_control(
                    "side-chat-toggle",
                    "Side chat requires an open conversation",
                )
                .debug_selector(|| "side-chat-toggle".into())
                .tooltip(|window, cx| {
                    crate::views::components::tooltip(
                        "Open a conversation to use side chat",
                        window,
                    )
                    .build(window, cx)
                })
                .child(icon("message-circle", ICON_LG, c::FG_DIM())),
            )
        } else {
            None
        };
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .when_some(side_chat_toggle, gpui::ParentElement::child)
            .when_some(panel_toggle, gpui::ParentElement::child)
            .child(self.files_control(cx))
    }

    pub(super) fn render_content_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_header_task_disclosure(window, cx);
        if let Some(settings) = &self.settings_panel {
            if settings.read(cx).is_open() {
                return motion::slow(
                    div().size_full().child(settings.clone()),
                    "settings-panel-enter",
                    cx,
                );
            }
        }
        if let Some(panel) = &self.project_panel {
            return motion::slow(
                div().size_full().child(panel.clone()),
                "project-panel-enter",
                cx,
            );
        }
        self.restore_session_review(cx);
        if self.review_is_open(cx) {
            return self.render_review(window, cx);
        }
        self.restore_session_description(cx);
        if self.description_is_open(cx) {
            return self.render_description(window, cx);
        }
        if self.destination == super::global_navigation::Destination::History
            && self.session_history_selected.is_some()
        {
            return self.render_session_history(window, cx);
        }
        if self.dashboard_open {
            return self.render_dashboard(window, cx);
        }
        let canvas_destination =
            if self.destination == super::global_navigation::Destination::History {
                self.history_canvas_destination
            } else {
                self.destination
            };
        if self.is_zen() {
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
        if self.mode == ViewMode::Grid && !self.is_zen() {
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
        if self.selection.is_none() && self.mode != ViewMode::Grid {
            return div()
                .id("navigation-empty-canvas")
                .debug_selector(|| "navigation-empty-canvas".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(rpx(TEXT_BODY))
                .text_color(c::FG_DIM())
                .child(
                    div()
                        .debug_selector(|| "navigation-empty-message".into())
                        .child(
                            if canvas_destination
                                == super::global_navigation::Destination::Terminals
                            {
                                "Select a terminal to view here"
                            } else {
                                "Select a session to view here"
                            },
                        ),
                )
                .into_any_element();
        }
        let selected_worktree = match (&self.mode, &self.selection) {
            (ViewMode::Grid, _) => None,
            (_, Some(Selection::Worktree(idx, path))) => self
                .snapshot
                .projects
                .iter()
                .find(|project| project.idx == *idx)
                .and_then(|project| {
                    project
                        .worktrees
                        .iter()
                        .find(|worktree| worktree.path == *path)
                        .map(|worktree| (project.clone(), worktree.clone()))
                }),
            _ => None,
        };
        let selected_project = match (&self.mode, &self.selection) {
            (ViewMode::Grid, _) => None,
            (_, Some(Selection::Project(idx))) => self
                .snapshot
                .projects
                .iter()
                .find(|project| project.idx == *idx)
                .cloned(),
            _ => None,
        };
        let workspace = &cx.global::<SettingsState>().store.workspaces;
        let workspace_name = workspace.name(workspace.active).to_string();
        let empty_grid = self.mode == ViewMode::Grid && self.active_canvas_sessions(cx).is_empty();
        let no_projects = self.snapshot.projects.is_empty() && !empty_grid;
        let mut section = div()
            .id("canvas-overview")
            .debug_selector(|| "canvas-overview".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col();
        if let Some(project) = selected_project.filter(|project| project.sessions.is_empty()) {
            let path = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(project.idx)
                .map(|project| project.path.clone());
            if let Some(path) = path {
                section = section
                    .child(canvas_section_header(project.name.clone()))
                    .when_some(
                        self.checkout_panel.narrow_content.take(),
                        gpui::ParentElement::child,
                    )
                    .child(
                        div()
                            .id("project-start-scroll")
                            .debug_selector(|| "project-start-scroll".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .id("project-start-scroll-content")
                                    .debug_selector(|| "project-start-scroll-content".into())
                                    .w_full()
                                    .h_auto()
                                    .min_h_full()
                                    .p(rpx(SPACE_3XL))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .child(self.render_start_panel(
                                        project.idx,
                                        &project.name,
                                        &path,
                                        true,
                                        window,
                                        cx,
                                    )),
                            ),
                    );
            }
        } else if let Some((project, selected_worktree)) = selected_worktree {
            if selected_worktree.sessions.is_empty() {
                let path = selected_worktree.path.clone();
                section = section
                    .child(canvas_section_header(
                        sidebar_worktree_name(&selected_worktree.name, selected_worktree.is_main)
                            .into(),
                    ))
                    .when_some(
                        self.checkout_panel.narrow_content.take(),
                        gpui::ParentElement::child,
                    )
                    .child(
                        div()
                            .id("worktree-start-scroll")
                            .debug_selector(|| "worktree-start-scroll".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .id("worktree-start-scroll-content")
                                    .debug_selector(|| "worktree-start-scroll-content".into())
                                    .w_full()
                                    .h_auto()
                                    .min_h_full()
                                    .p(rpx(SPACE_3XL))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .child(self.render_start_panel(
                                        project.idx,
                                        &project.name,
                                        &path,
                                        selected_worktree.is_main,
                                        window,
                                        cx,
                                    )),
                            ),
                    );
            } else {
                let title = selected_worktree.name.clone();
                let details = format!(
                    "{workspace_name} / {} / {} · {} · {} sessions",
                    project.name,
                    selected_worktree.name,
                    selected_worktree.branch,
                    selected_worktree.sessions.len()
                );
                let prompt = div()
                    .w_full()
                    .min_w_0()
                    .max_w(rpx(MODAL_W_XL))
                    .p(rpx(EMPTY_CARD_PAD))
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .gap(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FG_DIM())
                    .child(details)
                    .child("Choose a session in the sidebar, or hover or focus a worktree to reveal Codex, Claude Code, and Terminal launch actions.");
                section = section
                    .child(canvas_section_header(title))
                    .when_some(
                        self.checkout_panel.narrow_content.take(),
                        gpui::ParentElement::child,
                    )
                    .child(
                        div()
                            .id("worktree-prompt-scroll")
                            .flex_1()
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
                                    .child(prompt),
                            ),
                    );
            }
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
                            .mt(rpx(SPACE_LG))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                            ),
                    )
                });
            section = section
                .child(canvas_section_header(title.into()))
                .when_some(
                    self.checkout_panel.narrow_content.take(),
                    gpui::ParentElement::child,
                )
                .child(
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
                        .text_color(c::RED())
                        .child(super::super::status::label(
                            "content-error",
                            super::super::status::StatusKind::Failed,
                            error,
                            ICON_SM,
                            cx,
                        )),
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

pub(super) fn validate_worktree_fields(
    name: &str,
    branch: &str,
    base: &str,
) -> [Option<String>; 3] {
    let name = name.trim();
    let branch = branch.trim();
    let base = base.trim();
    [
        if name.is_empty() {
            Some("Enter a worktree name.".into())
        } else if !grove_core::git::valid_worktree_name(name) {
            Some(
                "Use letters, numbers, spaces, dots, hyphens, or underscores for the worktree name.".into(),
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
#[cfg(test)]
mod tests;
