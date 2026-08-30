//! tmux sidecar reconciliation — pure, and decided before any tmux runs. Nothing here shells out, touches gpui, or
//! knows what a PTY is.
//!
//! Startup and the Settings tmux-toggle re-scan are the same call, as iced runs `discover_sessions` from both
//! `App::new` (`src/app/mod.rs:219-224`) and `discover_tmux_sessions` (`:347-366`) — sound only because of rule 1 below.

use grove_core::agent::Agent;
use grove_core::tmux::DiscoveredSession;

use crate::entities::session_registry::SessionMeta;

/// One session to attach, and where it belongs in the registry's order.
#[derive(Debug, Clone)]
pub struct Reattach {
    pub session: DiscoveredSession,
    /// Index in the registry's insertion order, computed after the preceding entries of the same plan are inserted.
    pub at: usize,
}

/// Rules: (1) a tmux name already live in the registry is skipped, making the re-scan idempotent; (2) placement
/// groups by project and follows the project's worktree order (`session_insert_index`, `src/app/spawn.rs:52-80`),
/// unknown appends; (3) total — an unknown project or deleted worktree path places at the end rather than panicking.
///
/// `wt_order` maps a project name to its worktree paths in tree order (empty or missing = unknown, which appends).
#[must_use]
pub fn plan(
    discovered: &[DiscoveredSession],
    existing: &[SessionMeta],
    wt_order: &dyn Fn(&str) -> Vec<String>,
) -> Vec<Reattach> {
    // Only the fields placement reads, so the plan can be simulated without constructing `SessionMeta`s.
    let mut order: Vec<(String, String)> = existing
        .iter()
        .map(|m| (m.project.clone(), m.wt_path.clone()))
        .collect();
    let live: Vec<String> = existing
        .iter()
        .filter_map(|m| m.tmux_name.clone())
        .collect();

    let mut out = Vec::new();
    for d in discovered {
        // Home terminals/panel shells are never tmux-backed; a discovery claiming to be one is a leaked pre-fix terminal.
        if d.agent == Agent::Terminal {
            continue;
        }
        if live.contains(&d.name) || out.iter().any(|r: &Reattach| r.session.name == d.name) {
            continue;
        }
        let at = insert_index(&order, &d.project, &d.wt_path, wt_order);
        order.insert(at, (d.project.clone(), d.wt_path.clone()));
        out.push(Reattach {
            session: d.clone(),
            at,
        });
    }
    out
}

/// Port of `App::session_insert_index` (`src/app/spawn.rs:52-80`).
fn insert_index(
    order: &[(String, String)],
    project: &str,
    wt_path: &str,
    wt_order: &dyn Fn(&str) -> Vec<String>,
) -> usize {
    let block: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|(_, (p, _))| p == project)
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = block.first() else {
        return order.len();
    };
    let Some(&last) = block.last() else {
        return order.len();
    };
    let paths = wt_order(project);
    let Some(new_pos) = paths.iter().position(|p| p == wt_path) else {
        // A worktree the project no longer lists still belongs to its project's block.
        return last + 1;
    };
    for &i in &block {
        let Some((_, path)) = order.get(i) else {
            continue;
        };
        match paths.iter().position(|p| p == path) {
            Some(pos) if pos > new_pos => return i,
            // An unplaceable neighbour never displaces a placeable newcomer.
            _ => {}
        }
    }
    let _ = first;
    last + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::session_registry::SessionId;
    use grove_core::agent::Agent;

    fn discovered(name: &str, project: &str, wt: &str) -> DiscoveredSession {
        DiscoveredSession {
            name: name.to_string(),
            wt_path: wt.to_string(),
            project: project.to_string(),
            label: "claude 1".to_string(),
            agent: Agent::Claude,
            context_roots: Vec::new(),
        }
    }

    fn meta(id: u64, project: &str, wt: &str, tmux_name: Option<&str>) -> SessionMeta {
        SessionMeta {
            id: SessionId::from_raw(id),
            project: project.to_string(),
            wt_path: wt.to_string(),
            agent: Agent::Claude,
            context_roots: Vec::new(),
            label: "claude 1".to_string(),
            spawned_at: std::time::Instant::now(),
            attention: None,
            tmux: tmux_name.is_some(),
            tmux_name: tmux_name.map(str::to_string),
        }
    }

    fn order_of(project: &str) -> Vec<String> {
        match project {
            "alpha" => vec!["/a/one".into(), "/a/two".into(), "/a/three".into()],
            _ => Vec::new(),
        }
    }

    #[test]
    fn an_empty_discovery_is_a_no_op() {
        assert!(plan(&[], &[], &order_of).is_empty());
        assert!(plan(&[], &[meta(1, "alpha", "/a/one", None)], &order_of).is_empty());
    }

    #[test]
    fn an_already_attached_tmux_name_is_skipped() {
        let d = [discovered("grove-x", "alpha", "/a/one")];
        let existing = [meta(1, "alpha", "/a/one", Some("grove-x"))];
        assert!(
            plan(&d, &existing, &order_of).is_empty(),
            "the re-scan must not double-insert"
        );
        // A different session in the same worktree still attaches.
        let d2 = [discovered("grove-y", "alpha", "/a/one")];
        assert_eq!(plan(&d2, &existing, &order_of).len(), 1);
    }

    #[test]
    fn a_duplicate_inside_one_discovery_list_is_skipped_too() {
        let d = [
            discovered("grove-x", "alpha", "/a/one"),
            discovered("grove-x", "alpha", "/a/one"),
        ];
        assert_eq!(plan(&d, &[], &order_of).len(), 1);
    }

    #[test]
    fn placement_follows_the_projects_worktree_order() {
        // `/a/three` is last in the project's order, so it lands after the
        // existing `/a/one` session; `/a/two` lands between them.
        let existing = [
            meta(1, "alpha", "/a/one", Some("grove-1")),
            meta(2, "alpha", "/a/three", Some("grove-3")),
        ];
        let d = [discovered("grove-2", "alpha", "/a/two")];
        let got = plan(&d, &existing, &order_of);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].at, 1, "must land between /a/one and /a/three");
    }

    #[test]
    fn sessions_group_by_project() {
        let existing = [
            meta(1, "alpha", "/a/one", Some("grove-1")),
            meta(2, "beta", "/b/one", Some("grove-b")),
        ];
        let d = [discovered("grove-2", "alpha", "/a/two")];
        let got = plan(&d, &existing, &order_of);
        assert_eq!(got[0].at, 1, "must stay inside alpha's block");
    }

    #[test]
    fn an_unknown_project_appends() {
        let existing = [meta(1, "alpha", "/a/one", Some("grove-1"))];
        let d = [discovered("grove-z", "gamma", "/g/one")];
        let got = plan(&d, &existing, &order_of);
        assert_eq!(got[0].at, 1);
    }

    #[test]
    fn a_worktree_that_no_longer_exists_still_places_without_panicking() {
        // The sidecar outlives the checkout: `/a/gone` is in no worktree list.
        let existing = [meta(1, "alpha", "/a/one", Some("grove-1"))];
        let d = [discovered("grove-gone", "alpha", "/a/gone")];
        let got = plan(&d, &existing, &order_of);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].at, 1);
    }

    #[test]
    fn several_new_sessions_keep_their_relative_order() {
        let d = [
            discovered("grove-1", "alpha", "/a/one"),
            discovered("grove-2", "alpha", "/a/two"),
            discovered("grove-3", "alpha", "/a/three"),
        ];
        let got = plan(&d, &[], &order_of);
        assert_eq!(
            got.iter().map(|r| r.at).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "each insert accounts for the ones planned before it"
        );
    }
}
