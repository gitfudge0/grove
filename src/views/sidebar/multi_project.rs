//! A render-only navigation partition; the runtime snapshot retains session ownership.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SidebarContext {
    Project(String),
    MultiProject,
}

impl SidebarContext {
    pub(super) fn project_path(&self) -> Option<&String> {
        match self {
            Self::Project(path) => Some(path),
            Self::MultiProject => None,
        }
    }
}

pub(super) fn project_names(meta: &SessionMeta) -> Option<String> {
    grove_core::session_meta::multi_project_identity(&meta.project, &meta.context_roots)?;
    let mut names = vec![meta.project.as_str()];
    for root in &meta.context_roots {
        if !names.contains(&root.project.as_str()) {
            names.push(&root.project);
        }
    }
    Some(names.join(" · "))
}

pub(super) fn root_details(meta: &SessionMeta, snapshot: &TreeSnapshot) -> String {
    let inventory = session_root_inventory(meta, snapshot).join(" · ");
    let mut paths = vec![meta.wt_path.as_str()];
    for root in &meta.context_roots {
        if !paths.contains(&root.wt_path.as_str()) {
            paths.push(&root.wt_path);
        }
    }
    format!("{inventory} · {}", paths.join(" · "))
}

#[derive(Debug, PartialEq, Eq)]
struct RootLabel {
    project: String,
    worktree: String,
    path: String,
}

fn resolved_roots(meta: &SessionMeta, snapshot: &TreeSnapshot) -> Vec<RootLabel> {
    let mut seen = HashSet::new();
    std::iter::once((&meta.project, &meta.wt_path))
        .chain(
            meta.context_roots
                .iter()
                .map(|root| (&root.project, &root.wt_path)),
        )
        .filter_map(|(project, path)| {
            let normalized = crate::paths::normalize_wt_path(path);
            if !seen.insert(normalized.to_string()) {
                return None;
            }
            let worktree = snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| crate::paths::normalize_wt_path(&worktree.path) == normalized)
                .map_or_else(
                    || {
                        std::path::Path::new(normalized).file_name().map_or_else(
                            || normalized.to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                    },
                    |worktree| sidebar_worktree_name(&worktree.name, worktree.is_main).to_string(),
                );
            Some(RootLabel {
                project: project.clone(),
                worktree,
                path: path.clone(),
            })
        })
        .collect()
}

impl Sidebar {
    pub(super) fn multi_project_content(
        &self,
        meta: &SessionMeta,
        title: String,
        status: (&'static str, gpui::Hsla),
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = meta.id;
        let roots = resolved_roots(meta, &self.snapshot);
        let last = roots.len().saturating_sub(1);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XS))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .pr(rpx(CHROME_CONTROL_H))
                    .line_height(rpx(SESSION_TITLE_LINE_H))
                    .child(
                        div()
                            .id(("multi-session-title", id.raw()))
                            .debug_selector(move || format!("multi-session-title-{}", id.raw()))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .font_weight(if selected {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::MEDIUM
                            })
                            .child(title),
                    ),
            )
            .children(roots.into_iter().enumerate().map(|(index, root)| {
                let path = root.path;
                div()
                    .id(SharedString::from(format!(
                        "multi-root-row-{}-{index}",
                        id.raw()
                    )))
                    .debug_selector(move || format!("multi-root-row-{}-{index}", id.raw()))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .line_height(rpx(SESSION_META_LINE_H))
                    .tooltip(move |window, cx| {
                        crate::views::components::tooltip(path.clone(), window).build(window, cx)
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "multi-root-project-{}-{index}",
                                id.raw()
                            )))
                            .debug_selector(move || {
                                format!("multi-root-project-{}-{index}", id.raw())
                            })
                            .min_w_0()
                            .max_w(gpui::relative(0.5))
                            .truncate()
                            .text_size(rpx(HIERARCHY_META_TEXT))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(c::FG_DIM())
                            .child(root.project),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rpx(TEXT_MICRO))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(c::FG_MUTE())
                            .child("·"),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "multi-root-worktree-{}-{index}",
                                id.raw()
                            )))
                            .debug_selector(move || {
                                format!("multi-root-worktree-{}-{index}", id.raw())
                            })
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(c::FG_DIM())
                            .child(root.worktree),
                    )
                    .when(index == last, |row| {
                        row.child(motion::fast(
                            div()
                                .id(("multi-session-status", id.raw()))
                                .debug_selector(move || {
                                    format!("multi-session-status-{}", id.raw())
                                })
                                .flex_shrink_0()
                                .text_size(rpx(HIERARCHY_META_TEXT))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(status.1)
                                .child(status.0),
                            format!("session-status-tree-{}-{}", id.raw(), status.0),
                            cx,
                        ))
                    })
            }))
            .into_any_element()
    }

    pub(super) fn multi_project_sessions(&self, cx: &App) -> Vec<SessionMeta> {
        let registry = self.runtime.read(cx).registry.read(cx);
        let mut seen = HashSet::new();
        self.snapshot
            .projects
            .iter()
            .flat_map(|project| &project.sessions)
            .filter(|id| seen.insert(**id))
            .filter_map(|id| registry.meta(*id))
            .filter(|meta| project_names(meta).is_some())
            .cloned()
            .collect()
    }

    pub(super) fn project_navigation_snapshot(&self, cx: &App) -> TreeSnapshot {
        let multi: HashSet<_> = self
            .multi_project_sessions(cx)
            .iter()
            .map(|meta| meta.id)
            .collect();
        let mut snapshot = self.snapshot.clone();
        for project in &mut snapshot.projects {
            project.sessions.retain(|id| !multi.contains(id));
            for worktree in &mut project.worktrees {
                worktree.sessions.retain(|id| !multi.contains(id));
            }
        }
        snapshot
    }

    pub(super) fn sidebar_context_is_active(&self, target: &SidebarContext, cx: &App) -> bool {
        match target {
            SidebarContext::Project(path) => self.project_path_is_active(path, cx),
            SidebarContext::MultiProject => !self.multi_project_sessions(cx).is_empty(),
        }
    }

    pub(super) fn sidebar_context_trigger(
        &self,
        target: &SidebarContext,
    ) -> Option<std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>> {
        match target {
            SidebarContext::MultiProject => Some(self.multi_project_bounds.clone()),
            SidebarContext::Project(path) => self
                .project_paths
                .iter()
                .find(|(_, candidate)| *candidate == path)
                .and_then(|(idx, _)| self.project_menu_bounds.get(idx))
                .cloned(),
        }
    }

    fn multi_project_header(
        &self,
        count: usize,
        flyout: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rpx(if flyout { TEXT_TITLE } else { TEXT_SMALL }))
                    .font_weight(if flyout {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::MEDIUM
                    })
                    .text_color(c::FG_DIM())
                    .child(format!("Cross-project sessions ({count})")),
            )
            .child(
                self.control(
                    if flyout {
                        "multi-project-flyout-new"
                    } else {
                        "multi-project-new"
                    },
                    "New cross-project session",
                    Action::NewMultiProjectSession,
                    cx,
                )
                .tab_index(if self.navigation_available() { 0 } else { -1 })
                .debug_selector(move || {
                    if flyout {
                        "multi-project-flyout-new".into()
                    } else {
                        "multi-project-new".into()
                    }
                })
                .child(icon("plus", ICON_SM, c::FG_DIM())),
            )
            .into_any_element()
    }

    pub(super) fn multi_project_section(
        &self,
        sessions: &[SessionMeta],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut section = div()
            .id("multi-project-sessions")
            .debug_selector(|| "multi-project-sessions".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XS))
            .child(
                div()
                    .px(rpx(HIERARCHY_INSET))
                    .mb(rpx(SPACE_SM))
                    .child(self.multi_project_header(sessions.len(), false, cx)),
            );
        for (index, meta) in sessions.iter().enumerate() {
            section =
                section.child(self.session_row(meta, false, Some(index + 1), false, window, cx));
        }
        section
            .child(
                div()
                    .mt(rpx(SPACE_3XL))
                    .mx(rpx(HIERARCHY_INSET))
                    .border_b_1()
                    .border_color(c::BORDER_SOFT()),
            )
            .into_any_element()
    }

    pub(super) fn compact_multi_project_group(
        &self,
        sessions: &[SessionMeta],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = SidebarContext::MultiProject;
        let open = self.project_flyout.as_ref() == Some(&target);
        let trigger = self.multi_project_bounds.clone();
        let group_bounds = self.multi_project_group_bounds.clone();
        let mut group = div()
            .id("compact-multi-project-sessions")
            .debug_selector(|| "compact-multi-project-sessions".into())
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(rpx(SPACE_XS))
            .mb(rpx(SPACE_LG))
            .pb(rpx(SPACE_SM))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.open_sidebar_context_on_hover(SidebarContext::MultiProject, window, cx);
                } else {
                    this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                }
            }))
            .child(
                gpui::canvas(
                    move |bounds, _, _| group_bounds.set(bounds),
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                self.control(
                    "multi-project-anchor",
                    format!("Cross-project sessions · {} open", sessions.len()),
                    Action::ProjectFlyout(target),
                    cx,
                )
                .debug_selector(|| "multi-project-anchor".into())
                .track_focus(&self.multi_project_focus)
                .aria_expanded(open)
                .w(rpx(collapsed::COMPACT_ITEM_W))
                .h(rpx(collapsed::COMPACT_ROW_H))
                .rounded(rpx(RADIUS_CHROME))
                .when(open, |row| row.bg(c::BG_HOVER()))
                .child(icon("workspaces", ICON_MD, c::FG_DIM()))
                .child(
                    gpui::canvas(move |bounds, _, _| trigger.set(bounds), |_, (), _, _| {})
                        .absolute()
                        .inset_0(),
                ),
            );
        for meta in sessions {
            group = group.child(self.compact_session(meta, window, cx));
        }
        if open {
            group = group.child(gpui::deferred(
                self.multi_project_flyout(sessions, window, cx),
            ));
        }
        group.into_any_element()
    }

    fn multi_project_flyout(
        &self,
        sessions: &[SessionMeta],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let height = (f32::from(window.viewport_size().height) / scale - SPACE_LG * 2.0).max(0.0);
        let mut body = div()
            .id("multi-project-flyout-scroll")
            .overflow_y_scroll()
            .track_scroll(&self.project_flyout_scroll)
            .max_h(rpx(height))
            .flex()
            .flex_col()
            .p(rpx(SPACE_2XL))
            .gap(rpx(SPACE_SM))
            .child(self.multi_project_header(sessions.len(), true, cx));
        let mut actions = Vec::new();
        let mut indices = Vec::new();
        for (index, meta) in sessions.iter().enumerate() {
            let id = meta.id;
            let mut row = div()
                .id(("multi-flyout-session", id.raw()))
                .debug_selector(move || format!("multi-flyout-session-{}", id.raw()))
                .child(self.session_row(meta, false, None, false, window, cx))
                .into_any_element();
            if !self.project_flyout_hover_open && self.project_flyout_index == index {
                row = div().bg(c::BG_HOVER()).child(row).into_any_element();
            }
            body = body.child(row);
            actions.push(Action::Select(Selection::Session(meta.id)));
            indices.push(index + 1);
        }
        self.sidebar_context_popup(
            body,
            actions,
            indices,
            "Cross-project sessions".into(),
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    use grove_core::session_meta::ContextRoot;

    #[test]
    fn multi_project_root_labels_resolve_names_and_keep_distinct_roots() {
        let meta = SessionMeta {
            id: SessionId::from_raw(1),
            project: "SERVER".into(),
            wt_path: "/server/main/".into(),
            agent: Agent::Claude,
            context_roots: [
                ("WEB", "/web/missing/"),
                ("WEB", "/web/missing"),
                ("SERVER", "/server/topic/"),
                ("SERVER", "/server/main"),
            ]
            .into_iter()
            .map(|(project, path)| ContextRoot {
                project: project.into(),
                wt_path: path.into(),
            })
            .collect(),
            temp_bundle_path: None,
            label: "Mocks".into(),
            restored_title: None,
            spawned_at: std::time::Instant::now(),
            attention: None,
            tmux: false,
            tmux_name: None,
        };
        let snapshot = TreeSnapshot {
            projects: vec![SnapshotProject {
                worktrees: vec![
                    SnapshotWorktree {
                        path: "/server/main".into(),
                        name: "SERVER".into(),
                        is_main: true,
                        ..Default::default()
                    },
                    SnapshotWorktree {
                        path: "/server/topic".into(),
                        name: "Improve logging".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            resolved_roots(&meta, &snapshot),
            vec![
                RootLabel {
                    project: "SERVER".into(),
                    worktree: "Main checkout".into(),
                    path: "/server/main/".into()
                },
                RootLabel {
                    project: "WEB".into(),
                    worktree: "missing".into(),
                    path: "/web/missing/".into()
                },
                RootLabel {
                    project: "SERVER".into(),
                    worktree: "Improve logging".into(),
                    path: "/server/topic/".into()
                },
            ]
        );
    }
}
