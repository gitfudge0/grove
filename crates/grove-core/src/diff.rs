//! Uncommitted-diff computation for the diff viewer.
//! Pure parsing lives next to the `git`-shelling wrappers that feed it, so the parsers are unit-testable without a repo.

use fs_err as fs;
use std::collections::HashSet;
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
    let Ok(meta) = fs::metadata(full_path) else {
        return 0;
    };
    if meta.len() > DIFF_MAX_BYTES {
        return 0;
    }
    let Ok(content) = fs::read_to_string(full_path) else {
        return 0;
    };
    content.lines().count().try_into().unwrap_or(u32::MAX)
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

    let mut result: Vec<FileChange> = Vec::new();
    for entry in numstat_lines {
        let status = match (&entry.added, &entry.rename_from) {
            (None, _) => Status::Binary,
            (Some(_), Some(from)) => Status::Renamed { from: from.clone() },
            (Some(_), None) => {
                match status_lines
                    .iter()
                    .find(|s| s.path == entry.path)
                    .map(|s| (s.x, s.y))
                {
                    Some(('A', _) | (_, 'A')) => Status::Added,
                    Some(('D', _) | (_, 'D')) => Status::Deleted,
                    _ => Status::Modified,
                }
            }
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

/// Parent category shared by checkout summaries and filtered diffs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeGroup {
    Code,
    Tests,
    Docs,
    Config,
    Uncategorized,
}

impl ChangeGroup {
    pub const ALL: [Self; 5] = [
        Self::Code,
        Self::Tests,
        Self::Docs,
        Self::Config,
        Self::Uncategorized,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Tests => "Tests",
            Self::Docs => "Docs",
            Self::Config => "Config",
            Self::Uncategorized => "Uncategorized",
        }
    }

    pub fn of(path: &str) -> Self {
        ChangeClassifier::default().classify(path).0
    }
}

/// A missing child means the path stays directly in its parent, without an "Other" guess.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeClassifier {
    rules: Vec<ChangeRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRule {
    path: String,
    category: ChangeGroup,
    #[serde(default)]
    subcategory: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeMappings {
    rules: Vec<ChangeRule>,
}

impl ChangeClassifier {
    /// Optional checkout-local overrides. Read off the UI thread with the changed-file snapshot.
    /// Malformed/oversize mappings are logged and fall back as a whole; no files are excluded.
    pub fn load(checkout: &str) -> Self {
        let path = Path::new(checkout).join(".grove/change-categories.json");
        match fs::File::open(&path) {
            Ok(file) => {
                use std::io::Read;
                let mut bytes = Vec::new();
                let result = file.take(64 * 1024 + 1).read_to_end(&mut bytes);
                if result.is_ok() && bytes.len() <= 64 * 1024 {
                    if let Ok(classifier) = Self::from_json(&bytes) {
                        return classifier;
                    }
                }
                tracing::warn!(path = %path.display(), "Invalid change category mappings; using built-in classification");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "Cannot read change category mappings; using built-in classification");
            }
        }
        Self::default()
    }

    fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let config: ChangeMappings = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if config.rules.len() > 256
            || config.rules.iter().any(|rule| {
                rule.path.is_empty()
                    || rule.path.starts_with('/')
                    || rule.path.contains('\\')
                    || rule.path.split('/').any(|part| matches!(part, "." | ".."))
                    || rule.subcategory.as_ref().is_some_and(|label| {
                        label.trim().is_empty()
                            || label.len() > 64
                            || label.chars().any(char::is_control)
                    })
            })
        {
            return Err("Expected at most 256 relative paths and nonempty subcategory labels up to 64 bytes".into());
        }
        Ok(Self {
            rules: config.rules,
        })
    }

    pub fn classify(&self, path: &str) -> (ChangeGroup, Option<String>) {
        use ChangeGroup::{Code, Config, Docs, Tests, Uncategorized};

        for rule in &self.rules {
            if path == rule.path || (rule.path.ends_with('/') && path.starts_with(&rule.path)) {
                return (rule.category, rule.subcategory.clone());
            }
        }
        let lower = path.to_ascii_lowercase();
        let parts: Vec<_> = lower.split('/').collect();
        let name = parts.last().copied().unwrap_or_default();
        let dirs = &parts[..parts.len().saturating_sub(1)];
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let has_dir = |names: &[&str]| dirs.iter().any(|d| names.contains(d));
        let category = |group, child: &str| (group, Some(child.to_string()));
        // Specific context outranks extensions: fixture JSON and workflow YAML have distinct purposes.
        if has_dir(&["fixtures", "__fixtures__", "testdata", "__snapshots__"]) || ext == "snap" {
            return category(Tests, "Fixtures & snapshots");
        }
        if has_dir(&["e2e", "end-to-end"]) {
            return category(Tests, "End-to-end");
        }
        if has_dir(&["integration", "integration-tests", "integration_tests"])
            && has_dir(&[
                "test",
                "tests",
                "spec",
                "specs",
                "integration-tests",
                "integration_tests",
            ])
        {
            return category(Tests, "Integration");
        }
        if has_dir(&["unit", "unit-tests", "unit_tests"])
            && has_dir(&["test", "tests", "spec", "specs", "unit-tests", "unit_tests"])
        {
            return category(Tests, "Unit");
        }
        if path.starts_with(".github/workflows/")
            || path.starts_with(".circleci/")
            || matches!(
                name,
                ".gitlab-ci.yml" | "jenkinsfile" | "azure-pipelines.yml"
            )
        {
            return category(Config, "CI/CD");
        }
        if has_dir(&["test", "tests", "__tests__", "spec", "specs"])
            || stem.starts_with("test_")
            || matches!(stem, "test" | "tests")
            || [
                "_test", "_tests", ".test", "-test", "_spec", ".spec", "-spec",
            ]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
        {
            return (Tests, None);
        }
        if has_dir(&["docs", "doc"])
            || matches!(ext, "md" | "mdx" | "rst" | "adoc")
            || stem == "license"
            || stem.starts_with("license-")
            || name == "copying"
        {
            return (Docs, None);
        }
        if matches!(
            name,
            "cargo.toml"
                | "cargo.lock"
                | "package.json"
                | "package-lock.json"
                | "yarn.lock"
                | "pnpm-lock.yaml"
                | "bun.lock"
                | "bun.lockb"
                | "go.mod"
                | "go.sum"
                | "gemfile"
                | "gemfile.lock"
                | "composer.json"
                | "composer.lock"
                | "pyproject.toml"
                | "poetry.lock"
                | "uv.lock"
                | "pipfile"
                | "pipfile.lock"
                | "pom.xml"
        ) || (name.starts_with("requirements") && ext == "txt")
        {
            return category(Config, "Dependencies & lockfiles");
        }
        if matches!(
            name,
            "dockerfile"
                | "compose.yml"
                | "compose.yaml"
                | "docker-compose.yml"
                | "docker-compose.yaml"
        ) || name.starts_with("dockerfile.")
            || matches!(ext, "tf" | "tfvars")
        {
            return category(Config, "Deployment & infrastructure");
        }
        if matches!(
            name,
            "makefile"
                | "justfile"
                | "cmakelists.txt"
                | "build.gradle"
                | "build.gradle.kts"
                | "tsconfig.json"
        ) || [
            "vite.config.",
            "webpack.config.",
            "rollup.config.",
            "eslint.config.",
            "jest.config.",
            "vitest.config.",
            "playwright.config.",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        {
            return category(Config, "Build & tooling");
        }
        if name == ".env" || name.starts_with(".env.") {
            return category(Config, "Environment & app settings");
        }
        if matches!(
            name,
            ".gitignore" | ".gitattributes" | ".editorconfig" | ".dockerignore"
        ) || path.starts_with(".grove/")
            || matches!(ext, "ini" | "cfg" | "conf" | "plist")
        {
            return (Config, None);
        }
        if matches!(ext, "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd")
            || has_dir(&["scripts", "bin"])
                && matches!(
                    ext,
                    "sh" | "bash"
                        | "zsh"
                        | "fish"
                        | "ps1"
                        | "bat"
                        | "cmd"
                        | "py"
                        | "rb"
                        | "js"
                        | "ts"
                )
        {
            return category(Code, "Scripts & automation");
        }
        if matches!(
            ext,
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "svg"
                | "ico"
                | "avif"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
                | "mp3"
                | "wav"
                | "mp4"
                | "webm"
        ) {
            return category(Code, "Assets");
        }
        if matches!(
            ext,
            "rs" | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "cs"
                | "java"
                | "kt"
                | "kts"
                | "swift"
                | "m"
                | "mm"
                | "go"
                | "py"
                | "rb"
                | "php"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "vue"
                | "svelte"
                | "html"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "sql"
                | "sh"
                | "bash"
                | "zsh"
                | "ps1"
                | "lua"
                | "ex"
                | "exs"
                | "erl"
                | "hs"
                | "scala"
                | "dart"
                | "r"
                | "jl"
                | "clj"
                | "cljs"
                | "el"
                | "vim"
                | "zig"
                | "v"
                | "ipynb"
                | "graphql"
                | "gql"
                | "proto"
        ) {
            return (Code, None);
        }
        (Uncategorized, None)
    }

    pub fn matches(&self, path: &str, group: ChangeGroup, child: Option<&str>) -> bool {
        let (actual, subcategory) = self.classify(path);
        actual == group && child.is_none_or(|child| subcategory.as_deref() == Some(child))
    }
}

/// Populated parents and children only; parent totals include paths without a child.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSummary {
    pub files: usize,
    pub groups: Vec<(ChangeGroup, u32, u32)>,
    pub subgroups: Vec<(ChangeGroup, String, u32, u32)>,
    pub classifier: ChangeClassifier,
}

pub fn summarize(files: &[FileChange]) -> ChangeSummary {
    summarize_with(files, ChangeClassifier::default())
}

pub fn summarize_checkout(checkout: &str) -> ChangeSummary {
    summarize_with(&changed_files(checkout), ChangeClassifier::load(checkout))
}

pub fn summarize_with(files: &[FileChange], classifier: ChangeClassifier) -> ChangeSummary {
    let mut parents = std::collections::BTreeMap::<_, (u32, u32)>::new();
    let mut children = std::collections::BTreeMap::<_, (u32, u32)>::new();
    for file in files {
        let (group, child) = classifier.classify(&file.path);
        let totals = parents.entry(group).or_default();
        totals.0 = totals.0.saturating_add(file.added);
        totals.1 = totals.1.saturating_add(file.removed);
        if let Some(child) = child {
            let totals = children.entry((group, child)).or_default();
            totals.0 = totals.0.saturating_add(file.added);
            totals.1 = totals.1.saturating_add(file.removed);
        }
    }
    ChangeSummary {
        files: files.len(),
        groups: parents.into_iter().map(|(g, (a, r))| (g, a, r)).collect(),
        subgroups: children
            .into_iter()
            .map(|((g, child), (a, r))| (g, child, a, r))
            .collect(),
        classifier,
    }
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
fn parse_patch(diff_text: &str) -> Patch {
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
    let out = Command::new("git")
        .args(["-C", wt, "diff", "-U3", "-M", "HEAD", "--", path])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_patch(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            tracing::warn!(
                status = ?o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "git command failed"
            );
            Patch::Text {
                hunks: vec![],
                no_newline_at_eof: false,
            }
        }
        Err(_) => Patch::Text {
            hunks: vec![],
            no_newline_at_eof: false,
        },
    }
}

/// `None` covers every reason there isn't a `HEAD` version — callers treat them all the same.
pub fn head_blob(wt: &str, path: &str) -> Option<String> {
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
    fs::read_to_string(Path::new(wt).join(path)).ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub changed: bool,
}

/// Splits on `char` boundaries into word/separator tokens so a marked span always lands on a word boundary; concatenation reproduces the input exactly.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_is_word = false;
    for ch in text.chars() {
        let is_word = ch.is_alphanumeric();
        if !current.is_empty() && is_word != current_is_word {
            tokens.push(std::mem::take(&mut current));
        }
        current_is_word = is_word;
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Coalesces adjacent same-`changed` tokens into one `Run` per contiguous region.
fn coalesce(tokens: &[String], changed: &[bool]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (token, &is_changed) in tokens.iter().zip(changed) {
        match runs.last_mut() {
            Some(last) if last.changed == is_changed => last.text.push_str(token),
            _ => runs.push(Run {
                text: token.clone(),
                changed: is_changed,
            }),
        }
    }
    runs
}

/// Trims the common token prefix/suffix and marks the remaining middle changed; O(n), never needs a full LCS since only emphasis, not a minimal edit script, is required.
pub fn word_runs(old: &str, new: &str) -> (Vec<Run>, Vec<Run>) {
    if old == new {
        let mut runs = Vec::new();
        if !old.is_empty() {
            runs.push(Run {
                text: old.to_string(),
                changed: false,
            });
        }
        return (runs.clone(), runs);
    }

    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);

    let mut prefix = 0;
    while prefix < old_tokens.len()
        && prefix < new_tokens.len()
        && old_tokens[prefix] == new_tokens[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix < old_tokens.len() - prefix
        && suffix < new_tokens.len() - prefix
        && old_tokens[old_tokens.len() - 1 - suffix] == new_tokens[new_tokens.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mark = |tokens: &[String]| -> Vec<bool> {
        let len = tokens.len();
        (0..len)
            .map(|i| !(i < prefix || i >= len - suffix))
            .collect()
    };

    let old_changed = mark(&old_tokens);
    let new_changed = mark(&new_tokens);

    (
        coalesce(&old_tokens, &old_changed),
        coalesce(&new_tokens, &new_changed),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciled {
    /// Selection always follows the *path*, never the index, so it survives `new_files` reordering; a newly-appeared file can never steal it.
    pub selected: Option<String>,
    /// Paths dropped from `new_files` — the caller evicts their cache entries so the patch cache cannot grow unbounded.
    pub evicted: Vec<String>,
}

#[must_use]
pub fn reconcile(
    old_files: &[FileChange],
    new_files: &[FileChange],
    selected: Option<&str>,
) -> Reconciled {
    let new_paths: HashSet<&str> = new_files.iter().map(|f| f.path.as_str()).collect();
    let selected = match selected {
        Some(path) => Some(path.to_string()),
        None => new_files.first().map(|f| f.path.clone()),
    };
    let evicted = old_files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|path| !new_paths.contains(path))
        .map(ToString::to_string)
        .collect();
    Reconciled { selected, evicted }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeNode {
    Dir {
        /// No trailing slash — the key the tree toggle uses in the `collapsed` set.
        path: String,
        name: String,
        depth: usize,
        expanded: bool,
    },
    File {
        file: FileChange,
        depth: usize,
    },
}

/// `files` must already be sorted by path; this walks it in one pass tracking open directory prefixes on a stack, rather than building a nested tree.
#[must_use]
pub fn flatten_file_tree<S: std::hash::BuildHasher>(
    files: &[FileChange],
    collapsed: &HashSet<String, S>,
) -> Vec<TreeNode> {
    let mut rows = Vec::new();
    let mut open_dirs: Vec<String> = Vec::new();
    for file in files {
        let parts: Vec<&str> = file.path.split('/').collect();
        let dir_parts = &parts[..parts.len().saturating_sub(1)];

        let mut common = 0;
        while common < open_dirs.len() && common < dir_parts.len() {
            let expected = dir_parts[..=common].join("/");
            if open_dirs[common] == expected {
                common += 1;
            } else {
                break;
            }
        }
        open_dirs.truncate(common);

        for (i, part) in dir_parts.iter().enumerate().skip(common) {
            let path = dir_parts[..=i].join("/");
            // A collapsed directory hides every descendant, not just its immediate children.
            let ancestor_hidden = open_dirs.iter().any(|d| collapsed.contains(d));
            if !ancestor_hidden {
                rows.push(TreeNode::Dir {
                    path: path.clone(),
                    name: (*part).to_string(),
                    depth: i,
                    expanded: !collapsed.contains(&path),
                });
            }
            open_dirs.push(path);
        }

        let hidden = open_dirs.iter().any(|d| collapsed.contains(d));
        if !hidden {
            rows.push(TreeNode::File {
                file: file.clone(),
                depth: dir_parts.len(),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn conservative_subcategories_preserve_context_and_parent_fallback() {
        let classifier = ChangeClassifier::default();
        for (path, parent, child) in [
            (
                "tests/fixtures/users.json",
                ChangeGroup::Tests,
                Some("Fixtures & snapshots"),
            ),
            ("tests/unit/parser.rs", ChangeGroup::Tests, Some("Unit")),
            (
                "tests/integration/client.rs",
                ChangeGroup::Tests,
                Some("Integration"),
            ),
            ("e2e/login.spec.ts", ChangeGroup::Tests, Some("End-to-end")),
            ("src/parser_test.go", ChangeGroup::Tests, None),
            ("src/unit/price.rs", ChangeGroup::Code, None),
            (
                ".github/workflows/test.yml",
                ChangeGroup::Config,
                Some("CI/CD"),
            ),
            (
                "Cargo.lock",
                ChangeGroup::Config,
                Some("Dependencies & lockfiles"),
            ),
            (
                "Dockerfile",
                ChangeGroup::Config,
                Some("Deployment & infrastructure"),
            ),
            (
                "vite.config.ts",
                ChangeGroup::Config,
                Some("Build & tooling"),
            ),
            (
                ".env.example",
                ChangeGroup::Config,
                Some("Environment & app settings"),
            ),
            (
                "install.sh",
                ChangeGroup::Code,
                Some("Scripts & automation"),
            ),
            (
                "scripts/clean.py",
                ChangeGroup::Code,
                Some("Scripts & automation"),
            ),
            ("assets/logo.png", ChangeGroup::Code, Some("Assets")),
            ("data/users.json", ChangeGroup::Uncategorized, None),
            ("data/users.yaml", ChangeGroup::Uncategorized, None),
            ("data/unknown.xyz", ChangeGroup::Uncategorized, None),
            (".hidden/data.json", ChangeGroup::Uncategorized, None),
        ] {
            assert_eq!(
                classifier.classify(path),
                (parent, child.map(str::to_owned)),
                "{path}"
            );
        }
    }

    #[test]
    fn project_mappings_are_ordered_case_sensitive_and_segment_safe() {
        let classifier = ChangeClassifier::from_json(
            br#"{"rules":[
            {"path":"src/ui/special.rs","category":"code","subcategory":"Special"},
            {"path":"src/ui/","category":"code","subcategory":"UI"},
            {"path":"data/custom.json","category":"config"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(
            classifier.classify("src/ui/special.rs").1.as_deref(),
            Some("Special")
        );
        assert_eq!(
            classifier.classify("src/ui/button.rs").1.as_deref(),
            Some("UI")
        );
        assert_eq!(
            classifier.classify("src/uikit/button.rs"),
            (ChangeGroup::Code, None)
        );
        assert_eq!(
            classifier.classify("src/UI/button.rs"),
            (ChangeGroup::Code, None)
        );
        assert_eq!(
            classifier.classify("data/custom.json"),
            (ChangeGroup::Config, None)
        );
        assert!(!classifier.matches("src/ui/special.rs", ChangeGroup::Code, Some("UI")));
        assert!(classifier.matches("src/ui/special.rs", ChangeGroup::Code, None));
    }

    #[test]
    fn invalid_or_missing_mappings_fall_back_without_losing_files() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path().to_str().unwrap();
        assert_eq!(ChangeClassifier::load(root), ChangeClassifier::default());
        fs::create_dir(repo.path().join(".grove")).unwrap();
        let path = repo.path().join(".grove/change-categories.json");
        for contents in [
            r#"{"rules":[{"path":"../src/","category":"code"}]}"#.to_owned(),
            r#"{"rules":[{"path":"src/","category":"fiction"}]}"#.to_owned(),
            r#"{"rules":[{"path":"src/","category":"code","subcategory":""}]}"#.to_owned(),
            "invalid JSON".to_owned(),
            " ".repeat(64 * 1024 + 1),
        ] {
            fs::write(&path, contents).unwrap();
            let classifier = ChangeClassifier::load(root);
            assert_eq!(
                classifier.classify("src/main.rs"),
                (ChangeGroup::Code, None)
            );
        }
    }

    #[test]
    fn hierarchical_totals_and_filters_cover_every_file_once() {
        let files = [
            ("tests/unit/a.rs", 3, 1),
            ("tests/unit/b.rs", 2, 0),
            ("tests/fixtures/a.json", 4, 2),
            ("tests/generic.rs", 1, 1),
            ("data/unknown.json", 5, 0),
        ]
        .map(|(path, added, removed)| FileChange {
            path: path.into(),
            status: Status::Modified,
            added,
            removed,
        });
        let summary = summarize(&files);
        assert_eq!(summary.files, 5);
        assert_eq!(
            summary.groups,
            vec![
                (ChangeGroup::Tests, 10, 4),
                (ChangeGroup::Uncategorized, 5, 0)
            ]
        );
        assert_eq!(
            summary.subgroups,
            vec![
                (ChangeGroup::Tests, "Fixtures & snapshots".into(), 4, 2),
                (ChangeGroup::Tests, "Unit".into(), 5, 1),
            ]
        );
        for (group, added, removed) in &summary.groups {
            let filtered: Vec<_> = files
                .iter()
                .filter(|f| summary.classifier.matches(&f.path, *group, None))
                .collect();
            assert_eq!(filtered.iter().map(|f| f.added).sum::<u32>(), *added);
            assert_eq!(filtered.iter().map(|f| f.removed).sum::<u32>(), *removed);
        }
        for (group, child, added, removed) in &summary.subgroups {
            let filtered: Vec<_> = files
                .iter()
                .filter(|f| summary.classifier.matches(&f.path, *group, Some(child)))
                .collect();
            assert_eq!(filtered.iter().map(|f| f.added).sum::<u32>(), *added);
            assert_eq!(filtered.iter().map(|f| f.removed).sum::<u32>(), *removed);
        }
    }

    #[test]
    fn change_groups_classify_by_path() {
        for (path, group) in [
            ("src/views/sidebar.rs", ChangeGroup::Code),
            ("tests/checkout.rs", ChangeGroup::Tests),
            ("web/app/__tests__/menu.tsx", ChangeGroup::Tests),
            ("pkg/parse_test.go", ChangeGroup::Tests),
            ("ui/button.spec.ts", ChangeGroup::Tests),
            ("test_utils.py", ChangeGroup::Tests),
            ("README.md", ChangeGroup::Docs),
            ("docs/guide.html", ChangeGroup::Docs),
            ("LICENSE-MIT", ChangeGroup::Docs),
            ("Cargo.lock", ChangeGroup::Config),
            (".github/workflows/ci.yml", ChangeGroup::Config),
            (".gitignore", ChangeGroup::Config),
            ("src/latest.rs", ChangeGroup::Code),
            ("src/contest.rs", ChangeGroup::Code),
            ("src/views/tests.rs", ChangeGroup::Tests),
        ] {
            assert_eq!(ChangeGroup::of(path), group, "{path}");
        }
    }

    #[test]
    fn summarize_totals_nonempty_groups_in_order() {
        let change = |path: &str, added, removed| FileChange {
            path: path.into(),
            status: Status::Modified,
            added,
            removed,
        };
        let summary = summarize(&[
            change("tests/a.rs", 10, 1),
            change("src/a.rs", 5, 2),
            change("src/b.rs", 3, 4),
            change("Cargo.toml", 1, 1),
        ]);
        assert_eq!(summary.files, 4);
        assert_eq!(
            summary.groups,
            vec![
                (ChangeGroup::Code, 8, 6),
                (ChangeGroup::Tests, 10, 1),
                (ChangeGroup::Config, 1, 1),
            ]
        );
    }

    #[test]
    fn numstat_common_case() {
        let out = "12\t3\tsrc/main.rs\n";
        let entries = parse_numstat(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].added, Some(12));
        assert_eq!(entries[0].removed, Some(3));
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[0].rename_from, None);
    }

    #[test]
    fn numstat_binary_dash_dash() {
        let out = "-\t-\tassets/logo.png\n";
        let entries = parse_numstat(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].added, None);
        assert_eq!(entries[0].removed, None);
        assert_eq!(entries[0].path, "assets/logo.png");
    }

    #[test]
    fn numstat_rename_full_path() {
        let out = "5\t2\told/name.rs => new/name.rs\n";
        let entries = parse_numstat(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "new/name.rs");
        assert_eq!(entries[0].rename_from.as_deref(), Some("old/name.rs"));
    }

    #[test]
    fn numstat_rename_common_affix() {
        let out = "1\t1\tsrc/{old.rs => new.rs}\n";
        let entries = parse_numstat(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "src/new.rs");
        assert_eq!(entries[0].rename_from.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn numstat_malformed_line_skipped() {
        let out = "not a valid line\n5\t1\tok.rs\n";
        let entries = parse_numstat(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "ok.rs");
    }

    #[test]
    fn numstat_overflow_counts_skipped() {
        let out = "99999999999999\t1\tbig.rs\n";
        assert_eq!(parse_numstat(out).len(), 0);
    }

    #[test]
    fn numstat_empty_input() {
        assert_eq!(parse_numstat("").len(), 0);
        assert_eq!(parse_numstat("\n").len(), 0);
    }

    #[test]
    fn numstat_lopsided_dash_is_malformed() {
        let out = "-\t3\tweird.bin\n";
        assert_eq!(parse_numstat(out).len(), 0);
    }

    #[test]
    fn status_porcelain_untracked() {
        let out = "?? new_file.txt\n";
        let entries = parse_status_porcelain(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].x, '?');
        assert_eq!(entries[0].y, '?');
        assert_eq!(entries[0].path, "new_file.txt");
    }

    #[test]
    fn status_porcelain_rename_takes_destination() {
        let out = "R  old.rs -> new.rs\n";
        let entries = parse_status_porcelain(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "new.rs");
    }

    #[test]
    fn status_porcelain_malformed_short_line_skipped() {
        assert_eq!(parse_status_porcelain("?\n").len(), 0);
        assert_eq!(parse_status_porcelain("\n").len(), 0);
    }

    #[test]
    fn patch_single_hunk_common_case() {
        let diff = "diff --git a/f.rs b/f.rs\n\
index 111..222 100644\n\
--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,3 +1,3 @@ fn main\n\
 unchanged\n\
-old\n\
+new\n\
 tail\n";
        let patch = parse_patch(diff);
        let Patch::Text {
            hunks,
            no_newline_at_eof,
        } = patch
        else {
            panic!("expected Text patch");
        };
        assert!(!no_newline_at_eof);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.header, "fn main");
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind, LineKind::Context);
        assert_eq!(h.lines[1].kind, LineKind::Del);
        assert_eq!(h.lines[1].text, "old");
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[1].new_no, None);
        assert_eq!(h.lines[2].kind, LineKind::Add);
        assert_eq!(h.lines[2].new_no, Some(2));
    }

    #[test]
    fn patch_no_header_after_at_at() {
        let diff = "@@ -1,1 +1,1 @@\n-a\n+b\n";
        let Patch::Text { hunks, .. } = parse_patch(diff) else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks[0].header, "");
    }

    #[test]
    fn patch_multi_hunk() {
        let diff = "--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,1 +1,1 @@\n\
-a\n\
+b\n\
@@ -10,1 +10,1 @@\n\
-c\n\
+d\n";
        let Patch::Text { hunks, .. } = parse_patch(diff) else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[1].old_start, 10);
        assert_eq!(hunks[1].lines[0].text, "c");
    }

    #[test]
    fn patch_no_newline_at_eof() {
        let diff = "--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,1 +1,1 @@\n\
-old\n\
\\ No newline at end of file\n\
+new\n\
\\ No newline at end of file\n";
        let Patch::Text {
            no_newline_at_eof, ..
        } = parse_patch(diff)
        else {
            panic!("expected Text patch");
        };
        assert!(no_newline_at_eof);
    }

    #[test]
    fn patch_binary_marker() {
        let diff = "diff --git a/x.png b/x.png\n\
Binary files a/x.png and b/x.png differ\n";
        assert_eq!(parse_patch(diff), Patch::Binary);
    }

    #[test]
    fn patch_malformed_hunk_header_skips_hunk() {
        let diff = "@@ garbage @@\n-a\n+b\n";
        let Patch::Text { hunks, .. } = parse_patch(diff) else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks.len(), 0);
    }

    #[test]
    fn patch_overflowing_line_numbers_skips_hunk() {
        let diff = "@@ -99999999999999,1 +1,1 @@\n-a\n+b\n";
        let Patch::Text { hunks, .. } = parse_patch(diff) else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks.len(), 0);
    }

    #[test]
    fn patch_empty_input() {
        let Patch::Text {
            hunks,
            no_newline_at_eof,
        } = parse_patch("")
        else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks.len(), 0);
        assert!(!no_newline_at_eof);
    }

    #[test]
    fn synthesize_added_patch_all_lines_added() {
        let Patch::Text { hunks, .. } = synthesize_added_patch("one\ntwo\nthree\n") else {
            panic!("expected Text patch");
        };
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines.len(), 3);
        assert!(hunks[0].lines.iter().all(|l| l.kind == LineKind::Add));
        assert_eq!(hunks[0].lines[0].new_no, Some(1));
        assert_eq!(hunks[0].lines[2].new_no, Some(3));
    }

    fn line(kind: LineKind, text: &str, old_no: Option<u32>, new_no: Option<u32>) -> Line {
        Line {
            kind,
            text: text.to_string(),
            old_no,
            new_no,
        }
    }

    #[test]
    fn unified_rows_orders_header_then_lines() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: "h".into(),
                lines: vec![
                    line(LineKind::Context, "ctx", Some(1), Some(1)),
                    line(LineKind::Del, "old", Some(2), None),
                    line(LineKind::Add, "new", None, Some(2)),
                ],
            }],
            no_newline_at_eof: false,
        };
        let rows = patch.unified_rows();
        assert_eq!(rows.len(), 4);
        assert!(matches!(&rows[0], UnifiedRow::HunkHeader(h) if h == "h"));
        assert!(matches!(&rows[1], UnifiedRow::Line(l) if l.kind == LineKind::Context));
        assert!(matches!(&rows[2], UnifiedRow::Line(l) if l.kind == LineKind::Del));
        assert!(matches!(&rows[3], UnifiedRow::Line(l) if l.kind == LineKind::Add));
    }

    #[test]
    fn unified_rows_empty_for_too_large_and_binary() {
        assert_eq!(
            Patch::TooLarge {
                added: 1,
                removed: 1
            }
            .unified_rows()
            .len(),
            0
        );
        assert_eq!(Patch::Binary.unified_rows().len(), 0);
    }

    #[test]
    fn paired_rows_context_pairs_with_itself() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: String::new(),
                lines: vec![line(LineKind::Context, "ctx", Some(1), Some(1))],
            }],
            no_newline_at_eof: false,
        };
        let rows = patch.paired_rows();
        assert_eq!(rows.len(), 2); // header + one paired row
        let PairedRow::Lines { old, new } = &rows[1] else {
            panic!("expected Lines row");
        };
        assert_eq!(old.as_ref().unwrap().text, "ctx");
        assert_eq!(new.as_ref().unwrap().text, "ctx");
    }

    #[test]
    fn paired_rows_equal_run_lengths_pair_positionally() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: String::new(),
                lines: vec![
                    line(LineKind::Del, "d1", Some(1), None),
                    line(LineKind::Del, "d2", Some(2), None),
                    line(LineKind::Add, "a1", None, Some(1)),
                    line(LineKind::Add, "a2", None, Some(2)),
                ],
            }],
            no_newline_at_eof: false,
        };
        let rows = patch.paired_rows();
        assert_eq!(rows.len(), 3); // header + 2 paired rows
        let PairedRow::Lines { old, new } = &rows[1] else {
            panic!()
        };
        assert_eq!(old.as_ref().unwrap().text, "d1");
        assert_eq!(new.as_ref().unwrap().text, "a1");
        let PairedRow::Lines { old, new } = &rows[2] else {
            panic!()
        };
        assert_eq!(old.as_ref().unwrap().text, "d2");
        assert_eq!(new.as_ref().unwrap().text, "a2");
    }

    #[test]
    fn paired_rows_leftover_dels_become_half_empty_rows() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: String::new(),
                lines: vec![
                    line(LineKind::Del, "d1", Some(1), None),
                    line(LineKind::Del, "d2", Some(2), None),
                    line(LineKind::Del, "d3", Some(3), None),
                    line(LineKind::Add, "a1", None, Some(1)),
                ],
            }],
            no_newline_at_eof: false,
        };
        let rows = patch.paired_rows();
        assert_eq!(rows.len(), 4); // header + 3 rows (max(3,1))
        let PairedRow::Lines { old, new } = &rows[1] else {
            panic!()
        };
        assert_eq!(old.as_ref().unwrap().text, "d1");
        assert_eq!(new.as_ref().unwrap().text, "a1");
        let PairedRow::Lines { old, new } = &rows[2] else {
            panic!()
        };
        assert_eq!(old.as_ref().unwrap().text, "d2");
        assert!(new.is_none());
        let PairedRow::Lines { old, new } = &rows[3] else {
            panic!()
        };
        assert_eq!(old.as_ref().unwrap().text, "d3");
        assert!(new.is_none());
    }

    #[test]
    fn paired_rows_leftover_adds_become_half_empty_rows() {
        let patch = Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: String::new(),
                lines: vec![
                    line(LineKind::Add, "a1", None, Some(1)),
                    line(LineKind::Add, "a2", None, Some(2)),
                ],
            }],
            no_newline_at_eof: false,
        };
        let rows = patch.paired_rows();
        assert_eq!(rows.len(), 3);
        let PairedRow::Lines { old, new } = &rows[1] else {
            panic!()
        };
        assert!(old.is_none());
        assert_eq!(new.as_ref().unwrap().text, "a1");
    }

    fn reconstruct(runs: &[Run]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn word_runs_identical_lines_are_one_unchanged_run_each() {
        let (old, new) = word_runs("let x = 1;", "let x = 1;");
        assert_eq!(old, new);
        assert_eq!(old.len(), 1);
        assert!(!old[0].changed);
        assert_eq!(reconstruct(&old), "let x = 1;");
    }

    #[test]
    fn word_runs_one_token_changed_in_the_middle() {
        let (old, new) = word_runs("let x = 1;", "let x = 2;");
        assert_eq!(reconstruct(&old), "let x = 1;");
        assert_eq!(reconstruct(&new), "let x = 2;");
        assert!(old.iter().any(|r| r.changed && r.text == "1"));
        assert!(new.iter().any(|r| r.changed && r.text == "2"));
        assert!(old.iter().any(|r| !r.changed && r.text.starts_with("let")));
    }

    #[test]
    fn word_runs_insertion_at_start() {
        let (old, new) = word_runs("world", "hello world");
        assert_eq!(reconstruct(&old), "world");
        assert_eq!(reconstruct(&new), "hello world");
        assert!(old.iter().all(|r| !r.changed));
        assert!(new[0].changed && new[0].text == "hello ");
        assert!(!new.last().unwrap().changed);
    }

    #[test]
    fn word_runs_insertion_at_end() {
        let (old, new) = word_runs("hello", "hello world");
        assert_eq!(reconstruct(&old), "hello");
        assert_eq!(reconstruct(&new), "hello world");
        assert!(old.iter().all(|r| !r.changed));
        assert!(new.last().unwrap().changed);
        assert!(!new[0].changed);
    }

    #[test]
    fn word_runs_whole_line_replacement_no_common_affix() {
        let (old, new) = word_runs("abc", "xyz");
        assert_eq!(reconstruct(&old), "abc");
        assert_eq!(reconstruct(&new), "xyz");
        assert_eq!(old.len(), 1);
        assert_eq!(new.len(), 1);
        assert!(old[0].changed);
        assert!(new[0].changed);
    }

    #[test]
    fn word_runs_empty_old() {
        let (old, new) = word_runs("", "new text");
        assert!(old.is_empty());
        assert_eq!(reconstruct(&new), "new text");
        assert!(new.iter().all(|r| r.changed));
    }

    #[test]
    fn word_runs_empty_new() {
        let (old, new) = word_runs("old text", "");
        assert_eq!(reconstruct(&old), "old text");
        assert!(new.is_empty());
        assert!(old.iter().all(|r| r.changed));
    }

    #[test]
    fn word_runs_both_empty() {
        let (old, new) = word_runs("", "");
        assert!(old.is_empty());
        assert!(new.is_empty());
    }

    #[test]
    fn word_runs_no_empty_text_runs_anywhere() {
        for (a, b) in [
            ("", ""),
            ("", "x"),
            ("x", ""),
            ("abc", "abc"),
            ("abc", "xyz"),
            ("let x = 1;", "let x = 2;"),
        ] {
            let (old, new) = word_runs(a, b);
            assert!(old.iter().all(|r| !r.text.is_empty()));
            assert!(new.iter().all(|r| !r.text.is_empty()));
        }
    }

    #[test]
    fn word_runs_cjk_multibyte_does_not_panic_and_reconstructs() {
        let (old, new) = word_runs("日本語のテキスト", "日本語のテキストです");
        assert_eq!(reconstruct(&old), "日本語のテキスト");
        assert_eq!(reconstruct(&new), "日本語のテキストです");
        assert!(old.iter().all(|r| r.changed));
        assert!(new.iter().all(|r| r.changed));
    }

    #[test]
    fn word_runs_emoji_and_accented_chars_do_not_panic_and_reconstruct() {
        let old = "café 🎉 party";
        let new = "café 🎊 party time";
        let (old_runs, new_runs) = word_runs(old, new);
        assert_eq!(reconstruct(&old_runs), old);
        assert_eq!(reconstruct(&new_runs), new);
    }

    #[test]
    fn word_runs_marks_land_on_word_boundaries_not_mid_character() {
        let (old, new) = word_runs("changed", "chunged");
        assert_eq!(reconstruct(&old), "changed");
        assert_eq!(reconstruct(&new), "chunged");
        assert_eq!(old.len(), 1);
        assert_eq!(new.len(), 1);
        assert!(old[0].changed && old[0].text == "changed");
        assert!(new[0].changed && new[0].text == "chunged");
    }

    fn fc(path: &str) -> FileChange {
        FileChange {
            path: path.to_string(),
            status: Status::Modified,
            added: 1,
            removed: 1,
        }
    }

    #[test]
    fn reconcile_keeps_selection_on_a_path_that_dropped_out() {
        let old = vec![fc("a.rs"), fc("b.rs")];
        let new = vec![fc("a.rs")];
        let r = reconcile(&old, &new, Some("b.rs"));
        assert_eq!(r.selected.as_deref(), Some("b.rs"));
        assert_eq!(r.evicted, vec!["b.rs".to_string()]);
    }

    #[test]
    fn reconcile_a_newly_appeared_file_does_not_steal_selection() {
        let old = vec![fc("a.rs")];
        let new = vec![fc("a.rs"), fc("new.rs")];
        let r = reconcile(&old, &new, Some("a.rs"));
        assert_eq!(r.selected.as_deref(), Some("a.rs"));
        assert!(r.evicted.is_empty());
    }

    #[test]
    fn reconcile_selects_first_file_when_nothing_was_selected() {
        let old: Vec<FileChange> = vec![];
        let new = vec![fc("a.rs"), fc("b.rs")];
        let r = reconcile(&old, &new, None);
        assert_eq!(r.selected.as_deref(), Some("a.rs"));
    }

    #[test]
    fn reconcile_none_selected_and_no_files_stays_none() {
        let r = reconcile(&[], &[], None);
        assert_eq!(r.selected, None);
        assert!(r.evicted.is_empty());
    }

    #[test]
    fn reconcile_evicts_every_path_that_dropped_out_not_just_the_selected_one() {
        let old = vec![fc("a.rs"), fc("b.rs"), fc("c.rs")];
        let new = vec![fc("b.rs")];
        let r = reconcile(&old, &new, Some("b.rs"));
        let mut evicted = r.evicted;
        evicted.sort();
        assert_eq!(evicted, vec!["a.rs".to_string(), "c.rs".to_string()]);
    }

    #[test]
    fn reconcile_untouched_list_evicts_nothing() {
        let old = vec![fc("a.rs")];
        let new = vec![fc("a.rs")];
        let r = reconcile(&old, &new, Some("a.rs"));
        assert!(r.evicted.is_empty());
        assert_eq!(r.selected.as_deref(), Some("a.rs"));
    }

    fn dirs(rows: &[TreeNode]) -> Vec<(&str, usize, bool)> {
        rows.iter()
            .filter_map(|n| match n {
                TreeNode::Dir {
                    path,
                    depth,
                    expanded,
                    ..
                } => Some((path.as_str(), *depth, *expanded)),
                TreeNode::File { .. } => None,
            })
            .collect()
    }

    fn files(rows: &[TreeNode]) -> Vec<&str> {
        rows.iter()
            .filter_map(|n| match n {
                TreeNode::File { file, .. } => Some(file.path.as_str()),
                TreeNode::Dir { .. } => None,
            })
            .collect()
    }

    #[test]
    fn flatten_flat_files_have_no_dir_rows() {
        let files_in = vec![fc("a.rs"), fc("b.rs")];
        let rows = flatten_file_tree(&files_in, &HashSet::new());
        assert!(dirs(&rows).is_empty());
        assert_eq!(files(&rows), vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn flatten_nested_dirs_emit_one_dir_row_per_level() {
        let files_in = vec![fc("src/views/rows.rs")];
        let rows = flatten_file_tree(&files_in, &HashSet::new());
        assert_eq!(dirs(&rows), vec![("src", 0, true), ("src/views", 1, true)]);
        assert_eq!(files(&rows), vec!["src/views/rows.rs"]);
    }

    #[test]
    fn flatten_single_deep_path() {
        let files_in = vec![fc("a/b/c/d/e.rs")];
        let rows = flatten_file_tree(&files_in, &HashSet::new());
        assert_eq!(
            dirs(&rows),
            vec![
                ("a", 0, true),
                ("a/b", 1, true),
                ("a/b/c", 2, true),
                ("a/b/c/d", 3, true)
            ]
        );
        assert_eq!(files(&rows), vec!["a/b/c/d/e.rs"]);
    }

    #[test]
    fn flatten_sibling_files_at_multiple_levels() {
        let files_in = vec![fc("a.rs"), fc("src/b.rs"), fc("src/nested/c.rs")];
        let rows = flatten_file_tree(&files_in, &HashSet::new());
        assert_eq!(files(&rows), vec!["a.rs", "src/b.rs", "src/nested/c.rs"]);
        assert_eq!(dirs(&rows), vec![("src", 0, true), ("src/nested", 1, true)]);
    }

    #[test]
    fn flatten_a_collapsed_dir_hides_its_children() {
        let files_in = vec![fc("src/a.rs"), fc("src/nested/b.rs"), fc("top.rs")];
        let mut collapsed = HashSet::new();
        collapsed.insert("src".to_string());
        let rows = flatten_file_tree(&files_in, &collapsed);
        // "src" itself still renders (so it can be reopened); its children do not.
        assert_eq!(dirs(&rows), vec![("src", 0, false)]);
        assert_eq!(files(&rows), vec!["top.rs"]);
    }

    #[test]
    fn flatten_collapsing_a_shared_ancestor_reuses_the_stack_correctly() {
        let files_in = vec![fc("a/one.rs"), fc("a/two.rs"), fc("b/three.rs")];
        let mut collapsed = HashSet::new();
        collapsed.insert("a".to_string());
        let rows = flatten_file_tree(&files_in, &collapsed);
        assert_eq!(dirs(&rows), vec![("a", 0, false), ("b", 0, true)]);
        assert_eq!(files(&rows), vec!["b/three.rs"]);
    }
}
