//! Session and home terminal rows, status, and list presentation.

use super::{
    c, icon, managed_session_status, motion, multi_project, rpx, session_age_label,
    session_diff_status, session_display_title, session_root_inventory, session_scope,
    session_status_icon, session_status_icon_sized, single_line_session_title,
    structured_session_status, task_glance, Action, ActivityState, Duration, Instant, Selection,
    SessionId, SessionMeta, Sidebar, HEAD_H, HIERARCHY_INSET, ROW_H, SESSION_LAUNCHER_SIZE,
    SESSION_LEADING_INSET, SESSION_LIST_CARD_H, SESSION_LIST_FOOTER_H, SESSION_LIST_HEADER_H,
    SESSION_LIST_TITLE_H, SESSION_META_LINE_H, SESSION_TITLE_LINE_H, SESSION_TREE_ROW_H,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, CONTROL_H, ICON_LG, ICON_MD, ICON_SM, ICON_XS, MOTION_FAST_MS, RADIUS_GROUP,
    SIDEBAR_CONTENT_INSET, SIDEBAR_ICON_SLOT, SPACE_2XL, SPACE_3XL, SPACE_LG, SPACE_MD, SPACE_SM,
    SPACE_XS, TEXT_MICRO, TEXT_SMALL, TEXT_TITLE,
};
use gpui::prelude::{FluentBuilder, InteractiveElement, IntoElement, ParentElement, Styled};
use gpui::{div, AnyElement, App, Context, Window};

const SESSION_ROW_H: f32 = 43.0;
const SESSION_ROW_RADIUS: f32 = 7.0;

impl Sidebar {
    pub(super) fn session_title_label(
        &self,
        id: SessionId,
        title: &str,
        surface: &'static str,
        cx: &App,
    ) -> AnyElement {
        let pending = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .is_some_and(|session| session.read(cx).state.initial_title_pending());
        crate::views::components::shimmer_label(
            (surface, id.raw()).into(),
            single_line_session_title(title),
            pending,
            cx,
        )
    }

    pub(super) fn status(&self, meta: &SessionMeta, cx: &App) -> (&'static str, gpui::Hsla) {
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        if let Some(session) = registry.codex(meta.id) {
            let state = &session.read(cx).state;
            let label =
                structured_session_status(state, runtime.activity.read(cx).state_of(meta.id));
            return (label, crate::views::status::session_kind(label).color());
        }
        let failed = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).spawn_error().is_some());
        let pending_attach = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).is_pending_attach());
        let activity = runtime.activity.read(cx);
        let status = managed_session_status(failed, pending_attach, activity.state_of(meta.id));
        if !failed
            && !pending_attach
            && activity.terminal_review(meta.id).is_some()
            && matches!(status.0, "Ready" | "Done")
        {
            let label = if activity.state_of(meta.id) == ActivityState::Done {
                "Review · unread"
            } else {
                "Review"
            };
            return (label, crate::views::status::session_kind(label).color());
        }
        status
    }

    pub(super) fn session_row(
        &self,
        meta: &SessionMeta,
        list: bool,
        number: Option<usize>,
        diff_focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (status, _) = self.status(meta, cx);
        let id = meta.id;
        let tasks = self.task_glance(id).cloned();
        let tasks_suffix = tasks.as_ref().map_or_else(String::new, |tasks| {
            format!(" · {}", tasks.tooltip_suffix())
        });
        let registry = self.runtime.read(cx).registry.read(cx);
        let title = session_display_title(
            meta,
            registry
                .codex(id)
                .and_then(|session| session.read(cx).state.saved.title.clone())
                .or_else(|| {
                    registry
                        .session(id)
                        .and_then(|session| session.read(cx).title())
                }),
        );
        let label = if list {
            format!("{} · {}", title, meta.project)
        } else if multi_project::project_names(meta).is_some() {
            format!(
                "{title} · {}",
                multi_project::root_details(meta, &self.snapshot)
            )
        } else {
            title.clone()
        };
        let row = if list {
            let selected = self.selection == Some(Selection::Session(id)) && !self.dashboard_open;
            let worktree = self
                .snapshot
                .projects
                .iter()
                .flat_map(|project| project.worktrees.iter())
                .find(|worktree| worktree.path == meta.wt_path);
            let name = worktree.map_or_else(
                || {
                    std::path::Path::new(&meta.wt_path).file_name().map_or_else(
                        || meta.wt_path.clone(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                },
                |worktree| worktree.name.clone(),
            );
            let branch = worktree
                .map(|worktree| worktree.branch.trim())
                .filter(|branch| {
                    !branch.is_empty()
                        && !matches!(*branch, "—" | "-")
                        && *branch != name
                        && *branch != meta.project
                });
            let context = if name == meta.project {
                name.clone()
            } else {
                format!("{} · {}", meta.project, name)
            };
            let since = self
                .runtime
                .read(cx)
                .activity
                .read(cx)
                .since_of(id)
                .unwrap_or(meta.spawned_at);
            let age = session_age_label(since, Instant::now());
            let scope = session_scope::session_scope(meta, &self.snapshot);
            let diff = self
                .runtime
                .read(cx)
                .tree
                .read(cx)
                .git_state(crate::paths::normalize_wt_path(&meta.wt_path))
                .map_or_else(
                    || ("Git status unavailable".into(), false),
                    session_diff_status,
                );
            let full_context = scope.as_ref().map_or_else(
                || branch.map_or_else(|| context.clone(), |branch| format!("{context} · {branch}")),
                |scope| {
                    session_scope::scope_accessible_details(
                        scope,
                        &session_root_inventory(meta, &self.snapshot),
                    )
                },
            );
            let (header_project, header_worktree) = scope.as_ref().map_or_else(
                || (meta.project.clone(), name.clone()),
                |scope| (scope.projects.clone(), scope.shared.clone()),
            );
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{title} · {full_context} · {status}{tasks_suffix} · {} · {age} · {} session{}",
                    diff.0,
                    meta.agent.label(),
                    number.filter(|number| *number <= 9).map_or_else(
                        String::new,
                        |number| format!(
                            " · shortcut {}{number}",
                            crate::keymap::alt_chord_prefix().replace('-', "+")
                        )
                    )
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .relative()
            .group("session-row")
            .h_auto()
            .min_h(rpx(SESSION_LIST_CARD_H))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_LG))
            .rounded(rpx(RADIUS_GROUP))
            .bg(if selected {
                c::alpha(c::FG(), 0.14)
            } else {
                c::alpha(c::BG(), 0.0)
            })
            .items_start()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w_0()
                            .h(rpx(SESSION_LIST_HEADER_H))
                            .gap(rpx(SPACE_SM))
                            .child(crate::views::components::project_symbol_for_path(
                                &meta.wt_path,
                                ICON_MD,
                                cx,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(if selected { c::FG() } else { c::FG_DIM() })
                                    .group_hover("session-row", |label| label.text_color(c::FG()))
                                    .id(("session-list-context", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-context-{}", id.raw())
                                    })
                                    .child(header_project),
                            )
                            .child(div().flex_shrink_0().ml(rpx(SPACE_SM)).child(icon(
                                "git-branch",
                                ICON_SM,
                                c::FG_DIM(),
                            )))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .text_color(c::FG_DIM())
                                    .id(("session-list-worktree", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-worktree-{}", id.raw())
                                    })
                                    .child(header_worktree),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(rpx(SIDEBAR_ICON_SLOT))
                                    .h(rpx(SESSION_LIST_HEADER_H))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .id(("session-list-agent", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-agent-{}", id.raw())
                                    })
                                    .child(icon(meta.agent.icon_name(), TEXT_MICRO, c::FG_DIM())),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .w_full()
                            .truncate()
                            .mt(rpx(SPACE_SM))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_size(rpx(TEXT_TITLE))
                            .line_height(rpx(SESSION_LIST_TITLE_H))
                            .id(("session-list-title", id.raw()))
                            .debug_selector(move || format!("session-list-title-{}", id.raw()))
                            .child(self.session_title_label(
                                id,
                                &title,
                                "session-list-title-shimmer",
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w_0()
                            .mt(rpx(SPACE_XS))
                            .h(rpx(CHROME_CONTROL_H))
                            .gap(rpx(SPACE_SM))
                            .child(motion::fast(
                                div()
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .id(("session-list-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-status-{}", id.raw())
                                    })
                                    .text_color(crate::views::status::session_kind(status).color())
                                    .child(
                                        div().flex_shrink_0().child(session_status_icon_sized(
                                            id, status, ICON_MD, cx,
                                        )),
                                    )
                                    .child(div().min_w_0().truncate().child(status)),
                                format!("session-status-list-{}-{status}", id.raw()),
                                cx,
                            ))
                            .when_some(tasks.as_ref(), |line, tasks| {
                                line.child(task_glance::row_progress(id, tasks))
                            })
                            .child(div().flex_1().min_w_0())
                            .child(
                                self.control(
                                    ("close-session", id.raw()),
                                    format!("Done: close {} in {}", meta.label, meta.project),
                                    Action::FinishSession(id),
                                    cx,
                                )
                                .debug_selector(move || format!("close-session-{}", id.raw()))
                                .w_auto()
                                .px(rpx(SPACE_MD))
                                .gap(rpx(SPACE_SM))
                                .text_size(rpx(TEXT_MICRO))
                                .when_some(
                                    self.session_close_focus.get(&id),
                                    gpui::InteractiveElement::track_focus,
                                )
                                .opacity(0.0)
                                .group_hover("session-row", |button| button.opacity(1.0))
                                .focus_visible(|button| button.opacity(1.0))
                                .when(
                                    self.session_close_focus
                                        .get(&id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                                    |button| button.opacity(1.0),
                                )
                                .when_some(
                                    self.session_close_bounds.get(&id).cloned(),
                                    |button, bounds| {
                                        button.child(
                                            gpui::canvas(
                                                move |rect, _, _| bounds.set(rect),
                                                |_, (), _, _| {},
                                            )
                                            .absolute()
                                            .inset_0(),
                                        )
                                    },
                                )
                                .relative()
                                .child(icon("check", ICON_XS, c::FG_DIM()))
                                .child("Done"),
                            ),
                    )
                    .children(scope.iter().flat_map(|scope| {
                        scope
                            .exceptions
                            .iter()
                            .map(session_scope::scope_exception_line)
                    })),
            )
        } else {
            let selected = self.selection == Some(Selection::Session(id));
            let worktree_names = multi_project::project_names(meta)
                .map(|_| multi_project::worktree_names(meta, &self.snapshot));
            let row_height = if worktree_names.is_some() {
                SESSION_ROW_H
            } else {
                SESSION_TREE_ROW_H
            };
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{label} · {} session · {status}{tasks_suffix}{}",
                    meta.agent.label(),
                    number
                        .filter(|n| *n <= 9)
                        .map_or_else(String::new, |n| format!(
                            " · shortcut {}{n}",
                            crate::keymap::alt_chord_prefix().replace('-', "+")
                        ))
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .when(diff_focused && !selected, |row| row.bg(c::BG_HOVER()))
            .relative()
            .group("session-row")
            .h(rpx(row_height))
            .pl(rpx(SESSION_LEADING_INSET))
            .pr(rpx(HIERARCHY_INSET))
            .gap(rpx(SPACE_SM))
            .items_center()
            .rounded(rpx(SESSION_ROW_RADIUS))
            .child(
                div()
                    .id(("session-agent", id.raw()))
                    .debug_selector(move || format!("session-agent-{}", id.raw()))
                    .size(rpx(SESSION_LAUNCHER_SIZE))
                    .flex_shrink_0()
                    .child(icon(
                        meta.agent.icon_name(),
                        SESSION_LAUNCHER_SIZE,
                        c::BLUE(),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("session-tree-title", id.raw()))
                            .debug_selector(move || format!("session-tree-title-{}", id.raw()))
                            .truncate()
                            .line_height(rpx(SESSION_TITLE_LINE_H))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_size(rpx(TEXT_SMALL))
                            .child(self.session_title_label(
                                id,
                                &title,
                                "session-tree-title-shimmer",
                                cx,
                            )),
                    )
                    .when_some(worktree_names, |column, names| {
                        column.child(
                            div()
                                .id(("session-tree-worktrees", id.raw()))
                                .debug_selector(move || {
                                    format!("session-tree-worktrees-{}", id.raw())
                                })
                                .truncate()
                                .line_height(rpx(SESSION_META_LINE_H))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(names),
                        )
                    }),
            )
            .child(
                div()
                    .id(("session-tree-trailing", id.raw()))
                    .group("session-trailing")
                    .debug_selector(move || format!("session-tree-trailing-{}", id.raw()))
                    .relative()
                    .min_w(rpx(CONTROL_H))
                    .h(rpx(CONTROL_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(rpx(SPACE_SM))
                    .when_some(tasks.as_ref(), |slot, tasks| {
                        slot.child(task_glance::row_progress(id, tasks))
                    })
                    .child(
                        div()
                            .size(rpx(CONTROL_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover("session-row", Styled::invisible)
                            .when(
                                self.session_close_focus
                                    .get(&id)
                                    .is_some_and(|focus| focus.is_focused(window)),
                                Styled::invisible,
                            )
                            .child(motion::fast(
                                div()
                                    .id(("session-tree-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-tree-status-{}", id.raw())
                                    })
                                    .size(rpx(ICON_MD))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(session_status_icon(id, status, cx)),
                                format!("session-status-tree-{}-{status}", id.raw()),
                                cx,
                            )),
                    )
                    .child(
                        self.control_with_tooltip(
                            ("close-session", id.raw()),
                            format!("Close {} in {}", meta.label, meta.project),
                            Action::Close(id),
                            cx,
                        )
                        .absolute()
                        .right_0()
                        .top_0()
                        .size(rpx(CONTROL_H))
                        .debug_selector(move || format!("session-tree-close-{}", id.raw()))
                        .when_some(
                            self.session_close_focus.get(&id),
                            gpui::InteractiveElement::track_focus,
                        )
                        .opacity(0.0)
                        .group_hover("session-row", |button| button.opacity(1.0))
                        .focus_visible(|button| button.opacity(1.0))
                        .when(
                            self.session_close_focus
                                .get(&id)
                                .is_some_and(|focus| focus.is_focused(window)),
                            |button| button.opacity(1.0),
                        )
                        .when_some(
                            self.session_close_bounds.get(&id).cloned(),
                            |button, bounds| {
                                button.child(
                                    gpui::canvas(
                                        move |rect, _, _| bounds.set(rect),
                                        |_, (), _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            },
                        )
                        .child(icon("close", ICON_XS, c::FG_DIM())),
                    ),
            )
        };
        let row = if list {
            let selected = self.selection == Some(Selection::Session(id));
            let attention = matches!(status, "Needs you" | "Failed");
            let idle_fill = if attention {
                c::AMBER_ROW_TINT()
            } else {
                c::alpha(c::BG(), 0.0)
            };
            let selected_fill = c::alpha(c::FG(), 0.14);
            motion::background(
                row,
                format!("session-row-fill-{}-{selected}", id.raw()),
                if selected { idle_fill } else { selected_fill },
                if selected { selected_fill } else { idle_fill },
                Duration::from_millis(MOTION_FAST_MS),
                cx,
            )
        } else {
            row.into_any_element()
        };
        let mut result = div().when(list, |d| d.px(rpx(SPACE_LG))).child(row);
        if status == "Failed" {
            result = result.child(
                self.control(
                    ("retry-session", id.raw()),
                    "Retry session",
                    Action::Retry(id),
                    cx,
                )
                .w_auto()
                .ml(rpx(SPACE_3XL))
                .child(icon("restart", ICON_SM, c::FG_DIM()))
                .child("Retry"),
            );
        }
        result.into_any_element()
    }

    pub(super) fn list(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.list_session_groups(cx);
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        let mut number = 0;
        for (group, items) in &groups {
            body = body.child(
                div()
                    .px(rpx(SPACE_LG))
                    .pt(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_MICRO))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .debug_selector(|| "session-list-section-heading".into())
                            .child(format!("{group} · {}", items.len())),
                    ),
            );
            for id in items {
                if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(*id).cloned() {
                    number += 1;
                    body =
                        body.child(self.session_row(&meta, true, Some(number), false, window, cx));
                }
            }
        }
        if number == 0 {
            body = body.child(
                div()
                    .p(rpx(SPACE_2XL))
                    .text_color(c::FG_DIM())
                    .child("No sessions yet. Start one with +."),
            );
        }
        body.into_any_element()
    }

    pub(super) fn terminals(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div().flex().flex_col().flex_shrink_0().child(
            div()
                .h(rpx(HEAD_H))
                .px(rpx(SIDEBAR_CONTENT_INSET))
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .text_color(c::FG_DIM())
                .child(crate::icons::action_icon("terminal", ICON_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(rpx(TEXT_TITLE))
                        .text_color(c::FG())
                        .font_weight(gpui::FontWeight::NORMAL)
                        .debug_selector(|| "sidebar-terminals-heading".into())
                        .child("Terminals"),
                )
                .child(
                    self.control_with_tooltip(
                        "add-terminal",
                        "Add terminal",
                        Action::AddTerminal,
                        cx,
                    )
                    .child(icon("plus", ICON_SM, c::FG_DIM())),
                ),
        );
        for meta in self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .home_terminals()
            .to_vec()
        {
            if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) != self.active_workspace {
                continue;
            }
            let id = meta.id;
            let terminal = {
                let registry = self.runtime.read(cx).registry.read(cx);
                let Some(index) = registry
                    .home_terminals()
                    .iter()
                    .position(|row| row.id == id)
                else {
                    continue;
                };
                let Some(terminal) = registry.home_terminal(index) else {
                    continue;
                };
                terminal.clone()
            };
            let title = terminal.update(cx, |terminal, _| {
                terminal
                    .title()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| meta.label.clone())
            });
            panel = panel.child(
                self.row(
                    format!("home-{}", id.raw()),
                    title.clone(),
                    self.selection == Some(Selection::Home(id)),
                    Action::Select(Selection::Home(id)),
                    cx,
                )
                .w_auto()
                .mx(rpx(SPACE_LG))
                .px(rpx(SPACE_LG))
                .h_auto()
                .min_h(rpx(ROW_H))
                .py(rpx(SPACE_SM))
                .my(rpx(SPACE_XS))
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .w(rpx(SIDEBAR_ICON_SLOT))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .id(("home-icon", id.raw()))
                        .debug_selector(move || format!("home-icon-{}", id.raw()))
                        .child(icon("terminal", ICON_SM, c::FG_DIM())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .id(("home-title", id.raw()))
                        .debug_selector(move || format!("home-title-{}", id.raw()))
                        .truncate()
                        .child(single_line_session_title(&title)),
                )
                .child(
                    self.control_with_tooltip(
                        ("close-home", id.raw()),
                        format!("Close {}", meta.label),
                        Action::CloseHome(id),
                        cx,
                    )
                    .debug_selector(move || format!("close-home-{}", id.raw()))
                    .when_some(
                        self.home_close_bounds.get(&id).cloned(),
                        |button, bounds| {
                            button.child(
                                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                        },
                    )
                    .child(icon("close", ICON_XS, c::FG_DIM())),
                ),
            );
        }
        panel.into_any_element()
    }
}
