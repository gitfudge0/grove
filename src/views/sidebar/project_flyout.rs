//! Deliberately opened project context without changing the selected terminal.
use super::*;

const FLYOUT_W: f32 = 320.0;
const HOVER_DISMISS_DELAY: Duration = Duration::from_millis(MOTION_SLOW_MS);

pub(super) fn project_identifiers(names: &[&str]) -> Vec<String> {
    let bases = names
        .iter()
        .map(|name| {
            let words = name
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let letters = if words.len() > 1 {
                words
                    .iter()
                    .filter_map(|s| s.chars().next())
                    .take(2)
                    .collect::<String>()
            } else {
                name.chars()
                    .filter(|c| c.is_alphanumeric())
                    .take(2)
                    .collect()
            };
            if letters.is_empty() {
                "P".to_string()
            } else {
                letters.to_uppercase()
            }
        })
        .collect::<Vec<_>>();
    let mut used = HashSet::new();
    bases
        .iter()
        .map(|base| {
            let mut candidate = base.clone();
            let mut suffix = 1;
            while !used.insert(candidate.clone()) {
                suffix += 1;
                candidate = format!("{base}{suffix}");
            }
            candidate
        })
        .collect()
}

impl Sidebar {
    pub(crate) fn set_project_hover_blocked(&mut self, blocked: bool, cx: &mut Context<Self>) {
        if self.project_flyout_hover_blocked != blocked {
            self.project_flyout_hover_blocked = blocked;
            if blocked {
                self.dismiss_project_context(cx);
            } else {
                self.project_flyout_suppressed = None;
            }
        }
    }

    fn project_hover_allowed(&self, cx: &App) -> bool {
        self.is_collapsed(cx)
            && self.mode == ViewMode::Project
            && !self.is_zen()
            && self.navigation_available()
            && !self.project_flyout_hover_blocked
            && !self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            && !self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.read(cx).is_open())
    }

    pub(super) fn open_sidebar_context_on_hover(
        &mut self,
        target: SidebarContext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.project_hover_allowed(cx)
            || self.project_flyout_suppressed.as_ref() == Some(&target)
            || !self.sidebar_context_is_active(&target, cx)
        {
            return;
        }
        self.project_flyout_hide_task = None;
        if self.project_flyout.as_ref() == Some(&target) {
            return;
        }
        if let Some(bounds) = self.sidebar_context_trigger(&target) {
            self.project_flyout_bounds = bounds;
        }
        self.project_flyout_hover_open =
            self.project_flyout.is_none() || self.project_flyout_hover_open;
        self.project_flyout_launch_path = None;
        self.project_flyout_launch_error = None;
        self.project_flyout = Some(target);
        self.project_flyout_index = 0;
        self.project_flyout_scroll = ScrollHandle::new();
        cx.notify();
    }

    fn project_group_rect(&self, target: &SidebarContext) -> Option<gpui::Bounds<gpui::Pixels>> {
        match target {
            SidebarContext::MultiProject => Some(self.multi_project_group_bounds.get()),
            SidebarContext::Project(path) => self
                .project_paths
                .iter()
                .find(|(_, candidate)| *candidate == path)
                .and_then(|(idx, _)| self.project_group_bounds.get(idx))
                .map(|bounds| bounds.get()),
        }
    }

    fn pointer_in_project_context(&self, point: gpui::Point<gpui::Pixels>) -> bool {
        let Some(group) = self
            .project_flyout
            .as_ref()
            .and_then(|target| self.project_group_rect(target))
        else {
            return false;
        };
        let popup = self.project_flyout_popup_bounds.get();
        group.contains(&point)
            || popup.contains(&point)
            || (point.x >= group.right()
                && point.x <= popup.left()
                && point.y >= group.top().max(popup.top())
                && point.y <= group.bottom().min(popup.bottom()))
    }

    pub(super) fn track_project_flyout_pointer(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .project_flyout_suppressed
            .as_ref()
            .is_some_and(|target| {
                self.project_group_rect(target)
                    .is_none_or(|bounds| !bounds.contains(&point))
            })
        {
            self.project_flyout_suppressed = None;
        }
        if self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject)
            && self.confirmation_open()
        {
            // Keep the close dialog's owning context mounted until the decision resolves.
            self.project_flyout_hide_task = None;
            return;
        }
        if !self.project_flyout_hover_open || self.project_flyout.is_none() {
            return;
        }
        if self.pointer_in_project_context(point) {
            self.project_flyout_hide_task = None;
        } else if self.project_flyout_hide_task.is_none() {
            let path = self.project_flyout.clone();
            self.project_flyout_hide_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(HOVER_DISMISS_DELAY).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.project_flyout == path
                        && this.project_flyout_hover_open
                        && (!this.project_hover_allowed(cx)
                            || !this.pointer_in_project_context(window.mouse_position()))
                    {
                        this.dismiss_project_context(cx);
                        this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                    }
                    this.project_flyout_hide_task = None;
                });
            }));
        }
    }

    pub(crate) fn dismiss_project_context(&mut self, cx: &mut Context<Self>) {
        self.project_flyout_hide_task = None;
        self.project_flyout_hover_open = false;
        self.project_flyout_launch_path = None;
        self.project_flyout_launch_error = None;
        if let Some(path) = self.project_flyout.take() {
            self.project_flyout_suppressed = Some(path);
            cx.notify();
        }
    }
    pub(super) fn collapse_project_launch_options(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self.project_flyout_launch_path.take() else {
            return false;
        };
        self.project_flyout_launch_error = None;
        if let Some(focus) = self.project_flyout_plus_focus.get(&path).cloned() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
        true
    }

    pub(super) fn close_project_flyout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .project_flyout
            .as_ref()
            .and_then(|target| match target {
                SidebarContext::Project(path) => self.project_toggle_focus.get(path),
                SidebarContext::MultiProject => Some(&self.multi_project_focus),
            })
            .cloned();
        self.dismiss_project_context(cx);
        self.track_project_flyout_pointer(window.mouse_position(), window, cx);
        if let Some(focus) = return_focus {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
    }

    fn project_popup_action_focus(&self, action: &Action) -> Option<FocusHandle> {
        match action {
            Action::ProjectFlyoutLaunchOptions(path) => {
                self.project_flyout_plus_focus.get(path).cloned()
            }
            Action::Launch(_, _, Agent::Terminal) => Some(self.project_flyout_launch_focus.clone()),
            Action::Launch(_, path, agent) => self
                .project_flyout_item_focus
                .get(&format!("launch-{}-{path}", agent.label()))
                .cloned(),
            Action::Select(Selection::Session(id)) => self
                .project_flyout_item_focus
                .get(&format!("session-{}", id.raw()))
                .cloned(),
            Action::Select(Selection::Project(idx)) => self
                .project_flyout_item_focus
                .get(&format!("overview-{idx}"))
                .cloned(),
            Action::Menu(idx) => self
                .project_menu_focus
                .get(idx)
                .map(|focus| focus.clone().tab_stop(true)),
            _ => None,
        }
    }

    pub(super) fn project_session_flyout(
        &self,
        idx: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshot = self.project_navigation_snapshot(cx);
        let Some(project) = snapshot.projects.iter().find(|p| p.idx == idx) else {
            return div().into_any_element();
        };
        let Some(path) = self.project_paths.get(&idx) else {
            return div().into_any_element();
        };
        let ids = project
            .worktrees
            .iter()
            .flat_map(|wt| wt.sessions.iter())
            .copied()
            .collect::<Vec<_>>();
        let needs_you = ids
            .iter()
            .filter(|id| {
                self.runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(**id)
                    .is_some_and(|meta| self.status(meta, cx).0 == "Needs you")
            })
            .count();
        let mut actions = Vec::new();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let height = (f32::from(window.viewport_size().height) / scale - SPACE_LG * 2.0).max(0.0);
        let mut body = div()
            .id("project-session-flyout-scroll")
            .overflow_y_scroll()
            .track_scroll(&self.project_flyout_scroll)
            .max_h(rpx(height))
            .flex()
            .flex_col()
            .p(rpx(SPACE_2XL))
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .id("flyout-project-heading")
                            .debug_selector(|| "flyout-project-heading".into())
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(rpx(TEXT_TITLE))
                            .line_height(rpx(SESSION_TITLE_LINE_H))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(project.name.clone()),
                    ),
            )
            .when(needs_you > 0, |body| {
                body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::AMBER())
                        .child(format!(
                            "{needs_you} session{} need{} you",
                            if needs_you == 1 { "" } else { "s" },
                            if needs_you == 1 { "s" } else { "" }
                        )),
                )
            });
        let mut index = 0;
        let mut child_index = 1 + usize::from(needs_you > 0);
        let mut scroll_indices = Vec::new();
        for (worktree_index, worktree) in project.worktrees.iter().enumerate() {
            let name = sidebar_worktree_name(&worktree.name, worktree.is_main);
            let expanded = self.project_flyout_launch_path.as_ref() == Some(&worktree.path);
            let mut launches = div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .gap(rpx(SPACE_XS));
            if expanded {
                for agent in [
                    Agent::Terminal,
                    Agent::Claude,
                    Agent::Codex,
                    Agent::OpenCode,
                ] {
                    let agent_name = match agent {
                        Agent::Terminal => "Terminal",
                        Agent::Claude => "Claude Code",
                        Agent::Codex => "Codex",
                        Agent::OpenCode => "OpenCode",
                    };
                    let action = Action::Launch(idx, worktree.path.clone(), agent);
                    let selector = format!("flyout-launch-{}-{}", agent.label(), worktree.path);
                    launches = launches.child(
                        self.control(
                            SharedString::from(selector.clone()),
                            format!("Start {agent_name} in {} · {name}", project.name),
                            action.clone(),
                            cx,
                        )
                        .debug_selector(move || selector.clone())
                        .when(
                            !self.project_flyout_hover_open
                                && self.project_flyout_focus.is_focused(window)
                                && self.project_flyout_index == index,
                            |button| button.bg(c::BG_HOVER()),
                        )
                        .when_some(self.project_popup_action_focus(&action), |button, focus| {
                            button.track_focus(&focus)
                        })
                        .child(icon(
                            agent.icon_name(),
                            ICON_SM,
                            if agent == Agent::Terminal {
                                c::FG_DIM()
                            } else {
                                c::MAGENTA()
                            },
                        )),
                    );
                    actions.push(action);
                    scroll_indices.push(child_index);
                    index += 1;
                }
            } else {
                let action = Action::ProjectFlyoutLaunchOptions(worktree.path.clone());
                let selector = format!("flyout-launch-plus-{}", worktree.path);
                launches = launches.child(
                    self.control(
                        SharedString::from(selector.clone()),
                        format!("Show launch options in {} · {name}", project.name),
                        action.clone(),
                        cx,
                    )
                    .debug_selector(move || selector.clone())
                    .when_some(
                        self.project_flyout_plus_focus.get(&worktree.path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .when(
                        !self.project_flyout_hover_open
                            && self.project_flyout_focus.is_focused(window)
                            && self.project_flyout_index == index,
                        |button| button.bg(c::BG_HOVER()),
                    )
                    .child(icon("plus", ICON_SM, c::FG_DIM())),
                );
                actions.push(action);
                scroll_indices.push(child_index);
                index += 1;
            }
            body = body.child(
                div()
                    .id(SharedString::from(format!(
                        "flyout-worktree-{}",
                        worktree.path
                    )))
                    .debug_selector({
                        let path = worktree.path.clone();
                        move || format!("flyout-worktree-{path}")
                    })
                    .min_w_0()
                    .mt(rpx(SPACE_2XL))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS))
                    .when(worktree_index > 0, |group| {
                        group.child(
                            div()
                                .absolute()
                                .top(rpx(-SPACE_MD))
                                .left_0()
                                .w_full()
                                .h(gpui::px(1.0))
                                .bg(c::BORDER_SOFT()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(rpx(TEXT_SMALL))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .line_height(rpx(SESSION_TITLE_LINE_H))
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "flyout-worktree-title-{}",
                                        worktree.path
                                    )))
                                    .debug_selector({
                                        let path = worktree.path.clone();
                                        move || format!("flyout-worktree-title-{path}")
                                    })
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .child(name.to_string()),
                            )
                            .child(launches),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .font_weight(gpui::FontWeight::NORMAL)
                            .line_height(rpx(SESSION_META_LINE_H))
                            .child(format!("Branch: {}", worktree.branch)),
                    ),
            );
            child_index += 1;
            if expanded {
                if let Some(error) = &self.project_flyout_launch_error {
                    body = body.child(
                        div()
                            .id(SharedString::from(format!(
                                "flyout-launch-error-{}",
                                worktree.path
                            )))
                            .debug_selector({
                                let path = worktree.path.clone();
                                move || format!("flyout-launch-error-{path}")
                            })
                            .text_size(rpx(TEXT_SMALL))
                            .line_height(rpx(SESSION_META_LINE_H))
                            .text_color(c::RED())
                            .whitespace_normal()
                            .child(error.clone()),
                    );
                    child_index += 1;
                }
            }
            if worktree.sessions.is_empty() {
                child_index += 1;
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .id(SharedString::from(format!(
                            "flyout-empty-{}",
                            worktree.path
                        )))
                        .debug_selector({
                            let path = worktree.path.clone();
                            move || format!("flyout-empty-{path}")
                        })
                        .child("No open sessions"),
                );
            }
            for id in &worktree.sessions {
                actions.push(Action::Select(Selection::Session(*id)));
                scroll_indices.push(child_index);
                child_index += 1;
                if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(*id).cloned() {
                    let terminal = self
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .session(*id)
                        .map(|s| s.read(cx));
                    let title = session_display_title(
                        &meta,
                        terminal
                            .and_then(crate::entities::terminal_session::TerminalSession::title),
                    );
                    let (status, color) = if terminal.is_some_and(|t| t.spawn_error().is_some()) {
                        ("Failed", c::RED())
                    } else if terminal.is_some_and(
                        crate::entities::terminal_session::TerminalSession::is_pending_attach,
                    ) {
                        ("Starting", c::FG_DIM())
                    } else {
                        self.status(&meta, cx)
                    };
                    body = body.child(
                        self.control(
                            ("flyout-session", id.raw()),
                            format!("{title} · {} · {status}", meta.agent.label()),
                            Action::Select(Selection::Session(*id)),
                            cx,
                        )
                        .debug_selector({
                            let id = *id;
                            move || format!("flyout-session-{}", id.raw())
                        })
                        .when_some(
                            self.project_popup_action_focus(&Action::Select(Selection::Session(
                                *id,
                            ))),
                            |row, focus| row.track_focus(&focus),
                        )
                        .relative()
                        .aria_selected(self.selection == Some(Selection::Session(*id)))
                        .when(self.selection == Some(Selection::Session(*id)), |row| {
                            row.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .h_full()
                                    .w(rpx(SPACE_XS))
                                    .flex()
                                    .items_center()
                                    .child(
                                        div().w_full().h(rpx(SPACE_3XL)).rounded_full().bg(c::FG()),
                                    ),
                            )
                        })
                        .w_full()
                        .h_auto()
                        .min_h(rpx(SESSION_ROW_H))
                        .justify_start()
                        .px(rpx(SPACE_SM))
                        .py(rpx(SPACE_SM))
                        .gap(rpx(SPACE_MD))
                        .rounded(rpx(RADIUS_CHROME))
                        .when(
                            (!self.project_flyout_hover_open && self.project_flyout_index == index)
                                || self.selection == Some(Selection::Session(*id)),
                            |row| row.bg(c::BG_HOVER()),
                        )
                        .child(
                            div()
                                .w(rpx(HIERARCHY_ICON_SLOT))
                                .h(rpx(SESSION_TITLE_LINE_H))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(
                                    meta.agent.icon_name(),
                                    ICON_SM,
                                    if meta.agent == Agent::Terminal {
                                        c::FG_DIM()
                                    } else {
                                        c::MAGENTA()
                                    },
                                )),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(rpx(SPACE_XS))
                                .text_size(rpx(TEXT_SMALL))
                                .child(
                                    div()
                                        .min_w_0()
                                        .id(("flyout-session-title", id.raw()))
                                        .truncate()
                                        .debug_selector({
                                            let id = *id;
                                            move || format!("flyout-session-title-{}", id.raw())
                                        })
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .line_height(rpx(SESSION_TITLE_LINE_H))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_MICRO))
                                        .text_color(color)
                                        .font_weight(gpui::FontWeight::NORMAL)
                                        .line_height(rpx(SESSION_META_LINE_H))
                                        .child(format!("{} · {status}", meta.agent.label())),
                                ),
                        ),
                    );
                }
                index += 1;
            }
        }
        body = body.child(
            div()
                .mt(rpx(SPACE_SM))
                .h(gpui::px(1.0))
                .bg(c::BORDER_SOFT()),
        );
        child_index += 1;
        body = body
            .child(
                self.control(
                    "flyout-project-overview",
                    "Open project",
                    Action::Select(Selection::Project(idx)),
                    cx,
                )
                .when_some(
                    self.project_popup_action_focus(&Action::Select(Selection::Project(idx))),
                    |row, focus| row.track_focus(&focus),
                )
                .w_full()
                .h(rpx(ROW_H))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(rpx(TEXT_SMALL))
                .line_height(rpx(SESSION_META_LINE_H))
                .justify_start()
                .gap(rpx(SPACE_MD))
                .px(rpx(SPACE_SM))
                .when(
                    !self.project_flyout_hover_open && self.project_flyout_index == index,
                    |row| row.bg(c::BG_HOVER()),
                )
                .child(
                    div()
                        .w(rpx(HIERARCHY_ICON_SLOT))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(icon("folder", ICON_SM, c::FG_DIM())),
                )
                .child("Open project"),
            )
            .child(
                self.control(
                    ("flyout-project-actions", idx),
                    "Project actions",
                    Action::Menu(idx),
                    cx,
                )
                .w_full()
                .h(rpx(ROW_H))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(rpx(TEXT_SMALL))
                .line_height(rpx(SESSION_META_LINE_H))
                .justify_start()
                .gap(rpx(SPACE_MD))
                .px(rpx(SPACE_SM))
                .when(
                    !self.project_flyout_hover_open && self.project_flyout_index == index + 1,
                    |row| row.bg(c::BG_HOVER()),
                )
                .when_some(self.project_menu_focus.get(&idx), |row, focus| {
                    row.track_focus(&focus.clone().tab_stop(true))
                })
                .debug_selector(|| "flyout-project-actions".into())
                .child(
                    div()
                        .w(rpx(HIERARCHY_ICON_SLOT))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(icon("more", ICON_SM, c::FG_DIM())),
                )
                .child(
                    div()
                        .id("flyout-actions-label")
                        .debug_selector(|| "flyout-actions-label".into())
                        .child("Project actions…"),
                ),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child("Esc to close"),
            );
        actions.extend([Action::Select(Selection::Project(idx)), Action::Menu(idx)]);
        scroll_indices.extend([child_index, child_index + 1]);
        self.sidebar_context_popup(
            body,
            actions,
            scroll_indices,
            format!("{} sessions · {path}", project.name),
            window,
            cx,
        )
    }

    pub(super) fn sidebar_context_popup(
        &self,
        body: Stateful<Div>,
        actions: Vec<Action>,
        scroll_indices: Vec<usize>,
        label: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let multi = self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject);
        let action_count = actions.len();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = FLYOUT_W.min(
            (f32::from(window.viewport_size().width) / scale - SIDEBAR_COLLAPSED_W - SPACE_LG)
                .max(0.0),
        );
        let panel = div()
            .id("project-session-flyout")
            .relative()
            .font_weight(gpui::FontWeight::NORMAL)
            .text_size(rpx(TEXT_BODY))
            .line_height(rpx(SESSION_TITLE_LINE_H))
            .child(
                gpui::canvas(
                    {
                        let bounds = self.project_flyout_popup_bounds.clone();
                        move |rect, _, _| bounds.set(rect)
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.project_flyout_hide_task = None;
                } else {
                    this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                }
            }))
            .debug_selector(move || {
                if multi {
                    "multi-project-session-flyout"
                } else {
                    "project-session-flyout"
                }
                .into()
            })
            .track_focus(&self.project_flyout_focus)
            .tab_group()
            .role(gpui::Role::Menu)
            .aria_label(label)
            .w(rpx(width))
            .rounded(rpx(RADIUS_CHROME))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .text_color(c::FG())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.project_flyout_bounds.get().contains(&event.position)
                        && !this.confirmation_open()
                    {
                        let focused = this.project_flyout_focus.contains_focused(window, cx);
                        this.dismiss_project_context(cx);
                        this.track_project_flyout_pointer(event.position, window, cx);
                        if focused {
                            this.focus.focus(window, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    // Modal controls use the sidebar's existing confirmation trap.
                    if this.confirmation_open() {
                        return;
                    }
                    let key = &event.keystroke;
                    if key.key == "escape" {
                        if !this.collapse_project_launch_options(window, cx) {
                            this.close_project_flyout(window, cx);
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        return;
                    }
                    if key.key == "tab"
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && !key.modifiers.platform
                        && !key.modifiers.function
                    {
                        window.prevent_default();
                        let initial = window.focused(cx);
                        loop {
                            let before = window.focused(cx);
                            if key.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            if this.project_flyout_focus.contains_focused(window, cx) {
                                break;
                            }
                            let current = window.focused(cx);
                            if current == before || current == initial {
                                this.project_flyout_focus.focus(window, cx);
                                break;
                            }
                        }
                        if !multi {
                            if let Some(index) = actions.iter().position(|action| {
                                this.project_popup_action_focus(action)
                                    .is_some_and(|focus| focus.is_focused(window))
                            }) {
                                this.project_flyout_index = index;
                                this.project_flyout_scroll
                                    .scroll_to_item(scroll_indices[index]);
                                cx.notify();
                            }
                        }
                        cx.stop_propagation();
                        return;
                    }
                    // Close/Retry descendants own Enter/Space themselves.
                    if !this.project_flyout_focus.is_focused(window) {
                        return;
                    }
                    window.prevent_default();
                    match event.keystroke.key.as_str() {
                        "escape" => this.close_project_flyout(window, cx),
                        "up" | "down" | "tab" => {
                            let backward = event.keystroke.key == "up"
                                || (event.keystroke.key == "tab"
                                    && event.keystroke.modifiers.shift);
                            this.project_flyout_index = if backward {
                                (this.project_flyout_index + action_count - 1) % action_count
                            } else {
                                (this.project_flyout_index + 1) % action_count
                            };
                            this.project_flyout_scroll
                                .scroll_to_item(scroll_indices[this.project_flyout_index]);
                            this.project_flyout_focus.focus(window, cx);
                            cx.notify();
                        }
                        "enter" | "space" => {
                            if let Some(action) = actions.get(this.project_flyout_index).cloned() {
                                this.act(action, window, cx);
                            }
                        }
                        _ => {}
                    }
                    cx.stop_propagation();
                }),
            )
            .child(body);
        ProjectFlyoutAnchor {
            inner: gpui::anchored()
                .position_mode(gpui::AnchoredPositionMode::Window)
                .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
                .child(panel),
            rail: self.rail_bounds.clone(),
            trigger: self.project_flyout_bounds.clone(),
        }
        .into_any_element()
    }
}

// Deferred prepaint runs after the rail and trigger canvases. Read their current
// geometry here so zoom, resize and scrolling never use the previous frame.
struct ProjectFlyoutAnchor {
    inner: gpui::Anchored,
    rail: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    trigger: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
}
impl gpui::Element for ProjectFlyoutAnchor {
    type RequestLayoutState = <gpui::Anchored as gpui::Element>::RequestLayoutState;
    type PrepaintState = <gpui::Anchored as gpui::Element>::PrepaintState;
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner = std::mem::replace(&mut self.inner, gpui::anchored()).position(gpui::point(
            self.rail.get().right(),
            self.trigger.get().top(),
        ));
        self.inner
            .prepaint(id, inspector, bounds, state, window, cx);
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .paint(id, inspector, bounds, layout, prepaint, window, cx);
    }
}
impl gpui::IntoElement for ProjectFlyoutAnchor {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_are_recognizable_and_collision_safe() {
        let labels =
            project_identifiers(&["grove", "careconvoy", "green", "gr2", "", "", "éclair"]);
        assert_eq!(&labels[..3], &["GR", "CA", "GR2"]);
        assert_eq!(labels.iter().collect::<HashSet<_>>().len(), labels.len());
        assert!(labels.last().unwrap().starts_with('É'));
    }
}
