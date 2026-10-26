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
    let mut details = vec![project_names(meta).unwrap_or_else(|| meta.project.clone())];
    if !inventory.is_empty() {
        details.push(inventory);
    }
    details.extend(paths.into_iter().map(str::to_string));
    details.join(" · ")
}

pub(super) fn worktree_names(meta: &SessionMeta, snapshot: &TreeSnapshot) -> String {
    let mut seen = HashSet::new();
    std::iter::once(meta.wt_path.as_str())
        .chain(meta.context_roots.iter().map(|root| root.wt_path.as_str()))
        .filter(|path| seen.insert(crate::paths::normalize_wt_path(path)))
        .map(|path| {
            snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| {
                    crate::paths::normalize_wt_path(&worktree.path)
                        == crate::paths::normalize_wt_path(path)
                })
                .map_or_else(
                    || crate::paths::path_basename(path),
                    |worktree| worktree.name.clone(),
                )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

impl Sidebar {
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
            SidebarContext::MultiProject => true,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    use grove_core::session_meta::ContextRoot;

    #[test]
    fn multi_project_root_details_keep_project_and_path_identities() {
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
        let details = root_details(&meta, &snapshot);
        assert!(details.contains("SERVER"));
        assert!(details.contains("WEB"));
        assert!(details.contains("/server/main/"));
        assert!(details.contains("/web/missing/"));
        assert!(details.contains("/server/topic/"));
        assert_eq!(
            worktree_names(&meta, &snapshot),
            "SERVER · missing · Improve logging"
        );
    }
}
