//! Exercises `grove_core::git` against a genuine `git` subprocess and real
//! throwaway repositories on disk. The unit tests inside `src/git.rs`
//! stub nothing but structurally cannot reach this boundary: every function
//! here shells out to a real `git` binary and touches a real working tree,
//! neither of which a `#[test]` running in-process without a repo fixture
//! can observe. These tests build disposable repos under `tempfile::TempDir`
//! and drive the public API against them.
//!
//! Every `git` invocation used to *set up* a fixture repo is intentionally
//! hermetic (explicit `-c user.name`/`-c user.email`/`-c commit.gpgsign` and
//! `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM`/`GIT_TERMINAL_PROMPT` overrides) so
//! these tests never read the developer's global git config or identity.
//! If `git` is not on `PATH` at all, every test skips cleanly via an early
//! `return` with an explanatory `eprintln!` rather than failing the suite.

use fs_err as fs;
use grove_core::git::{
    add_worktree, current_branch, list_worktrees, remove_worktree, worktree_git_state,
    worktrees_root,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// True when a `git` binary is reachable on `PATH`. Tests skip (not fail)
/// when this is false, since a stock CI/dev box without git is a valid
/// (if unusual) environment for this crate to be tested in otherwise.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Builds a `git` `Command` for fixture setup only — never for exercising
/// `grove_core::git` itself (that always shells out to a bare `git` with no
/// special environment, matching what the app does in production). Pins
/// identity/signing config explicitly and blanks the global/system config
/// files so these tests can never read or depend on the developer's real
/// git configuration.
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

/// Initializes `dir` as a real git repo with a single commit on `main`.
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

/// RAII cleanup for the real `~/.config/grove/worktrees/<project>` directory
/// that `add_worktree` unavoidably writes into (`worktrees_root()` is a fixed
/// location under the user's home, not something the public API lets a
/// caller override). Removes it on drop regardless of whether the test
/// panics, so a failing assertion never leaves residue behind on the
/// developer's real machine.
struct WorktreeRootGuard(PathBuf);

impl Drop for WorktreeRootGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A freshly-initialized repo with a single commit and no linked worktrees
/// must list exactly the main checkout, marked `is_main`.
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
    assert_eq!(worktrees[0].path, repo_str);
}

/// Pointing `list_worktrees` at a directory that is not a git repository
/// must never panic — it degrades to a synthetic single-entry root listing
/// so the caller still has a row to render.
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

/// End-to-end: `add_worktree` on a real repo makes a new worktree appear in
/// `list_worktrees`'s output with the expected branch name, and
/// `remove_worktree` makes it disappear again.
#[test]
fn add_worktree_appears_in_listing_and_remove_worktree_makes_it_disappear() {
    if !git_available() {
        eprintln!("git not found on PATH; skipping add_worktree_appears_in_listing_and_remove_worktree_makes_it_disappear");
        return;
    }
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo_with_commit(repo.path());
    let repo_str = repo.path().to_str().expect("utf8 path");

    // Unique per-run project name so concurrent/repeated test runs never
    // collide on the shared, real `~/.config/grove/worktrees` root.
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

    let dest = add_worktree(repo_str, &project_name, "feature-x").expect("add_worktree");

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

/// `remove_worktree` must return `Err` (not panic) when pointed at a
/// directory that is not a git repository at all.
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
    // wt_path must differ from project_path so the fast in-process guard
    // (which never shells out) is bypassed and the real git subprocess path
    // is exercised instead.
    let bogus_wt = dir.path().join("not-a-worktree");

    let result = remove_worktree(dir_str, bogus_wt.to_str().expect("utf8 path"));

    assert!(
        result.is_err(),
        "removing a worktree from a non-git directory must return Err, not panic"
    );
}

/// `worktree_git_state` must report clean on a freshly committed repo and
/// dirty once a tracked file is modified — the real `git status` boundary
/// that no unit test can reach without a live repo.
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

/// `current_branch` must report the branch actually checked out in the
/// fixture repo (pinned to `main` via `init.defaultBranch=main`).
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
