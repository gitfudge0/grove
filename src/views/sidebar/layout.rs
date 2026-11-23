//! Sidebar frame, canvas composition and overlays.

use super::{
    branch_history, c, rpx, Action, Agent, HashSet, Sidebar, ViewMode, WorktreeRemovalStage,
};
use crate::views::tokens::{APPBAR_TOTAL_H, RADIUS_PANEL, SPACE_LG, TEXT_BODY};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, Render, Styled,
};
use gpui::{div, Context, FocusHandle, MouseButton, MouseMoveEvent, Window};

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mode != ViewMode::Grid || self.is_zen() || !self.navigation_available() {
            self.grid_session_drag = None;
        }
        self.window_active = window.is_window_active();
        self.sync_dirty |=
            self.registry_sync_revision != self.runtime.read(cx).registry.read(cx).revision();
        crate::views::inline_html::exclude_native_html(None, cx);
        if self.sync_dirty {
            self.sync(window, cx);
        }
        if self.history.is_some() && self.history_selection != self.selection {
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
        }
        self.sync_age_timer(cx);
        self.sync_session_placement(cx);
        if self.project_flyout.is_some() {
            let mut keys = HashSet::new();
            for project in self.project_navigation_snapshot(cx).projects {
                keys.insert(format!("overview-{}", project.idx));
                for worktree in project.worktrees {
                    for agent in [Agent::Claude, Agent::Codex, Agent::OpenCode] {
                        keys.insert(format!("launch-{}-{}", agent.label(), worktree.path));
                    }
                    for id in worktree.sessions {
                        keys.insert(format!("session-{}", id.raw()));
                    }
                }
            }
            self.project_flyout_item_focus
                .retain(|key, _| keys.contains(key));
            for key in keys {
                self.project_flyout_item_focus
                    .entry(key)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            let paths = self
                .project_navigation_snapshot(cx)
                .projects
                .iter()
                .flat_map(|project| {
                    project
                        .worktrees
                        .iter()
                        .map(|worktree| worktree.path.clone())
                })
                .collect::<HashSet<_>>();
            self.project_flyout_plus_focus
                .retain(|path, _| paths.contains(path));
            for path in paths {
                self.project_flyout_plus_focus
                    .entry(path)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            if self
                .project_flyout_launch_path
                .as_ref()
                .is_some_and(|path| !self.project_flyout_plus_focus.contains_key(path))
            {
                self.project_flyout_launch_path = None;
                self.project_flyout_launch_error = None;
            }
        }
        let global_rail = self.global_rail(window, cx);
        self.rendered_terminal_ids.clear();
        let content = self.render_content(window, cx);
        if self.rendered_terminal_ids != self.cached_terminal_ids {
            let registry = self.runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                registry.release_hidden_scenes(&self.rendered_terminal_ids, cx);
            });
            self.cached_terminal_ids
                .clone_from(&self.rendered_terminal_ids);
        }
        let navigation_close_confirmation = self.navigation_close_confirmation(window, cx);
        let confirming = self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some();
        div()
            .size_full()
            .relative()
            .flex()
            .min_h_0()
            .track_focus(&self.focus)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .bg(c::BG_RAIL())
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.divider_move(event, window, cx);
                this.track_project_flyout_pointer(event.position, window, cx);
                this.track_usage_pointer(event.position, window, cx);
                if this.menu_opened_by_hover
                    && !this.menu_trigger_bounds.get().contains(&event.position)
                    && !this.menu_popup_bounds.get().contains(&event.position)
                {
                    this.close_menu(window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.divider_release(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.divider_release(cx);
                }),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.usage_panel.is_some() {
                    this.dismiss_usage(!this.usage_hover_open, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if event.keystroke.key == "escape" && this.branch_peek.is_some() {
                    this.dismiss_branch_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.checkout_panel.plan_peek.is_some()
                    && !this.confirmation_open()
                {
                    this.dismiss_plan_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.checkout_panel.script_peek.is_some() {
                    this.dismiss_script_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.checkout_panel.peek.is_some()
                    && !this.confirmation_open()
                {
                    this.dismiss_agent_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "tab" && this.header_tasks.open.is_some() {
                    this.focus_header_task_disclosure(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.header_tasks.open.is_some() {
                    this.dismiss_header_tasks(true, window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" {
                    if let Some(selector) = this.workspace_selector.clone() {
                        if selector
                            .update(cx, |selector, cx| selector.dismiss_hover_menu(window, cx))
                        {
                            cx.stop_propagation();
                            return;
                        }
                    }
                }
                if event.keystroke.key == "escape"
                    && this.project_flyout.is_some()
                    && !this.confirmation_open()
                {
                    if this.collapse_project_launch_options(window, cx) {
                        window.prevent_default();
                        cx.stop_propagation();
                        return;
                    }
                    if this.project_flyout_hover_open
                        && !this.project_flyout_focus.contains_focused(window, cx)
                    {
                        this.dismiss_project_context(cx);
                        this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                    } else {
                        this.close_project_flyout(window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if this.history.is_some() {
                    if event.keystroke.key == "tab" {
                        for _ in 0..64 {
                            if event.keystroke.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            if this.history_focus.contains_focused(window, cx) {
                                break;
                            }
                        }
                        if !this.history_focus.contains_focused(window, cx) {
                            this.history_focus.focus(window, cx);
                        }
                        cx.stop_propagation();
                    }
                    if event.keystroke.key == "escape" {
                        this.act(Action::CloseHistory, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if this.review_is_open(cx) && event.keystroke.key == "escape" {
                    this.close_review(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if this.description_is_open(cx) && event.keystroke.key == "escape" {
                    this.close_description(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if this.diff_viewer.is_some() {
                    if event.keystroke.key == "escape" {
                        this.act(Action::CloseDiff, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if let (Some(removal), "tab") = (
                    this.pending_worktree_removal.as_ref(),
                    event.keystroke.key.as_str(),
                ) {
                    let status = this
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .cloned();
                    let finished = removal.started
                        && (removal.error.is_some()
                            || status
                                .as_ref()
                                .is_some_and(|s| s.stage == WorktreeRemovalStage::Finished));
                    let handles: Vec<FocusHandle> = if !removal.started {
                        vec![this.cancel_focus.clone(), this.confirm_focus.clone()]
                    } else if finished
                        || status
                            .as_ref()
                            .is_some_and(|s| s.stage == WorktreeRemovalStage::RunningScript)
                    {
                        vec![this.confirm_focus.clone()]
                    } else {
                        Vec::new()
                    };
                    window.prevent_default();
                    if handles.is_empty() {
                        this.worktree_removal_focus.focus(window, cx);
                    } else {
                        let current = handles.iter().position(|focus| focus.is_focused(window));
                        let shift = event.keystroke.modifiers.shift;
                        let next = match current {
                            Some(index) if shift => (index + handles.len() - 1) % handles.len(),
                            Some(index) => (index + 1) % handles.len(),
                            None if shift => handles.len() - 1,
                            None => 0,
                        };
                        handles[next].focus(window, cx);
                    }
                    cx.stop_propagation();
                } else if this.confirmation_open()
                    && !this.project_decision
                    && this.pending_worktree_removal.is_none()
                    && event.keystroke.key == "tab"
                {
                    window.prevent_default();
                    if this.cancel_focus.is_focused(window) {
                        this.confirm_focus.focus(window, cx);
                    } else {
                        this.cancel_focus.focus(window, cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape"
                    && !this.project_decision
                    && (this.confirmation_open() || this.menu.is_some())
                {
                    if this.checkout_confirmation_open() {
                        this.checkout_cancel_confirmation(window, cx);
                    } else {
                        this.act(Action::Cancel, window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(global_rail)
            .child(
                div()
                    .id("sidebar-canvas")
                    .debug_selector(|| "sidebar-canvas".into())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(cfg!(target_os = "macos"), gpui::Styled::h_full)
                    .when(!cfg!(target_os = "macos"), |canvas| {
                        canvas.mt(rpx(APPBAR_TOTAL_H))
                    })
                    .bg(c::BG())
                    .rounded_tl(rpx(RADIUS_PANEL))
                    .overflow_hidden()
                    .child(content)
                    .when(self.history.is_some(), |canvas| {
                        canvas.child(gpui::deferred(self.render_history(window, cx)))
                    })
                    .when_some(
                        crate::views::native_plan::render_expanded(cx),
                        |canvas, plan| canvas.child(gpui::deferred(plan)),
                    )
                    .when(confirming && !self.project_decision, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(c::QUIET_VEIL())
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                }),
                        )
                    }),
            )
            .when_some(self.render_usage_popup(window, cx), |root, popup| {
                root.child(popup)
            })
            .when_some(navigation_close_confirmation, |root, popup| {
                root.child(popup)
            })
            .when_some(self.render_branch_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_script_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_agent_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_plan_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when(self.checkout_confirmation_open(), |root| {
                let source = self.checkout_confirmation_bounds();
                root.child(gpui::deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(c::QUIET_VEIL())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            gpui::anchored()
                                .position_mode(gpui::AnchoredPositionMode::Window)
                                .position(gpui::point(source.left(), source.bottom()))
                                .snap_to_window_with_margin(
                                    rpx(SPACE_LG).to_pixels(window.rem_size()),
                                )
                                .child(self.render_checkout_confirmation(window, cx)),
                        ),
                ))
            })
            .when_some(self.pending_worktree_removal.as_ref(), |root, removal| {
                let source = self.action_source_bounds.get();
                root.child(gpui::deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(c::QUIET_VEIL())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            gpui::anchored()
                                .position_mode(gpui::AnchoredPositionMode::Window)
                                .position(gpui::point(source.left(), source.bottom()))
                                .snap_to_window_with_margin(
                                    rpx(SPACE_LG).to_pixels(window.rem_size()),
                                )
                                .child(self.render_worktree_removal(removal, cx)),
                        ),
                ))
            })
            .when(self.diff_viewer.is_some(), |d| {
                d.child(gpui::deferred(self.render_diff(window, cx)))
            })
    }
}
