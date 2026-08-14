use fs_err as fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("refusing to remove the project root checkout")]
    RefusesProjectRoot,
    #[error("invalid worktree name: use letters, digits, '-', '_' or '.'")]
    InvalidWorktreeName,
    #[error(
        "invalid project name: must not be empty, start with '-', or contain '/', '\\' or '..'"
    )]
    InvalidProjectName,
    #[error("git {cmd} failed: {stderr}")]
    Command { cmd: String, stderr: String },
    #[error("no home dir")]
    NoHomeDir,
    #[error("create {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to copy: {0}")]
    Copy(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = GitError> = std::result::Result<T, E>;

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub mtime: Option<SystemTime>,
    pub is_main: bool,
}

pub fn list_worktrees(project_path: &str) -> Vec<Worktree> {
    tracing::debug!(
        args = "worktree list --porcelain",
        cwd = %project_path,
        "running git command"
    );
    let out = Command::new("git")
        .args(["-C", project_path, "worktree", "list", "--porcelain"])
        .output();
    // Not a git repo: surface a synthetic root worktree so the project still has a row.
    let Ok(out) = out else {
        return vec![root_worktree(project_path)];
    };
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return vec![root_worktree(project_path)];
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = vec![];
    let mut cur_path: Option<String> = None;
    let mut cur_branch: String = String::new();
    for line in stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                let mtime = worktree_mtime(&path);
                let is_main = result.is_empty();
                result.push(Worktree {
                    path,
                    branch: std::mem::take(&mut cur_branch),
                    mtime,
                    is_main,
                });
            }
            cur_path = Some(p.to_string());
            cur_branch = String::new();
        } else if let Some(b) = line.strip_prefix("branch ") {
            cur_branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line == "detached" {
            cur_branch = "(detached)".to_string();
        }
    }
    if let Some(path) = cur_path {
        let mtime = worktree_mtime(&path);
        let is_main = result.is_empty();
        result.push(Worktree {
            path,
            branch: cur_branch,
            mtime,
            is_main,
        });
    }
    // git worktree list always emits the main checkout first; keep it pinned, sort only the rest by recency.
    if result.len() > 1 {
        result[1..].sort_by_key(|w| std::cmp::Reverse(w.mtime));
    }
    // Compare canonicalized paths — git resolves symlinks (e.g. macOS /tmp -> /private/tmp), a raw compare would duplicate the root.
    let canon = |p: &str| fs_err::canonicalize(p).unwrap_or_else(|_| p.into());
    let project_canon = canon(project_path);
    let has_root = result
        .iter()
        .any(|w| w.path == project_path || canon(&w.path) == project_canon);
    if !has_root {
        result.insert(0, root_worktree(project_path));
    }
    result
}

/// Runs [`list_worktrees`] for each path concurrently (one thread per path), preserving input order.
// Still runs on (and blocks) the calling thread; if this shows up as jank, the fix is moving scanning off the UI thread entirely.
pub fn list_worktrees_many(paths: &[String]) -> Vec<Vec<Worktree>> {
    if paths.len() <= 1 {
        return paths.iter().map(|p| list_worktrees(p)).collect();
    }
    let mut results: Vec<Option<Vec<Worktree>>> = (0..paths.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .iter()
            .map(|p| scope.spawn(move || list_worktrees(p)))
            .collect();
        for (slot, handle) in results.iter_mut().zip(handles) {
            // A panic here degrades to "no worktrees" rather than propagating across threads.
            *slot = handle.join().ok();
        }
    });
    results.into_iter().map(Option::unwrap_or_default).collect()
}

/// The implicit main worktree for a project root, git or not.
fn root_worktree(project_path: &str) -> Worktree {
    let branch = current_branch(project_path);
    Worktree {
        path: project_path.to_string(),
        branch: if branch.is_empty() {
            "—".into()
        } else {
            branch
        },
        mtime: worktree_mtime(project_path),
        is_main: true,
    }
}

/// Cheap (one stat), safe to call per render.
pub fn is_repo(path: &str) -> bool {
    Path::new(path).join(".git").exists()
}

/// Branch checked out at `wt_path`, or `(detached)` if HEAD is detached.
/// Single fast shell-out — safe to call at session spawn but not per render.
pub fn current_branch(wt_path: &str) -> String {
    tracing::debug!(
        args = "branch --show-current",
        cwd = %wt_path,
        "running git command"
    );
    let out = Command::new("git")
        .args(["-C", wt_path, "branch", "--show-current"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "(detached)".into()
            } else {
                s
            }
        }
        Ok(o) => {
            tracing::warn!(
                status = ?o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "git command failed"
            );
            String::new()
        }
        Err(_) => String::new(),
    }
}

fn worktree_mtime(path: &str) -> Option<SystemTime> {
    let p = std::path::Path::new(path);
    let mut best: Option<SystemTime> = fs::metadata(p).and_then(|m| m.modified()).ok();
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let name = e.file_name();
            if name == ".git" {
                continue;
            }
            if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                best = Some(best.map_or(m, |b| b.max(m)));
            }
        }
    }
    best
}

pub fn remove_worktree(project_path: &str, wt_path: &str) -> Result<()> {
    if Path::new(wt_path) == Path::new(project_path) {
        return Err(GitError::RefusesProjectRoot);
    }
    tracing::debug!(
        args = format!("worktree remove {wt_path} --force"),
        cwd = %project_path,
        "running git command"
    );
    let out = Command::new("git")
        .args(["-C", project_path, "worktree", "remove", wt_path, "--force"])
        .output()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return Err(GitError::Command {
            cmd: "worktree remove".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}

/// `--git-common-dir` (not `--git-dir`) yields the owning repo's real `.git`, whose parent is the main checkout — correct even when the worktree lives far from the repo, as grove-managed ones do.
pub fn worktree_owner_repo(wt_path: &str) -> Option<PathBuf> {
    tracing::debug!(
        args = "rev-parse --path-format=absolute --git-common-dir",
        cwd = %wt_path,
        "running git command"
    );
    let out = Command::new("git")
        .args([
            "-C",
            wt_path,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return None;
    }
    // `<repo>/.git` -> `<repo>`; a bare repo has no such parent.
    let parent = Path::new(&common).parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.to_path_buf())
}

pub fn worktrees_root() -> Result<PathBuf> {
    // Not `dirs::config_dir()`: on macOS that resolves to ~/Library/Application Support, but we want ~/.config on every platform.
    let home = dirs::home_dir().ok_or(GitError::NoHomeDir)?;
    Ok(home.join(".config").join("grove").join("worktrees"))
}

/// Restricts to 0700 — worktrees hold the user's source and copied `.env` files.
fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|source| GitError::CreateDir {
        path: dir.display().to_string(),
        source,
    })?;
    crate::attention::restrict_dir(dir);
    Ok(())
}

pub fn valid_worktree_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Looser than `valid_worktree_name` (not a git ref), but still must not escape its parent directory or read as a git flag.
pub fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && !name.starts_with('-')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

/// `worktree_dir` is the project's pinned directory key, not its current display name — frozen at first rename so renaming a project can't orphan its worktree directories.
pub fn add_worktree(project_path: &str, worktree_dir: &str, name: &str) -> Result<String> {
    if !valid_worktree_name(name) {
        return Err(GitError::InvalidWorktreeName);
    }
    if !valid_project_name(worktree_dir) {
        return Err(GitError::InvalidProjectName);
    }
    let root = worktrees_root()?;
    create_private_dir(&root)?;
    let dest = root.join(worktree_dir).join(name);
    if let Some(parent) = dest.parent() {
        create_private_dir(parent)?;
    }
    let dest_str = dest.to_string_lossy().to_string();

    tracing::debug!(
        args = format!("show-ref --verify --quiet refs/heads/{name}"),
        cwd = %project_path,
        "running git command"
    );
    let branch_exists_status = Command::new("git")
        .args([
            "-C",
            project_path,
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .status();
    if let Ok(s) = &branch_exists_status {
        if !s.success() {
            tracing::debug!(status = ?s, "git show-ref: branch does not exist");
        }
    }
    let branch_exists = branch_exists_status.is_ok_and(|s| s.success());

    let mut args = vec!["-C", project_path, "worktree", "add"];
    if !branch_exists {
        args.extend(["-b", name]);
        args.push(&dest_str);
    } else {
        args.push(&dest_str);
        args.push(name);
    }
    tracing::debug!(args = ?args, cwd = %project_path, "running git command");
    let out = Command::new("git").args(&args).output()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return Err(GitError::Command {
            cmd: "worktree add".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    crate::attention::restrict_dir(&dest);
    Ok(dest_str)
}

/// Copies files matching `.worktreeinclude` (gitignore syntax) into the new worktree, e.g. `.env`.
pub fn copy_worktree_includes(project_path: &str, wt_path: &str) -> Result<()> {
    let include = Path::new(project_path).join(".worktreeinclude");
    if !include.exists() {
        return Ok(());
    }
    tracing::debug!(
        args = "ls-files --others --ignored --exclude-from=.worktreeinclude -z",
        cwd = %project_path,
        "running git command"
    );
    let out = Command::new("git")
        .args([
            "-C",
            project_path,
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-from=.worktreeinclude",
            "-z",
        ])
        .output()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return Err(GitError::Command {
            cmd: "ls-files".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let src_root = Path::new(project_path);
    let dst_root = Path::new(wt_path);
    let mut failed: Vec<String> = vec![];
    for rel in out.stdout.split(|&b| b == 0) {
        let Ok(rel) = std::str::from_utf8(rel) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let dst = dst_root.join(rel);
        if let Some(parent) = dst.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                failed.push(format!("{rel}: {e}"));
                continue;
            }
        }
        if let Err(e) = fs::copy(src_root.join(rel), &dst) {
            failed.push(format!("{rel}: {e}"));
        }
    }
    if !failed.is_empty() {
        return Err(GitError::Copy(failed.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn list_worktrees_many_short_circuits_on_len_le_1() {
        assert_eq!(list_worktrees_many(&[]).len(), 0);

        let one = vec!["/nonexistent/grove-test-path-a".to_string()];
        let result = list_worktrees_many(&one);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].path, one[0]);
    }

    #[test]
    fn valid_project_name_rejects_path_escapes_and_flags() {
        assert!(valid_project_name("grove"));
        assert!(valid_project_name("My Project.v2"));
        assert!(!valid_project_name(""));
        assert!(!valid_project_name("."));
        assert!(!valid_project_name(".."));
        assert!(!valid_project_name("a/b"));
        assert!(!valid_project_name("a\\b"));
        assert!(!valid_project_name("../etc"));
        assert!(!valid_project_name("--force"));
    }

    #[test]
    fn list_worktrees_many_preserves_order() {
        let paths: Vec<String> = (0..8)
            .map(|i| format!("/nonexistent/grove-test-path-{i}"))
            .collect();
        let results = list_worktrees_many(&paths);
        assert_eq!(results.len(), paths.len());
        for (path, worktrees) in paths.iter().zip(results.iter()) {
            assert_eq!(&worktrees[0].path, path);
        }
    }

    #[test]
    fn valid_names_accepted() {
        for name in &["feature-x", "fix_1", "v1.2", "abc", "a-b_c.d", "123"] {
            assert!(
                valid_worktree_name(name),
                "{name:?} should be accepted as a valid worktree name"
            );
        }
    }

    #[test]
    fn empty_name_rejected() {
        assert!(!valid_worktree_name(""));
    }

    #[test]
    fn dot_names_rejected() {
        assert!(!valid_worktree_name("."));
        assert!(!valid_worktree_name(".."));
    }

    #[test]
    fn leading_dash_rejected() {
        assert!(!valid_worktree_name("-bad"));
        assert!(!valid_worktree_name("--force"));
        assert!(!valid_worktree_name("-"));
    }

    #[test]
    fn lock_suffix_rejected() {
        assert!(!valid_worktree_name("HEAD.lock"));
        assert!(!valid_worktree_name("feature.lock"));
    }

    #[test]
    fn slash_in_name_rejected() {
        assert!(!valid_worktree_name("feat/scope"));
        assert!(!valid_worktree_name("a/b"));
    }

    #[test]
    fn double_dot_inside_rejected() {
        assert!(!valid_worktree_name("a..b"));
        assert!(!valid_worktree_name("..bad"));
    }

    #[test]
    fn at_brace_rejected() {
        assert!(!valid_worktree_name("ref@{0}"));
        assert!(!valid_worktree_name("a@{b}"));
    }

    #[test]
    fn space_and_shell_metacharacters_rejected() {
        let bad = [
            "my name", "a;b", "a|b", "a&b", "a>b", "a<b", "a`b", "a$b", "a!b",
        ];
        for name in &bad {
            assert!(
                !valid_worktree_name(name),
                "{name:?} should be rejected (space/metachar)"
            );
        }
    }

    #[test]
    fn remove_worktree_refuses_to_remove_project_root() {
        let path = "/nonexistent/path/that/does/not/exist";
        let result = remove_worktree(path, path);
        assert!(
            result.is_err(),
            "remove_worktree must refuse when wt_path == project_path"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("refusing"),
            "error message should mention refusing, got: {msg}"
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorktreeGitState {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Only populated when `dirty`; a clean worktree skips the second git invocation.
    pub added: u32,
    pub removed: u32,
}

pub fn worktree_git_state(path: &str) -> Option<WorktreeGitState> {
    tracing::debug!(
        args = "status --porcelain=v2 -b",
        cwd = %path,
        "running git command"
    );
    let out = Command::new("git")
        .args(["-C", path, "status", "--porcelain=v2", "-b"])
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return None;
    }
    let mut state = parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout));
    if state.dirty {
        let (added, removed) = worktree_diff_stat(path);
        state.added = added;
        state.removed = removed;
    }
    Some(state)
}

/// Any failure yields `(0, 0)` rather than a stale or bogus number.
fn worktree_diff_stat(path: &str) -> (u32, u32) {
    tracing::debug!(
        args = "diff --shortstat HEAD",
        cwd = %path,
        "running git command"
    );
    let Ok(out) = Command::new("git")
        .args(["-C", path, "diff", "--shortstat", "HEAD"])
        .output()
    else {
        return (0, 0);
    };
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return (0, 0);
    }
    parse_shortstat(&String::from_utf8_lossy(&out.stdout))
}

/// Either clause may be absent; anything unrecognised contributes 0.
fn parse_shortstat(out: &str) -> (u32, u32) {
    let mut added = 0;
    let mut removed = 0;
    for clause in out.split(',') {
        let clause = clause.trim();
        let mut toks = clause.split_whitespace();
        let (Some(n), Some(word)) = (toks.next(), toks.next()) else {
            continue;
        };
        let Ok(n) = n.parse::<u32>() else { continue };
        if word.starts_with("insertion") {
            added = n;
        } else if word.starts_with("deletion") {
            removed = n;
        }
    }
    (added, removed)
}

/// `# branch.ab +N -M` supplies ahead/behind; any other non-`#` line means dirty.
fn parse_porcelain_v2(out: &str) -> WorktreeGitState {
    let mut state = WorktreeGitState::default();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for tok in rest.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    state.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    state.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            state.dirty = true;
        }
    }
    state
}

/// e.g. `* ↑1 ↓2`, or `None` when clean and in sync.
pub fn git_state_suffix(state: &WorktreeGitState) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if state.dirty {
        parts.push("*".to_string());
    }
    if state.ahead > 0 {
        parts.push(format!("↑{}", state.ahead));
    }
    if state.behind > 0 {
        parts.push(format!("↓{}", state.behind));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod git_state_tests {
    use super::*;

    #[test]
    fn clean_no_upstream() {
        let out = "# branch.oid abc123\n# branch.head main\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state, WorktreeGitState::default());
        assert_eq!(git_state_suffix(&state), None);
    }

    #[test]
    fn clean_with_upstream_in_sync() {
        let out = "# branch.oid abc123\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
        let state = parse_porcelain_v2(out);
        assert!(!state.dirty);
        assert_eq!(state.ahead, 0);
        assert_eq!(state.behind, 0);
        assert_eq!(git_state_suffix(&state), None);
    }

    #[test]
    fn ahead_only() {
        let out = "# branch.ab +2 -0\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 2);
        assert_eq!(state.behind, 0);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↑2"));
    }

    #[test]
    fn behind_only() {
        let out = "# branch.ab +0 -3\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 0);
        assert_eq!(state.behind, 3);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↓3"));
    }

    #[test]
    fn ahead_and_behind() {
        let out = "# branch.ab +1 -2\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 1);
        assert_eq!(state.behind, 2);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↑1 ↓2"));
    }

    #[test]
    fn dirty_detection_tracked_change() {
        let out =
            "# branch.oid abc123\n1 .M N... 100644 100644 100644 abcd1234 abcd5678 src/main.rs\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("*"));
    }

    #[test]
    fn dirty_detection_untracked() {
        let out = "# branch.oid abc123\n? new_file.txt\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("*"));
    }

    #[test]
    fn dirty_and_ahead_combine() {
        let out =
            "# branch.ab +1 -0\n1 .M N... 100644 100644 100644 abcd1234 abcd5678 src/main.rs\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(state.ahead, 1);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("* ↑1"));
    }

    #[test]
    fn shortstat_both_clauses() {
        let out = " 3 files changed, 128 insertions(+), 9 deletions(-)\n";
        assert_eq!(parse_shortstat(out), (128, 9));
    }

    #[test]
    fn shortstat_insertions_only() {
        let out = " 1 file changed, 5 insertions(+)\n";
        assert_eq!(parse_shortstat(out), (5, 0));
    }

    #[test]
    fn shortstat_deletions_only() {
        let out = " 1 file changed, 7 deletions(-)\n";
        assert_eq!(parse_shortstat(out), (0, 7));
    }

    #[test]
    fn shortstat_singular_wording() {
        let out = " 1 file changed, 1 insertion(+), 1 deletion(-)\n";
        assert_eq!(parse_shortstat(out), (1, 1));
    }

    #[test]
    fn shortstat_empty() {
        assert_eq!(parse_shortstat(""), (0, 0));
        assert_eq!(parse_shortstat("\n"), (0, 0));
    }

    #[test]
    fn shortstat_garbage() {
        assert_eq!(parse_shortstat("fatal: not a git repository"), (0, 0));
        assert_eq!(parse_shortstat(",,,"), (0, 0));
        assert_eq!(parse_shortstat("many insertions(+)"), (0, 0));
        assert_eq!(parse_shortstat(" 99999999999999 insertions(+)"), (0, 0));
    }
}

pub fn init_if_needed(project_path: &str) -> Result<()> {
    let git_dir = std::path::Path::new(project_path).join(".git");
    if git_dir.exists() {
        return Ok(());
    }
    tracing::debug!(args = "init -q", cwd = %project_path, "running git command");
    let out = Command::new("git")
        .args(["-C", project_path, "init", "-q"])
        .output()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return Err(GitError::Command {
            cmd: "init".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}
