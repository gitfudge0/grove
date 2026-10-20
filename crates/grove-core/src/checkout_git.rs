//! Bounded, noninteractive Git operations for the checkout context card.
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutGitState {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub remotes: Vec<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub dirty: bool,
    pub staged_files: Vec<String>,
    pub changed_files: Vec<String>,
    staged_identity: [u8; 32],
    working_identity: Option<[u8; 32]>,
    configuration_identity: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitAction {
    Fetch,
    Commit { message: String },
    CommitAll { message: String },
    Pull,
    Push,
    Publish { remote: String },
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    git_with_index(path, args, None)
}

fn git_with_index(path: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    git_with_index_limit(path, args, index, None)
}

fn git_with_index_limit(
    path: &Path,
    args: &[&str],
    index: Option<&Path>,
    limit: Option<usize>,
) -> Result<String, String> {
    let output = tempfile::tempfile().map_err(|_| "Could not prepare Git operation".to_string())?;
    let errors = tempfile::tempfile().map_err(|_| "Could not prepare Git operation".to_string())?;
    let mut command = Command::new("git");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    if matches!(args.first(), Some(&"fetch" | &"push" | &"pull")) {
        let configured = std::env::var("GIT_SSH_COMMAND")
            .ok()
            .or_else(|| git(path, &["config", "--get", "core.sshCommand"]).ok());
        if configured.is_none() {
            command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15");
        }
    }
    command
        .arg("-C")
        .arg(path)
        .args(["-c", "credential.interactive=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            output
                .try_clone()
                .map_err(|_| "Could not prepare Git output")?,
        ))
        .stderr(Stdio::from(
            errors
                .try_clone()
                .map_err(|_| "Could not prepare Git output")?,
        ));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Git. Check that Git is installed".to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut errors = errors;
                    let _ = errors.seek(SeekFrom::Start(0));
                    let mut detail = String::new();
                    let _ = errors.take(65536).read_to_string(&mut detail);
                    let detail = detail.to_ascii_lowercase();
                    let message = if detail.contains("author identity unknown")
                        || detail.contains("unable to auto-detect email")
                    {
                        "Configure your Git user.name and user.email before committing"
                    } else if detail.contains("gpg failed")
                        || detail.contains("failed to sign")
                        || detail.contains("signing failed")
                    {
                        "Commit signing failed. Check your Git signing configuration and retry"
                    } else if args.first() == Some(&"commit") && !detail.contains("authentication")
                    {
                        "Commit failed. Check your commit hooks, signing setup, and staged changes, then retry"
                    } else if detail.contains("authentication")
                        || detail.contains("permission denied")
                        || detail.contains("could not read username")
                        || detail.contains("repository not found")
                    {
                        "Git access failed. Check your authentication and repository permissions"
                    } else if detail.contains("rejected")
                        || detail.contains("non-fast-forward")
                        || detail.contains("not possible to fast-forward")
                        || detail.contains("divergent")
                    {
                        "Branches have diverged or the push was rejected. Fetch and resolve changes before retrying"
                    } else if detail.contains("could not resolve")
                        || detail.contains("unable to access")
                        || detail.contains("connection")
                    {
                        "Could not reach the remote. Check your network connection and retry"
                    } else {
                        "Git operation failed. Check repository access and refresh before retrying"
                    };
                    return Err(message.into());
                }
                let mut output: File = output;
                output
                    .seek(SeekFrom::Start(0))
                    .map_err(|_| "Could not read Git output")?;
                if let Some(limit) = limit {
                    let mut bytes = Vec::new();
                    output
                        .take((limit + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "Could not read Git output")?;
                    let truncated = bytes.len() > limit;
                    bytes.truncate(limit);
                    let mut text = String::from_utf8_lossy(&bytes).into_owned();
                    let marker = "\n[context truncated]";
                    if truncated || text.len() > limit {
                        let mut end = limit.saturating_sub(marker.len()).min(text.len());
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.truncate(end);
                        text.push_str(marker);
                    }
                    return Ok(text);
                }
                let mut text = String::new();
                output
                    .read_to_string(&mut text)
                    .map_err(|_| "Could not read Git output")?;
                return Ok(text.trim_end().to_string());
            }
            Ok(None) if started.elapsed() < Duration::from_secs(30) => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("Git operation timed out. Check connectivity and retry".into());
            }
        }
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut result = [0; 32];
    result.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref());
    result
}

fn nul_files(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn changed_files(path: &Path) -> Result<Vec<String>, String> {
    let mut files = nul_files(&git(
        path,
        &[
            "ls-files",
            "--modified",
            "--deleted",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?);
    files.extend(nul_files(&git(
        path,
        &["diff", "--cached", "--name-only", "--no-renames", "-z"],
    )?));
    files.sort();
    files.dedup();
    Ok(files)
}

fn working_snapshot(path: &Path) -> Result<[u8; 32], String> {
    let files = changed_files(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    for name in files {
        context.update(name.as_bytes());
        context.update(&[0]);
        let file = path.join(&name);
        match fs_err::symlink_metadata(&file) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                context.update(b"symlink");
                context.update(
                    fs_err::read_link(&file)
                        .map_err(|_| "Could not inspect changed file")?
                        .as_os_str()
                        .as_encoded_bytes(),
                );
            }
            Ok(metadata) if metadata.is_file() => {
                context.update(b"file");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    context.update(&metadata.permissions().mode().to_be_bytes());
                }
                // Git streams the file rather than loading large untracked files into memory.
                context.update(git(path, &["hash-object", "--", &name])?.as_bytes());
            }
            Ok(_) => {
                return Err(
                    "A changed path is not a regular file. Review the checkout before committing"
                        .into(),
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                context.update(b"deleted");
            }
            Err(_) => return Err("Could not inspect changed file".into()),
        }
        context.update(&[0]);
    }
    let mut result = [0; 32];
    result.copy_from_slice(context.finish().as_ref());
    Ok(result)
}

/// Capture commit scope only when the commit editor opens; routine polling stays lightweight.
pub fn prepare_commit(path: &Path) -> Result<CheckoutGitState, String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    let mut state = inspect(path)?;
    state.working_identity = Some(working_snapshot(path)?);
    let checked = inspect(path)?;
    if state.head != checked.head
        || state.branch != checked.branch
        || state.staged_identity != checked.staged_identity
        || state.changed_files != checked.changed_files
    {
        return Err("Checkout changed while preparing commit. Review it again".into());
    }
    Ok(state)
}

/// Exact temporary-index scope supplied to the commit-message generator.
/// This contains private repository content and must never be logged.
pub struct CommitContext {
    pub state: CheckoutGitState,
    pub all_changes: bool,
    pub summary: String,
    pub patch: String,
}

pub fn prepare_commit_context(path: &Path) -> Result<CommitContext, String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    let state = prepare_commit(path)?;
    let all_changes = state.staged_files.is_empty();
    if state.changed_files.is_empty() {
        return Err("There are no changes to commit".into());
    }
    let index = path.join(git(path, &["rev-parse", "--git-path", "index"])?);
    let lock_path = index.with_extension("lock");
    let _lock_file = fs_err::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Git index is busy. Finish the other Git operation and retry")?;
    let _lock = IndexLock(lock_path);
    let temporary =
        tempfile::NamedTempFile::new_in(index.parent().ok_or("Could not locate Git index")?)
            .map_err(|_| "Could not prepare commit context")?;
    if index.exists() {
        fs_err::copy(&index, temporary.path()).map_err(|_| "Could not prepare commit context")?;
    } else {
        fs_err::remove_file(temporary.path()).map_err(|_| "Could not prepare commit context")?;
        git_with_index(path, &["read-tree", "--empty"], Some(temporary.path()))?;
    }
    let current = inspect(path)?;
    if current.head != state.head
        || current.branch != state.branch
        || current.staged_identity != state.staged_identity
        || current.configuration_identity != state.configuration_identity
    {
        return Err(
            "Checkout changed while preparing commit context. Retry after reviewing changes".into(),
        );
    }
    if all_changes {
        if Some(working_snapshot(path)?) != state.working_identity {
            return Err(
                "Checkout changed while preparing commit context. Retry after reviewing changes"
                    .into(),
            );
        }
        git_with_index(path, &["add", "-A", "--", ":/"], Some(temporary.path()))?;
        if Some(working_snapshot(path)?) != state.working_identity {
            return Err(
                "Checkout changed while preparing commit context. Retry after reviewing changes"
                    .into(),
            );
        }
    }
    let summary = git_with_index_limit(
        path,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--stat",
            "--no-renames",
        ],
        Some(temporary.path()),
        Some(8_000),
    )?;
    let patch = git_with_index_limit(
        path,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--unified=3",
        ],
        Some(temporary.path()),
        Some(50_000),
    )?;
    if patch.trim().is_empty() {
        return Err("There are no changes to commit".into());
    }
    Ok(CommitContext {
        state,
        all_changes,
        summary,
        patch,
    })
}

fn validate_message(message: &str) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("Enter a commit message".into());
    }
    if message.contains('\0') {
        return Err("Commit message contains an unsupported character".into());
    }
    Ok(())
}

struct IndexLock(std::path::PathBuf);
impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.0);
    }
}

fn commit_snapshot(
    path: &Path,
    expected: &CheckoutGitState,
    message: &str,
    all: bool,
) -> Result<(), String> {
    let index_name = git(path, &["rev-parse", "--git-path", "index"])?;
    let index = path.join(index_name);
    let lock_path = index.with_extension("lock");
    let _lock_file = fs_err::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Git index is busy. Finish the other Git operation and retry")?;
    let _lock = IndexLock(lock_path);
    let parent = index.parent().ok_or("Could not locate Git index")?;
    let temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Could not prepare commit")?;
    if index.exists() {
        fs_err::copy(&index, temporary.path()).map_err(|_| "Could not prepare commit index")?;
    } else {
        fs_err::remove_file(temporary.path()).map_err(|_| "Could not prepare commit index")?;
        git_with_index(path, &["read-tree", "--empty"], Some(temporary.path()))?;
    }
    // Recheck after acquiring the real-index lock, before preparing reviewed scope.
    let current = inspect(path)?;
    if current.head != expected.head
        || current.branch != expected.branch
        || current.configuration_identity != expected.configuration_identity
        || current.staged_identity != expected.staged_identity
    {
        return Err("Checkout changed. Review changes before retrying".into());
    }
    if all {
        if Some(working_snapshot(path)?) != expected.working_identity {
            return Err("Checkout changed. Review changes before retrying".into());
        }
        git_with_index(path, &["add", "-A", "--", ":/"], Some(temporary.path()))?;
        if Some(working_snapshot(path)?) != expected.working_identity {
            return Err(
                "Checkout changed while preparing commit. Review changes before retrying".into(),
            );
        }
    }
    if git(path, &["rev-parse", "--verify", "HEAD"]).ok() != expected.head
        || git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok() != expected.branch
    {
        return Err("Checkout changed before commit. Review changes before retrying".into());
    }
    let result = git_with_index(path, &["commit", "-m", message], Some(temporary.path()));
    if let Err(error) = result {
        // A post-commit hook can fail or time out after Git has already advanced HEAD.
        // Reconcile only a commit whose parent and tree match our isolated snapshot.
        let head = git(path, &["rev-parse", "--verify", "HEAD"]).ok();
        if head != expected.head {
            let parent = git(path, &["rev-parse", "--verify", "HEAD^"]).ok();
            let tree = git(path, &["rev-parse", "HEAD^{tree}"]).ok();
            let snapshot_tree = git_with_index(path, &["write-tree"], Some(temporary.path())).ok();
            if parent == expected.head && tree.is_some() && tree == snapshot_tree {
                temporary.persist(&index).map_err(|_| "Commit created, but Git index could not be updated. Refresh and inspect repository status")?;
                return Err("Commit was created, but post-commit processing failed. Refresh and check repository hooks before retrying".into());
            }
            return Err("Repository HEAD changed during commit. Refresh and inspect the commit history before retrying".into());
        }
        return Err(error);
    }
    temporary.persist(&index).map_err(|_| {
        "Commit created, but Git index could not be updated. Refresh and inspect repository status"
    })?;
    Ok(())
}

pub fn inspect(path: &Path) -> Result<CheckoutGitState, String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    if git(path, &["rev-parse", "--is-inside-work-tree"])? != "true" {
        return Err("This folder is not a Git checkout".into());
    }
    let branch = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let head = git(path, &["rev-parse", "--verify", "HEAD"]).ok();
    let upstream = git(
        path,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok();
    let remotes = git(path, &["remote"])?.lines().map(str::to_owned).collect();
    let counts = upstream
        .as_ref()
        .and_then(|_| {
            git(
                path,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
            .ok()
        })
        .and_then(|s| {
            let mut values = s.split_whitespace();
            Some((values.next()?.parse().ok()?, values.next()?.parse().ok()?))
        });
    // Keep destination and exact tracking configuration out of UI/debug output.
    let config = git(path, &["config", "--get-regexp", "^(remote|branch)\\."]).unwrap_or_default();
    let mut configuration_identity = [0; 32];
    configuration_identity
        .copy_from_slice(ring::digest::digest(&ring::digest::SHA256, config.as_bytes()).as_ref());
    let staged_files = nul_files(&git(
        path,
        &["diff", "--cached", "--name-only", "--no-renames", "-z"],
    )?);
    let staged_identity = digest(git(path, &["ls-files", "--stage", "-z"])?.as_bytes());
    let status = git(path, &["status", "--porcelain", "--untracked-files=normal"])?;
    let changed_files = changed_files(path)?;
    let working_identity = None;
    Ok(CheckoutGitState {
        staged_files,
        changed_files,
        staged_identity,
        working_identity,
        configuration_identity,
        branch,
        head,
        upstream,
        remotes,
        ahead: counts.map(|v| v.0),
        behind: counts.map(|v| v.1),
        dirty: !status.is_empty(),
    })
}

pub fn execute(path: &Path, expected: &CheckoutGitState, action: GitAction) -> Result<(), String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    let current = inspect(path)?;
    if current.configuration_identity != expected.configuration_identity
        || current.branch != expected.branch
        || current.head != expected.head
        || current.upstream != expected.upstream
        || current.remotes != expected.remotes
    {
        return Err("Checkout changed. Refresh before retrying".into());
    }
    if let GitAction::Commit { ref message } | GitAction::CommitAll { ref message } = action {
        validate_message(message)?;
        if expected.working_identity.is_none() {
            return Err("Review commit changes before committing".into());
        }
        if current.branch.is_none() {
            return Err("Switch to a branch before committing".into());
        }
        if matches!(action, GitAction::Commit { .. }) {
            if current.staged_files.is_empty() {
                return Err("Stage changes before committing".into());
            }
            if current.staged_identity != expected.staged_identity {
                return Err("Staged changes changed. Review them before retrying".into());
            }
            commit_snapshot(path, expected, message, false)?;
        } else {
            if current.changed_files.is_empty() {
                return Err("There are no changes to commit".into());
            }
            if Some(working_snapshot(path)?) != expected.working_identity
                || current.staged_identity != expected.staged_identity
            {
                return Err("Checkout changes changed. Review them before retrying".into());
            }
            commit_snapshot(path, expected, message, true)?;
        }
        return Ok(());
    }
    if action == GitAction::Fetch {
        if current.remotes.is_empty() {
            return Err("Add a Git remote before fetching".into());
        }
        for remote in &current.remotes {
            git(path, &["fetch", "--", remote])?;
        }
        return Ok(());
    }
    let branch = current
        .branch
        .as_deref()
        .ok_or("Switch to a branch before syncing")?;
    if current.head.is_none() {
        return Err("Create a commit before syncing this branch".into());
    }
    match action {
        GitAction::Pull | GitAction::Push => {
            if current.upstream.is_none() {
                return Err("Publish this branch before syncing".into());
            }
            let remote = git(
                path,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )?;
            let merge = git(
                path,
                &["config", "--get", &format!("branch.{branch}.merge")],
            )?;
            if !current.remotes.contains(&remote) || !merge.starts_with("refs/heads/") {
                return Err("Upstream is unavailable. Check branch tracking configuration".into());
            }
            if action == GitAction::Pull {
                if current.dirty {
                    return Err("Commit or stash changes before pulling".into());
                }
                git(
                    path,
                    &["pull", "--ff-only", "--no-rebase", "--", &remote, &merge],
                )?;
            } else {
                let refspec = format!("refs/heads/{branch}:{merge}");
                git(path, &["push", "--", &remote, &refspec])?;
            }
        }
        GitAction::Publish { remote } => {
            if current.upstream.is_some() {
                return Err("This branch already has an upstream. Refresh before syncing".into());
            }
            if !current.remotes.contains(&remote) {
                return Err("Selected remote is unavailable. Refresh before retrying".into());
            }
            let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
            git(path, &["push", "--set-upstream", "--", &remote, &refspec])?;
        }
        GitAction::Fetch | GitAction::Commit { .. } | GitAction::CommitAll { .. } => unreachable!(),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    fn run(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "{args:?}"
        );
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        run(dir.path(), &["config", "user.email", "test@example.test"]);
        dir
    }
    fn commit(path: &Path) {
        run(path, &["add", "."]);
        run(path, &["commit", "--allow-empty", "-m", "test"]);
    }
    #[test]
    fn commits_staged_only_and_rejects_invalid_or_changed_scope() {
        let dir = fixture();
        fs_err::write(dir.path().join("file"), "staged\n").unwrap();
        run(dir.path(), &["add", "file"]);
        assert!(inspect(dir.path()).unwrap().working_identity.is_none());
        let staged = prepare_commit(dir.path()).unwrap();
        assert!(staged.working_identity.is_some());
        assert_eq!(staged.staged_files, vec!["file"]);
        for message in ["", "   ", "bad\0message"] {
            assert!(execute(
                dir.path(),
                &staged,
                GitAction::Commit {
                    message: message.into()
                }
            )
            .is_err());
        }
        fs_err::write(dir.path().join("file"), "unstaged\n").unwrap();
        execute(
            dir.path(),
            &staged,
            GitAction::Commit {
                message: "Commit reviewed stage".into(),
            },
        )
        .unwrap();
        assert_eq!(git(dir.path(), &["show", "HEAD:file"]).unwrap(), "staged");
        assert_eq!(
            fs_err::read_to_string(dir.path().join("file")).unwrap(),
            "unstaged\n"
        );
        let empty = prepare_commit(dir.path()).unwrap();
        assert!(execute(
            dir.path(),
            &empty,
            GitAction::Commit {
                message: "No stage".into()
            }
        )
        .is_err());
        run(dir.path(), &["add", "file"]);
        let stale = prepare_commit(dir.path()).unwrap();
        fs_err::write(dir.path().join("file"), "changed stage\n").unwrap();
        run(dir.path(), &["add", "file"]);
        assert!(execute(
            dir.path(),
            &stale,
            GitAction::Commit {
                message: "Stale scope".into()
            }
        )
        .unwrap_err()
        .contains("Staged"));
    }

    #[test]
    fn commit_all_reviews_scope_and_preserves_real_index_on_hook_error() {
        let dir = fixture();
        commit(dir.path());
        fs_err::write(dir.path().join(".gitignore"), "ignored\n").unwrap();
        fs_err::write(dir.path().join("ignored"), "secret").unwrap();
        fs_err::write(dir.path().join("tracked"), "first").unwrap();
        run(dir.path(), &["add", "tracked"]);
        fs_err::write(dir.path().join("tracked"), "latest").unwrap();
        fs_err::write(dir.path().join("untracked"), "included").unwrap();
        let expected = prepare_commit(dir.path()).unwrap();
        assert!(!expected.changed_files.contains(&"ignored".into()));
        fs_err::write(dir.path().join("untracked"), "changed").unwrap();
        assert!(execute(
            dir.path(),
            &expected,
            GitAction::CommitAll {
                message: "Stale".into()
            }
        )
        .is_err());
        let hook = dir.path().join(".git/hooks/pre-commit");
        fs_err::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs_err::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let before = git(dir.path(), &["ls-files", "--stage"]).unwrap();
        let expected = prepare_commit(dir.path()).unwrap();
        assert!(execute(
            dir.path(),
            &expected,
            GitAction::CommitAll {
                message: "Rejected by hook".into()
            }
        )
        .is_err());
        assert_eq!(git(dir.path(), &["ls-files", "--stage"]).unwrap(), before);
        assert_eq!(
            fs_err::read_to_string(dir.path().join("tracked")).unwrap(),
            "latest"
        );
        fs_err::remove_file(hook).unwrap();
        let nested = dir.path().join("nested");
        fs_err::create_dir(&nested).unwrap();
        execute(
            &nested,
            &prepare_commit(&nested).unwrap(),
            GitAction::CommitAll {
                message: "Reviewed all changes".into(),
            },
        )
        .unwrap();
        assert_eq!(
            git(dir.path(), &["show", "HEAD:tracked"]).unwrap(),
            "latest"
        );
        assert_eq!(
            git(dir.path(), &["show", "HEAD:untracked"]).unwrap(),
            "changed"
        );
        assert!(!prepare_commit(dir.path()).unwrap().dirty);
    }
    #[test]
    fn unborn_detached_dirty_and_stale() {
        let dir = fixture();
        let unborn = inspect(dir.path()).unwrap();
        assert_eq!(unborn.branch.as_deref(), Some("main"));
        assert!(unborn.head.is_none());
        assert!(!unborn.dirty);
        commit(dir.path());
        assert!(execute(dir.path(), &unborn, GitAction::Push)
            .unwrap_err()
            .contains("changed"));
        fs_err::write(dir.path().join("file"), "content").unwrap();
        assert!(inspect(dir.path()).unwrap().dirty);
        run(dir.path(), &["checkout", "--detach"]);
        let detached = inspect(dir.path()).unwrap();
        assert!(detached.branch.is_none());
        assert!(execute(dir.path(), &detached, GitAction::Push).is_err());
    }
    #[test]
    fn remote_retarget_and_missing_remote_are_rejected() {
        let dir = fixture();
        commit(dir.path());
        let state = inspect(dir.path()).unwrap();
        assert!(execute(dir.path(), &state, GitAction::Fetch)
            .unwrap_err()
            .contains("remote"));
        let remote = tempfile::tempdir().unwrap();
        run(remote.path(), &["init", "--bare"]);
        run(
            dir.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        let state = inspect(dir.path()).unwrap();
        run(dir.path(), &["remote", "set-url", "origin", "/missing"]);
        assert!(execute(
            dir.path(),
            &state,
            GitAction::Publish {
                remote: "origin".into()
            }
        )
        .unwrap_err()
        .contains("changed"));
    }
    #[test]
    fn local_publish_fetch_pull_and_push() {
        let dir = fixture();
        commit(dir.path());
        let remote = tempfile::tempdir().unwrap();
        run(remote.path(), &["init", "--bare"]);
        run(
            dir.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        execute(
            dir.path(),
            &inspect(dir.path()).unwrap(),
            GitAction::Publish {
                remote: "origin".into(),
            },
        )
        .unwrap();
        let state = inspect(dir.path()).unwrap();
        assert_eq!(state.upstream.as_deref(), Some("origin/main"));
        assert_eq!(state.ahead, Some(0));
        assert_eq!(state.behind, Some(0));
        execute(dir.path(), &state, GitAction::Fetch).unwrap();
        commit(dir.path());
        let ahead = inspect(dir.path()).unwrap();
        assert_eq!(ahead.ahead, Some(1));
        execute(dir.path(), &ahead, GitAction::Push).unwrap();
        let peer = fixture();
        run(
            peer.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        run(peer.path(), &["fetch", "origin"]);
        run(peer.path(), &["reset", "--hard", "origin/main"]);
        fs_err::write(peer.path().join("peer-change"), "peer").unwrap();
        commit(peer.path());
        run(peer.path(), &["push", "origin", "main"]);
        commit(dir.path());
        assert!(
            execute(dir.path(), &inspect(dir.path()).unwrap(), GitAction::Push)
                .unwrap_err()
                .contains("rejected")
        );
        run(dir.path(), &["reset", "--hard", "HEAD~1"]);
        execute(dir.path(), &inspect(dir.path()).unwrap(), GitAction::Fetch).unwrap();
        let behind = inspect(dir.path()).unwrap();
        assert_eq!(behind.behind, Some(1));
        fs_err::write(dir.path().join("dirty"), "x").unwrap();
        assert!(execute(dir.path(), &behind, GitAction::Pull)
            .unwrap_err()
            .contains("stash"));
        assert_eq!(
            fs_err::read_to_string(dir.path().join("dirty")).unwrap(),
            "x"
        );
        fs_err::remove_file(dir.path().join("dirty")).unwrap();
        execute(dir.path(), &inspect(dir.path()).unwrap(), GitAction::Pull).unwrap();
        assert_eq!(inspect(dir.path()).unwrap().behind, Some(0));
    }
}
