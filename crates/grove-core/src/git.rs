use fs_err as fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use thiserror::Error;

/// Everything the git layer can fail with. Each variant corresponds to a real
/// failure site below; callers that only bubble errors upward keep working
/// unchanged (`anyhow` converts any `std::error::Error` via `?`), while callers
/// that want to react to a specific failure can now match instead of grepping
/// a formatted string.
#[derive(Debug, Error)]
pub enum GitError {
    /// A stale or buggy path asked us to remove the project's main checkout.
    #[error("refusing to remove the project root checkout")]
    RefusesProjectRoot,
    /// The user-supplied worktree name failed [`valid_worktree_name`].
    #[error("invalid worktree name: use letters, digits, '-', '_' or '.'")]
    InvalidWorktreeName,
    /// The project name failed [`valid_project_name`]. It becomes a path
    /// component under `worktrees_root()`, so it must not escape it.
    #[error(
        "invalid project name: must not be empty, start with '-', or contain '/', '\\' or '..'"
    )]
    InvalidProjectName,
    /// A `git` subprocess ran but exited non-zero. `cmd` is the subcommand
    /// (e.g. `worktree add`), `stderr` its captured error output.
    #[error("git {cmd} failed: {stderr}")]
    Command { cmd: String, stderr: String },
    /// `dirs::home_dir()` returned nothing, so `worktrees_root` has no base.
    #[error("no home dir")]
    NoHomeDir,
    /// Creating a parent directory for a new worktree failed.
    #[error("create {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// One or more `.worktreeinclude` files could not be copied; the payload
    /// is the joined `path: reason` list.
    #[error("failed to copy: {0}")]
    Copy(String),
    /// Spawning `git` (or any other raw I/O on the git path) failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Shorthand for this module's fallible functions.
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
    // Not a git repo (or git unavailable): surface a single synthetic root
    // worktree so the project still has a row to host sessions/terminals. Git
    // is optional — sessions run directly in the project path, no isolation.
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
    // `git worktree list` always emits the main checkout first; keep it pinned
    // at the top and sort only the linked worktrees by recency.
    if result.len() > 1 {
        result[1..].sort_by_key(|w| std::cmp::Reverse(w.mtime));
    }
    // Guarantee the project root is always present at the top, even if git
    // emitted nothing (not a repo / fresh init with no HEAD) or somehow didn't
    // include it. The root is the user's default landing spot.
    // Compare canonicalized paths: git prints resolved paths (on macOS,
    // /tmp and /var are symlinks into /private), so a raw string compare
    // would miss the root and duplicate it whenever the project was added
    // via a symlinked path.
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

/// Runs `list_worktrees` for each of `paths` concurrently (one thread per
/// path, via `std::thread::scope`), returning results in the same order as
/// the input. Each `list_worktrees` call spawns a `git` subprocess and does
/// filesystem metadata work, so doing them concurrently instead of
/// sequentially turns N round-trips into roughly one.
///
// ponytail: still runs on (and blocks) the calling thread — this only
// collapses N sequential blocking calls into one concurrent batch, bounded
// by project count. If that ever shows up as real jank, the actual fix is
// moving worktree scanning off the UI thread entirely (Task::perform).
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
            // A panic inside `list_worktrees` (subprocess spawn/parsing) would
            // poison the join; treat that project as having no worktrees
            // rather than propagating the panic across threads.
            *slot = handle.join().ok();
        }
    });
    results.into_iter().map(Option::unwrap_or_default).collect()
}

/// The implicit main worktree for a project root — used both for git repos that
/// didn't emit their root and for non-git projects (where it's the only entry).
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

/// Whether `path` is a git repository. The app's single definition of "is a
/// repo" — a `.git` entry exists (directory for a normal repo, file for a linked
/// worktree/submodule). Cheap (one stat), safe to call per render. Used to gate
/// git-only affordances (worktrees, lifecycle scripts) consistently.
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
    // Never let a stale/buggy path force-remove the main checkout itself.
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

pub fn worktrees_root() -> Result<PathBuf> {
    // Worktrees always live under `~/.config/grove/worktrees` on both macOS and
    // Linux. We intentionally do not use `dirs::config_dir()` here because on
    // macOS that resolves to `~/Library/Application Support` — we want the
    // identical `~/.config` location on every platform.
    let home = dirs::home_dir().ok_or(GitError::NoHomeDir)?;
    Ok(home.join(".config").join("grove").join("worktrees"))
}

/// Create `dir` (and its parents) and restrict it to 0700, matching the
/// attention dir: worktrees hold the user's source, including whatever
/// `.worktreeinclude` copied in (`.env` files and friends).
fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|source| GitError::CreateDir {
        path: dir.display().to_string(),
        source,
    })?;
    crate::attention::restrict_dir(dir);
    Ok(())
}

/// Validate a user-supplied worktree/branch name before it's used as both a
/// git branch name and a filesystem path component.
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

/// Validate a project name before it's used as a path component under
/// `worktrees_root()`. Looser than `valid_worktree_name` — a project name is
/// never a git ref, and users legitimately have names with spaces or dots in
/// them — but it must never escape its parent directory or read as a flag to
/// the `git` commands it gets spliced into.
pub fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && !name.starts_with('-')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

pub fn add_worktree(project_path: &str, project_name: &str, name: &str) -> Result<String> {
    if !valid_worktree_name(name) {
        return Err(GitError::InvalidWorktreeName);
    }
    // `project_name` is a path component under `worktrees_root()` just like
    // `name`, so it gets the same treatment rather than being trusted because
    // it came from our own config file.
    if !valid_project_name(project_name) {
        return Err(GitError::InvalidProjectName);
    }
    let root = worktrees_root()?;
    create_private_dir(&root)?;
    let dest = root.join(project_name).join(name);
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
    // `git worktree add` creates the leaf itself, under git's umask — tighten
    // it to 0700 like the parents above.
    crate::attention::restrict_dir(&dest);
    Ok(dest_str)
}

/// Seed a new worktree with files matching `.worktreeinclude` (gitignore
/// syntax — gitignored files that should still be copied so the worktree can
/// run, e.g. `.env`).
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
    use super::*;

    // ── list_worktrees_many ──────────────────────────────────────────────────

    /// Degenerate cases (0 or 1 paths) must not spawn threads; verify they
    /// still produce the expected result via the direct `list_worktrees` path.
    #[test]
    fn list_worktrees_many_short_circuits_on_len_le_1() {
        assert_eq!(list_worktrees_many(&[]).len(), 0);

        let one = vec!["/nonexistent/grove-test-path-a".to_string()];
        let result = list_worktrees_many(&one);
        assert_eq!(result.len(), 1);
        // Non-git path still yields a synthetic root worktree matching the input path.
        assert_eq!(result[0][0].path, one[0]);
    }

    /// Results must preserve input order even though the underlying scans
    /// run concurrently on separate threads.
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

    // ── valid_worktree_name ──────────────────────────────────────────────────

    /// Ordinary names that are safe as both branch names and path components.
    #[test]
    fn valid_names_accepted() {
        for name in &["feature-x", "fix_1", "v1.2", "abc", "a-b_c.d", "123"] {
            assert!(
                valid_worktree_name(name),
                "{name:?} should be accepted as a valid worktree name"
            );
        }
    }

    /// An empty string is never a valid worktree name.
    #[test]
    fn empty_name_rejected() {
        assert!(!valid_worktree_name(""));
    }

    /// `.` and `..` are invalid as path components.
    #[test]
    fn dot_names_rejected() {
        assert!(!valid_worktree_name("."));
        assert!(!valid_worktree_name(".."));
    }

    /// Names starting with `-` can be misinterpreted as git flags.
    #[test]
    fn leading_dash_rejected() {
        assert!(!valid_worktree_name("-bad"));
        assert!(!valid_worktree_name("--force"));
        assert!(!valid_worktree_name("-"));
    }

    /// `.lock` suffix is reserved by git's ref locking mechanism.
    #[test]
    fn lock_suffix_rejected() {
        assert!(!valid_worktree_name("HEAD.lock"));
        assert!(!valid_worktree_name("feature.lock"));
    }

    /// Path separators must never appear in a name used as a path component.
    #[test]
    fn slash_in_name_rejected() {
        assert!(!valid_worktree_name("feat/scope"));
        assert!(!valid_worktree_name("a/b"));
    }

    /// `..` anywhere inside the name is a path-traversal vector.
    #[test]
    fn double_dot_inside_rejected() {
        assert!(!valid_worktree_name("a..b"));
        assert!(!valid_worktree_name("..bad"));
    }

    /// `@{` is rejected by git's refname rules.
    #[test]
    fn at_brace_rejected() {
        assert!(!valid_worktree_name("ref@{0}"));
        assert!(!valid_worktree_name("a@{b}"));
    }

    /// Spaces and common shell metacharacters must be rejected.
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

    // ── remove_worktree ──────────────────────────────────────────────────────

    /// `remove_worktree` must return `Err` immediately — without shelling out
    /// to git — when `wt_path` equals `project_path`. Use a nonexistent path
    /// so no filesystem side-effects can occur even if the guard is bypassed.
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

/// Per-worktree git status snapshot used for the sidebar's compact suffix
/// (`*` dirty, `↑N`/`↓M` ahead/behind upstream). Populated by
/// [`worktree_git_state`] via a single `git status --porcelain=v2 -b`
/// invocation per worktree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorktreeGitState {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

/// Query `path`'s git status via one `git status --porcelain=v2 -b` call.
/// Returns `None` on any failure (not a repo, git missing, etc.) so callers
/// degrade to showing nothing rather than a stale or error value.
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
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `git status --porcelain=v2 -b` output into a [`WorktreeGitState`].
/// The `# branch.ab +N -M` header line (present only when an upstream is
/// configured) supplies ahead/behind counts; any other non-`#` line means
/// the worktree has uncommitted changes (tracked, staged, or untracked).
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

/// Render a [`WorktreeGitState`] as the sidebar's compact suffix (e.g.
/// `* ↑1 ↓2`), or `None` when the worktree is clean and in sync (nothing to
/// show).
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

    /// A clean worktree with no upstream configured (no `branch.ab` line):
    /// nothing dirty, nothing ahead/behind.
    #[test]
    fn clean_no_upstream() {
        let out = "# branch.oid abc123\n# branch.head main\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state, WorktreeGitState::default());
        assert_eq!(git_state_suffix(&state), None);
    }

    /// A clean worktree that is even with its upstream: `+0 -0`.
    #[test]
    fn clean_with_upstream_in_sync() {
        let out = "# branch.oid abc123\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
        let state = parse_porcelain_v2(out);
        assert!(!state.dirty);
        assert_eq!(state.ahead, 0);
        assert_eq!(state.behind, 0);
        assert_eq!(git_state_suffix(&state), None);
    }

    /// Ahead-only: `+2 -0` shows only `↑2`.
    #[test]
    fn ahead_only() {
        let out = "# branch.ab +2 -0\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 2);
        assert_eq!(state.behind, 0);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↑2"));
    }

    /// Behind-only: `+0 -3` shows only `↓3`.
    #[test]
    fn behind_only() {
        let out = "# branch.ab +0 -3\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 0);
        assert_eq!(state.behind, 3);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↓3"));
    }

    /// Ahead and behind combine: `+1 -2` shows `↑1 ↓2`.
    #[test]
    fn ahead_and_behind() {
        let out = "# branch.ab +1 -2\n";
        let state = parse_porcelain_v2(out);
        assert_eq!(state.ahead, 1);
        assert_eq!(state.behind, 2);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("↑1 ↓2"));
    }

    /// A non-`#` line (tracked-modified, staged, or untracked entry) marks the
    /// worktree dirty regardless of ahead/behind.
    #[test]
    fn dirty_detection_tracked_change() {
        let out =
            "# branch.oid abc123\n1 .M N... 100644 100644 100644 abcd1234 abcd5678 src/main.rs\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("*"));
    }

    /// Untracked files (`?` entries in porcelain v2) count as dirty too.
    #[test]
    fn dirty_detection_untracked() {
        let out = "# branch.oid abc123\n? new_file.txt\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("*"));
    }

    /// Dirty + diverged combine into `* ↑1`.
    #[test]
    fn dirty_and_ahead_combine() {
        let out =
            "# branch.ab +1 -0\n1 .M N... 100644 100644 100644 abcd1234 abcd5678 src/main.rs\n";
        let state = parse_porcelain_v2(out);
        assert!(state.dirty);
        assert_eq!(state.ahead, 1);
        assert_eq!(git_state_suffix(&state).as_deref(), Some("* ↑1"));
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
