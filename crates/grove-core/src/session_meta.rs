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

/// A closed CLI session's identity. CLI backends do not provide a retained
/// structured transcript, so this record deliberately contains no resume action,
/// command arguments, authentication material, or scraped terminal contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedCliSession {
    pub key: String,
    pub agent: Agent,
    pub label: String,
    pub title: Option<String>,
    pub project: String,
    pub cwd: String,
    /// Unix milliseconds when Grove explicitly closed the session.
    pub closed_at: u64,
    #[serde(default)]
    pub context_roots: Vec<ContextRoot>,
    pub tmux_name: Option<String>,
}

fn valid_cli_history_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() < 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn cli_session_history_dir() -> std::result::Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|dir| dir.join("cli-session-history"))
        .map_err(|e| e.to_string())
}

pub fn archive_cli_session(record: &ClosedCliSession) -> std::result::Result<(), String> {
    archive_cli_session_in_dir(record, &cli_session_history_dir()?)
}

fn archive_cli_session_in_dir(
    record: &ClosedCliSession,
    dir: &std::path::Path,
) -> std::result::Result<(), String> {
    if !valid_cli_history_key(&record.key) {
        return Err("Invalid CLI session history key".into());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    crate::storage::write_atomic_private(&dir.join(format!("{}.json", record.key)), &bytes)
        .map_err(|e| e.to_string())
}

#[derive(Default)]
pub struct CliHistory {
    pub sessions: Vec<ClosedCliSession>,
    pub warnings: Vec<String>,
}

pub fn load_cli_session_history() -> std::result::Result<CliHistory, String> {
    load_cli_session_history_in_dir(&cli_session_history_dir()?)
}

fn load_cli_session_history_in_dir(
    dir: &std::path::Path,
) -> std::result::Result<CliHistory, String> {
    if !dir.exists() {
        return Ok(CliHistory::default());
    }
    let mut history = CliHistory::default();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let parsed = fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<ClosedCliSession>(&bytes).map_err(|e| e.to_string())
            });
        let Ok(record) = parsed else {
            history.warnings.push(format!(
                "Could not read closed CLI session {}. Its file was preserved.",
                path.display()
            ));
            continue;
        };
        if !valid_cli_history_key(&record.key)
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(record.key.as_str())
        {
            history.warnings.push(format!(
                "Invalid closed CLI session {}. Its file was preserved.",
                path.display()
            ));
            continue;
        }
        history.sessions.push(record);
    }
    history
        .sessions
        .sort_by_cached_key(|record| std::cmp::Reverse(record.closed_at));
    Ok(history)
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
    /// Explicitly identifies a managed worktree Terminal. Older Terminal sidecars were home/panel shells.
    #[serde(default)]
    pub managed_worktree_terminal: bool,
    /// Ordered roots for a multi-worktree session. Older sidecars deserialize as empty.
    #[serde(default)]
    pub context_roots: Vec<ContextRoot>,
    #[serde(default)]
    pub temp_bundle_path: Option<String>,
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
            if let Some(meta) = read(&name) {
                if let Some(path) = meta.temp_bundle_path {
                    crate::multi_root::cleanup_path(std::path::Path::new(&path));
                }
            }
            delete(&name);
        }
    }
}

/// A record that fails to read/write is skipped and warned about, not propagated — a rename must never fail on one bad sidecar.
pub fn rename_project(old_name: &str, new_name: &str) -> usize {
    if old_name == new_name {
        return 0;
    }
    rename_project_matching(new_name, |project, _| project == old_name)
}

/// Resolve conservatively: ambiguous legacy managed directories and unknown
/// external roots must never inherit the first matching project's ownership.
pub fn project_owner<'a>(
    projects: &'a [crate::storage::Project],
    wt_path: &str,
) -> std::result::Result<&'a crate::storage::Project, String> {
    let canonical = |path: &str| {
        fs::canonicalize(path).unwrap_or_else(|_| std::path::Path::new(path).to_path_buf())
    };
    let wt = canonical(wt_path);
    let exact: Vec<_> = projects
        .iter()
        .filter(|project| canonical(&project.path) == wt)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(format!("Ambiguous project ownership for '{wt_path}': multiple project registrations share this path."));
    }
    let managed = crate::git::worktrees_root().ok();
    let candidates: Vec<_> = projects
        .iter()
        .filter(|project| {
            wt.starts_with(canonical(&project.path))
                || managed
                    .as_ref()
                    .is_some_and(|root| wt.starts_with(root.join(project.worktree_dir())))
        })
        .collect();
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    Err(format!("Cannot uniquely identify the project owning '{wt_path}'. Resolve duplicate worktree directories or register the external root before continuing."))
}

pub fn validate_project_ownership(
    projects: &[crate::storage::Project],
) -> std::result::Result<(), String> {
    for name in session_names() {
        if let Some(meta) = read(&name) {
            project_owner(projects, &meta.wt_path)?;
            for root in &meta.context_roots {
                project_owner(projects, &root.wt_path)?;
            }
        }
    }
    Ok(())
}

/// Prepared from the pre-update project inventory. Originals support rollback
/// if another participant (for example settings persistence) fails.
pub struct ProjectRenamePlan {
    records: Vec<(String, SessionMeta, SessionMeta)>,
}

impl ProjectRenamePlan {
    pub fn prepare(
        projects: &[crate::storage::Project],
        path: &str,
        new_name: &str,
    ) -> std::result::Result<Self, String> {
        let mut records = Vec::new();
        for name in session_names() {
            let Some(original) = read(&name) else {
                continue;
            };
            let mut updated = original.clone();
            if project_owner(projects, &original.wt_path)?.path == path {
                updated.project = new_name.to_string();
            }
            for root in &mut updated.context_roots {
                if project_owner(projects, &root.wt_path)?.path == path {
                    root.project = new_name.to_string();
                }
            }
            if updated.project != original.project
                || updated.context_roots != original.context_roots
            {
                records.push((name, original, updated));
            }
        }
        Ok(Self { records })
    }

    pub fn apply(&self) -> std::result::Result<usize, String> {
        self.apply_with(|name, meta| write(name, meta).map_err(|error| error.to_string()))
    }

    fn apply_with(
        &self,
        mut writer: impl FnMut(&str, &SessionMeta) -> std::result::Result<(), String>,
    ) -> std::result::Result<usize, String> {
        for (index, (name, _, updated)) in self.records.iter().enumerate() {
            if let Err(error) = writer(name, updated) {
                let mut errors = vec![format!("Could not rename session '{name}': {error}")];
                for (name, original, _) in &self.records[..=index] {
                    if let Err(error) = writer(name, original) {
                        errors.push(format!("Could not restore session '{name}': {error}"));
                    }
                }
                return Err(errors.join("; "));
            }
        }
        Ok(self.records.len())
    }

    pub fn rollback(&self) -> std::result::Result<(), String> {
        let mut errors = Vec::new();
        for (name, original, _) in &self.records {
            if let Err(error) = write(name, original) {
                errors.push(format!("Could not restore session '{name}': {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

pub fn rename_project_by_path(
    projects: &[crate::storage::Project],
    path: &str,
    new_name: &str,
) -> std::result::Result<usize, String> {
    ProjectRenamePlan::prepare(projects, path, new_name)?.apply()
}

fn rename_project_matching(new_name: &str, matches: impl Fn(&str, &str) -> bool) -> usize {
    let mut count = 0;
    for name in session_names() {
        let Some(mut meta) = read(&name) else {
            continue;
        };
        let mut changed = false;
        if matches(&meta.project, &meta.wt_path) {
            meta.project = new_name.to_string();
            changed = true;
        }
        for root in &mut meta.context_roots {
            if matches(&root.project, &root.wt_path) {
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

    #[test]
    fn closed_cli_history_round_trips_codex_and_claude_metadata_without_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        for (index, agent) in [Agent::Codex, Agent::Claude].into_iter().enumerate() {
            let record = ClosedCliSession {
                key: format!("cli-123-{}-456", index + 1),
                agent,
                label: format!("{} 1", agent.label()),
                title: Some("Inspect navigation".into()),
                project: "Project".into(),
                cwd: "/project/topic".into(),
                closed_at: 123_000 + index as u64,
                context_roots: vec![ContextRoot {
                    project: "Other".into(),
                    wt_path: "/other".into(),
                }],
                tmux_name: Some(format!("grove-cli-{index}")),
            };
            archive_cli_session_in_dir(&record, dir.path()).unwrap();
            let bytes = fs::read(dir.path().join(format!("{}.json", record.key))).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(json.get("items").is_none());
            assert!(json.get("args").is_none());
            assert_eq!(
                serde_json::from_slice::<ClosedCliSession>(&bytes).unwrap(),
                record
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(dir.path().join(format!("{}.json", record.key)))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        let loaded = load_cli_session_history_in_dir(dir.path()).unwrap();
        assert_eq!(loaded.sessions.len(), 2);
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.sessions[0].agent, Agent::Claude);
        assert_eq!(loaded.sessions[1].agent, Agent::Codex);
    }

    #[test]
    fn closed_cli_history_preserves_corrupt_records_and_rejects_unsafe_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        fs::write(&path, "invalid json").unwrap();
        let unsafe_record = ClosedCliSession {
            key: "../escape".into(),
            agent: Agent::Codex,
            label: "Codex".into(),
            title: None,
            project: "P".into(),
            cwd: "/p".into(),
            closed_at: 0,
            context_roots: Vec::new(),
            tmux_name: None,
        };
        assert!(archive_cli_session_in_dir(&unsafe_record, dir.path()).is_err());
        let mut valid = unsafe_record;
        valid.key = "cli-valid-record".into();
        archive_cli_session_in_dir(&valid, dir.path()).unwrap();
        let loaded = load_cli_session_history_in_dir(dir.path()).unwrap();
        assert_eq!(loaded.sessions, vec![valid]);
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(fs::read_to_string(path).unwrap(), "invalid json");
    }

    fn make_meta() -> SessionMeta {
        SessionMeta {
            wt_path: "/tmp/test-wt".into(),
            project: "testproject".into(),
            label: "test-label".into(),
            agent: Agent::Claude,
            managed_worktree_terminal: false,
            context_roots: vec![ContextRoot {
                project: "testproject".into(),
                wt_path: "/tmp/test-wt".into(),
            }],
            temp_bundle_path: None,
        }
    }

    #[test]
    fn legacy_sidecar_defaults_to_unmanaged_terminal() {
        let legacy = serde_json::json!({
            "wt_path": "/worktree",
            "project": "project",
            "label": "Terminal 1",
            "agent": "Terminal"
        });
        let meta: SessionMeta = serde_json::from_value(legacy).unwrap();
        assert!(!meta.managed_worktree_terminal);
        let mut managed = meta;
        managed.managed_worktree_terminal = true;
        let encoded = serde_json::to_value(&managed).unwrap();
        assert_eq!(encoded["managed_worktree_terminal"], true);
        let decoded: SessionMeta = serde_json::from_value(encoded).unwrap();
        assert!(decoded.managed_worktree_terminal);
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
            managed_worktree_terminal: false,
            context_roots: Vec::new(),
            temp_bundle_path: None,
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
    #[test]
    fn path_scoped_rename_preserves_duplicate_name_primary_and_updates_context() {
        with_temp_config_dir(|| {
            let make_project = |path: &str| crate::storage::Project {
                name: "same".into(),
                path: path.into(),
                scripts: crate::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            };
            let projects = vec![make_project("/fixture/a"), make_project("/fixture/b")];
            let mut a = meta_with("same", "/fixture/a");
            a.context_roots = vec![ContextRoot {
                project: "same".into(),
                wt_path: "/fixture/b".into(),
            }];
            let mut b = meta_with("same", "/fixture/b");
            b.context_roots = vec![ContextRoot {
                project: "same".into(),
                wt_path: "/fixture/a".into(),
            }];
            write("a", &a).unwrap();
            write("b", &b).unwrap();
            assert_eq!(
                rename_project_by_path(&projects, "/fixture/a", "new").unwrap(),
                2
            );
            let a = read("a").unwrap();
            let b = read("b").unwrap();
            assert_eq!(a.project, "new");
            assert_eq!(a.context_roots[0].project, "same");
            assert_eq!(b.project, "same");
            assert_eq!(b.context_roots[0].project, "new");
        });
    }
    #[test]
    fn ownership_refuses_shared_managed_and_external_roots() {
        with_temp_config_dir(|| {
            let make = |path: &str| crate::storage::Project {
                name: "same".into(),
                path: path.into(),
                scripts: crate::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            };
            let mut projects = vec![make("/fixture/a"), make("/fixture/b")];
            assert_eq!(
                project_owner(&projects, "/fixture/a").unwrap().path,
                "/fixture/a"
            );
            let managed = crate::git::worktrees_root()
                .unwrap()
                .join("same")
                .join("feature");
            assert!(project_owner(&projects, managed.to_str().unwrap()).is_err());
            assert!(project_owner(&projects, "/external/unknown").is_err());
            projects[0].worktree_dir = Some("pinned-a".into());
            assert_eq!(
                project_owner(&projects, managed.to_str().unwrap())
                    .unwrap()
                    .path,
                "/fixture/b"
            );
            let pinned = crate::git::worktrees_root()
                .unwrap()
                .join("pinned-a")
                .join("feature");
            assert_eq!(
                project_owner(&projects, pinned.to_str().unwrap())
                    .unwrap()
                    .path,
                "/fixture/a"
            );
        });
    }

    #[test]
    fn ambiguous_sidecar_rename_does_not_partially_write() {
        with_temp_config_dir(|| {
            let projects = vec![crate::storage::Project {
                name: "same".into(),
                path: "/fixture/a".into(),
                scripts: crate::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }];
            write("known", &meta_with("same", "/fixture/a")).unwrap();
            write("unknown", &meta_with("same", "/external/unknown")).unwrap();
            assert!(rename_project_by_path(&projects, "/fixture/a", "new").is_err());
            assert_eq!(read("known").unwrap().project, "same");
        });
    }
    #[test]
    fn managed_worktree_rename_uses_prepared_ownership_and_can_rollback() {
        with_temp_config_dir(|| {
            let mut projects = vec![crate::storage::Project {
                name: "old".into(),
                path: "/fixture/main".into(),
                scripts: crate::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }];
            let wt = crate::git::worktrees_root()
                .unwrap()
                .join("old")
                .join("feature");
            write("managed", &meta_with("old", wt.to_str().unwrap())).unwrap();
            let plan = ProjectRenamePlan::prepare(&projects, "/fixture/main", "new").unwrap();
            crate::storage::pin_worktree_dir_on_rename(&mut projects[0], "old");
            projects[0].name = "new".into();
            assert_eq!(plan.apply().unwrap(), 1);
            assert_eq!(read("managed").unwrap().project, "new");
            // A later settings participant can fail after sidecar application.
            plan.rollback().unwrap();
            assert_eq!(read("managed").unwrap().project, "old");
        });
    }

    #[test]
    fn sidecar_write_failure_restores_every_earlier_record() {
        let original_a = meta_with("old", "/fixture/a");
        let original_b = meta_with("old", "/fixture/b");
        let plan = ProjectRenamePlan {
            records: vec![
                (
                    "a".into(),
                    original_a.clone(),
                    meta_with("new", "/fixture/a"),
                ),
                (
                    "b".into(),
                    original_b.clone(),
                    meta_with("new", "/fixture/b"),
                ),
            ],
        };
        let mut records = std::collections::BTreeMap::from([
            ("a".to_string(), original_a),
            ("b".to_string(), original_b),
        ]);
        let result = plan.apply_with(|name, meta| {
            if name == "b" && meta.project == "new" {
                return Err("injected write failure".into());
            }
            records.insert(name.to_string(), meta.clone());
            Ok(())
        });
        assert!(result.unwrap_err().contains("injected write failure"));
        assert_eq!(records["a"].project, "old");
        assert_eq!(records["b"].project, "old");
    }
}
