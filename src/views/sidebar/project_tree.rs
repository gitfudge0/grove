//! Project and worktree tree rows and project menu presentation.

use super::*;

#[cfg(test)]
const HIERARCHY_ICON: f32 = 15.0;
#[cfg(test)]
const HIERARCHY_LABEL_INSET: f32 = HIERARCHY_INSET + HIERARCHY_ICON_SLOT + HIERARCHY_GAP;
#[cfg(test)]
const PROJECT_GROUP_GAP: f32 = 14.0;
#[cfg(test)]
const PROJECT_TEXT: f32 = TEXT_TITLE;
#[cfg(test)]
const WORKTREE_ROW_H: f32 = ROW_H;
#[cfg(test)]
const WORKTREE_TITLE_LINE_H: f32 = 17.0;
#[cfg(test)]
const WORKTREE_ACTION_W: f32 = CHROME_CONTROL_H;

impl Sidebar {
    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_opened_by_hover = false;
        if let Some(focus) = self.menu_return_focus.take() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    fn open_menu_on_hover(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.menu == Some(idx) || self.confirmation_open() {
            return;
        }
        self.menu = Some(idx);
        self.menu_opened_by_hover = true;
        self.menu_index = 0;
        self.menu_return_focus = None;
        if let Some(bounds) = self.project_menu_bounds.get(&idx) {
            self.menu_trigger_bounds = bounds.clone();
        }
        cx.notify();
    }

    #[cfg(test)]
    fn project_popup(&self, idx: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = cx.global::<SettingsState>().store.projects.get(idx) else {
            return div().into_any_element();
        };
        let path = project.path.clone();
        let readiness = self
            .worktree_readiness
            .get(&path)
            .and_then(|(_, value)| *value);
        let mut actions = vec![
            ("Edit project", "edit", Action::EditProject(path.clone())),
            (
                "Move to workspace…",
                "folder",
                Action::MoveProject(path.clone()),
            ),
        ];
        if readiness == Some(WorktreeReadiness::Ready) {
            actions.push(("New worktree", "plus", Action::NewWorktree(path.clone())));
        } else if readiness == Some(WorktreeReadiness::NeedsGit) {
            actions.push((
                "Initialize Git",
                "plus",
                Action::InitializeGit(path.clone()),
            ));
        } else if readiness == Some(WorktreeReadiness::NeedsCommit) {
            actions.push((
                "Check for first commit",
                "refresh",
                Action::RefreshGit(path.clone()),
            ));
        }
        actions.extend([
            (
                "Open in file manager",
                "folder",
                Action::Reveal(path.clone()),
            ),
            (
                "Remove project",
                "close",
                Action::RemoveProject(path.clone()),
            ),
        ]);
        let keyboard_actions = actions
            .iter()
            .map(|(_, _, action)| action.clone())
            .collect::<Vec<_>>();
        let action_count = actions.len();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = SIDEBAR_W.min(f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0);
        let mut panel = div()
            .text_color(c::FG())
            .relative()
            .id("project-actions-popup")
            .debug_selector(|| "project-actions-popup".into())
            .role(gpui::Role::Menu)
            .aria_label(format!(
                "Actions for {}",
                cx.global::<SettingsState>().store.projects[idx].name
            ))
            .track_focus(&self.menu_focus)
            .w(rpx(width))
            .p(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_CHROME))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .occlude()
            .on_prepaint({
                let bounds = self.menu_popup_bounds.clone();
                move |rect, _, cx| {
                    bounds.set(rect);
                    crate::views::inline_html::exclude_native_html(Some(rect), cx);
                }
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.menu_trigger_bounds.get().contains(&event.position) {
                        this.close_menu(window, cx);
                    }
                }),
            )
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "down" | "up" | "tab" => {
                            window.prevent_default();
                            let backward = event.keystroke.key == "up"
                                || (event.keystroke.key == "tab"
                                    && event.keystroke.modifiers.shift);
                            this.menu_index = if backward {
                                (this.menu_index + action_count - 1) % action_count
                            } else {
                                (this.menu_index + 1) % action_count
                            };
                            this.menu_focus.focus(window, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                        "enter" | "space" => {
                            if let Some(action) = keyboard_actions
                                .get(this.menu_index % action_count)
                                .cloned()
                            {
                                this.act(action, window, cx);
                            }
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.close_menu(window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            );
        for (index, (label, glyph, action)) in actions.into_iter().enumerate() {
            panel = panel.child(
                self.control(("project-action", index), label, action, cx)
                    .role(gpui::Role::MenuItem)
                    .w_full()
                    .h(rpx(ROW_H))
                    .justify_start()
                    .px(rpx(SPACE_LG))
                    .gap(rpx(SPACE_LG))
                    .when(index == self.menu_index, |d| d.bg(c::BG_HOVER()))
                    .child(icon(glyph, ICON_SM, c::FG_DIM()))
                    .child(label),
            );
        }
        let trigger = self.menu_trigger_bounds.get();
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::point(trigger.right(), trigger.top()))
            .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
            .child(motion::fast(panel, format!("project-menu-enter-{idx}"), cx))
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn project_tree(
        &self,
        project_filter: Option<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible_sessions = self.visible_session_order(cx);
        let mut snapshot = self.project_navigation_snapshot(cx);
        if let Some(idx) = project_filter {
            snapshot.projects.retain(|p| p.idx == idx);
        }
        let mut body = div().flex().flex_col();
        if self.mode == ViewMode::Project {
            body = body.child(
                div()
                    .mt(rpx(SPACE_2XL))
                    .mb(rpx(SPACE_SM))
                    .px(rpx(HIERARCHY_INSET))
                    .child(self.navigation_heading(window, cx)),
            );
            if snapshot.projects.is_empty() {
                body = body.child(
                    div()
                        .p(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("No projects yet"),
                );
            }
        }
        for (project_position, project) in snapshot.projects.iter().enumerate() {
            let idx = project.idx;
            let project_path = self
                .project_paths
                .get(&idx)
                .cloned()
                .or_else(|| {
                    cx.global::<SettingsState>()
                        .store
                        .projects
                        .get(idx)
                        .map(|stored| stored.path.clone())
                })
                .unwrap_or_else(|| format!("missing-project-{idx}"));
            let expanded = !self.collapsed_projects.contains(&project_path);
            let previous_collapsed = project_position > 0
                && self
                    .snapshot
                    .projects
                    .get(project_position - 1)
                    .is_some_and(|previous| {
                        self.project_paths
                            .get(&previous.idx)
                            .is_some_and(|path| self.collapsed_projects.contains(path))
                    });
            let mut group = div().flex().flex_col().when(project_position > 0, |group| {
                group
                    .mt(rpx(if previous_collapsed {
                        SPACE_SM
                    } else {
                        PROJECT_GROUP_GAP
                    }))
                    .pt(rpx(if previous_collapsed {
                        SPACE_XS
                    } else {
                        SPACE_LG
                    }))
                    .border_t_1()
                    .border_color(c::alpha(c::FG(), 0.08))
            });
            group = group.child(
                self.row(
                    format!("project-{idx}"),
                    format!("{} project", project.name),
                    false,
                    Action::ToggleProject(project_path.clone()),
                    cx,
                )
                .h(rpx(PROJECT_ROW_H))
                .group("project-row")
                .px(rpx(HIERARCHY_INSET))
                .gap(rpx(HIERARCHY_GAP))
                .text_size(rpx(PROJECT_TEXT))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c::FG())
                .child(
                    self.control_with_tooltip(
                        ("project-folder-toggle", idx),
                        format!(
                            "{} {} project",
                            if expanded { "Collapse" } else { "Expand" },
                            project.name
                        ),
                        Action::ToggleProject(project_path.clone()),
                        cx,
                    )
                    .debug_selector(move || format!("project-folder-toggle-{idx}"))
                    .size(rpx(HIERARCHY_ICON_SLOT))
                    .justify_start()
                    .when_some(
                        self.project_toggle_focus.get(&project_path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .aria_expanded(expanded)
                    .child(motion::fast(
                        icon(
                            if expanded { "folder-open" } else { "folder" },
                            HIERARCHY_ICON,
                            c::FG_DIM(),
                        ),
                        format!("project-disclosure-icon-{idx}-{expanded}"),
                        cx,
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(
                            div()
                                .id(("project-title", idx))
                                .debug_selector(move || format!("project-title-{idx}"))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .truncate()
                                .line_height(rpx(PROJECT_TITLE_LINE_H))
                                .child(project.name.clone()),
                        )
                        .when(!project.is_git, |name| {
                            name.child(
                                div()
                                    .id(("project-no-git", idx))
                                    .debug_selector(move || format!("project-no-git-{idx}"))
                                    .role(gpui::Role::Image)
                                    .aria_label("Not a Git repository")
                                    .size(rpx(ICON_MD))
                                    .flex_shrink_0()
                                    .tooltip(|window, cx| {
                                        crate::views::components::tooltip(
                                            "Not a Git repository",
                                            window,
                                        )
                                        .build(window, cx)
                                    })
                                    .child(icon("no-git", ICON_MD, c::YELLOW())),
                            )
                        }),
                )
                .child(
                    self.control_with_tooltip(
                        ("project-menu", idx),
                        format!("Actions for {}", project.name),
                        Action::Menu(idx),
                        cx,
                    )
                    .relative()
                    .size(rpx(HIERARCHY_TRAILING_W))
                    .opacity(0.0)
                    .group_hover("project-row", |button| button.opacity(1.0))
                    .focus_visible(|button| button.opacity(1.0))
                    .when_some(
                        self.project_menu_focus.get(&idx),
                        gpui::InteractiveElement::track_focus,
                    )
                    .child(
                        div()
                            .id(("project-menu-glyph", idx))
                            .debug_selector(move || format!("project-menu-glyph-{idx}"))
                            .size(rpx(ICON_SM))
                            .child(icon("more", ICON_SM, c::FG_DIM())),
                    )
                    .debug_selector(move || format!("project-menu-{idx}"))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.open_menu_on_hover(idx, cx);
                        }
                    }))
                    .when_some(
                        self.project_menu_bounds.get(&idx).cloned(),
                        |button, bounds| {
                            button.child(
                                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                        },
                    )
                    .when(self.menu == Some(idx), |button| {
                        button.child(gpui::deferred(self.project_popup(idx, window, cx)))
                    }),
                ),
            );
            if self.pending_remove == Some(idx) {
                group = group.child(self.confirmation(
                    &format!("Remove {} from Grove? Its sessions will stop. The repository and worktrees remain on disk.",project.name),
                    Action::ConfirmRemove(idx),
                    cx,
                ));
            }
            if !expanded {
                body = body.child(group);
                continue;
            }
            if project.worktrees.is_empty() {
                group = group.child(
                    div()
                        .pl(rpx(HIERARCHY_LABEL_INSET))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(crate::views::status::label(
                            format!("worktrees-loading-{idx}"),
                            crate::views::status::StatusKind::Working,
                            "Loading worktrees…",
                            ICON_SM,
                            cx,
                        )),
                );
            }
            for (worktree_position, worktree) in project.worktrees.iter().enumerate() {
                let path = worktree.path.clone();
                let worktree_name = sidebar_worktree_name(&worktree.name, worktree.is_main);
                let action_count = WORKTREE_LAUNCH_AGENTS.len() + (!worktree.is_main) as usize;
                let launch_width = WORKTREE_ACTION_W * action_count as f32;
                let selection = Selection::Worktree(idx, path.clone());
                let selected = self.selection == Some(selection.clone());
                let focused = self
                    .worktree_focus
                    .get(&path)
                    .is_some_and(|f| f.contains_focused(window, cx));
                let hovered = self.hovered_worktree.as_ref() == Some(&path);
                let diff_focused = self
                    .worktree_diff_focus
                    .get(&path)
                    .is_some_and(|focus| focus.is_focused(window));
                let show_launchers = (focused || hovered) && !diff_focused;
                let mut launches = div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .w(rpx(if show_launchers { launch_width } else { 0.0 }))
                    .overflow_hidden()
                    .h(rpx(CHROME_CONTROL_H))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .opacity(if show_launchers { 1.0 } else { 0.0 });
                for (n, agent) in WORKTREE_LAUNCH_AGENTS.into_iter().enumerate() {
                    let agent_name = match agent {
                        Agent::Codex => "Codex",
                        Agent::Claude => "Claude",
                        Agent::OpenCode => "OpenCode",
                        Agent::Terminal => "Terminal",
                    };
                    let label = if self.available[n] {
                        format!(
                            "Start {} in {} · {}",
                            agent_name, project.name, worktree_name
                        )
                    } else {
                        format!("{agent_name} is not installed")
                    };
                    launches = launches.child(
                        self.control_with_tooltip(
                            SharedString::from(format!("launch-{path}-{n}")),
                            label,
                            Action::Launch(idx, path.clone(), agent),
                            cx,
                        )
                        .size(rpx(WORKTREE_ACTION_W))
                        .debug_selector({
                            let path = path.clone();
                            move || format!("launch-{path}-{n}")
                        })
                        .when(!self.available[n], |d| {
                            d.tab_index(-1).opacity(OPACITY_DISABLED)
                        })
                        .child(icon(
                            agent.icon_name(),
                            ICON_SM,
                            c::FG_DIM(),
                        )),
                    );
                }
                if !worktree.is_main {
                    launches = launches.child(
                        self.control_with_tooltip(
                            SharedString::from(format!("delete-worktree-{path}")),
                            format!("Delete worktree {}", worktree.name),
                            Action::RemoveWorktree(idx, path.clone()),
                            cx,
                        )
                        .size(rpx(WORKTREE_ACTION_W))
                        .debug_selector({
                            let path = path.clone();
                            move || format!("delete-worktree-{path}")
                        })
                        .when_some(
                            self.worktree_delete_focus.get(&path),
                            gpui::InteractiveElement::track_focus,
                        )
                        .child(icon("trash", ICON_SM, c::FG_DIM())),
                    );
                }
                let diff = self
                    .runtime
                    .read(cx)
                    .tree
                    .read(cx)
                    .git_state(crate::paths::normalize_wt_path(&path))
                    .copied();
                let diff_label = match &diff {
                    Some(state) if state.dirty || state.added > 0 || state.removed > 0 => {
                        format!("Worktree changes: +{} −{}", state.added, state.removed)
                    }
                    Some(_) => "Worktree clean".into(),
                    None => "Git status unavailable".into(),
                };
                let diff_element = match &diff {
                    Some(state) if state.dirty || state.added > 0 || state.removed > 0 => self
                        .control(
                            SharedString::from(format!("worktree-diff-{path}")),
                            format!("{diff_label}; open changes in {path}"),
                            Action::OpenWorktreeDiff(path.clone()),
                            cx,
                        )
                        .debug_selector({
                            let path = path.clone();
                            move || {
                                if show_launchers {
                                    format!("worktree-diff-hidden-{path}")
                                } else {
                                    format!("worktree-diff-{path}")
                                }
                            }
                        })
                        .when_some(
                            self.worktree_diff_focus.get(&path),
                            gpui::InteractiveElement::track_focus,
                        )
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .w_auto()
                        .h_auto()
                        .flex_shrink_0()
                        .gap(rpx(SPACE_MD))
                        .pr(rpx((CONTROL_H - ICON_MD) / 2.0))
                        .text_size(rpx(HIERARCHY_META_TEXT))
                        .when(state.added > 0, |chip| {
                            chip.child(
                                div()
                                    .text_color(c::GREEN())
                                    .child(format!("+{}", state.added)),
                            )
                        })
                        .when(state.removed > 0, |chip| {
                            chip.child(
                                div()
                                    .text_color(c::RED())
                                    .child(format!("−{}", state.removed)),
                            )
                        })
                        .when(state.added == 0 && state.removed == 0, |chip| {
                            chip.child(div().text_color(c::GREEN()).child("Changes"))
                        })
                        .into_any_element(),
                    _ => div().into_any_element(),
                };
                group = group.child(
                    self.row(
                        format!("worktree-{path}"),
                        format!(
                            "{} worktree, branch {} · {diff_label}",
                            worktree_name, worktree.branch
                        ),
                        false,
                        Action::Select(selection),
                        cx,
                    )
                    .relative()
                    .group("worktree-row")
                    .tab_group()
                    .on_hover(cx.listener({
                        let path = path.clone();
                        move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.hovered_worktree = Some(path.clone());
                            } else if this.hovered_worktree.as_ref() == Some(&path) {
                                this.hovered_worktree = None;
                            }
                            cx.notify();
                        }
                    }))
                    .when_some(
                        self.worktree_focus.get(&path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .h(rpx(WORKTREE_ROW_H))
                    .mt(rpx(if worktree_position == 0 {
                        SPACE_SM
                    } else {
                        WORKTREE_GROUP_GAP
                    }))
                    .pl(rpx(WORKTREE_LEADING_INSET))
                    .pr(rpx(HIERARCHY_INSET))
                    .gap(rpx(HIERARCHY_GAP))
                    .text_size(rpx(TEXT_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c::FG())
                    .child(
                        div()
                            .w(rpx(HIERARCHY_ICON_SLOT))
                            .h(rpx(WORKTREE_TITLE_LINE_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_start()
                            .child(icon(
                                "git-branch",
                                HIERARCHY_ICON,
                                if selected { c::FG() } else { c::FG_DIM() },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(rpx(WORKTREE_TITLE_LINE_H))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_XS))
                            .id(SharedString::from(format!("worktree-title-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-title-{path}")
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_ellipsis_middle()
                                    .line_height(rpx(WORKTREE_TITLE_LINE_H))
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_SM))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(worktree_name.to_owned()),
                                    ),
                            ),
                    )
                    .child(
                        // Launch controls replace the checkout metadata; focusing changes reveals its keyboard target.
                        div()
                            .id(SharedString::from(format!("worktree-actions-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-actions-{path}")
                            })
                            .relative()
                            .min_w(rpx(if show_launchers {
                                launch_width
                            } else {
                                HIERARCHY_TRAILING_W
                            }))
                            .h(rpx(CHROME_CONTROL_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .opacity(if show_launchers { 0.0 } else { 1.0 })
                                    .child(diff_element),
                            )
                            .child(launches),
                    ),
                );
                if worktree.sessions.is_empty() {
                    group = group.child(
                        div()
                            .id(SharedString::from(format!("worktree-empty-{path}")))
                            .pl(rpx(SESSION_LEADING_INSET
                                + SESSION_LAUNCHER_SIZE
                                + SPACE_SM))
                            .py(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .id(SharedString::from(format!("worktree-empty-text-{path}")))
                                    .debug_selector({
                                        let path = path.clone();
                                        move || format!("worktree-empty-{path}")
                                    })
                                    .child("No sessions"),
                            ),
                    );
                }
                for (session_position, id) in worktree.sessions.iter().enumerate() {
                    if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(*id).cloned() {
                        let number = visible_sessions
                            .iter()
                            .position(|visible| *visible == *id)
                            .map(|index| index + 1);
                        group = group.child(
                            div()
                                .when(session_position == 0, |row| row.mt(rpx(SESSION_FIRST_GAP)))
                                .child(
                                    self.session_row(
                                        &meta,
                                        false,
                                        number,
                                        self.session_diff_focus
                                            .get(id)
                                            .is_some_and(|focus| focus.is_focused(window)),
                                        window,
                                        cx,
                                    ),
                                ),
                        );
                    }
                }
            }
            body = body.child(group);
        }
        body.into_any_element()
    }
}
