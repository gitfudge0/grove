use crate::agent::Agent;
use crate::error::{Result, SessionError};
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One writable worktree available to a multi-root agent session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRoot {
    pub project: String,
    pub wt_path: String,
}

/// The compact identity shown for a session spanning distinct projects.
#[must_use]
pub fn multi_project_identity(
    primary_project: &str,
    context_roots: &[ContextRoot],
) -> Option<String> {
    let mut projects = vec![primary_project];
    for root in context_roots {
        if !projects.iter().any(|project| *project == root.project) {
            projects.push(&root.project);
        }
    }
    let extra = projects.len().saturating_sub(1);
    (extra > 0).then(|| {
        format!(
            "{primary_project} + {extra} project{}",
            if extra == 1 { "" } else { "s" }
        )
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeta {
    pub wt_path: String,
    pub project: String,
    pub label: String,
    pub agent: Agent,
    /// Ordered roots for a multi-worktree session. Older sidecars deserialize as empty.
    #[serde(default)]
    pub context_roots: Vec<ContextRoot>,
}

fn sessions_dir() -> Result<PathBuf> {
    let dir = crate::storage::config_dir()?.join("sessions");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn path_for(name: &str) -> Result<PathBuf> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(SessionError::InvalidName(name.to_string()));
    }
    Ok(sessions_dir()?.join(format!("{name}.json")))
}

pub fn write(name: &str, meta: &SessionMeta) -> Result<()> {
    let p = path_for(name)?;
    let s = serde_json::to_string_pretty(meta).map_err(SessionError::Serialize)?;
    crate::storage::write_atomic(&p, s.as_bytes()).map_err(|e| {
        tracing::debug!(name, error = %e, "session_meta: write failed");
        e
    })?;
    Ok(())
}

pub fn read(name: &str) -> Option<SessionMeta> {
    let p = path_for(name).ok()?;
    let s = match fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(name, error = %e, "session_meta: read failed");
            return None;
        }
    };
    match serde_json::from_str(&s) {
        Ok(meta) => Some(meta),
        Err(e) => {
            tracing::debug!(name, error = %e, "session_meta: parse failed");
            None
        }
    }
}

pub fn delete(name: &str) {
    if let Ok(p) = path_for(name) {
        if let Err(e) = fs::remove_file(&p) {
            tracing::debug!(name, error = %e, "session_meta: delete failed");
        }
    }
}

fn session_names() -> Vec<String> {
    let Ok(dir) = sessions_dir() else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect()
}

pub fn prune(live: &[String]) {
    for name in session_names() {
        if !live.iter().any(|n| n == &name) {
            delete(&name);
        }
    }
}

/// A record that fails to read/write is skipped and warned about, not propagated — a rename must never fail on one bad sidecar.
pub fn rename_project(old_name: &str, new_name: &str) -> usize {
    if old_name == new_name {
        return 0;
    }
    let mut count = 0;
    for name in session_names() {
        let Some(mut meta) = read(&name) else {
            continue;
        };
        let mut changed = false;
        if meta.project == old_name {
            meta.project = new_name.to_string();
            changed = true;
        }
        for root in &mut meta.context_roots {
            if root.project == old_name {
                root.project = new_name.to_string();
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        match write(&name, &meta) {
            Ok(()) => count += 1,
            Err(e) => {
                tracing::warn!(
                    name,
                    error = %e,
                    "session_meta: rename_project failed to rewrite a record"
                );
            }
        }
    }
    count
}

/// `resolve` maps a stale record's `wt_path` to its correct project name — path resolution stays out of this module (see `storage::project_for_worktree_path`) so there's one source of truth.
pub fn repair_stale_projects(
    projects: &[(String, String)],
    resolve: impl Fn(&str) -> Option<String>,
) -> usize {
    let mut count = 0;
    for name in session_names() {
        let Some(mut meta) = read(&name) else {
            continue;
        };
        if projects.iter().any(|(n, _)| n == &meta.project) {
            continue;
        }
        let Some(correct) = resolve(&meta.wt_path) else {
            continue;
        };
        if correct == meta.project {
            continue;
        }
        let old_project = meta.project.clone();
        meta.project.clone_from(&correct);
        match write(&name, &meta) {
            Ok(()) => {
                tracing::info!(
                    name,
                    old_project,
                    new_project = correct,
                    "session_meta: repaired a stale project name"
                );
                count += 1;
            }
            Err(e) => {
                tracing::warn!(
                    name,
                    error = %e,
                    "session_meta: repair_stale_projects failed to rewrite a record"
                );
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::agent::Agent;

    fn make_meta() -> SessionMeta {
        SessionMeta {
            wt_path: "/tmp/test-wt".into(),
            project: "testproject".into(),
            label: "test-label".into(),
            agent: Agent::Claude,
            context_roots: vec![ContextRoot {
                project: "testproject".into(),
                wt_path: "/tmp/test-wt".into(),
            }],
        }
    }

    #[test]
    fn write_rejects_slash_in_name() {
        let result = write("evil/path", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '/'"
        );
    }

    #[test]
    fn write_rejects_backslash_in_name() {
        let result = write("evil\\path", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '\\\\'"
        );
    }

    #[test]
    fn write_rejects_double_dot_in_name() {
        let result = write("..evil", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '..'"
        );
        let result2 = write("a..b", &make_meta());
        assert!(
            result2.is_err(),
            "write must reject a session name with '..' in the middle"
        );
    }

    #[test]
    fn read_returns_none_for_slash_in_name() {
        assert!(
            read("evil/path").is_none(),
            "read must return None for a name containing '/'"
        );
    }

    #[test]
    fn read_returns_none_for_double_dot_in_name() {
        assert!(
            read("../../../etc/passwd").is_none(),
            "read must return None for a path-traversal session name"
        );
    }

    #[test]
    fn session_meta_serde_round_trip() {
        let meta = make_meta();
        let json = serde_json::to_string_pretty(&meta).expect("serialize");
        let back: SessionMeta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.wt_path, meta.wt_path);
        assert_eq!(back.project, meta.project);
        assert_eq!(back.label, meta.label);
        assert_eq!(back.agent, meta.agent);
        assert_eq!(back.context_roots, meta.context_roots);
    }

    #[test]
    fn legacy_sidecar_defaults_context_roots() {
        let json = r#"{"wt_path":"/tmp/test-wt","project":"testproject","label":"test-label","agent":"Claude"}"#;
        let meta: SessionMeta = serde_json::from_str(json).expect("legacy sidecar deserialize");
        assert!(meta.context_roots.is_empty());
    }

    #[test]
    fn multi_project_identity_counts_distinct_projects_only() {
        let roots = vec![
            ContextRoot {
                project: "portfolio".into(),
                wt_path: "/p/a".into(),
            },
            ContextRoot {
                project: "portfolio".into(),
                wt_path: "/p/b".into(),
            },
            ContextRoot {
                project: "api".into(),
                wt_path: "/a".into(),
            },
            ContextRoot {
                project: "web".into(),
                wt_path: "/w".into(),
            },
        ];
        assert_eq!(
            multi_project_identity("portfolio", &roots),
            Some("portfolio + 2 projects".into())
        );
        assert_eq!(multi_project_identity("portfolio", &roots[..2]), None);
        assert_eq!(
            multi_project_identity("portfolio", &roots[..3]),
            Some("portfolio + 1 project".into())
        );
    }

    #[test]
    fn multi_project_identity_keeps_single_project_sessions_unmarked() {
        let roots = vec![
            ContextRoot {
                project: "portfolio".into(),
                wt_path: "/p/main".into(),
            },
            ContextRoot {
                project: "portfolio".into(),
                wt_path: "/p/feature".into(),
            },
        ];
        assert_eq!(multi_project_identity("portfolio", &roots), None);
    }

    /// Isolated per test via `GROVE_CONFIG_DIR`; serializes against `storage::tests::CONFIG_DIR_ENV_TEST_LOCK` since that env var is process-global and both modules' tests run concurrently.
    fn with_temp_config_dir<R>(f: impl FnOnce() -> R) -> R {
        let _lock = crate::storage::tests::CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!(
            "grove-session-meta-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("create temp config dir");
        let prev = std::env::var("GROVE_CONFIG_DIR").ok();
        std::env::set_var("GROVE_CONFIG_DIR", &dir);
        let result = f();
        match prev {
            Some(p) => std::env::set_var("GROVE_CONFIG_DIR", p),
            None => std::env::remove_var("GROVE_CONFIG_DIR"),
        }
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn meta_with(project: &str, wt_path: &str) -> SessionMeta {
        SessionMeta {
            wt_path: wt_path.into(),
            project: project.into(),
            label: "test-label".into(),
            agent: Agent::Claude,
            context_roots: Vec::new(),
        }
    }

    #[test]
    fn rename_project_rewrites_matching_records() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("old-name", "/tmp/wt-1")).expect("write s1");
            let count = rename_project("old-name", "new-name");
            assert_eq!(count, 1);
            let m = read("s1").expect("read s1");
            assert_eq!(m.project, "new-name");
        });
    }

    #[test]
    fn rename_project_leaves_other_names_untouched() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("old-name", "/tmp/wt-1")).expect("write s1");
            write("s2", &meta_with("unrelated", "/tmp/wt-2")).expect("write s2");
            let count = rename_project("old-name", "new-name");
            assert_eq!(count, 1);
            let m2 = read("s2").expect("read s2");
            assert_eq!(m2.project, "unrelated");
        });
    }

    #[test]
    fn rename_project_preserves_other_fields() {
        with_temp_config_dir(|| {
            let before = meta_with("old-name", "/tmp/wt-1");
            write("s1", &before).expect("write s1");
            rename_project("old-name", "new-name");
            let after = read("s1").expect("read s1");
            assert_eq!(after.wt_path, before.wt_path);
            assert_eq!(after.label, before.label);
            assert_eq!(after.agent, before.agent);
            assert_eq!(after.project, "new-name");
        });
    }

    #[test]
    fn rename_project_is_a_noop_when_names_match() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("same", "/tmp/wt-1")).expect("write s1");
            assert_eq!(rename_project("same", "same"), 0);
        });
    }

    #[test]
    fn repair_stale_projects_skips_known_names() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("known", "/tmp/wt-1")).expect("write s1");
            let projects = [("known".to_string(), "/tmp/wt-1".to_string())];
            let count =
                repair_stale_projects(&projects, |_| Some("should-not-be-used".to_string()));
            assert_eq!(count, 0);
            assert_eq!(read("s1").expect("read s1").project, "known");
        });
    }

    #[test]
    fn repair_stale_projects_repairs_using_the_resolver() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("GLOBUS-PORTAL", "/tmp/renamed-project")).expect("write s1");
            let projects = [("SIP-WEB".to_string(), "/tmp/renamed-project".to_string())];
            let count = repair_stale_projects(&projects, |wt_path| {
                (wt_path == "/tmp/renamed-project").then(|| "SIP-WEB".to_string())
            });
            assert_eq!(count, 1);
            assert_eq!(read("s1").expect("read s1").project, "SIP-WEB");
        });
    }

    #[test]
    fn repair_stale_projects_leaves_unresolvable_records_alone() {
        with_temp_config_dir(|| {
            write("s1", &meta_with("ghost-project", "/tmp/nowhere")).expect("write s1");
            let projects: [(String, String); 0] = [];
            let count = repair_stale_projects(&projects, |_| None);
            assert_eq!(count, 0);
            assert_eq!(read("s1").expect("read s1").project, "ghost-project");
        });
    }
}
