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
    configuration_identity: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitAction {
    Fetch,
    Pull,
    Push,
    Publish { remote: String },
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = tempfile::tempfile().map_err(|_| "Could not prepare Git operation".to_string())?;
    let errors = tempfile::tempfile().map_err(|_| "Could not prepare Git operation".to_string())?;
    let mut command = Command::new("git");
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
                    let message = if detail.contains("authentication")
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

pub fn inspect(path: &Path) -> Result<CheckoutGitState, String> {
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
    Ok(CheckoutGitState {
        configuration_identity,
        branch,
        head,
        upstream,
        remotes,
        ahead: counts.map(|v| v.0),
        behind: counts.map(|v| v.1),
        dirty: !git(path, &["status", "--porcelain", "--untracked-files=normal"])?.is_empty(),
    })
}

pub fn execute(path: &Path, expected: &CheckoutGitState, action: GitAction) -> Result<(), String> {
    let current = inspect(path)?;
    if current.configuration_identity != expected.configuration_identity
        || current.branch != expected.branch
        || current.head != expected.head
        || current.upstream != expected.upstream
        || current.remotes != expected.remotes
    {
        return Err("Checkout changed. Refresh before retrying".into());
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
        GitAction::Fetch => unreachable!(),
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
