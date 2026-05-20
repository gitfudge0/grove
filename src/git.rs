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
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
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
    result
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
    let base = dirs::state_dir()
        .or_else(dirs::data_dir)
        .context("no state/data dir")?;
    Ok(base.join("grove").join("worktrees"))
}

pub fn add_worktree(project_path: &str, project_name: &str, name: &str) -> Result<String> {
    let dest = worktrees_root()?.join(project_name).join(name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
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
    for rel in out.stdout.split(|&b| b == 0) {
        let Ok(rel) = std::str::from_utf8(rel) else { continue };
        if rel.is_empty() {
            continue;
        }
        let dst = dst_root.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::copy(src_root.join(rel), &dst).ok();
    }
    Ok(())
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
