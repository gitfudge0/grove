use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub mtime: Option<SystemTime>,
    pub is_main: bool,
}

pub fn list_worktrees(project_path: &str) -> Vec<Worktree> {
    let out = Command::new("git")
        .args(["-C", project_path, "worktree", "list", "--porcelain"])
        .output();
    // Not a git repo (or git unavailable): surface a single synthetic root
    // worktree so the project still has a row to host sessions/terminals. Git
    // is optional — sessions run directly in the project path, no isolation.
    let Ok(out) = out else { return vec![root_worktree(project_path)] };
    if !out.status.success() {
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
        result[1..].sort_by(|a, b| b.mtime.cmp(&a.mtime));
    }
    // Guarantee the project root is always present at the top, even if git
    // emitted nothing (not a repo / fresh init with no HEAD) or somehow didn't
    // include it. The root is the user's default landing spot.
    let has_root = result.iter().any(|w| w.path == project_path);
    if !has_root {
        result.insert(0, root_worktree(project_path));
    }
    result
}

/// The implicit main worktree for a project root — used both for git repos that
/// didn't emit their root and for non-git projects (where it's the only entry).
fn root_worktree(project_path: &str) -> Worktree {
    let branch = current_branch(project_path);
    Worktree {
        path: project_path.to_string(),
        branch: if branch.is_empty() { "—".into() } else { branch },
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
        _ => String::new(),
    }
}

fn worktree_mtime(path: &str) -> Option<SystemTime> {
    let p = std::path::Path::new(path);
    let mut best: Option<SystemTime> = std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if let Ok(rd) = std::fs::read_dir(p) {
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
        anyhow::bail!("refusing to remove the project root checkout");
    }
    let out = Command::new("git")
        .args(["-C", project_path, "worktree", "remove", wt_path, "--force"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn worktrees_root() -> Result<PathBuf> {
    // Worktrees always live under `~/.config/grove/worktrees` on both macOS and
    // Linux. We intentionally do not use `dirs::config_dir()` here because on
    // macOS that resolves to `~/Library/Application Support` — we want the
    // identical `~/.config` location on every platform.
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".config").join("grove").join("worktrees"))
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

pub fn add_worktree(project_path: &str, project_name: &str, name: &str) -> Result<String> {
    if !valid_worktree_name(name) {
        anyhow::bail!("invalid worktree name: use letters, digits, '-', '_' or '.'");
    }
    let dest = worktrees_root()?.join(project_name).join(name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let dest_str = dest.to_string_lossy().to_string();

    let branch_exists = Command::new("git")
        .args([
            "-C",
            project_path,
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let mut args = vec!["-C", project_path, "worktree", "add"];
    if !branch_exists {
        args.extend(["-b", name]);
        args.push(&dest_str);
    } else {
        args.push(&dest_str);
        args.push(name);
    }
    let out = Command::new("git").args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
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
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
            if let Err(e) = std::fs::create_dir_all(parent) {
                failed.push(format!("{rel}: {e}"));
                continue;
            }
        }
        if let Err(e) = std::fs::copy(src_root.join(rel), &dst) {
            failed.push(format!("{rel}: {e}"));
        }
    }
    if !failed.is_empty() {
        anyhow::bail!("failed to copy: {}", failed.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

pub fn init_if_needed(project_path: &str) -> Result<()> {
    let git_dir = std::path::Path::new(project_path).join(".git");
    if git_dir.exists() {
        return Ok(());
    }
    let out = Command::new("git")
        .args(["-C", project_path, "init", "-q"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("git init failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}
