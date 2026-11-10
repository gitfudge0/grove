//! Bounded, noninteractive Git operations for the checkout context card.
use std::io::Write as _;
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

impl CheckoutGitState {
    /// Identifies the branch, commit and remote configuration without exposing remote URLs.
    pub fn target_identity(&self) -> [u8; 32] {
        let mut bytes = self.configuration_identity.to_vec();
        for value in [&self.branch, &self.head, &self.upstream] {
            bytes.extend_from_slice(value.as_deref().unwrap_or_default().as_bytes());
            bytes.push(0);
        }
        digest(&bytes)
    }
}

/// A comparison against a specific fetched remote default-branch commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainBranchState {
    pub remote: String,
    pub branch: String,
    pub head: String,
    pub behind: usize,
    checkout_identity: [u8; 32],
    comparison_identity: [u8; 32],
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

/// Return only app-authored copy. Raw stderr may contain remote credentials,
/// hook secrets and terminal escapes and must never reach a display or clipboard.
fn git_failure_message(args: &[&str], detail: &str) -> &'static str {
    let detail = detail.to_ascii_lowercase();
    if detail.contains("author identity unknown") || detail.contains("unable to auto-detect email")
    {
        "Configure your Git user.name and user.email before committing"
    } else if detail.contains("gpg failed")
        || detail.contains("failed to sign")
        || detail.contains("signing failed")
    {
        "Commit signing failed. Check your Git signing configuration and retry"
    } else if detail.contains("conflict") && matches!(args.first(), Some(&"merge" | &"revert")) {
        "The operation has conflicts. Resolve or abort it in the checkout before continuing"
    } else if args.first() == Some(&"commit") && !detail.contains("authentication") {
        "Commit failed. Check your commit hooks, signing setup, and staged changes, then retry"
    } else if detail.contains("authentication")
        || detail.contains("permission denied")
        || detail.contains("could not read username")
        || detail.contains("repository not found")
    {
        "Git access failed. Check your authentication and repository permissions"
    } else if args.first() == Some(&"pull")
        && (detail.contains("not possible to fast-forward") || detail.contains("divergent"))
    {
        "Branches have diverged. Resolve the branch divergence before pulling again"
    } else if args.first() == Some(&"push") && detail.contains("rejected") {
        "Push was rejected. Review remote changes and repository push policies before retrying"
    } else if detail.contains("could not resolve")
        || detail.contains("unable to access")
        || detail.contains("connection")
    {
        "Could not reach the remote. Check your network connection and retry"
    } else {
        "Git operation failed. Check repository access and refresh before retrying"
    }
}

pub(crate) fn git(path: &Path, args: &[&str]) -> Result<String, String> {
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
    if matches!(
        args.first(),
        Some(&"fetch" | &"push" | &"pull" | &"ls-remote")
    ) {
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
        .env("LC_ALL", "C")
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
                    let message = git_failure_message(args, &detail);
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

fn checked_target(path: &Path, expected: &CheckoutGitState) -> Result<CheckoutGitState, String> {
    let current = inspect(path)?;
    if current.target_identity() != expected.target_identity() {
        return Err("Checkout changed. Refresh before retrying".into());
    }
    Ok(current)
}

fn comparison_remote(path: &Path, state: &CheckoutGitState) -> Result<Option<String>, String> {
    if state.remotes.is_empty() {
        return Ok(None);
    }
    if let Some(branch) = &state.branch {
        if let Ok(remote) = git(
            path,
            &["config", "--get", &format!("branch.{branch}.remote")],
        ) {
            if state.remotes.contains(&remote) {
                return Ok(Some(remote));
            }
            return Err("The branch tracking remote is unavailable".into());
        }
    }
    if state.remotes.iter().any(|remote| remote == "origin") {
        return Ok(Some("origin".into()));
    }
    if state.remotes.len() == 1 {
        return Ok(state.remotes.first().cloned());
    }
    Err("Choose a branch tracking remote before comparing main".into())
}

/// Refresh only this checkout's selected remote tracking refs and compare its default branch.
/// Fetching changes remote-tracking refs; it never changes a local branch or working files.
/// With `fetch = false`, the comparison is cached and does not establish remote freshness.
pub fn inspect_main(
    path: &Path,
    expected: &CheckoutGitState,
    fetch: bool,
) -> Result<Option<MainBranchState>, String> {
    let current = checked_target(path, expected)?;
    let Some(local_head) = current.head.as_deref() else {
        return Ok(None);
    };
    let Some(remote) = comparison_remote(path, &current)? else {
        return Ok(None);
    };
    let branch = if fetch {
        let advertised = git(
            path,
            &[
                "ls-remote",
                "--symref",
                "--",
                &remote,
                "HEAD",
                "refs/heads/main",
                "refs/heads/master",
            ],
        )?;
        let default = advertised.lines().find_map(|line| {
            let (value, name) = line.split_once('\t')?;
            (name == "HEAD")
                .then(|| value.strip_prefix("ref: refs/heads/").map(str::to_owned))
                .flatten()
        });
        default.or_else(|| {
            ["main", "master"]
                .into_iter()
                .find(|branch| {
                    advertised.lines().any(|line| {
                        line.split_once('\t')
                            .is_some_and(|(_, name)| name == format!("refs/heads/{branch}"))
                    })
                })
                .map(str::to_owned)
        })
    } else {
        git(
            path,
            &[
                "symbolic-ref",
                "--quiet",
                &format!("refs/remotes/{remote}/HEAD"),
            ],
        )
        .ok()
        .and_then(|reference| {
            reference
                .strip_prefix(&format!("refs/remotes/{remote}/"))
                .map(str::to_owned)
        })
        .or_else(|| {
            ["main", "master"]
                .into_iter()
                .find(|branch| {
                    git(
                        path,
                        &[
                            "rev-parse",
                            "--verify",
                            &format!("refs/remotes/{remote}/{branch}^{{commit}}"),
                        ],
                    )
                    .is_ok()
                })
                .map(str::to_owned)
        })
    };
    if let Some(branch) = &branch {
        git(path, &["check-ref-format", &format!("refs/heads/{branch}")])?;
    }
    if fetch {
        // Explicit destinations prevent a remote's arbitrary fetch refspec from changing
        // local heads. Refresh the configured upstream alongside the default branch.
        let mut refspecs = Vec::new();
        if let Some(branch) = &branch {
            refspecs.push(format!(
                "+refs/heads/{branch}:refs/remotes/{remote}/{branch}"
            ));
        }
        if let Some(local_branch) = &current.branch {
            if current.upstream.is_some() {
                let merge = git(
                    path,
                    &["config", "--get", &format!("branch.{local_branch}.merge")],
                )?;
                let destination = git(path, &["rev-parse", "--symbolic-full-name", "@{upstream}"])?;
                if !merge.starts_with("refs/heads/")
                    || !destination.starts_with(&format!("refs/remotes/{remote}/"))
                {
                    return Err(
                        "The branch tracking configuration cannot be refreshed safely".into(),
                    );
                }
                refspecs.push(format!("+{merge}:{destination}"));
            }
        }
        refspecs.sort();
        refspecs.dedup();
        if !refspecs.is_empty() {
            let mut args = vec![
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--",
                remote.as_str(),
            ];
            args.extend(refspecs.iter().map(String::as_str));
            git(path, &args)?;
        }
        checked_target(path, expected)?;
    }
    let Some(branch) = branch else {
        return Ok(None);
    };
    let head = git(
        path,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/{remote}/{branch}^{{commit}}"),
        ],
    )?;
    let behind = git(
        path,
        &["rev-list", "--count", &format!("{local_head}..{head}")],
    )?
    .parse()
    .map_err(|_| "Could not compare the remote default branch")?;
    checked_target(path, expected)?;
    let comparison_identity = digest(format!("{remote}\0{branch}\0{head}").as_bytes());
    Ok(Some(MainBranchState {
        remote,
        branch,
        head,
        behind,
        checkout_identity: expected.target_identity(),
        comparison_identity,
    }))
}

/// Merge exactly the reviewed default-branch commit. No rebase, reset or implicit pull.
pub fn merge_main(
    path: &Path,
    expected: &CheckoutGitState,
    main: &MainBranchState,
) -> Result<(), String> {
    let current = checked_target(path, expected)?;
    if current.dirty || current.branch.is_none() || current.head.is_none() {
        return Err("Commit or discard local edits on a branch before merging main".into());
    }
    if main.comparison_identity
        != digest(format!("{}\0{}\0{}", main.remote, main.branch, main.head).as_bytes())
        || main.checkout_identity != current.target_identity()
        || comparison_remote(path, &current)?.as_ref() != Some(&main.remote)
        || git(
            path,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{}/{}^{{commit}}", main.remote, main.branch),
            ],
        )? != main.head
    {
        return Err("The main comparison changed. Review it again before merging".into());
    }
    if main.behind == 0 {
        return Err("There are no new commits on main to merge".into());
    }
    if checked_target(path, expected)?.dirty {
        return Err("Checkout changed. Review local edits before merging main".into());
    }
    git(path, &["merge", "--no-ff", "--no-edit", "--", &main.head])?;
    Ok(())
}

/// Return the exact tracked discard scope for an explicitly captured working snapshot.
/// The caller can show these names in confirmation; untracked files are excluded.
pub fn discard_tracked_paths(
    path: &Path,
    expected: &CheckoutGitState,
) -> Result<Vec<String>, String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    let current = checked_target(path, expected)?;
    let Some(head) = current.head.as_deref() else {
        return Err("Create a commit before discarding tracked changes".into());
    };
    if expected.working_identity.is_none()
        || expected.working_identity != Some(working_snapshot(path)?)
        || current.staged_identity != expected.staged_identity
    {
        return Err("Checkout changes changed. Review them before discarding".into());
    }
    let mut tracked = nul_files(&git(path, &["ls-files", "-z"])?);
    tracked.extend(nul_files(&git(
        path,
        &["ls-tree", "-r", "--name-only", "-z", head],
    )?));
    let paths: Vec<_> = current
        .changed_files
        .into_iter()
        .filter(|name| tracked.contains(name))
        .collect();
    if paths.is_empty() {
        return Err("There are no tracked changes to discard".into());
    }
    Ok(paths)
}

/// Discard only tracked paths from an explicitly captured working snapshot.
/// Files which were untracked in the snapshot are never removed or restored.
pub fn discard_tracked(path: &Path, expected: &CheckoutGitState) -> Result<(), String> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let path = Path::new(&root);
    let current = checked_target(path, expected)?;
    let head = current
        .head
        .as_deref()
        .ok_or("Create a commit before discarding tracked changes")?;
    let paths = discard_tracked_paths(path, expected)?;
    let mut pathspec =
        tempfile::NamedTempFile::new().map_err(|_| "Could not prepare discard scope")?;
    for name in paths {
        pathspec
            .write_all(format!(":(top,literal){name}\0").as_bytes())
            .map_err(|_| "Could not prepare discard scope")?;
    }
    // Re-read the captured scope immediately before the one native restore operation.
    let latest = checked_target(path, expected)?;
    if latest.staged_identity != expected.staged_identity
        || expected.working_identity != Some(working_snapshot(path)?)
    {
        return Err("Checkout changes changed. Review them before discarding".into());
    }
    let source = format!("--source={head}");
    let paths = format!("--pathspec-from-file={}", pathspec.path().display());
    git(
        path,
        &[
            "restore",
            &source,
            "--staged",
            "--worktree",
            &paths,
            "--pathspec-file-nul",
        ],
    )?;
    Ok(())
}

/// Revert one exact reachable non-merge commit by creating a new inverse commit.
pub fn revert_commit(path: &Path, expected: &CheckoutGitState, commit: &str) -> Result<(), String> {
    let current = checked_target(path, expected)?;
    if current.dirty || current.branch.is_none() || current.head.is_none() {
        return Err("Use a clean checkout on a branch before reverting a commit".into());
    }
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Choose an exact commit from this branch's history".into());
    }
    let resolved = git(
        path,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )?;
    if resolved != commit.to_ascii_lowercase() {
        return Err("The selected commit changed. Review history again".into());
    }
    git(path, &["merge-base", "--is-ancestor", commit, "HEAD"])
        .map_err(|_| "Choose a commit from this branch's history")?;
    let parents = git(path, &["rev-list", "--parents", "-n", "1", commit])?;
    if parents.split_whitespace().count() > 2 {
        return Err("Merge commits require a parent choice and cannot be reverted here".into());
    }
    if checked_target(path, expected)?.dirty {
        return Err("Checkout changed. Review local edits before reverting".into());
    }
    git(path, &["revert", "--no-edit", "--", commit])?;
    Ok(())
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
    fn failures_report_only_known_causes_and_never_raw_diagnostics() {
        assert_eq!(
            git_failure_message(&["push"], "remote: hook declined\n! [remote rejected]"),
            "Push was rejected. Review remote changes and repository push policies before retrying"
        );
        for diagnostic in [
            "fatal: unknown failure https://secret@example.test/repo?token=secret",
            "fatal: rejected",
            "\u{1b}[31mprivate hook output",
        ] {
            let message = git_failure_message(&["pull"], diagnostic);
            assert!(!message.contains("diverged"));
            assert!(!message.contains("secret"));
            assert!(!message.contains('\u{1b}'));
        }
        assert!(
            git_failure_message(&["pull"], "fatal: Authentication failed")
                .contains("authentication")
        );
        assert!(
            git_failure_message(&["pull"], "fatal: Could not resolve host").contains("network")
        );
        assert!(git_failure_message(&["merge"], "CONFLICT (content)").contains("conflicts"));
        assert!(
            git_failure_message(&["commit"], "gpg failed to sign the data").contains("signing")
        );
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
        let diverged =
            execute(dir.path(), &inspect(dir.path()).unwrap(), GitAction::Pull).unwrap_err();
        assert_eq!(
            diverged,
            "Branches have diverged. Resolve the branch divergence before pulling again"
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

    fn remote_fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let checkout = fixture();
        fs_err::write(checkout.path().join("tracked"), "base\n").unwrap();
        commit(checkout.path());
        let remote = tempfile::tempdir().unwrap();
        run(remote.path(), &["init", "--bare", "--initial-branch=main"]);
        run(
            checkout.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        run(
            checkout.path(),
            &["push", "--set-upstream", "origin", "main"],
        );
        (checkout, remote)
    }

    #[test]
    fn main_comparison_fetches_default_and_upstream_without_integrating() {
        let (checkout, remote) = remote_fixture();
        run(checkout.path(), &["checkout", "-b", "feature"]);
        fs_err::write(checkout.path().join("feature"), "feature\n").unwrap();
        commit(checkout.path());
        run(
            checkout.path(),
            &["push", "--set-upstream", "origin", "feature"],
        );
        let peer = fixture();
        run(
            peer.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        run(peer.path(), &["fetch", "origin"]);
        run(peer.path(), &["reset", "--hard", "origin/main"]);
        fs_err::write(peer.path().join("main-change"), "main\n").unwrap();
        commit(peer.path());
        run(peer.path(), &["push", "origin", "main"]);
        run(
            peer.path(),
            &["checkout", "-b", "feature", "origin/feature"],
        );
        fs_err::write(peer.path().join("peer-feature"), "peer\n").unwrap();
        commit(peer.path());
        run(peer.path(), &["push", "origin", "feature"]);
        let expected = inspect(checkout.path()).unwrap();
        let before_head = expected.head.clone();
        let comparison = inspect_main(checkout.path(), &expected, true)
            .unwrap()
            .unwrap();
        assert_eq!(comparison.remote, "origin");
        assert_eq!(comparison.branch, "main");
        assert_eq!(comparison.behind, 1);
        let refreshed = inspect(checkout.path()).unwrap();
        assert_eq!(refreshed.head, before_head);
        assert_eq!(refreshed.behind, Some(1));
        assert!(!checkout.path().join("main-change").exists());
        assert!(!checkout.path().join("peer-feature").exists());
        merge_main(checkout.path(), &refreshed, &comparison).unwrap();
        assert!(checkout.path().join("main-change").exists());
        assert!(!checkout.path().join("peer-feature").exists());
        assert_ne!(inspect(checkout.path()).unwrap().head, before_head);
    }

    #[test]
    fn main_comparison_rejects_stale_target_and_main_snapshot() {
        let (checkout, remote) = remote_fixture();
        run(checkout.path(), &["checkout", "-b", "feature"]);
        let expected = inspect(checkout.path()).unwrap();
        let comparison = inspect_main(checkout.path(), &expected, true)
            .unwrap()
            .unwrap();
        run(checkout.path(), &["checkout", "main"]);
        assert!(inspect_main(checkout.path(), &expected, false).is_err());
        assert!(merge_main(checkout.path(), &expected, &comparison).is_err());
        run(checkout.path(), &["checkout", "feature"]);
        let peer = fixture();
        run(
            peer.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        run(peer.path(), &["fetch", "origin"]);
        run(peer.path(), &["reset", "--hard", "origin/main"]);
        fs_err::write(peer.path().join("new"), "new\n").unwrap();
        commit(peer.path());
        run(peer.path(), &["push", "origin", "main"]);
        let comparison = inspect_main(checkout.path(), &expected, true)
            .unwrap()
            .unwrap();
        run(
            checkout.path(),
            &[
                "update-ref",
                "refs/remotes/origin/main",
                expected.head.as_deref().unwrap(),
            ],
        );
        assert!(merge_main(checkout.path(), &expected, &comparison)
            .unwrap_err()
            .contains("comparison changed"));
    }

    #[test]
    fn main_comparison_prefers_tracking_remote_and_does_not_infer_missing_main() {
        let (checkout, remote) = remote_fixture();
        run(checkout.path(), &["remote", "rename", "origin", "upstream"]);
        let unrelated = tempfile::tempdir().unwrap();
        run(
            unrelated.path(),
            &["init", "--bare", "--initial-branch=main"],
        );
        run(
            checkout.path(),
            &[
                "remote",
                "add",
                "origin",
                unrelated.path().to_str().unwrap(),
            ],
        );
        let state = inspect(checkout.path()).unwrap();
        let main = inspect_main(checkout.path(), &state, true)
            .unwrap()
            .unwrap();
        assert_eq!(main.remote, "upstream");
        run(
            checkout.path(),
            &["remote", "set-url", "upstream", "/does-not-exist"],
        );
        assert!(inspect_main(checkout.path(), &state, true).is_err());
        // Keep the temporary remote alive until comparison has finished.
        drop(remote);
    }

    #[test]
    fn discard_tracked_rejects_stale_scope_and_preserves_untracked_files() {
        let checkout = fixture();
        fs_err::write(checkout.path().join("tracked"), "base\n").unwrap();
        fs_err::write(checkout.path().join("deleted"), "base\n").unwrap();
        commit(checkout.path());
        fs_err::write(checkout.path().join("tracked"), "staged\n").unwrap();
        run(checkout.path(), &["add", "tracked"]);
        fs_err::write(checkout.path().join("tracked"), "working\n").unwrap();
        fs_err::remove_file(checkout.path().join("deleted")).unwrap();
        fs_err::write(checkout.path().join("untracked"), "keep me\n").unwrap();
        fs_err::write(checkout.path().join(":(glob)added"), "new staged file\n").unwrap();
        run(checkout.path(), &["add", "--", ":(literal):(glob)added"]);
        let stale = prepare_commit(checkout.path()).unwrap();
        fs_err::write(checkout.path().join("tracked"), "newer\n").unwrap();
        assert!(discard_tracked(checkout.path(), &stale).is_err());
        assert_eq!(
            fs_err::read_to_string(checkout.path().join("tracked")).unwrap(),
            "newer\n"
        );
        let expected = prepare_commit(checkout.path()).unwrap();
        assert_eq!(
            discard_tracked_paths(checkout.path(), &expected).unwrap(),
            vec![":(glob)added", "deleted", "tracked"]
        );
        let nested = checkout.path().join("nested");
        fs_err::create_dir(&nested).unwrap();
        discard_tracked(&nested, &expected).unwrap();
        assert_eq!(
            fs_err::read_to_string(checkout.path().join("tracked")).unwrap(),
            "base\n"
        );
        assert_eq!(
            fs_err::read_to_string(checkout.path().join("deleted")).unwrap(),
            "base\n"
        );
        assert_eq!(
            fs_err::read_to_string(checkout.path().join("untracked")).unwrap(),
            "keep me\n"
        );
        assert!(!checkout.path().join(":(glob)added").exists());
        assert!(git(checkout.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert_eq!(
            inspect(checkout.path()).unwrap().changed_files,
            vec!["untracked"]
        );
        assert!(discard_tracked(checkout.path(), &inspect(checkout.path()).unwrap()).is_err());
    }

    #[test]
    fn revert_creates_inverse_commit_and_rejects_dirty_or_unreachable_scope() {
        let checkout = fixture();
        fs_err::write(checkout.path().join("tracked"), "base\n").unwrap();
        commit(checkout.path());
        fs_err::write(checkout.path().join("tracked"), "feature\n").unwrap();
        commit(checkout.path());
        let expected = inspect(checkout.path()).unwrap();
        let selected = expected.head.clone().unwrap();
        fs_err::write(checkout.path().join("untracked"), "keep\n").unwrap();
        assert!(revert_commit(checkout.path(), &expected, &selected).is_err());
        fs_err::remove_file(checkout.path().join("untracked")).unwrap();
        assert!(revert_commit(checkout.path(), &expected, "HEAD").is_err());
        let tree = git(checkout.path(), &["rev-parse", "HEAD^{tree}"]).unwrap();
        let unreachable = git(
            checkout.path(),
            &["commit-tree", &tree, "-m", "unreachable"],
        )
        .unwrap();
        assert!(revert_commit(checkout.path(), &expected, &unreachable)
            .unwrap_err()
            .contains("history"));
        revert_commit(checkout.path(), &expected, &selected).unwrap();
        assert_eq!(
            fs_err::read_to_string(checkout.path().join("tracked")).unwrap(),
            "base\n"
        );
        assert_eq!(
            git(checkout.path(), &["rev-parse", "HEAD^"]).unwrap(),
            selected
        );
        assert!(git(checkout.path(), &["log", "-1", "--format=%s"])
            .unwrap()
            .starts_with("Revert"));
        assert!(revert_commit(checkout.path(), &expected, &selected).is_err());
    }
}
