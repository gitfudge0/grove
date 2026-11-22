//! Uncommitted-diff computation for the diff viewer.
//! Pure parsing lives next to the `git`-shelling wrappers that feed it, so the parsers are unit-testable without a repo.

mod classification;
mod projection;

pub use classification::{
    summarize, summarize_checkout, summarize_with, ChangeClassifier, ChangeGroup, ChangeSummary,
};
pub use projection::{flatten_file_tree, reconcile, word_runs, Reconciled, Run, TreeNode};

use fs_err as fs;
use std::io::Read as _;
use std::path::Path;
use std::process::Command;

/// Oversize guard: whichever of this and [`DIFF_MAX_BYTES`] trips first stalls loading.
pub const DIFF_MAX_LINES: u32 = 3000;
/// Byte-size guard alongside [`DIFF_MAX_LINES`] — a file can be short on lines yet huge (one long minified line).
pub const DIFF_MAX_BYTES: u64 = 1024 * 1024;

/// How a path differs from `HEAD`, plus two states `git status` has no letter for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Untracked,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub status: Status,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

/// `old_no`/`new_no` are `None` on the side a line doesn't exist on; a `Context` line carries both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    /// Git's "which function is this hunk in" heuristic; empty when git didn't print one.
    pub header: String,
    pub lines: Vec<Line>,
}

/// The content of a single file's diff, or a reason there isn't any to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Patch {
    Text {
        hunks: Vec<Hunk>,
        no_newline_at_eof: bool,
    },
    /// Oversize guard tripped — counts only, no content was read.
    TooLarge {
        added: u32,
        removed: u32,
    },
    Binary,
}

/// One row of the unified-view render: either a hunk separator or a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnifiedRow {
    HunkHeader(String),
    Line(Line),
}

/// See [`Patch::paired_rows`] for the pairing rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairedRow {
    HunkHeader(String),
    Lines {
        old: Option<Line>,
        new: Option<Line>,
    },
}

impl Patch {
    /// The only place unified layout is decided; the view does no diff arithmetic.
    pub fn unified_rows(&self) -> Vec<UnifiedRow> {
        let Patch::Text { hunks, .. } = self else {
            return vec![];
        };
        let mut rows = Vec::new();
        for hunk in hunks {
            rows.push(UnifiedRow::HunkHeader(hunk.header.clone()));
            rows.extend(hunk.lines.iter().cloned().map(UnifiedRow::Line));
        }
        rows
    }

    /// Split-mode rows: consecutive `Del` lines pair positionally with the `Add` run right after them; the longer run leaves half-empty rows on the shorter side.
    pub fn paired_rows(&self) -> Vec<PairedRow> {
        let Patch::Text { hunks, .. } = self else {
            return vec![];
        };
        let mut rows = Vec::new();
        for hunk in hunks {
            rows.push(PairedRow::HunkHeader(hunk.header.clone()));
            let lines = &hunk.lines;
            let mut i = 0;
            while i < lines.len() {
                match lines[i].kind {
                    LineKind::Context => {
                        rows.push(PairedRow::Lines {
                            old: Some(lines[i].clone()),
                            new: Some(lines[i].clone()),
                        });
                        i += 1;
                    }
                    LineKind::Del | LineKind::Add => {
                        let mut dels = Vec::new();
                        while i < lines.len() && lines[i].kind == LineKind::Del {
                            dels.push(lines[i].clone());
                            i += 1;
                        }
                        let mut adds = Vec::new();
                        while i < lines.len() && lines[i].kind == LineKind::Add {
                            adds.push(lines[i].clone());
                            i += 1;
                        }
                        let n = dels.len().max(adds.len());
                        for j in 0..n {
                            rows.push(PairedRow::Lines {
                                old: dels.get(j).cloned(),
                                new: adds.get(j).cloned(),
                            });
                        }
                    }
                }
            }
        }
        rows
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NumstatLine {
    /// `None` marks a binary file (`-` in both count columns).
    added: Option<u32>,
    removed: Option<u32>,
    path: String,
    rename_from: Option<String>,
}

/// Splits a numstat rename field into `(old, new)`; handles both git formats, full replacement and common-affix abbreviation.
fn split_rename_path(field: &str) -> Option<(String, String)> {
    if let Some(brace_start) = field.find('{') {
        let brace_end = field[brace_start..].find('}')? + brace_start;
        let prefix = &field[..brace_start];
        let suffix = &field[brace_end + 1..];
        let inner = &field[brace_start + 1..brace_end];
        let (old, new) = inner.split_once(" => ")?;
        return Some((
            format!("{prefix}{old}{suffix}"),
            format!("{prefix}{new}{suffix}"),
        ));
    }
    field
        .split_once(" => ")
        .map(|(old, new)| (old.to_string(), new.to_string()))
}

/// Malformed lines are skipped rather than failing the whole parse.
fn parse_numstat(out: &str) -> Vec<NumstatLine> {
    let mut result = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(added_s), Some(removed_s), Some(field)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let added = if added_s == "-" {
            None
        } else {
            match added_s.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => continue,
            }
        };
        let removed = if removed_s == "-" {
            None
        } else {
            match removed_s.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => continue,
            }
        };
        // A binary marker must be symmetric; a one-sided "-" is malformed.
        if added.is_none() != removed.is_none() {
            continue;
        }
        if let Some((old, new)) = split_rename_path(field) {
            result.push(NumstatLine {
                added,
                removed,
                path: new,
                rename_from: Some(old),
            });
        } else {
            result.push(NumstatLine {
                added,
                removed,
                path: field.to_string(),
                rename_from: None,
            });
        }
    }
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusLine {
    x: char,
    y: char,
    path: String,
}

/// Only the `??` (untracked) lines are used by [`changed_files`]; tracked-file status comes from `--numstat -M` instead.
fn parse_status_porcelain(out: &str) -> Vec<StatusLine> {
    let mut result = Vec::new();
    for line in out.lines() {
        if line.len() < 3 {
            continue;
        }
        let mut chars = line.chars();
        let Some(x) = chars.next() else { continue };
        let Some(y) = chars.next() else { continue };
        let rest = &line[2..];
        let Some(path) = rest.strip_prefix(' ') else {
            continue;
        };
        // Renames/copies print "old -> new"; keep the destination half.
        let path = path.rsplit(" -> ").next().unwrap_or(path);
        result.push(StatusLine {
            x,
            y,
            path: path.to_string(),
        });
    }
    result
}

/// A file over [`DIFF_MAX_BYTES`] is not read at all.
fn count_lines_guarded(full_path: &Path) -> u32 {
    type Entry = (std::time::SystemTime, u64, u32);
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Entry>>,
    > = std::sync::OnceLock::new();
    let Ok(meta) = fs::metadata(full_path) else {
        return 0;
    };
    if meta.len() > DIFF_MAX_BYTES {
        return 0;
    }
    let modified = meta.modified().ok();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(modified) = modified {
        if let Some((at, size, count)) = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(full_path)
        {
            if *at == modified && *size == meta.len() {
                return *count;
            }
        }
    }
    let Ok(file) = fs::File::open(full_path) else {
        return 0;
    };
    let mut content = String::new();
    if file
        .take(DIFF_MAX_BYTES + 1)
        .read_to_string(&mut content)
        .is_err()
        || content.len() as u64 > DIFF_MAX_BYTES
    {
        return 0;
    }
    let count = content.lines().count().try_into().unwrap_or(u32::MAX);
    if let Some(modified) = modified {
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= 1024 {
            cache.clear();
        }
        cache.insert(full_path.to_path_buf(), (modified, meta.len(), count));
    }
    count
}

/// Merges tracked (`--numstat -M`) and untracked (`status --porcelain -uall`) changes, sorted by path.
pub fn changed_files(wt: &str) -> Vec<FileChange> {
    tracing::debug!(args = "diff --numstat -M HEAD", cwd = %wt, "running git command");
    let numstat_out = Command::new("git")
        .args(["-C", wt, "diff", "--numstat", "-M", "HEAD"])
        .output();
    let numstat_lines = match numstat_out {
        Ok(o) if o.status.success() => parse_numstat(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            tracing::warn!(
                status = ?o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "git command failed"
            );
            vec![]
        }
        Err(_) => vec![],
    };

    tracing::debug!(args = "status --porcelain -uall", cwd = %wt, "running git command");
    let status_out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-C", wt, "status", "--porcelain", "-uall"])
        .output();
    let status_lines = match status_out {
        Ok(o) if o.status.success() => parse_status_porcelain(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            tracing::warn!(
                status = ?o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "git command failed"
            );
            vec![]
        }
        Err(_) => vec![],
    };

    let status_by_path: std::collections::HashMap<_, _> = status_lines
        .iter()
        .map(|entry| (entry.path.as_str(), (entry.x, entry.y)))
        .collect();
    let mut result: Vec<FileChange> = Vec::new();
    for entry in numstat_lines {
        let status = match (&entry.added, &entry.rename_from) {
            (None, _) => Status::Binary,
            (Some(_), Some(from)) => Status::Renamed { from: from.clone() },
            (Some(_), None) => match status_by_path.get(entry.path.as_str()).copied() {
                Some(('A', _) | (_, 'A')) => Status::Added,
                Some(('D', _) | (_, 'D')) => Status::Deleted,
                _ => Status::Modified,
            },
        };
        result.push(FileChange {
            path: entry.path,
            status,
            added: entry.added.unwrap_or(0),
            removed: entry.removed.unwrap_or(0),
        });
    }
    for s in status_lines.iter().filter(|s| s.x == '?' && s.y == '?') {
        let full_path = Path::new(wt).join(&s.path);
        result.push(FileChange {
            path: s.path.clone(),
            status: Status::Untracked,
            added: count_lines_guarded(&full_path),
            removed: 0,
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    result
}

/// Malformed input returns `None` so the caller can skip the hunk rather than panic or fabricate numbers.
fn parse_hunk_header(rest: &str) -> Option<(u32, u32, String)> {
    let (ranges, header) = match rest.split_once(" @@") {
        Some((r, h)) => (r, h.trim_start().to_string()),
        None => (rest.trim_end_matches("@@").trim_end(), String::new()),
    };
    let mut toks = ranges.split_whitespace();
    let old_tok = toks.next()?.strip_prefix('-')?;
    let new_tok = toks.next()?.strip_prefix('+')?;
    let old_start: u32 = old_tok.split(',').next()?.parse().ok()?;
    let new_start: u32 = new_tok.split(',').next()?.parse().ok()?;
    Some((old_start, new_start, header))
}

/// Preamble lines are skipped by virtue of appearing before any `@@` line opens a hunk.
pub(crate) fn parse_patch(diff_text: &str) -> Patch {
    if diff_text.contains("Binary files") {
        return Patch::Binary;
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut cur: Option<Hunk> = None;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut no_newline_at_eof = false;
    for line in diff_text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(h) = cur.take() {
                hunks.push(h);
            }
            if let Some((old_start, new_start, header)) = parse_hunk_header(rest) {
                old_no = old_start;
                new_no = new_start;
                cur = Some(Hunk {
                    old_start,
                    new_start,
                    header,
                    lines: Vec::new(),
                });
            }
            continue;
        }
        if line == "\\ No newline at end of file" {
            no_newline_at_eof = true;
            continue;
        }
        let Some(hunk) = cur.as_mut() else {
            continue;
        };
        if let Some(text) = line.strip_prefix('+') {
            hunk.lines.push(Line {
                kind: LineKind::Add,
                text: text.to_string(),
                old_no: None,
                new_no: Some(new_no),
            });
            new_no = new_no.saturating_add(1);
        } else if let Some(text) = line.strip_prefix('-') {
            hunk.lines.push(Line {
                kind: LineKind::Del,
                text: text.to_string(),
                old_no: Some(old_no),
                new_no: None,
            });
            old_no = old_no.saturating_add(1);
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line);
            hunk.lines.push(Line {
                kind: LineKind::Context,
                text: text.to_string(),
                old_no: Some(old_no),
                new_no: Some(new_no),
            });
            old_no = old_no.saturating_add(1);
            new_no = new_no.saturating_add(1);
        }
    }
    if let Some(h) = cur.take() {
        hunks.push(h);
    }
    Patch::Text {
        hunks,
        no_newline_at_eof,
    }
}

fn synthesize_added_patch(content: &str) -> Patch {
    let no_newline_at_eof = !content.is_empty() && !content.ends_with('\n');
    let lines: Vec<Line> = content
        .lines()
        .enumerate()
        .map(|(i, text)| Line {
            kind: LineKind::Add,
            text: text.to_string(),
            old_no: None,
            new_no: Some(i as u32 + 1),
        })
        .collect();
    Patch::Text {
        hunks: vec![Hunk {
            old_start: 0,
            new_start: 1,
            header: String::new(),
            lines,
        }],
        no_newline_at_eof,
    }
}

fn untracked_patch(wt: &str, path: &str) -> Patch {
    let full_path = Path::new(wt).join(path);
    let Ok(meta) = fs::metadata(&full_path) else {
        return Patch::TooLarge {
            added: 0,
            removed: 0,
        };
    };
    if meta.len() > DIFF_MAX_BYTES {
        let added = count_lines_guarded(&full_path);
        return Patch::TooLarge { added, removed: 0 };
    }
    let Ok(content) = fs::read_to_string(&full_path) else {
        return Patch::Binary;
    };
    let line_count = content.lines().count() as u32;
    if line_count > DIFF_MAX_LINES {
        return Patch::TooLarge {
            added: line_count,
            removed: 0,
        };
    }
    synthesize_added_patch(&content)
}

/// The oversize/binary guard is checked before the full diff is generated — a cheap `--numstat` probe supplies the `TooLarge` counts.
pub fn file_patch(wt: &str, path: &str, status: &Status) -> Patch {
    if let Status::Binary = status {
        return Patch::Binary;
    }
    if let Status::Untracked = status {
        return untracked_patch(wt, path);
    }

    // Guard byte size before asking Git to materialize a minified one-line
    // blob. Line-count limits alone cannot bound its memory cost.
    if fs::metadata(Path::new(wt).join(path)).is_ok_and(|meta| meta.len() > DIFF_MAX_BYTES)
        || head_blob_size(wt, path).is_some_and(|size| size > DIFF_MAX_BYTES)
    {
        return Patch::TooLarge {
            added: 0,
            removed: 0,
        };
    }
    tracing::debug!(
        args = format!("diff --numstat -M HEAD -- {path}"),
        cwd = %wt,
        "running git command"
    );
    let probe = Command::new("git")
        .args(["-C", wt, "diff", "--numstat", "-M", "HEAD", "--", path])
        .output();
    if let Ok(o) = &probe {
        if o.status.success() {
            if let Some(entry) = parse_numstat(&String::from_utf8_lossy(&o.stdout))
                .into_iter()
                .next()
            {
                if entry.added.is_none() {
                    return Patch::Binary;
                }
                let total = entry
                    .added
                    .unwrap_or(0)
                    .saturating_add(entry.removed.unwrap_or(0));
                if total > DIFF_MAX_LINES {
                    return Patch::TooLarge {
                        added: entry.added.unwrap_or(0),
                        removed: entry.removed.unwrap_or(0),
                    };
                }
            }
        }
    }

    tracing::debug!(
        args = format!("diff -U3 -M HEAD -- {path}"),
        cwd = %wt,
        "running git command"
    );
    match crate::checkout_git::git_limited(
        Path::new(wt),
        &["diff", "-U3", "-M", "HEAD", "--", path],
        (DIFF_MAX_BYTES * 3) as usize,
    ) {
        Ok(output) if output.ends_with("\n[context truncated]") => Patch::TooLarge {
            added: 0,
            removed: 0,
        },
        Ok(output) => parse_patch(&output),
        Err(_) => Patch::Text {
            hunks: vec![],
            no_newline_at_eof: false,
        },
    }
}

fn head_blob_size(wt: &str, path: &str) -> Option<u64> {
    let output = Command::new("git")
        .args(["-C", wt, "cat-file", "-s", &format!("HEAD:{path}")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

/// `None` covers every reason there isn't a `HEAD` version — callers treat them all the same.
pub fn head_blob(wt: &str, path: &str) -> Option<String> {
    if head_blob_size(wt, path)? > DIFF_MAX_BYTES {
        return None;
    }
    tracing::debug!(
        args = format!("show HEAD:{path}"),
        cwd = %wt,
        "running git command"
    );
    let out = Command::new("git")
        .args(["-C", wt, "show", &format!("HEAD:{path}")])
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command failed"
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn working_file(wt: &str, path: &str) -> Option<String> {
    let file = fs::File::open(Path::new(wt).join(path)).ok()?;
    if file.metadata().ok()?.len() > DIFF_MAX_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(DIFF_MAX_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= DIFF_MAX_BYTES).then_some(text)
}

#[cfg(test)]
mod tests;
