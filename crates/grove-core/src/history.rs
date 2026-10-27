//! Read-only, bounded history of the selected checkout's reachable HEAD.
use crate::diff::{FileChange, Patch, Status};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadIdentity {
    Branch { name: String, oid: Option<String> },
    Detached { oid: String },
}
#[derive(Clone, Debug)]
pub struct BranchHistory {
    pub head: HeadIdentity,
    pub commits: Vec<Commit>,
    pub has_more: bool,
}
#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: i64,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_timestamp: i64,
    pub committer_date: String,
    pub refs: Vec<CommitRef>,
    pub subject: String,
    pub message: String,
    pub lane: usize,
    pub color_lane: usize,
    pub edges: Vec<GraphEdge>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRef {
    pub name: String,
    pub kind: RefKind,
}
/// Edges connect this row's lane positions to the next row; parent IDs can lie beyond the loaded page.
#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub color_lane: usize,
    pub from_lane: usize,
    pub to_lane: usize,
    pub parent_id: String,
}
#[derive(Clone, Debug)]
pub struct CommitDetails {
    pub files: Vec<FileChange>,
    pub first_parent: Option<String>,
    /// Destination path to source path, retained even for binary renames.
    pub rename_sources: HashMap<String, String>,
}

fn git(path: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut out = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut err = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = Command::new("git")
        .args([
            "-C",
            path,
            "--no-pager",
            "-c",
            "core.quotePath=false",
            "-c",
            "color.ui=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(err.try_clone().map_err(|e| e.to_string())?))
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        let status = child.try_wait().map_err(|e| e.to_string())?;
        if out.metadata().map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024
            || err.metadata().map_err(|e| e.to_string())?.len() > 65536
            || started.elapsed() > Duration::from_secs(10)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Git history exceeded its time or output limit".into());
        }
        if let Some(status) = status {
            if !status.success() {
                err.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
                let mut message = String::new();
                err.take(65536)
                    .read_to_string(&mut message)
                    .map_err(|e| e.to_string())?;
                return Err(message.trim().to_owned());
            }
            out.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            out.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn oid(id: &str) -> Result<(), String> {
    if (id.len() == 40 || id.len() == 64) && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Invalid commit ID".into())
    }
}

pub fn load(checkout: &str, limit: usize) -> Result<BranchHistory, String> {
    git(checkout, &["rev-parse", "--git-dir"])?;
    let branch = git(checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|b| string(&b).trim().to_owned());
    let head_oid = git(checkout, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|b| string(&b).trim().to_owned());
    let head = match (branch.clone(), head_oid.as_ref()) {
        (Some(name), _) => HeadIdentity::Branch {
            name,
            oid: head_oid.clone(),
        },
        (None, Some(id)) => HeadIdentity::Detached { oid: id.clone() },
        _ => return Err("Cannot resolve checkout HEAD".into()),
    };
    let validate_head = || -> Result<(), String> {
        let current_branch = git(checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .map(|b| string(&b).trim().to_owned());
        let current_oid = git(checkout, &["rev-parse", "--verify", "HEAD"])
            .ok()
            .map(|b| string(&b).trim().to_owned());
        if current_branch != branch || current_oid != head_oid {
            return Err("Checkout HEAD changed while loading history. Refresh to try again".into());
        }
        Ok(())
    };
    if head_oid.is_none() {
        validate_head()?;
        return Ok(BranchHistory {
            head,
            commits: vec![],
            has_more: false,
        });
    }
    let limit = limit.clamp(1, 500);
    let count = format!("--max-count={}", limit + 1);
    let bytes = git(
        checkout,
        &[
            "log",
            "--topo-order",
            &count,
            "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%aI%x00%cn%x00%ce%x00%ct%x00%cI%x00%s%x00%B%x00",
            head_oid.as_deref().ok_or("Cannot resolve checkout HEAD")?,
            "--",
        ],
    )?;
    let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
    let mut commits = Vec::new();
    for chunk in fields.chunks(12) {
        if chunk.len() < 12 {
            break;
        }
        let id = string(chunk[0]).trim().to_owned();
        oid(&id)?;
        commits.push(Commit {
            id,
            parents: string(chunk[1])
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            author_name: string(chunk[2]),
            author_email: string(chunk[3]),
            timestamp: string(chunk[4])
                .parse()
                .map_err(|_| "Invalid Git timestamp")?,
            author_date: string(chunk[5]),
            committer_name: string(chunk[6]),
            committer_email: string(chunk[7]),
            committer_timestamp: string(chunk[8])
                .parse()
                .map_err(|_| "Invalid Git committer timestamp")?,
            committer_date: string(chunk[9]),
            refs: vec![],
            subject: string(chunk[10]),
            message: string(chunk[11]).trim_end().to_owned(),
            lane: 0,
            color_lane: 0,
            edges: vec![],
        });
    }
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    decorate_refs(
        checkout,
        &mut commits,
        head_oid.as_deref().ok_or("Cannot resolve checkout HEAD")?,
    )?;
    assign_graph(&mut commits)?;
    validate_head()?;
    Ok(BranchHistory {
        head,
        commits,
        has_more,
    })
}

// Read only refs already present on disk, including cached remote-tracking refs.
// No fetch/network access. Structured fields avoid ambiguous %D text;
// annotated tags point at their peeled commit rather than their tag object.
fn decorate_refs(checkout: &str, commits: &mut [Commit], head_oid: &str) -> Result<(), String> {
    let bytes = git(
        checkout,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(*objecttype)%00",
            "refs/heads",
            "refs/tags",
            "refs/remotes",
        ],
    )?;
    let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
    let mut nested_resolutions = 0;
    for chunk in fields.chunks(4) {
        if chunk.len() < 4 {
            break;
        }
        let name = string(chunk[0]).trim_start_matches('\n').to_owned();
        let full_ref = name.clone();
        let (name, kind) = if let Some(name) = name.strip_prefix("refs/heads/") {
            (name.to_owned(), RefKind::Branch)
        } else if let Some(name) = name.strip_prefix("refs/remotes/") {
            (name.to_owned(), RefKind::Remote)
        } else if let Some(name) = name.strip_prefix("refs/tags/") {
            (name.to_owned(), RefKind::Tag)
        } else {
            continue;
        };
        let resolved;
        let target = if chunk[3] == b"tag" {
            nested_resolutions += 1;
            if nested_resolutions > 64 {
                return Err("Git history exceeded its nested tag resolution limit (64)".into());
            }
            // full_ref comes from Git's structured ref listing and has passed one
            // of the explicit refs/heads, refs/tags, refs/remotes prefixes above.
            let expression = format!("{full_ref}^{{}}");
            resolved = string(&git(
                checkout,
                &["rev-parse", "--verify", "--end-of-options", &expression],
            )?)
            .trim()
            .to_owned();
            oid(&resolved)?;
            resolved.as_bytes()
        } else if chunk[2].is_empty() {
            chunk[1]
        } else {
            chunk[2]
        };
        if let Some(commit) = commits
            .iter_mut()
            .find(|commit| commit.id.as_bytes() == target)
        {
            commit.refs.push(CommitRef { name, kind });
        }
    }
    if let Some(commit) = commits.iter_mut().find(|commit| commit.id == head_oid) {
        commit.refs.insert(
            0,
            CommitRef {
                name: "HEAD".into(),
                kind: RefKind::Head,
            },
        );
    }
    Ok(())
}

// Strand identity stays stable when compacted visual lane positions change.
fn assign_graph(commits: &mut [Commit]) -> Result<(), String> {
    let mut lanes: Vec<(String, usize)> = vec![];
    let mut next_color = 0;
    for commit in commits {
        let lane = lanes
            .iter()
            .position(|(id, _)| id == &commit.id)
            .unwrap_or_else(|| {
                lanes.push((commit.id.clone(), next_color));
                next_color += 1;
                lanes.len() - 1
            });
        commit.lane = lane;
        commit.color_lane = lanes[lane].1;
        let before = lanes.clone();
        lanes.remove(lane);
        for (offset, parent) in commit.parents.iter().enumerate() {
            if !lanes.iter().any(|(id, _)| id == parent) {
                let color = if offset == 0 {
                    commit.color_lane
                } else {
                    let color = next_color;
                    next_color += 1;
                    color
                };
                lanes.insert((lane + offset).min(lanes.len()), (parent.clone(), color));
            }
        }
        for (from, (id, color)) in before.iter().enumerate() {
            if from == lane {
                for parent in &commit.parents {
                    let to = lanes
                        .iter()
                        .position(|(id, _)| id == parent)
                        .ok_or("Invalid Git graph topology")?;
                    commit.edges.push(GraphEdge {
                        from_lane: from,
                        to_lane: to,
                        parent_id: parent.clone(),
                        color_lane: lanes[to].1,
                    });
                }
            } else if let Some(to) = lanes.iter().position(|(next, _)| next == id) {
                commit.edges.push(GraphEdge {
                    from_lane: from,
                    to_lane: to,
                    parent_id: id.clone(),
                    color_lane: *color,
                });
            }
        }
    }
    Ok(())
}

/// Merge commits compare against their first parent; root commits compare against the empty tree.
pub fn details(checkout: &str, id: &str) -> Result<CommitDetails, String> {
    oid(id)?;
    let parents = git(checkout, &["show", "-s", "--format=%P", id, "--"])?;
    let first_parent = string(&parents)
        .split_whitespace()
        .next()
        .map(str::to_owned);
    let base_args = |mode| {
        let mut args = vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "-M",
            mode,
            "-z",
        ];
        if let Some(parent) = first_parent.as_deref() {
            args.push(parent);
        }
        args.extend([id, "--"]);
        args
    };
    let bytes = git(checkout, &base_args("--numstat"))?;
    let parts: Vec<_> = bytes.split(|b| *b == 0).collect();
    let mut files = vec![];
    let mut rename_sources = HashMap::new();
    let mut i = 0;
    while i < parts.len() {
        let entry = parts[i];
        i += 1;
        if entry.is_empty() {
            continue;
        }
        let fields: Vec<_> = entry.splitn(3, |b| *b == b'\t').collect();
        if fields.len() != 3 {
            return Err("Malformed Git file statistics".into());
        }
        let binary = fields[0] == b"-";
        let added = string(fields[0]).parse().unwrap_or(0);
        let removed = string(fields[1]).parse().unwrap_or(0);
        let (path, renamed) = if fields[2].is_empty() {
            if i + 1 >= parts.len() {
                return Err("Malformed Git rename".into());
            }
            let old = string(parts[i]);
            let new = string(parts[i + 1]);
            i += 2;
            (new, Some(old))
        } else {
            (string(fields[2]), None)
        };
        if let Some(from) = &renamed {
            rename_sources.insert(path.clone(), from.clone());
        }
        let status = if binary {
            Status::Binary
        } else if let Some(from) = renamed {
            Status::Renamed { from }
        } else {
            Status::Modified
        };
        files.push(FileChange {
            path,
            status,
            added,
            removed,
        });
    }
    // Name-status supplies additions/deletions without guessing from line counts.
    let statuses = git(checkout, &base_args("--name-status"))?;
    let fields: Vec<_> = statuses.split(|b| *b == 0).collect();
    let mut j = 0;
    while j + 1 < fields.len() {
        let code = fields[j];
        j += 1;
        let mut path = string(fields[j]);
        j += 1;
        if code.starts_with(b"R") && j < fields.len() {
            path = string(fields[j]);
            j += 1;
        }
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            if file.status != Status::Binary {
                match code.first() {
                    Some(b'A') => file.status = Status::Added,
                    Some(b'D') => file.status = Status::Deleted,
                    _ => {}
                }
            }
        }
    }
    Ok(CommitDetails {
        files,
        first_parent,
        rename_sources,
    })
}

pub fn file_patch(checkout: &str, id: &str, file: &FileChange) -> Result<Patch, String> {
    oid(id)?;
    if file.status == Status::Binary {
        return Ok(Patch::Binary);
    }
    if u64::from(file.added) + u64::from(file.removed) > u64::from(crate::diff::DIFF_MAX_LINES) {
        return Ok(Patch::TooLarge {
            added: file.added,
            removed: file.removed,
        });
    }
    let mut args = vec![
        "--literal-pathspecs",
        "show",
        "--format=",
        "--root",
        "--first-parent",
        "-m",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        id,
        "--",
        &file.path,
    ];
    if let Status::Renamed { from } = &file.status {
        args.push(from);
    }
    let bytes = git(checkout, &args)?;
    if bytes.len() as u64 > crate::diff::DIFF_MAX_BYTES {
        return Ok(Patch::TooLarge {
            added: file.added,
            removed: file.removed,
        });
    }
    Ok(crate::diff::parse_patch(&string(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(path: &str, args: &[&str]) -> String {
        string(&git(path, args).unwrap()).trim().to_owned()
    }
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_str().unwrap();
        command(p, &["init", "-b", "main"]);
        command(p, &["config", "user.name", "Test Author"]);
        command(p, &["config", "user.email", "test@example.test"]);
        command(p, &["config", "commit.gpgsign", "false"]);
        command(p, &["config", "tag.gpgsign", "false"]);
        dir
    }
    fn commit(p: &str, message: &str) -> String {
        command(p, &["add", "-A"]);
        command(p, &["commit", "-m", message]);
        command(p, &["rev-parse", "HEAD"])
    }
    #[test]
    fn metadata_preserves_identities_dates_message_and_structured_local_refs() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let output = Command::new("git")
            .args([
                "-C",
                p,
                "commit",
                "--allow-empty",
                "-m",
                "Explain the change\n\nDetailed body, with punctuation.\nSecond paragraph.",
            ])
            .env("GIT_AUTHOR_NAME", "Original Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.test")
            .env("GIT_AUTHOR_DATE", "2025-03-04T12:34:56+05:30")
            .env("GIT_COMMITTER_NAME", "Integration Bot")
            .env("GIT_COMMITTER_EMAIL", "bot@example.test")
            .env("GIT_COMMITTER_DATE", "2025-03-05T09:10:11-07:00")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", string(&output.stderr));
        command(p, &["branch", "release,preview"]);
        command(p, &["tag", "-a", "v1,preview", "-m", "Annotated release"]);
        command(p, &["tag", "lightweight"]);
        command(
            p,
            &[
                "tag",
                "-a",
                "nested-release",
                "v1,preview",
                "-m",
                "Nested annotated release",
            ],
        );
        command(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // Legal nested tags can target non-commit objects; they must not hide history.
        let blob = command(p, &["hash-object", "-w", "--stdin"]);
        command(
            p,
            &["tag", "-a", "blob-target", &blob, "-m", "Blob annotation"],
        );
        command(
            p,
            &[
                "tag",
                "-a",
                "nested-blob",
                "blob-target",
                "-m",
                "Nested blob annotation",
            ],
        );
        let page = load(p, 20).unwrap();
        let commit = &page.commits[0];
        assert_eq!(commit.author_name, "Original Author");
        assert_eq!(commit.author_email, "author@example.test");
        assert_eq!(commit.author_date, "2025-03-04T12:34:56+05:30");
        assert_eq!(commit.committer_name, "Integration Bot");
        assert_eq!(commit.committer_email, "bot@example.test");
        assert_eq!(commit.committer_date, "2025-03-05T09:10:11-07:00");
        assert_eq!(commit.timestamp, 1_741_071_896);
        assert_eq!(commit.committer_timestamp, 1_741_191_011);
        assert_eq!(
            commit.message,
            "Explain the change\n\nDetailed body, with punctuation.\nSecond paragraph."
        );
        for (name, kind) in [
            ("HEAD", RefKind::Head),
            ("main", RefKind::Branch),
            ("release,preview", RefKind::Branch),
            ("v1,preview", RefKind::Tag),
            ("lightweight", RefKind::Tag),
            ("nested-release", RefKind::Tag),
            ("origin/main", RefKind::Remote),
        ] {
            assert!(commit.refs.contains(&CommitRef {
                name: name.into(),
                kind
            }));
        }
        assert_eq!(commit.refs.len(), 7);
        assert!(!commit
            .refs
            .iter()
            .any(|reference| reference.name.contains("blob")));
        command(p, &["checkout", "--detach"]);
        let detached = load(p, 20).unwrap();
        assert!(matches!(detached.head, HeadIdentity::Detached { .. }));
        assert!(detached.commits[0].refs.contains(&CommitRef {
            name: "HEAD".into(),
            kind: RefKind::Head
        }));
        command(p, &["commit", "--allow-empty", "-m", "Child commit"]);
        let bounded = load(p, 1).unwrap();
        assert!(bounded.has_more);
        assert_eq!(
            bounded.commits[0].refs,
            vec![CommitRef {
                name: "HEAD".into(),
                kind: RefKind::Head
            }]
        );
    }
    #[test]
    fn binary_rename_retains_source_path_and_binary_patch() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        fs_err::write(dir.path().join("old\nimage.bin"), [0, 1, 2, 3, 255]).unwrap();
        commit(p, "Add binary");
        fs_err::rename(
            dir.path().join("old\nimage.bin"),
            dir.path().join("new\nimage.bin"),
        )
        .unwrap();
        let id = commit(p, "Rename binary");
        let detail = details(p, &id).unwrap();
        assert_eq!(detail.files.len(), 1);
        let file = &detail.files[0];
        assert_eq!(file.path, "new\nimage.bin");
        assert_eq!(file.status, Status::Binary);
        assert_eq!(
            detail.rename_sources.get(&file.path).map(String::as_str),
            Some("old\nimage.bin")
        );
        assert_eq!(file_patch(p, &id, file).unwrap(), Patch::Binary);
    }
    #[test]
    fn strand_color_survives_lane_compaction() {
        let make = |id: &str, parents: &[&str]| Commit {
            id: id.into(),
            parents: parents.iter().map(|p| (*p).into()).collect(),
            author_name: String::new(),
            author_email: String::new(),
            timestamp: 0,
            author_date: String::new(),
            committer_name: String::new(),
            committer_email: String::new(),
            committer_timestamp: 0,
            committer_date: String::new(),
            refs: vec![],
            subject: String::new(),
            message: String::new(),
            lane: 0,
            color_lane: 0,
            edges: vec![],
        };
        // A's strand ends, while B shifts from column 1 to column 0.
        let mut commits = vec![
            make("merge", &["a", "b"]),
            make("a", &[]),
            make("b", &["root"]),
            make("root", &[]),
        ];
        assign_graph(&mut commits).unwrap();
        let before = commits[0]
            .edges
            .iter()
            .find(|e| e.parent_id == "b")
            .unwrap();
        let after = commits[1]
            .edges
            .iter()
            .find(|e| e.parent_id == "b")
            .unwrap();
        assert_eq!((after.from_lane, after.to_lane), (1, 0));
        assert_eq!(before.color_lane, after.color_lane);
        assert_eq!(commits[2].color_lane, after.color_lane);
        assert_eq!(commits[2].edges[0].color_lane, after.color_lane);
    }
    #[test]
    fn unborn_root_rename_binary_detached_and_limit() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        assert!(load(p, 20).unwrap().commits.is_empty());
        fs_err::write(dir.path().join("line\nbreak.txt"), "first\n").unwrap();
        let root = commit(p, "Root\n\nBody");
        let page = load(p, 20).unwrap();
        assert_eq!(page.commits[0].message, "Root\n\nBody");
        assert!(page.commits[0].parents.is_empty());
        let d = details(p, &root).unwrap();
        assert_eq!(d.files[0].path, "line\nbreak.txt");
        assert_eq!(d.files[0].status, Status::Added);
        assert!(matches!(
            file_patch(p, &root, &d.files[0]).unwrap(),
            Patch::Text { .. }
        ));
        fs_err::rename(
            dir.path().join("line\nbreak.txt"),
            dir.path().join("renamed.txt"),
        )
        .unwrap();
        fs_err::write(dir.path().join("binary"), [0, 1, 2]).unwrap();
        let id = commit(p, "Rename and binary");
        let d = details(p, &id).unwrap();
        let renamed = d
            .files
            .iter()
            .find(|f| matches!(f.status, Status::Renamed { .. }))
            .unwrap();
        assert!(
            matches!(file_patch(p,&id,renamed).unwrap(),Patch::Text{hunks,..} if hunks.is_empty())
        );
        assert!(d.files.iter().any(|f| f.status == Status::Binary));
        assert!(load(p, 1).unwrap().has_more);
        command(p, &["checkout", "--detach", &root]);
        assert!(matches!(
            load(p, 10).unwrap().head,
            HeadIdentity::Detached { .. }
        ));
        assert!(load("/definitely/missing", 10).is_err());
        assert!(details(p, "HEAD").is_err());
    }
    #[test]
    fn user_color_configuration_does_not_corrupt_patches() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        command(p, &["config", "color.ui", "always"]);
        fs_err::write(dir.path().join("file.txt"), "literal text\n").unwrap();
        let id = commit(p, "Colored Git config");
        let details = details(p, &id).unwrap();
        let Patch::Text { hunks, .. } = file_patch(p, &id, &details.files[0]).unwrap() else {
            panic!("text patch")
        };
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines[0].text, "literal text");
        assert_eq!(hunks[0].lines[0].kind, crate::diff::LineKind::Add);
    }

    #[test]
    fn merge_topology_and_first_parent_details() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        fs_err::write(dir.path().join("base"), "base").unwrap();
        commit(p, "root");
        command(p, &["checkout", "-b", "side"]);
        fs_err::write(dir.path().join("side"), "side").unwrap();
        let side = commit(p, "side");
        command(p, &["checkout", "main"]);
        fs_err::write(dir.path().join("main"), "main").unwrap();
        let main = commit(p, "main");
        command(p, &["merge", "--no-ff", "side", "-m", "merge"]);
        let page = load(p, 20).unwrap();
        assert_eq!(page.commits[0].parents, vec![main.clone(), side]);
        assert_eq!(page.commits[0].edges.len(), 2);
        assert!(page.commits.iter().skip(1).any(|c| c.edges.len() >= 2));
        let d = details(p, &page.commits[0].id).unwrap();
        assert_eq!(d.first_parent, Some(main));
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "side");
        let Patch::Text { hunks, .. } = file_patch(p, &page.commits[0].id, &d.files[0]).unwrap()
        else {
            panic!("expected text")
        };
        assert_eq!(hunks.len(), 1);
    }
}
