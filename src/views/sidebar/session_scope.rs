//! Scope line for sessions that span several checkouts: which repositories the
//! session touches, what ties its checkouts together, and anything out of line.
use super::*;

/// Repository names shown before collapsing to "primary + N".
const MAX_SCOPE_PROJECTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeException {
    pub text: String,
    /// Unavailable checkouts warn; branch outliers are informational.
    pub warning: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionScope {
    /// Repository set, primary first: `SERVER + WEB` or `SERVER + 4`.
    pub projects: String,
    /// Shared worktree, else shared branch, else the worktree set or count.
    pub shared: String,
    pub exceptions: Vec<ScopeException>,
}

struct ScopeRoot<'a> {
    project: &'a str,
    name: String,
    /// `None` when the branch is unknown or the checkout is unavailable.
    branch: Option<&'a str>,
    available: bool,
}

fn known_branch(branch: &str) -> Option<&str> {
    let branch = branch.trim();
    (!branch.is_empty() && !matches!(branch, "—" | "-")).then_some(branch)
}

/// Every checkout of the session, primary first, deduplicated by path.
fn scope_roots<'a>(meta: &'a SessionMeta, snapshot: &'a TreeSnapshot) -> Vec<ScopeRoot<'a>> {
    let mut seen = HashSet::new();
    std::iter::once((meta.project.as_str(), meta.wt_path.as_str()))
        .chain(
            meta.context_roots
                .iter()
                .map(|root| (root.project.as_str(), root.wt_path.as_str())),
        )
        .filter(|(_, path)| seen.insert(crate::paths::normalize_wt_path(path)))
        .map(|(project, path)| {
            let known = snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| {
                    crate::paths::normalize_wt_path(&worktree.path)
                        == crate::paths::normalize_wt_path(path)
                });
            ScopeRoot {
                project,
                name: known.map_or_else(
                    || crate::paths::path_basename(path),
                    |worktree| worktree.name.clone(),
                ),
                branch: known.and_then(|worktree| known_branch(&worktree.branch)),
                available: known.is_some(),
            }
        })
        .collect()
}

/// `None` for single-checkout sessions, which keep the project/worktree header.
pub(super) fn session_scope(meta: &SessionMeta, snapshot: &TreeSnapshot) -> Option<SessionScope> {
    let roots = scope_roots(meta, snapshot);
    if roots.len() < 2 {
        return None;
    }
    let mut projects: Vec<&str> = Vec::new();
    for root in &roots {
        if !projects.contains(&root.project) {
            projects.push(root.project);
        }
    }
    let projects_label = if projects.len() > MAX_SCOPE_PROJECTS {
        format!("{} + {}", projects[0], projects.len() - 1)
    } else {
        projects.join(" + ")
    };
    // A checkout is named by its project unless the project repeats.
    let describe = |root: &ScopeRoot| {
        if roots.iter().filter(|r| r.project == root.project).count() > 1 {
            format!("{} / {}", root.project, root.name)
        } else {
            root.project.to_owned()
        }
    };

    let mut names: Vec<&str> = Vec::new();
    for root in &roots {
        if !names.contains(&root.name.as_str()) {
            names.push(&root.name);
        }
    }
    let live: Vec<&ScopeRoot> = roots.iter().filter(|root| root.available).collect();
    let live_branches: Option<Vec<&str>> = live.iter().map(|root| root.branch).collect();
    let shared = if names.len() == 1 {
        names[0].to_owned()
    } else if let Some(branch) = live_branches
        .as_ref()
        .filter(|branches| branches.len() > 1)
        .and_then(|branches| {
            branches
                .iter()
                .all(|b| *b == branches[0])
                .then(|| branches[0])
        })
    {
        branch.to_owned()
    } else if projects.len() == 1 && names.len() == 2 {
        names.join(" + ")
    } else {
        format!("{} worktrees", names.len())
    };

    let mut exceptions: Vec<ScopeException> = roots
        .iter()
        .filter(|root| !root.available)
        .map(|root| ScopeException {
            text: if roots.iter().filter(|r| r.project == root.project).count() > 1 {
                format!("{} unavailable", describe(root))
            } else {
                format!("{} checkout unavailable", root.project)
            },
            warning: true,
        })
        .collect();
    // Branch outliers differ from the strict-majority branch among live checkouts
    // with known branches, or from the primary checkout's branch without one.
    let known: Vec<(&ScopeRoot, &str)> = live
        .iter()
        .filter_map(|root| root.branch.map(|branch| (*root, branch)))
        .collect();
    let majority = known
        .iter()
        .map(|(_, branch)| *branch)
        .find(|candidate| {
            known
                .iter()
                .filter(|(_, branch)| branch == candidate)
                .count()
                * 2
                > known.len()
        })
        .or_else(|| roots[0].available.then_some(roots[0].branch).flatten());
    if let Some(majority) = majority {
        exceptions.extend(known.iter().filter(|(_, branch)| *branch != majority).map(
            |(root, branch)| ScopeException {
                text: format!("{} on {branch}", describe(root)),
                warning: false,
            },
        ));
    }

    Some(SessionScope {
        projects: projects_label,
        shared,
        exceptions,
    })
}

/// Full checkout inventory for accessible names, including unavailable roots.
pub(super) fn scope_accessible_details(scope: &SessionScope, inventory: &[String]) -> String {
    let mut details = vec![format!(
        "{} checkouts: {}",
        inventory.len(),
        inventory.join(", ")
    )];
    details.extend(
        scope
            .exceptions
            .iter()
            .map(|exception| exception.text.clone()),
    );
    details.join(" · ")
}

/// One exception line below the status row.
pub(super) fn scope_exception_line(exception: &ScopeException) -> AnyElement {
    let color = if exception.warning {
        c::STATUS_WARNING()
    } else {
        c::FG_DIM()
    };
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .mt(rpx(SPACE_XS))
        .gap(rpx(SPACE_SM))
        .h(rpx(SESSION_LIST_FOOTER_H))
        .text_size(rpx(TEXT_MICRO))
        .line_height(rpx(SESSION_LIST_FOOTER_H))
        .text_color(color)
        .child(div().flex_shrink_0().child(icon(
            if exception.warning {
                "status-warning"
            } else {
                "git-branch"
            },
            ICON_SM,
            color,
        )))
        .child(div().min_w_0().truncate().child(exception.text.clone()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
    use grove_core::session_meta::ContextRoot;

    fn meta(primary: (&str, &str), roots: &[(&str, &str)]) -> SessionMeta {
        SessionMeta {
            id: SessionId::from_raw(1),
            project: primary.0.into(),
            wt_path: primary.1.into(),
            agent: Agent::Claude,
            context_roots: roots
                .iter()
                .map(|(project, path)| ContextRoot {
                    project: (*project).into(),
                    wt_path: (*path).into(),
                })
                .collect(),
            temp_bundle_path: None,
            label: "claude 1".into(),
            restored_title: None,
            spawned_at: Instant::now(),
            attention: None,
            tmux: false,
            tmux_name: None,
        }
    }

    fn snapshot(worktrees: &[(&str, &str, &str)]) -> TreeSnapshot {
        TreeSnapshot {
            projects: vec![SnapshotProject {
                worktrees: worktrees
                    .iter()
                    .map(|(path, name, branch)| SnapshotWorktree {
                        path: (*path).into(),
                        name: (*name).into(),
                        branch: (*branch).into(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn single_checkout_keeps_its_header() {
        let snap = snapshot(&[("/s/381", "SIP-381", "feat/sip-381")]);
        assert_eq!(
            session_scope(&meta(("SERVER", "/s/381"), &[("SERVER", "/s/381/")]), &snap),
            None
        );
    }

    #[test]
    fn shared_worktree_names_the_scope() {
        let snap = snapshot(&[
            ("/s/381", "SIP-381", "feat/sip-381"),
            ("/w/381", "SIP-381", "feat/sip-381"),
        ]);
        let scope = session_scope(
            &meta(
                ("SERVER", "/s/381"),
                &[("SERVER", "/s/381"), ("WEB", "/w/381")],
            ),
            &snap,
        )
        .unwrap();
        assert_eq!(scope.projects, "SERVER + WEB");
        assert_eq!(scope.shared, "SIP-381");
        assert!(scope.exceptions.is_empty());
    }

    #[test]
    fn shared_branch_then_worktree_set_then_count() {
        let snap = snapshot(&[
            ("/s/rem", "reminders", "feat/sip-388"),
            ("/w/rem", "sip-388-web", "feat/sip-388"),
            ("/s/402", "SIP-402", "feat/sip-402"),
            ("/w/main", "WEB", "main"),
        ]);
        let branch = session_scope(&meta(("SERVER", "/s/rem"), &[("WEB", "/w/rem")]), &snap);
        assert_eq!(branch.unwrap().shared, "feat/sip-388");
        let same_project =
            session_scope(&meta(("SERVER", "/s/rem"), &[("SERVER", "/s/402")]), &snap).unwrap();
        assert_eq!(same_project.projects, "SERVER");
        assert_eq!(same_project.shared, "reminders + SIP-402");
        let nothing = session_scope(&meta(("SERVER", "/s/402"), &[("WEB", "/w/main")]), &snap);
        assert_eq!(nothing.unwrap().shared, "2 worktrees");
    }

    #[test]
    fn more_than_three_projects_collapse_to_primary_and_count() {
        let names = ["SERVER", "WEB", "MOBILE", "ADMIN", "INFRA"];
        let paths: Vec<String> = names.iter().map(|n| format!("/{n}/400")).collect();
        let worktrees: Vec<(&str, &str, &str)> = paths
            .iter()
            .map(|path| (path.as_str(), "SIP-400", "feat/sip-400"))
            .collect();
        let roots: Vec<(&str, &str)> = names
            .iter()
            .zip(&paths)
            .map(|(name, path)| (*name, path.as_str()))
            .collect();
        let scope = session_scope(&meta(roots[0], &roots), &snapshot(&worktrees)).unwrap();
        assert_eq!(scope.projects, "SERVER + 4");
        let three = session_scope(&meta(roots[0], &roots[..3]), &snapshot(&worktrees)).unwrap();
        assert_eq!(three.projects, "SERVER + WEB + MOBILE");
    }

    #[test]
    fn branch_outlier_and_unavailable_checkout_are_exceptions() {
        let snap = snapshot(&[
            ("/s/381", "SIP-381", "feat/sip-381"),
            ("/w/381", "SIP-381", "main"),
            ("/m/381", "SIP-381", "feat/sip-381"),
        ]);
        let outlier = session_scope(
            &meta(
                ("SERVER", "/s/381"),
                &[("WEB", "/w/381"), ("MOBILE", "/m/381")],
            ),
            &snap,
        )
        .unwrap();
        assert_eq!(outlier.shared, "SIP-381");
        assert_eq!(
            outlier.exceptions,
            [ScopeException {
                text: "WEB on main".into(),
                warning: false
            }]
        );
        let missing = session_scope(
            &meta(("SERVER", "/s/381"), &[("WEB", "/gone/SIP-381")]),
            &snap,
        )
        .unwrap();
        assert_eq!(missing.projects, "SERVER + WEB");
        assert_eq!(missing.shared, "SIP-381");
        assert_eq!(
            missing.exceptions,
            [ScopeException {
                text: "WEB checkout unavailable".into(),
                warning: true
            }]
        );
    }

    #[test]
    fn two_way_branch_split_compares_with_the_primary_checkout() {
        let snap = snapshot(&[("/s/a", "SIP-1", "feat/a"), ("/w/a", "SIP-1", "main")]);
        let scope = session_scope(&meta(("SERVER", "/s/a"), &[("WEB", "/w/a")]), &snap).unwrap();
        assert_eq!(scope.shared, "SIP-1");
        assert_eq!(
            scope.exceptions,
            [ScopeException {
                text: "WEB on main".into(),
                warning: false
            }]
        );
    }
}
