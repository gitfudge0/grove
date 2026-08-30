//! Session-owned symlink bundles for agents without native multi-root flags.

use fs_err as fs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tempfile::Builder;

const PREFIX: &str = "grove-multi-root-";
const ROOT_DIR: &str = "grove";

#[derive(Debug)]
pub struct SymlinkBundle {
    path: PathBuf,
}

impl SymlinkBundle {
    pub fn create(roots: &[String]) -> std::io::Result<Self> {
        let root = grove_temp_root();
        fs::create_dir_all(&root)?;
        let dir = Builder::new().prefix(PREFIX).tempdir_in(root)?;
        let path = dir.keep();
        let mut used = Vec::new();
        for root in roots {
            let base = Path::new(root)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("worktree");
            let name = unique_name(base, &mut used);
            let link = path.join(&name);
            if let Err(error) = symlink_dir(root, &link) {
                cleanup_path(&path);
                return Err(error);
            }
        }
        Ok(Self { path })
    }

    pub fn from_path(path: PathBuf) -> Option<Self> {
        is_owned_path(&path).then_some(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for SymlinkBundle {
    fn drop(&mut self) {
        cleanup_path(&self.path);
    }
}

pub fn cleanup_path(path: &Path) {
    if !is_owned_path(path) {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if let Err(error) = fs::remove_file(&entry_path) {
            tracing::debug!(?entry_path, %error, "multi-root: failed to remove symlink");
        }
    }
    if let Err(error) = fs::remove_dir(path) {
        tracing::debug!(?path, %error, "multi-root: failed to remove bundle");
    }
}

/// Removes abandoned bundle directories without traversing or deleting anything outside them.
/// Paths in `active` are preserved exactly as supplied.
pub fn cleanup_orphaned(active: &[PathBuf]) -> usize {
    cleanup_orphaned_in(&grove_temp_root(), active)
}

fn cleanup_orphaned_in(root: &Path, active: &[PathBuf]) -> usize {
    let active = active.iter().collect::<HashSet<_>>();
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !is_owned_path_in(&path, root)
            || active.contains(&path)
        {
            continue;
        }
        cleanup_path_in(&path, root);
        if !path.exists() {
            removed += 1;
        }
    }
    removed
}

fn is_owned_path(path: &Path) -> bool {
    is_owned_path_in(path, &grove_temp_root())
}

fn is_owned_path_in(path: &Path, root: &Path) -> bool {
    path.parent() == Some(root)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX))
}

fn cleanup_path_in(path: &Path, root: &Path) {
    if !is_owned_path_in(path, root) {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let _ = fs::remove_file(&entry_path);
    }
    let _ = fs::remove_dir(path);
}

fn grove_temp_root() -> PathBuf {
    std::env::temp_dir().join(ROOT_DIR)
}

fn unique_name(base: &str, used: &mut Vec<String>) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while used.iter().any(|existing| existing == &name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    used.push(name.clone());
    name
}

#[cfg(unix)]
fn symlink_dir(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable_and_collision_safe() {
        let mut used = Vec::new();
        assert_eq!(unique_name("repo", &mut used), "repo");
        assert_eq!(unique_name("repo", &mut used), "repo-2");
        assert_eq!(unique_name("repo", &mut used), "repo-3");
    }

    #[test]
    fn bundle_links_and_cleanup_do_not_touch_targets() {
        let first = tempfile::tempdir().expect("first root");
        let second = tempfile::tempdir().expect("second root");
        let first_path = first.path().to_string_lossy().into_owned();
        let second_path = second.path().to_string_lossy().into_owned();
        let bundle =
            SymlinkBundle::create(&[first_path.clone(), second_path.clone()]).expect("bundle");
        let bundle_path = bundle.path().to_path_buf();
        assert!(bundle_path.join(first.path().file_name().unwrap()).exists());
        drop(bundle);
        assert!(!bundle_path.exists());
        assert!(first.path().exists());
        assert!(second.path().exists());
    }

    #[test]
    fn cleanup_ignores_paths_outside_grove_temp_root() {
        let target = tempfile::tempdir().expect("target");
        let path = target.path().to_path_buf();
        cleanup_path(&path);
        assert!(path.exists());
    }

    #[test]
    fn orphan_sweep_removes_orphans_preserves_active_and_ignores_unrelated() {
        let root = tempfile::tempdir().expect("root");
        let orphan = root.path().join(format!("{PREFIX}orphan"));
        let active = root.path().join(format!("{PREFIX}active"));
        let unrelated = root.path().join("unrelated");
        fs::create_dir(&orphan).unwrap();
        fs::create_dir(&active).unwrap();
        fs::create_dir(&unrelated).unwrap();
        let count = cleanup_orphaned_in(root.path(), std::slice::from_ref(&active));
        assert_eq!(count, 1);
        assert!(!orphan.exists());
        assert!(active.exists());
        assert!(unrelated.exists());
    }
}
