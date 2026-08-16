//! Exercises `grove_core::git` against a genuine `git` subprocess and real throwaway
//! repositories under `tempfile::TempDir` — a boundary `src/git.rs`'s unit tests can't reach
//! in-process. Fixture setup is hermetic (explicit user/email/gpgsign config, blanked
//! global/system config) so tests never touch the developer's real git identity. Skips
//! cleanly if `git` isn't on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use fs_err as fs;
use grove_core::git::{
    add_worktree, current_branch, list_worktrees, remove_worktree, worktree_git_state,
    worktree_owner_repo, worktrees_root,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Tests skip, not fail, when this is false.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// For fixture setup only, never for exercising `grove_core::git` itself (which uses a bare git command).
fn fixture_git(repo_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_dir)
        .arg("-c")
        .arg("user.name=grove-test")
        .arg("-c")
        .arg("user.email=grove-test@example.invalid")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .arg("-c")
        .arg("init.defaultBranch=main")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

fn init_repo_with_commit(dir: &Path) {
    let status = fixture_git(dir)
        .args(["init", "-q"])
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init must succeed in fixture setup");

    fs::write(dir.join("README.md"), b"grove test fixture\n").expect("write fixture file");

    let status = fixture_git(dir)
        .args(["add", "."])
        .status()
        .expect("spawn git add");
    assert!(status.success(), "git add must succeed in fixture setup");

    let status = fixture_git(dir)
        .args(["commit", "-q", "-m", "chore: initial commit"])
        .status()
        .expect("spawn git commit");
    assert!(status.success(), "git commit must succeed in fixture setup");
}

/// Cleans up the real `~/.config/grove/worktrees/<project>` dir `add_worktree` writes into, even on panic.
struct WorktreeRootGuard(PathBuf);

impl Drop for WorktreeRootGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn list_worktrees_on_clean_repo_returns_only_main_worktree() {
    if !git_available() {
        eprintln!("git not found on PATH; skipping list_worktrees_on_clean_repo_returns_only_main_worktree");
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    let worktrees = list_worktrees(repo_str);

    assert_eq!(
        worktrees.len(),
        1,
        "a repo with no linked worktrees must list only the main checkout"
    );
    assert!(
        worktrees[0].is_main,
        "the sole entry must be the main worktree"
    );
    // git prints resolved paths (macOS: /var -> /private/var), so compare canonicalized forms.
    assert_eq!(
        fs::canonicalize(&worktrees[0].path).expect("canonicalize listed path"),
        fs::canonicalize(repo_str).expect("canonicalize repo path"),
    );
}

#[test]
fn list_worktrees_on_non_git_directory_returns_synthetic_root_without_panicking() {
    if !git_available() {
        eprintln!("git not found on PATH; skipping list_worktrees_on_non_git_directory_returns_synthetic_root_without_panicking");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_str = dir.path().to_str().expect("utf8 path");

    let worktrees = list_worktrees(dir_str);

    assert_eq!(
        worktrees.len(),
        1,
        "a non-repo directory must still yield exactly one synthetic root entry"
    );
    assert!(worktrees[0].is_main);
    assert_eq!(worktrees[0].path, dir_str);
}

#[test]
fn add_worktree_appears_in_listing_and_remove_worktree_makes_it_disappear() {
    if !git_available() {
        eprintln!("git not found on PATH; skipping add_worktree_appears_in_listing_and_remove_worktree_makes_it_disappear");
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    // Unique per-run name so concurrent test runs never collide on the shared, real worktrees root.
    let project_name = format!(
        "grove-integration-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let root = worktrees_root().expect("worktrees_root");
    let _guard = WorktreeRootGuard(root.join(&project_name));

    let dest = add_worktree(repo_str, &project_name, "feature-x", None).expect("add_worktree");

    let after_add = list_worktrees(repo_str);
    assert!(
        after_add
            .iter()
            .any(|w| w.path == dest && w.branch == "feature-x"),
        "newly added worktree must appear in the listing with its branch name; got {after_add:?}"
    );

    remove_worktree(repo_str, &dest).expect("remove_worktree");

    let after_remove = list_worktrees(repo_str);
    assert!(
        !after_remove.iter().any(|w| w.path == dest),
        "removed worktree must no longer appear in the listing; got {after_remove:?}"
    );
}

#[test]
fn remove_worktree_on_non_git_directory_returns_err() {
    if !git_available() {
        eprintln!(
            "git not found on PATH; skipping remove_worktree_on_non_git_directory_returns_err"
        );
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_str = dir.path().to_str().expect("utf8 path");
    // wt_path must differ from project_path to bypass the fast in-process guard and hit the real git path.
    let bogus_wt = dir.path().join("not-a-worktree");

    let result = remove_worktree(dir_str, bogus_wt.to_str().expect("utf8 path"));

    assert!(
        result.is_err(),
        "removing a worktree from a non-git directory must return Err, not panic"
    );
}

#[test]
fn worktree_git_state_reports_clean_then_dirty_after_modification() {
    if !git_available() {
        eprintln!("git not found on PATH; skipping worktree_git_state_reports_clean_then_dirty_after_modification");
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    let clean = worktree_git_state(repo_str).expect("worktree_git_state on clean repo");
    assert!(!clean.dirty, "freshly committed repo must report clean");

    fs::write(repo.path().join("README.md"), b"modified contents\n").expect("modify file");

    let dirty = worktree_git_state(repo_str).expect("worktree_git_state on dirty repo");
    assert!(
        dirty.dirty,
        "a repo with an uncommitted modification must report dirty"
    );
}

#[test]
fn current_branch_returns_the_checked_out_branch_name() {
    if !git_available() {
        eprintln!(
            "git not found on PATH; skipping current_branch_returns_the_checked_out_branch_name"
        );
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    assert_eq!(current_branch(repo_str), "main");
}

/// Backs `storage::adopt_orphaned_worktree_dirs`'s ownership answer; comes from `git rev-parse --git-common-dir`.
#[test]
fn worktree_owner_repo_reports_the_owning_main_checkout() {
    if !git_available() {
        eprintln!(
            "git not found on PATH; skipping worktree_owner_repo_reports_the_owning_main_checkout"
        );
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    let dir_key = format!(
        "grove-owner-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let root = worktrees_root().expect("worktrees_root");
    let _guard = WorktreeRootGuard(root.join(&dir_key));

    let dest = add_worktree(repo_str, &dir_key, "feature-x", None).expect("add_worktree");

    let owner = worktree_owner_repo(&dest).expect("worktree_owner_repo on a real worktree");
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    assert_eq!(
        canon(&owner),
        canon(repo.path()),
        "the owning repo of a grove-managed worktree must be the main checkout, \
         not the worktree's own directory"
    );

    remove_worktree(repo_str, &dest).expect("remove_worktree");
}

#[test]
fn worktree_owner_repo_on_non_git_directory_returns_none() {
    if !git_available() {
        eprintln!(
            "git not found on PATH; skipping worktree_owner_repo_on_non_git_directory_returns_none"
        );
        return;
    }
    let plain = tempfile::tempdir().expect("tempdir");
    assert!(
        worktree_owner_repo(plain.path().to_str().expect("utf8 path")).is_none(),
        "a plain directory has no owning repository"
    );
}
