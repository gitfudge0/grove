//! Filesystem snapshots using private Git indexes. HEAD and the user's index are never changed.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    roots: Vec<Root>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Root {
    path: PathBuf,
    tree: String,
    git_dir: PathBuf,
}

fn git(root: &Path, index: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Git checkpoint in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn validate_root(path: &Path) -> Result<PathBuf, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let top = git(&path, None, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(
        String::from_utf8(top)
            .map_err(|e| e.to_string())?
            .trim_end(),
    );
    if top.canonicalize().map_err(|e| e.to_string())? != path {
        return Err("Checkpoint roots must be Git worktree roots".into());
    }
    if git(&path, None, &["config", "--get", "core.sparseCheckout"]).is_ok_and(|v| v == b"true\n") {
        return Err("Checkpoints do not support sparse worktrees".into());
    }
    Ok(path)
}

fn snapshot(path: PathBuf) -> Result<Root, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let index = dir.path().join("index");
    // Seed from the actual index: tracked files remain tracked even when ignored.
    let real = git(
        &path,
        None,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let real = PathBuf::from(
        String::from_utf8(real)
            .map_err(|e| e.to_string())?
            .trim_end(),
    );
    if real.exists() {
        std::fs::copy(real, &index).map_err(|e| e.to_string())?;
    } else {
        git(&path, Some(&index), &["read-tree", "--empty"])?;
    }
    let flags = git(&path, Some(&index), &["ls-files", "-v", "-z"])?;
    if flags.split(|b| *b == 0).any(|entry| {
        entry
            .first()
            .is_some_and(|flag| *flag == b'S' || flag.is_ascii_lowercase())
    }) {
        return Err("Checkpoints do not support skip-worktree or assume-unchanged files".into());
    }
    git(&path, Some(&index), &["add", "--all", "--", "."])?;
    let entries = git(&path, Some(&index), &["ls-files", "--stage", "-z"])?;
    if entries
        .split(|b| *b == 0)
        .any(|entry| entry.starts_with(b"160000 "))
    {
        return Err("Checkpoints do not support submodules or embedded repositories".into());
    }
    let tree = git(&path, Some(&index), &["write-tree"])?;
    let tree = String::from_utf8(tree)
        .map_err(|e| e.to_string())?
        .trim()
        .to_owned();
    // Keep the tree reachable across ordinary GC without changing any user ref.
    git(
        &path,
        None,
        &[
            "update-ref",
            &format!("refs/grove/checkpoints/{tree}"),
            &tree,
        ],
    )?;
    let git_dir = git(&path, None, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = PathBuf::from(
        String::from_utf8(git_dir)
            .map_err(|e| e.to_string())?
            .trim_end(),
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    Ok(Root {
        path,
        tree,
        git_dir,
    })
}

/// Resolve working directories to distinct complete Git worktree roots.
pub fn repository_roots(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    for path in paths {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let top = git(&path, None, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(
            String::from_utf8(top)
                .map_err(|e| e.to_string())?
                .trim_end(),
        )
        .canonicalize()
        .map_err(|e| e.to_string())?;
        if roots.contains(&top) {
            continue;
        }
        if roots
            .iter()
            .any(|other| top.starts_with(other) || other.starts_with(&top))
        {
            return Err("Checkpoint worktrees must not overlap".into());
        }
        roots.push(top);
    }
    Ok(roots)
}

pub fn capture(roots: &[PathBuf]) -> Result<Checkpoint, String> {
    if roots.is_empty() {
        return Err("A checkpoint needs at least one worktree".into());
    }
    let paths = roots
        .iter()
        .map(|p| validate_root(p))
        .collect::<Result<Vec<_>, _>>()?;
    for (i, path) in paths.iter().enumerate() {
        if paths[..i]
            .iter()
            .any(|other| path.starts_with(other) || other.starts_with(path))
        {
            return Err("Checkpoint worktrees must not overlap".into());
        }
    }
    Ok(Checkpoint {
        roots: paths.into_iter().map(snapshot).collect::<Result<_, _>>()?,
    })
}

fn apply(target: &Root, current: &Root) -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let index = dir.path().join("index");
    git(&target.path, Some(&index), &["read-tree", &current.tree])?;
    git(&target.path, Some(&index), &["update-index", "--refresh"])?;
    let target_files = git(
        &target.path,
        None,
        &["ls-tree", "-r", "--name-only", "-z", &target.tree],
    )?;
    let ignored = git(
        &target.path,
        Some(&index),
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )?;
    for file in target_files.split(|b| *b == 0).filter(|v| !v.is_empty()) {
        if ignored
            .split(|b| *b == 0)
            .filter(|v| !v.is_empty())
            .any(|other| {
                file == other
                    || file
                        .strip_prefix(other)
                        .is_some_and(|rest| rest.starts_with(b"/"))
                    || other
                        .strip_prefix(file)
                        .is_some_and(|rest| rest.starts_with(b"/"))
            })
        {
            return Err("An ignored file would be overwritten by the checkpoint".into());
        }
    }
    git(
        &target.path,
        Some(&index),
        &["read-tree", "-m", "-u", &current.tree, &target.tree],
    )?;
    Ok(())
}

impl Checkpoint {
    /// Restore files, retaining HEAD, staging state, and ignored files. On failure,
    /// attempt to put every affected worktree back before returning the error.
    pub fn restore(&self) -> Result<(), String> {
        for root in &self.roots {
            if validate_root(&root.path)? != root.path {
                return Err("Checkpoint worktree path changed".into());
            }
            let git_dir = git(&root.path, None, &["rev-parse", "--absolute-git-dir"])?;
            let git_dir = PathBuf::from(
                String::from_utf8(git_dir)
                    .map_err(|e| e.to_string())?
                    .trim_end(),
            )
            .canonicalize()
            .map_err(|e| e.to_string())?;
            if git_dir != root.git_dir {
                return Err("Checkpoint repository changed".into());
            }
            if !matches!(root.tree.len(), 40 | 64)
                || !root.tree.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("Invalid checkpoint tree identifier".into());
            }
            let kind = git(&root.path, None, &["cat-file", "-t", &root.tree])?;
            if kind != b"tree\n" {
                return Err("Checkpoint object is not a tree".into());
            }
        }
        let paths = self
            .roots
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>();
        let backup = capture(&paths)?;
        for (i, (target, current)) in self.roots.iter().zip(&backup.roots).enumerate() {
            if let Err(error) = apply(target, current) {
                let mut errors = vec![error];
                for old in backup.roots[..=i].iter().rev() {
                    match snapshot(old.path.clone()).and_then(|now| apply(old, &now)) {
                        Ok(()) => {}
                        Err(e) => errors.push(format!("Rollback failed: {e}")),
                    }
                }
                return Err(errors.join("; "));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), None, &["init", "-q"]).unwrap();
        std::fs::write(dir.path().join("tracked"), "initial").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored\n").unwrap();
        git(dir.path(), None, &["add", "."]).unwrap();
        git(
            dir.path(),
            None,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--no-gpg-sign",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        dir
    }
    #[test]
    fn restores_files_and_preserves_git_and_ignored_state() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("untracked"), "saved").unwrap();
        std::fs::write(root.join("tracked"), "saved tracked").unwrap();
        let saved = capture(&[root.to_owned()]).unwrap();
        let saved: Checkpoint =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let head = git(root, None, &["rev-parse", "HEAD"]).unwrap();
        std::fs::remove_file(root.join("untracked")).unwrap();
        std::fs::write(root.join("tracked"), "changed").unwrap();
        std::fs::write(root.join("extra"), "remove me").unwrap();
        std::fs::write(root.join("ignored"), "keep me").unwrap();
        saved.restore().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("tracked")).unwrap(),
            "saved tracked"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("untracked")).unwrap(),
            "saved"
        );
        assert!(!root.join("extra").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("ignored")).unwrap(),
            "keep me"
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(git(root, None, &["rev-parse", "HEAD"]).unwrap(), head);
    }
    #[test]
    fn refuses_ignored_collisions_without_changing_files() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("future"), "old").unwrap();
        let saved = capture(&[root.to_owned()]).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored\nfuture\n").unwrap();
        std::fs::write(root.join("future"), "precious ignored").unwrap();
        std::fs::write(root.join("tracked"), "current").unwrap();
        assert!(saved.restore().is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("future")).unwrap(),
            "precious ignored"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("tracked")).unwrap(),
            "current"
        );
    }
    #[test]
    fn rolls_back_earlier_root_when_later_root_refuses_restore() {
        let first = repo();
        let second = repo();
        std::fs::write(second.path().join("future"), "old").unwrap();
        let saved = capture(&[first.path().to_owned(), second.path().to_owned()]).unwrap();
        std::fs::write(first.path().join("tracked"), "current first").unwrap();
        std::fs::write(second.path().join(".gitignore"), "future\n").unwrap();
        std::fs::write(second.path().join("future"), "precious ignored").unwrap();
        assert!(saved.restore().is_err());
        assert_eq!(
            std::fs::read_to_string(first.path().join("tracked")).unwrap(),
            "current first"
        );
        assert_eq!(
            std::fs::read_to_string(second.path().join("future")).unwrap(),
            "precious ignored"
        );
    }
    #[cfg(unix)]
    #[test]
    fn restores_symlinks_and_deleted_tracked_files() {
        let dir = repo();
        let root = dir.path();
        std::os::unix::fs::symlink("tracked", root.join("link")).unwrap();
        let saved = capture(&[root.to_owned()]).unwrap();
        std::fs::remove_file(root.join("tracked")).unwrap();
        std::fs::remove_file(root.join("link")).unwrap();
        std::fs::write(root.join("link"), "regular").unwrap();
        saved.restore().unwrap();
        assert_eq!(
            std::fs::read_link(root.join("link")).unwrap(),
            PathBuf::from("tracked")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("tracked")).unwrap(),
            "initial"
        );
    }
    #[cfg(unix)]
    #[test]
    fn restoring_directory_over_symlink_does_not_follow_it() {
        let dir = repo();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        std::fs::write(dir.path().join("folder/file"), "saved").unwrap();
        std::fs::write(outside.path().join("file"), "outside").unwrap();
        let saved = capture(&[dir.path().to_owned()]).unwrap();
        std::fs::remove_dir_all(dir.path().join("folder")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("folder")).unwrap();
        saved.restore().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("folder/file")).unwrap(),
            "saved"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("file")).unwrap(),
            "outside"
        );
    }
    #[test]
    fn checkpoint_survives_git_gc() {
        let dir = repo();
        std::fs::write(dir.path().join("untracked"), "durable").unwrap();
        let saved = capture(&[dir.path().to_owned()]).unwrap();
        std::fs::remove_file(dir.path().join("untracked")).unwrap();
        git(dir.path(), None, &["gc", "--prune=now"]).unwrap();
        saved.restore().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("untracked")).unwrap(),
            "durable"
        );
    }
    #[test]
    fn rejects_unsafe_roots() {
        let dir = repo();
        assert!(capture(&[dir.path().to_owned(), dir.path().to_owned()]).is_err());
        std::fs::create_dir(dir.path().join("child")).unwrap();
        assert!(capture(&[dir.path().join("child")]).is_err());
        assert!(capture(&[tempfile::tempdir().unwrap().path().to_owned()]).is_err());
    }
}
