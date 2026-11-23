//! Checkout-specific launch and first-run panel.

use super::{
    c, dense_action_with_colors, form_action, icon, rpx, Action, Agent, Disableable, SettingsState,
    Sidebar, WorktreeReadiness, PROJECT_START_ICON_SIZE, PROJECT_START_ROW_H,
    WORKTREE_LAUNCH_AGENTS,
};
use crate::views::tokens::{
    CONTROL_H, FORM_VALUE_SIZE, ICON_SM, MODAL_W_SM, RADIUS_CONTROL, RADIUS_GROUP, SPACE_20,
    SPACE_2XL, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS, TEXT_BODY, TEXT_DISPLAY, TEXT_SMALL,
    TEXT_TITLE,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{div, AnyElement, Context, FontWeight, Window};

impl Sidebar {
    pub(super) fn render_start_panel(
        &self,
        project_idx: usize,
        project_name: &str,
        path: &str,
        main: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project_path = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(project_idx)
            .map_or_else(|| path.to_string(), |project| project.path.clone());
        let readiness = self
            .worktree_readiness
            .get(&project_path)
            .and_then(|(_, value)| *value);
        let guidance = if main {
            match readiness {
                Some(WorktreeReadiness::Ready) => {
                    "Create a separate worktree for an isolated branch."
                }
                Some(WorktreeReadiness::NeedsGit) => {
                    "Initialize Git to create worktrees. Sessions in this folder still work."
                }
                Some(WorktreeReadiness::NeedsCommit) => {
                    "Make the first commit before creating a worktree. Sessions in the main checkout still work."
                }
                None => "Checking Git before offering another worktree…",
            }
        } else {
            "Start a session in this worktree."
        };
        let branch = self
            .snapshot
            .projects
            .iter()
            .find(|project| project.idx == project_idx)
            .and_then(|project| {
                project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.path == path)
            })
            .map(|worktree| worktree.branch.as_str())
            .filter(|branch| !branch.is_empty());
        let mut actions = div()
            .id("project-start-actions")
            .debug_selector(|| "project-start-actions".into())
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .mt(rpx(SPACE_20))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .flex()
            .flex_col();
        for (id, label, agent) in [
            ("start-terminal", "Terminal", Agent::Terminal),
            ("start-codex", "Codex", Agent::Codex),
            ("start-claude", "Claude Code", Agent::Claude),
            ("start-opencode", "OpenCode", Agent::OpenCode),
        ] {
            let target = path.to_string();
            actions = actions.child(
                div()
                    .w_full()
                    .min_w_0()
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .child(
                        dense_action_with_colors(
                            id,
                            label,
                            gpui::transparent_black(),
                            c::FG(),
                            c::BG_HOVER(),
                            window,
                            cx,
                        )
                        // Retain the button's accessible name while the custom
                        // content reserves the provider and destination slots.
                        .icon_only()
                        .h(rpx(PROJECT_START_ROW_H))
                        .rounded(gpui::px(0.))
                        .debug_selector(move || id.into())
                        .w_full()
                        .min_w_0()
                        .cursor_pointer()
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(div().flex_none().child(icon(
                                    agent.icon_name(),
                                    PROJECT_START_ICON_SIZE,
                                    c::FG(),
                                )))
                                .child(div().flex_1().min_w_0().truncate().child(label))
                                // A quiet destination indicator, not a second action.
                                .child(div().flex_none().child(icon(
                                    "arrow-right",
                                    ICON_SM,
                                    c::FG_DIM(),
                                ))),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.launch(project_idx, target.clone(), agent, cx);
                        })),
                    ),
            );
        }
        let missing = WORKTREE_LAUNCH_AGENTS
            .iter()
            .zip(self.available)
            .filter_map(|(agent, available)| {
                (!available && *agent != Agent::Terminal).then_some(match agent {
                    Agent::Codex => "Codex",
                    Agent::Claude => "Claude Code",
                    Agent::OpenCode => "OpenCode",
                    Agent::Terminal => "Terminal",
                })
            })
            .collect::<Vec<_>>();
        let mut card = div()
            .id("project-start-card")
            .debug_selector(|| "project-start-card".into())
            .w_full()
            .max_w(rpx(MODAL_W_SM))
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .mb(rpx(CONTROL_H + SPACE_SM))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(rpx(SPACE_2XL))
                            .mb(rpx(SPACE_LG))
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(gpui::relative(1.))
                                    .truncate()
                                    .text_size(rpx(FORM_VALUE_SIZE))
                                    .font_weight(FontWeight::BOLD)
                                    .child(project_name.to_string()),
                            )
                            .when_some(branch, |metadata, branch| {
                                metadata.child(
                                    div()
                                        .min_w_0()
                                        .max_w(gpui::relative(1.))
                                        .flex()
                                        .items_center()
                                        .gap(rpx(SPACE_SM))
                                        .px(rpx(SPACE_LG))
                                        .py(rpx(SPACE_XS))
                                        .border_1()
                                        .border_color(c::BORDER_SOFT())
                                        .rounded(rpx(RADIUS_GROUP))
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(icon("git-branch", ICON_SM, c::FG_DIM()))
                                        .child(
                                            div().min_w_0().truncate().child(branch.to_string()),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("project-start-path")
                            .w_full()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(path.replace('/', "/\u{200b}")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(rpx(TEXT_DISPLAY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if main {
                        "Start in this checkout"
                    } else {
                        "Start in this worktree"
                    }),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .mt(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_TITLE))
                    .text_color(c::FG_DIM())
                    .child(if main {
                        "Work in the existing checkout."
                    } else {
                        "Start a session in this worktree."
                    }),
            )
            .child(actions);
        if !missing.is_empty() {
            let noun = if missing.len() == 1 { "CLI" } else { "CLIs" };
            card = card.child(div().w_full().min_w_0().whitespace_normal().mt(rpx(TEXT_TITLE)).id("missing-agent-guidance").role(gpui::Role::Status).text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(format!("{} {noun} unavailable. Install the CLI, check your PATH, then restart Grove. Terminal is ready now.", missing.join(", "))));
        }
        if main {
            card = card.child(
                div()
                    .w_full()
                    .min_w_0()
                    .mt(rpx(CONTROL_H))
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .pt(rpx(SPACE_20))
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(div().w_full().min_w_0().whitespace_normal().text_size(rpx(TEXT_TITLE)).font_weight(FontWeight::SEMIBOLD).child("Work in isolation"))
                    .child(div().w_full().min_w_0().whitespace_normal().mt(rpx(SPACE_MD)).text_size(rpx(TEXT_BODY)).text_color(c::FG_DIM()).child(guidance))
                    .when(readiness == Some(WorktreeReadiness::Ready), |row| {
                        row.child(
                            form_action("project-new-worktree", "Create worktree…", false, window, cx)
                                .debug_selector(|| "project-new-worktree".into())
                                .icon(gpui_component::Icon::default().path("icons/plus.svg"))
                                .mt(rpx(SPACE_2XL))
                                .bg(c::BG())
                                .border_1()
                                .border_color(c::BORDER_STRONG())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.act(Action::NewWorktree(project_path.clone()), window, cx);
                                })),
                        )
                    })
                    .when(readiness == Some(WorktreeReadiness::NeedsGit), |row| {
                        let project_path = path.to_string();
                        row.child(
                            form_action(
                                "project-initialize-git",
                                "Initialize Git",
                                false,
                                window,
                                cx,
                            )
                            .debug_selector(|| "project-initialize-git".into())
                            .icon(gpui_component::Icon::default().path("icons/plus.svg"))
                            .disabled(self.git_init_pending.is_some())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.act(
                                        Action::InitializeGit(project_path.clone()),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        )
                    })
                    .when(readiness == Some(WorktreeReadiness::NeedsCommit), |row| {
                        let project_path = path.to_string();
                        row.child(
                            div().w_full().min_w_0().whitespace_normal().text_color(c::FG_DIM()).child(
                                "In the main checkout, stage changes and review the staged diff before committing:",
                            ),
                        )
                        .child(
                            div()
                                .id("first-commit-commands")
                                .debug_selector(|| "first-commit-commands".into())
                                .p(rpx(SPACE_LG))
                                .rounded(rpx(RADIUS_CONTROL))
                                .bg(c::FIELD_FILL())
                                .text_size(rpx(TEXT_SMALL))
                                .font_family(crate::fonts::UI_FAMILY)
                                .flex()
                                .flex_col()
                                .gap(rpx(SPACE_SM))
                                .child("git add .")
                                .child("git diff --cached")
                                .child("git commit -m \"Initial commit\""),
                        )
                        .child(
                            form_action("check-first-commit", "Check again", false, window, cx)
                                .debug_selector(|| "check-first-commit".into())
                                .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.act(Action::RefreshGit(project_path.clone()), window, cx);
                                })),
                        )
                    }),
            );
        }
        if self.git_init_pending.as_deref() == Some(path) {
            card = card.child(
                div()
                    .id("git-init-status")
                    .role(gpui::Role::Status)
                    .text_color(c::FG_DIM())
                    .child(crate::views::status::label(
                        "git-initializing",
                        crate::views::status::StatusKind::Working,
                        "Initializing Git…",
                        ICON_SM,
                        cx,
                    )),
            );
        }
        card.into_any_element()
    }
}
