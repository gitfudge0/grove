//! Shared sidebar row controls and project or list header controls.

use super::*;

impl Sidebar {
    #[cfg(test)]
    fn view_controls_with_gap(&self, gap: f32, cx: &mut Context<Self>) -> AnyElement {
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else if self.mode == ViewMode::Project {
            ViewMode::List
        } else {
            ViewMode::Project
        };
        let grid_selected = self.is_grid();
        let grid_control = self
            .control_with_tooltip(
                "sidebar-grid",
                "Open grid view",
                Action::Mode(if grid_selected {
                    self.last_mode
                } else {
                    ViewMode::Grid
                }),
                cx,
            )
            .debug_selector(|| "sidebar-grid".into())
            .child(icon("grid", ICON_MD, c::FG_DIM()));
        div()
            .flex()
            .items_center()
            .gap(rpx(gap))
            .child(
                self.control_with_tooltip(
                    "sidebar-view",
                    if target == ViewMode::List {
                        "Switch to list view"
                    } else {
                        "Switch to project view"
                    },
                    Action::Mode(target),
                    cx,
                )
                .debug_selector(|| "sidebar-view".into())
                .child(icon(
                    if target == ViewMode::List {
                        "list"
                    } else {
                        "folder"
                    },
                    ICON_MD,
                    c::FG_DIM(),
                )),
            )
            .child(motion::background(
                grid_control,
                format!("sidebar-grid-selection-{grid_selected}"),
                if grid_selected {
                    c::alpha(c::BG(), 0.0)
                } else {
                    c::BG_HOVER()
                },
                if grid_selected {
                    c::BG_HOVER()
                } else {
                    c::alpha(c::BG(), 0.0)
                },
                Duration::from_millis(MOTION_FAST_MS),
                cx,
            ))
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn navigation_heading(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let project = self.mode == ViewMode::Project;
        let gap = if project { SPACE_SM } else { SPACE_LG };
        let label = if self.mode == ViewMode::List {
            "Sessions"
        } else if !project && !self.multi_project_sessions(cx).is_empty() {
            ""
        } else {
            "Projects"
        };
        let mut run = window.text_style().to_run(label.len());
        run.font.weight = gpui::FontWeight::MEDIUM;
        let heading_size = if self.mode == ViewMode::List {
            TEXT_MICRO
        } else {
            TEXT_SMALL
        };
        let font_size = rpx(heading_size).to_pixels(window.rem_size());
        let label_width = crate::views::components::truncation_safe_label_width(
            label,
            font_size,
            &run,
            window.text_system(),
        );
        let count = format!("({})", self.snapshot.projects.len());
        let mut count_run = window.text_style().to_run(count.len());
        count_run.font.weight = gpui::FontWeight::NORMAL;
        let count_width = crate::views::components::truncation_safe_label_width(
            &count,
            font_size,
            &count_run,
            window.text_system(),
        );
        div()
            .id("sidebar-navigation-header")
            .debug_selector(|| "sidebar-navigation-header".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(heading_size))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c::FG_DIM())
            .child(motion::fast(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(project, |heading| {
                        heading.min_w(count_width + rpx(SPACE_SM).to_pixels(window.rem_size()))
                    })
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .debug_selector(|| "sidebar-heading-label".into())
                            .w(label_width)
                            .min_w_0()
                            .truncate()
                            .child(label),
                    )
                    .when(project, |heading| {
                        heading.child(
                            div()
                                .id("projects-count")
                                .debug_selector(|| "projects-count".into())
                                .flex_shrink_0()
                                .font_family(crate::fonts::UI_FAMILY)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .child(count),
                        )
                    }),
                format!("sidebar-heading-{:?}", self.mode),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rpx(gap))
                    .child(self.view_controls_with_gap(gap, cx))
                    .child(
                        self.control_with_tooltip(
                            "projects-add",
                            if self.mode == ViewMode::List {
                                "New session"
                            } else {
                                "Add project"
                            },
                            if self.mode == ViewMode::List {
                                Action::NewMultiProjectSession
                            } else {
                                Action::AddProject
                            },
                            cx,
                        )
                        .debug_selector(|| "projects-add".into())
                        .child(icon("plus", ICON_SM, c::FG_DIM())),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn control(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = label.into();
        let source_bounds = if matches!(action, Action::ToggleUsage) {
            self.usage_anchor.clone()
        } else {
            std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default()))
        };
        let click_bounds = source_bounds.clone();
        let key_bounds = source_bounds.clone();
        let click = action.clone();
        let icon_rail = self.is_collapsed(cx);
        let hierarchy_row = !icon_rail
            && matches!(
                &action,
                Action::Select(Selection::Project(_) | Selection::Worktree(..))
            );
        let project_session_row = !icon_rail
            && self.mode == ViewMode::Project
            && matches!(&action, Action::Select(Selection::Session(_)));
        let selected_session_row = matches!(
            &action,
            Action::Select(Selection::Session(id)) if self.selection == Some(Selection::Session(*id))
        );
        let project_diff_button = self.mode == ViewMode::Project
            && matches!(&action, Action::OpenDiff(_) | Action::OpenWorktreeDiff(_));
        let quiet_launch_control =
            matches!(&action, Action::RunScript(..) | Action::RunNamedScript(..));
        let quiet_launch_control = quiet_launch_control || action.is_fixture_launch();
        let project_disclosure = matches!(
            &action,
            Action::GroupSessions(_)
                | Action::ToggleGrid
                | Action::OpenWorkspaces
                | Action::MarkReviewed(..)
                | Action::MarkActivityDone(_)
        );
        let grouping_control = matches!(&action, Action::GroupSessions(_) | Action::ToggleGrid);
        let enabled = !matches!(&action, Action::ToggleGrid) || self.grid_navigation_available();
        let danger = matches!(action, Action::ConfirmWorktreeRemoval);
        let primary = matches!(action, Action::DismissWorktreeRemoval);
        let confirm = matches!(action, Action::ConfirmClose(_) | Action::ConfirmHome(_));
        let confirm = confirm || action.is_fixture_remove();
        let cancel = matches!(action, Action::Cancel);
        let confirming = self.confirmation_open();
        let decision = matches!(
            action,
            Action::ConfirmClose(_)
                | Action::ConfirmHome(_)
                | Action::ConfirmWorktreeRemoval
                | Action::SkipWorktreeTeardown
                | Action::DismissWorktreeRemoval
                | Action::Cancel
        );
        let decision = decision || action.is_fixture_remove();
        div()
            .id(id)
            .relative()
            .child(
                gpui::canvas(move |rect, _, _| source_bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .role(gpui::Role::Button)
            .when(
                enabled && (!confirming || decision),
                gpui::Styled::cursor_pointer,
            )
            .when(
                !enabled || confirming && !decision,
                gpui::Styled::cursor_default,
            )
            .when(!enabled, |control| control.opacity(OPACITY_DISABLED))
            .aria_label(label.clone())
            .tab_index(if !enabled || confirming && !decision {
                -1
            } else {
                0
            })
            .when(confirm, |d| d.track_focus(&self.confirm_focus))
            .when(cancel, |d| d.track_focus(&self.cancel_focus))
            .size(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .justify_center()
            .rounded(rpx(RADIUS_CONTROL))
            .text_color(c::FG_DIM())
            .when(
                enabled && (!confirming || decision) && !danger && !primary,
                |control| control.group("neutral-control"),
            )
            .hover(move |s| {
                if !enabled || confirming && !decision {
                    s
                } else if danger {
                    s.text_color(c::RED())
                } else if primary {
                    s.text_color(c::BG())
                } else if project_disclosure {
                    s.text_color(c::FG())
                } else {
                    s.bg(c::BG_HOVER()).text_color(c::FG())
                }
            })
            .focus_visible(move |s| {
                if !enabled {
                    s
                } else if grouping_control || hierarchy_row {
                    s.border_1().border_color(c::FG())
                } else if project_session_row {
                    s.bg(if selected_session_row {
                        c::alpha(c::FG(), 0.14)
                    } else {
                        c::BG_HOVER()
                    })
                    .border_1()
                    .border_color(c::FG())
                } else if project_diff_button || quiet_launch_control {
                    s.border_1().border_color(c::FG())
                } else if danger {
                    s.bg(c::RED_WASH()).text_color(c::RED())
                } else if primary {
                    s.bg(c::FG_DIM()).text_color(c::BG())
                } else {
                    let style = s.bg(c::BG_HOVER());
                    if icon_rail {
                        style.border_1().border_color(c::FG())
                    } else {
                        style
                    }
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                gpui_component::global_state::GlobalState::suppress_text_selection(cx);
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if !enabled {
                    return;
                }
                if click.is_fixture_launch()
                    || matches!(
                        &click,
                        Action::RunScript(..)
                            | Action::RunNamedScript(..)
                            | Action::RemoveWorktree(..)
                            | Action::HistoryFilterMenu(..)
                            | Action::ToggleUsage
                    )
                {
                    this.action_source_bounds.set(click_bounds.get());
                }
                #[cfg(test)]
                if matches!(
                    &click,
                    Action::OpenWorktreeDiff(_) | Action::Select(Selection::Worktree(..))
                ) {
                    eprintln!(
                        "worktree diff click {}",
                        if matches!(&click, Action::OpenWorktreeDiff(_)) {
                            "diff"
                        } else {
                            "row"
                        }
                    );
                }
                this.act(click.clone(), window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if !enabled {
                        return;
                    }
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        if action.is_fixture_launch()
                            || matches!(
                                &action,
                                Action::RunScript(..)
                                    | Action::RunNamedScript(..)
                                    | Action::RemoveWorktree(..)
                                    | Action::HistoryFilterMenu(..)
                                    | Action::ToggleUsage
                            )
                        {
                            this.action_source_bounds.set(key_bounds.get());
                        }
                        this.act(action.clone(), window, cx);
                    }
                }),
            )
    }

    /// Add a tooltip only for controls whose owner explicitly needs one.
    pub(super) fn control_with_tooltip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = label.into();
        let tooltip_label = label.clone();
        self.control(id, label, action, cx)
            .tooltip(move |window, cx| {
                crate::views::components::tooltip(tooltip_label.clone(), window).build(window, cx)
            })
    }

    pub(super) fn row(
        &self,
        id: String,
        label: String,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let worktree = matches!(&action, Action::Select(Selection::Worktree(..)));
        let tooltip_label = label.clone();
        self.control(SharedString::from(id.clone()), label, action, cx)
            .when(worktree, |row| {
                row.tooltip(move |window, cx| {
                    crate::views::components::tooltip(tooltip_label.clone(), window)
                        .build(window, cx)
                })
            })
            .debug_selector(move || id.clone())
            .w_full()
            .h(rpx(ROW_H))
            .justify_start()
            .px(rpx(SPACE_SM))
            .gap(rpx(SPACE_MD))
            .rounded(rpx(RADIUS_CHROME))
            .when(selected, |d| {
                d.bg(c::alpha(c::FG(), 0.14)).text_color(c::FG())
            })
    }
}
